//! Room-scoped search and attachment browsing over SDK-decrypted history.
use std::{collections::{BTreeMap, BTreeSet}, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}}};
use futures_util::future::{AbortHandle, Abortable};
use makepad_widgets::*;
use matrix_sdk::{media::MediaFormat, room::MessagesOptions};
use ruma::{
    OwnedEventId, OwnedUserId,
    events::room::message::{MessageType, RoomMessageEventContent},
};
use crate::{
    media_cache::{MediaCache, MediaCacheEntry},
    shared::{
        attachment_download::{
            DownloadKind, DownloadableAttachment, media_source_mxc, start_attachment_download, start_attachment_share,
        },
        navigation_bar_button::NavigationBarButtonAction,
        text_or_image::{TextOrImageAction, TextOrImageWidgetRefExt, TextOrImageWidgetExt},
    },
    sliding_sync::{current_user_id, get_client, spawn_async_task},
    utils::RoomNameId,
};
use super::back_swipe::BackSwipe;

const CAPTION_PADDING: f64 = if cfg!(target_os = "macos") { 28.0 } else { 0.0 };

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistoryFilter {
    #[default]
    Messages,
    Media,
    Files,
    Audio,
}

#[derive(Clone, Debug)]
pub enum RoomHistoryAction {
    Open {
        room: RoomNameId,
        filter: HistoryFilter,
    },
    Jump {
        room: RoomNameId,
        event: OwnedEventId,
    },
    Close,
}

#[derive(Clone, Debug)]
struct HistoryMessage {
    id: OwnedEventId,
    sender: OwnedUserId,
    name: String,
    timestamp: u64,
    content: MessageType,
}
impl HistoryMessage {
    fn attachment(&self) -> Option<DownloadableAttachment> {
        let (source, name, size, kind) = match &self.content {
            MessageType::Image(m) => (
                &m.source,
                m.filename(),
                m.info.as_ref().and_then(|i| i.size),
                DownloadKind::Image,
            ),
            MessageType::Video(m) => (
                &m.source,
                m.filename(),
                m.info.as_ref().and_then(|i| i.size),
                DownloadKind::Video,
            ),
            MessageType::File(m) => (
                &m.source,
                m.filename(),
                m.info.as_ref().and_then(|i| i.size),
                DownloadKind::File,
            ),
            MessageType::Audio(m) => (
                &m.source,
                m.filename(),
                m.info.as_ref().and_then(|i| i.size),
                DownloadKind::Audio,
            ),
            _ => return None,
        };
        Some(DownloadableAttachment {
            media_source: source.clone(),
            filename: name.into(),
            size: size.map(u64::from),
            kind,
        })
    }
    #[cfg(test)]
    fn matches(&self, query: &str, filter: HistoryFilter, content: &MessageType) -> bool {
        let kind_matches = match filter {
            HistoryFilter::Messages => true,
            HistoryFilter::Media => matches!(content, MessageType::Image(_) | MessageType::Video(_)),
            HistoryFilter::Files => matches!(content, MessageType::File(_)),
            HistoryFilter::Audio => matches!(content, MessageType::Audio(_)),
        };
        let filename = match content {
            MessageType::Image(m) => Some(m.filename()), MessageType::Video(m) => Some(m.filename()),
            MessageType::File(m) => Some(m.filename()), MessageType::Audio(m) => Some(m.filename()),
            _ => None,
        };
        kind_matches && (query.is_empty() || content.body().to_lowercase().contains(query)
            || self.name.to_lowercase().contains(query)
            || filename.is_some_and(|name| name.to_lowercase().contains(query)))
    }
    fn metadata(&self) -> String {
        let date =
            chrono::DateTime::from_timestamp_millis(self.timestamp.min(i64::MAX as u64) as i64)
                .map(|d| {
                    d.with_timezone(&chrono::Local)
                        .format("%Y/%m/%d %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
        let kind = match self.content {
            MessageType::Image(_) => crate::i18n::tr("Photo"),
            MessageType::Video(_) => crate::i18n::tr("Video"),
            MessageType::File(_) => crate::i18n::tr("File"),
            MessageType::Audio(_) => crate::i18n::tr("Audio"),
            _ => crate::i18n::tr("Message"),
        };
        format!("{} · {date} · {kind}", self.name)
    }
}

#[cfg(test)]
#[derive(Default)]
struct HistoryIndex {
    messages: BTreeMap<OwnedEventId, HistoryMessage>,
    edits: BTreeMap<OwnedEventId, Vec<HistoryMessage>>,
    redacted: BTreeSet<OwnedEventId>,
    seen: BTreeSet<OwnedEventId>,
    locked: usize,
    locked_ids: BTreeSet<OwnedEventId>,
}
#[cfg(test)]
impl HistoryIndex {
    fn insert_value(
        &mut self,
        value: serde_json::Value,
        names: &BTreeMap<OwnedUserId, String>,
        locked: bool,
    ) {
        let Some(id) = value
            .get("event_id")
            .and_then(|v| v.as_str())
            .and_then(|s| OwnedEventId::try_from(s).ok())
        else {
            return;
        };
        if value.pointer("/unsigned/redacted_because").is_some() {
            self.redacted.insert(id.clone());
            self.seen.insert(id.clone());
            if self.locked_ids.remove(&id) { self.locked = self.locked.saturating_sub(1); }
            return;
        }
        if locked {
            if self.seen.insert(id.clone()) {
                self.locked_ids.insert(id);
                self.locked += 1;
            }
            return;
        }
        if self.locked_ids.remove(&id) {
            self.locked = self.locked.saturating_sub(1);
        } else if self.seen.contains(&id) { return }
        self.seen.insert(id.clone());
        if value["type"] == "m.room.redaction" {
            if let Some(target) = value
                .get("redacts")
                .or_else(|| value.pointer("/content/redacts"))
                .and_then(|v| v.as_str())
                .and_then(|s| OwnedEventId::try_from(s).ok())
            {
                self.redacted.insert(target);
            }
            return;
        }
        if value["type"] != "m.room.message" {
            return;
        }
        let Some(sender) = value
            .get("sender")
            .and_then(|v| v.as_str())
            .and_then(|s| OwnedUserId::try_from(s).ok())
        else {
            return;
        };
        let Some(timestamp) = value.get("origin_server_ts").and_then(|v| v.as_u64()) else {
            return;
        };
        let content = &value["content"];
        let replacement = (content
            .pointer("/m.relates_to/rel_type")
            .and_then(|v| v.as_str())
            == Some("m.replace"))
        .then(|| {
            content
                .pointer("/m.relates_to/event_id")
                .and_then(|v| v.as_str())
                .and_then(|s| OwnedEventId::try_from(s).ok())
        })
        .flatten();
        let body = if replacement.is_some() {
            &content["m.new_content"]
        } else {
            content
        };
        let Ok(content) = serde_json::from_value::<RoomMessageEventContent>(body.clone()) else {
            return;
        };
        let message = HistoryMessage {
            id: id.clone(),
            name: names
                .get(&sender)
                .cloned()
                .unwrap_or_else(|| sender.to_string()),
            sender,
            timestamp,
            content: content.msgtype,
        };
        if let Some(target) = replacement {
            self.edits.entry(target).or_default().push(message);
        } else {
            self.messages.insert(id, message);
        }
    }
    #[cfg(test)]
    fn results(
        &self,
        query: &str,
        filter: HistoryFilter,
        cutoff: Option<u64>,
    ) -> Vec<HistoryMessage> {
        self.results_page(query, filter, cutoff, None)
    }
    #[cfg(test)]
    fn results_page(&self, query: &str, filter: HistoryFilter, cutoff: Option<u64>, before: Option<&HistoryPosition>) -> Vec<HistoryMessage> {
        self.results_page_checked(query, filter, cutoff, before, || false)
    }
    fn results_page_checked(&self, query: &str, filter: HistoryFilter, cutoff: Option<u64>, before: Option<&HistoryPosition>, cancelled: impl Fn() -> bool) -> Vec<HistoryMessage> {
        let query = query.trim().to_lowercase();
        let mut selected = BTreeSet::new();
        for (position, message) in self.messages.values().enumerate() {
            if position % 256 == 0 && cancelled() { return Vec::new() }
            if self.redacted.contains(&message.id) || cutoff.is_some_and(|t| message.timestamp <= t)
            {
                continue;
            }
            if before.is_some_and(|(timestamp, id)| message.timestamp > *timestamp
                || (message.timestamp == *timestamp && message.id <= *id)) { continue }
            let content = self.edits.get(&message.id).and_then(|edits| edits.iter()
                .filter(|edit| edit.sender == message.sender && !self.redacted.contains(&edit.id))
                .max_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id))))
                .map(|edit| &edit.content).unwrap_or(&message.content);
            if message.matches(&query, filter, content) {
                selected.insert((std::cmp::Reverse(message.timestamp), message.id.clone()));
                if selected.len() > 501 { selected.pop_last(); }
            }
        }
        selected.into_iter().filter_map(|(_, id)| {
            let mut message = self.messages.get(&id)?.clone();
            if let Some(edit) = self.edits.get(&id).and_then(|edits| edits.iter()
                .filter(|edit| edit.sender == message.sender && !self.redacted.contains(&edit.id))
                .max_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)))) {
                message.content = edit.content.clone();
            }
            Some(message)
        }).collect()
    }
}

type HistoryPosition = (u64, OwnedEventId);

type HistoryPageResult = Result<
    (usize, usize, Option<String>, bool),
    String,
>;

#[derive(Debug)]
struct HistoryPage {
    owner: OwnedUserId,
    epoch: u64,
    request: u64,
    result: HistoryPageResult,
    last: bool,
}

type HistorySearchResult = Option<(Vec<HistoryMessage>, (usize, usize))>;

#[derive(Debug)]
struct HistoryResults {
    owner: OwnedUserId,
    epoch: u64,
    request: u64,
    revision: u64,
    messages: Vec<HistoryMessage>,
    has_more: bool,
    error: Option<String>,
    stats: Option<(usize,usize)>,
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.RoomHistoryPanel = #(RoomHistoryPanel::register_widget(vm)) {
        ..mod.widgets.SolidView
        width: Fill height: Fill flow: Down draw_bg.color: mod.widgets.RINX_PAGE
        padding: Inset{top: mod.widgets.SAFE_INSET_PAD_TOP + #(CAPTION_PADDING) bottom: mod.widgets.SAFE_INSET_PAD_BOTTOM}
        header := DetailHeader {title.text: #(crate::i18n::tr("Search Chat History")) title.i18n_text: "Search Chat History"}
        room_name := RinxLabel {width: Fill height: Fit margin: 12 max_lines: 1 text_overflow: Ellipsis draw_text +: {color: mod.widgets.RINX_ACCENT text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}}
        browser := View {
            width: Fill height: Fill flow: Down spacing: 8
            search_bar := View {width: Fill height: 40 flow: Right margin: Inset{left: 12 right: 12} spacing: 8
                history_query := RinxInput {width: Fill height: Fill empty_text: #(crate::i18n::tr("Search messages or file names")) i18n_empty_text: "Search messages or file names"}
                search_history := RobrixNeutralIconButton {text: #(crate::i18n::tr("Search")) i18n_text: "Search" width: 64 height: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}}
            }
            filters := View {width: Fill height: 36 flow: Right margin: Inset{left: 12 right: 12} spacing: 4
                all_filter := RobrixNeutralIconButton {text: #(crate::i18n::tr("All")) i18n_text: "All" width: Fill height: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}}
                media_filter := RobrixNeutralIconButton {text: #(crate::i18n::tr("Media")) i18n_text: "Media" width: Fill height: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}}
                file_filter := RobrixNeutralIconButton {text: #(crate::i18n::tr("Files")) i18n_text: "Files" width: Fill height: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}}
                audio_filter := RobrixNeutralIconButton {text: #(crate::i18n::tr("Audio")) i18n_text: "Audio" width: Fill height: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}}
            }
            history_status := RinxLabel {width: Fill height: Fit margin: Inset{left: 12 right: 12} flow: Flow.Right{wrap: true} draw_text +: {color: mod.widgets.RINX_MUTED text_style: theme.font_regular {font_size: mod.widgets.RINX_UI_SIZE}}}
            result_navigation := View {width: Fill height: 36 flow: Right margin: Inset{left: 12 right: 12} spacing: 8
                results_previous := RobrixNeutralIconButton {text: #(crate::i18n::tr("Previous")) i18n_text: "Previous" width: Fill height: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}}
                results_next := RobrixNeutralIconButton {text: #(crate::i18n::tr("Next")) i18n_text: "Next" width: Fill height: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}}
            }
            results := PortalList {
                width: Fill height: Fill
                Entry := NavigationBarButton {
                    width: Fill height: 96 flow: Right padding: 12 spacing: 12 align: Align{y: 0.5}
                    draw_bg +: {color_hover: mod.widgets.RINX_SURFACE color_active: mod.widgets.RINX_SELECTED border_radius: 0}
                    thumbnail := TextOrImage {visible: false width: 64 height: 64
                        image_view +: {height: Fill image +: {height: Fill fit: ImageFit.Smallest}}
                        text_view +: {height: Fill label +: {max_lines: 2 draw_text.text_style.font_size: mod.widgets.RINX_META_SIZE}}
                    }
                    View {width: Fill height: Fit flow: Down spacing: 8
                        metadata := RinxLabel {width: Fill max_lines: 1 text_overflow: Ellipsis draw_text +: {color: mod.widgets.RINX_ACCENT text_style: theme.font_regular {font_size: mod.widgets.RINX_META_SIZE}}}
                        body := RinxLabel {width: Fill max_lines: 2 text_overflow: Ellipsis draw_text +: {color: mod.widgets.RINX_INK text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}}
                    }
                }
            }
            history_more := RobrixNeutralIconButton {
                height: RINX_BUTTON_HEIGHT padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
                text: #(crate::i18n::tr("Load Older")) i18n_text: "Load Older" width: Fill margin: 12 spacing: 0 icon_walk: Walk{width: 0 height: 0}
            }
        }
        detail := ScrollYView {
            visible: false width: Fill height: Fill flow: Down padding: 16 spacing: 16
            detail_metadata := RinxLabel {width: Fill flow: Flow.Right{wrap: true} draw_text +: {color: mod.widgets.RINX_ACCENT text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}}
            preview := TextOrImage {visible: false width: Fill height: 260
                image_view +: {height: Fill image +: {height: Fill fit: ImageFit.Smallest}}
            }
            detail_body := RinxLabel {width: Fill flow: Flow.Right{wrap: true} draw_text +: {color: mod.widgets.RINX_INK text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}}
            detail_hint := RinxLabel {width: Fill flow: Flow.Right{wrap: true} draw_text +: {color: mod.widgets.RINX_MUTED text_style: theme.font_regular {font_size: mod.widgets.RINX_UI_SIZE}}}
            view_in_chat := RobrixPositiveIconButton {
                height: RINX_BUTTON_HEIGHT padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
                text: #(crate::i18n::tr("View in Chat")) i18n_text: "View in Chat" width: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}
            }
            post_text_to_moments := RobrixNeutralIconButton {
                height: RINX_BUTTON_HEIGHT padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
                text: #(crate::i18n::tr("Post to Moments")) i18n_text: "Post to Moments" width: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}
            }
            attachment_actions := View {width: Fill height: Fit flow: Right spacing: 12
                download_attachment := RobrixNeutralIconButton {
                    height: RINX_BUTTON_HEIGHT padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
                    text: #(crate::i18n::tr("Download")) i18n_text: "Download" width: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}
                }
                share_attachment := RobrixNeutralIconButton {
                    height: RINX_BUTTON_HEIGHT padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
                    text: #(crate::i18n::tr("Share")) i18n_text: "Share" width: Fill spacing: 0 icon_walk: Walk{width: 0 height: 0}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct RoomHistoryPanel {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    owner: Option<OwnedUserId>,
    #[rust]
    epoch: u64,
    #[rust]
    room: Option<RoomNameId>,
    #[rust]
    filter: HistoryFilter,
    #[rust]
    index: Arc<Mutex<Option<hagency_history::Index>>>,
    #[rust]
    query_revision: Arc<AtomicU64>,
    #[rust]
    repair_index: Arc<AtomicBool>,
    #[rust]
    searched: usize,
    #[rust]
    locked: usize,
    #[rust]
    matches: Vec<HistoryMessage>,
    #[rust]
    results_before: Option<HistoryPosition>,
    #[rust]
    results_back: Vec<Option<HistoryPosition>>,
    #[rust]
    results_more: bool,
    #[rust]
    detail: Option<HistoryMessage>,
    #[rust]
    cursor: Option<String>,
    #[rust]
    exhausted: bool,
    #[rust]
    busy: bool,
    #[rust]
    ready: bool,
    #[rust]
    request: u64,
    #[rust]
    abort: Option<AbortHandle>,
    #[rust]
    error: Option<String>,
    #[rust]
    media: Option<MediaCache>,
    #[rust]
    back_swipe: BackSwipe,
}

impl RoomHistoryPanel {
    fn cancel(&mut self) {
        if let Some(abort) = self.abort.take() {
            abort.abort();
        }
        self.busy = false;
        self.request = next_request();
        self.query_revision.fetch_add(1, Ordering::Relaxed);
    }
    fn reset(&mut self) {
        self.cancel();
        self.owner = None;
        self.room = None;
        self.index = Arc::new(Mutex::new(None));
        self.repair_index = Arc::new(AtomicBool::new(false));
        self.query_revision.fetch_add(1, Ordering::Relaxed);
        self.searched = 0;
        self.locked = 0;
        self.matches.clear();
        self.results_before = None;
        self.results_back.clear();
        self.results_more = false;
        self.detail = None;
        self.cursor = None;
        self.exhausted = false;
        self.ready = false;
        self.error = None;
        self.media = None;
        self.back_swipe = Default::default();
    }
    fn load(&mut self, cx: &mut Cx, pages: usize) {
        if self.busy || self.exhausted {
            return;
        }
        let Some(client) = get_client() else { return };
        let Some(owner) = self.owner.clone().filter(|o| client.user_id() == Some(o)) else {
            return;
        };
        let Some(room) = self
            .room
            .as_ref()
            .and_then(|r| client.get_room(r.room_id()))
        else {
            return;
        };
        self.error = None;
        self.busy = true;
        self.request = next_request();
        let request = self.request;
        let epoch = self.epoch;
        let mut cursor = self.cursor.clone();
        let initial = cursor.is_none();
        let index = self.index.clone();
        let repair_index = self.repair_index.clone();
        let (abort, registration) = AbortHandle::new_pair();
        self.abort = Some(abort);
        spawn_async_task(async move {
            let _ = Abortable::new(
                async move {
                    if initial && let Err(error)=catch_up_history(&client,&room,&index,&repair_index).await {
                        Cx::post_action(HistoryPage {owner:owner.clone(),epoch,request,result:Err(error.to_string()),last:true});
                        return;
                    }
                    if initial {crate::history_store::retry_locked_on_open(client.clone(),room.clone());}
                    for page in 0..pages {
                        if !crate::matrix_context::is_current(&client) {
                            break;
                        }
                        let worker_slot=index.clone();
                        let worker_client=client.clone();
                        let room_id=room.room_id().to_owned();
                        let prepared=tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                            let mut slot=worker_slot.lock().unwrap();
                            if slot.is_none() {*slot=Some(crate::history_store::open(&worker_client,&room_id)?);}
                            let index=slot.as_ref().unwrap();
                            Ok((index.coverage()?,index.generation()?))
                        }).await;
                        let (previous,generation)=match prepared {
                            Ok(Ok(state))=>state,
                            result=>{
                                Cx::post_action(HistoryPage{owner:owner.clone(),epoch,request,
                                    result:Err(format!("{result:?}")),last:true});
                                break;
                            }
                        };
                        let mut options = MessagesOptions::backward();
                        options.from = cursor.clone();
                        options.limit = ruma::uint!(100);
                        match room.messages(options).await {
                            Ok(response) => {
                                let done = response.end.is_none()
                                    || response.end == cursor;
                                cursor = response.end;
                                let senders: BTreeSet<_> = response
                                    .chunk
                                    .iter()
                                    .filter_map(|e| e.kind.parse_sender())
                                    .collect();
                                let mut names = BTreeMap::new();
                                for sender in senders {
                                    if let Ok(Some(member)) = room.get_member_no_sync(&sender).await
                                    {
                                        if let Some(name) = member.display_name() {
                                            names.insert(sender, name.to_owned());
                                        }
                                    }
                                }
                                let worker_index = index.clone();
                                let worker_client = client.clone();
                                let worker_room = room.room_id().to_owned();
                                let next_cursor = cursor.clone();
                                let head_cursor = response.start.clone();
                                let indexed = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                                    let mut slot = worker_index.lock().unwrap();
                                    if slot.is_none() { *slot = Some(crate::history_store::open(&worker_client, &worker_room)?); }
                                    let index = slot.as_mut().unwrap();
                                    crate::matrix_context::ensure_current(&worker_client)?;
                                    let events = response.chunk.iter().map(|event| {
                                        let sender = event.kind.parse_sender();
                                        crate::history_store::event(event, sender.as_ref().and_then(|s|names.get(s)).map(String::as_str))
                                    }).collect::<anyhow::Result<Vec<_>>>()?.into_iter().flatten().collect::<Vec<_>>();
                                    // Reopening refreshes the live tail, then resumes the saved
                                    // older cursor. It must not overwrite completed coverage.
                                    let mut coverage = if initial && page == 0 && (previous.cursor.is_some() || previous.exhausted) {
                                        previous.clone()
                                    } else { hagency_history::Coverage { cursor: next_cursor, exhausted: done, head_cursor: previous.head_cursor.clone() } };
                                    if coverage.head_cursor.is_none() {coverage.head_cursor=Some(head_cursor);}
                                    let coverage=index.insert_page(&events,&previous,&coverage,&generation)?;
                                    let (searched, locked) = index.stats()?;
                                    Ok((searched, locked, coverage))
                                }).await;
                                let indexed = indexed.map_err(|e|e.to_string()).and_then(|r|r.map_err(|e|e.to_string()));
                                let (searched, locked, coverage) = match indexed {
                                    Ok(stats) => stats,
                                    Err(error) => {
                                        Cx::post_action(HistoryPage { owner: owner.clone(), epoch, request,
                                            result: Err(error.to_string()), last: true });
                                        break;
                                    }
                                };
                                cursor = coverage.cursor;
                                let done = coverage.exhausted;
                                if initial && page==0 {
                                    if let Err(error)=catch_up_history(&client,&room,&index,&repair_index).await {
                                        Cx::post_action(HistoryPage{owner:owner.clone(),epoch,request,
                                            result:Err(error.to_string()),last:true});
                                        break;
                                    }
                                }
                                if !crate::matrix_context::is_current(&client) { break }
                                Cx::post_action(HistoryPage {
                                    owner: owner.clone(),
                                    epoch,
                                    request,
                                    result: Ok((searched, locked, cursor.clone(), done)),
                                    last: done || page + 1 == pages,
                                });
                                if done {
                                    break;
                                }
                            }
                            Err(error) => {
                                Cx::post_action(HistoryPage {
                                    owner: owner.clone(),
                                    epoch,
                                    request,
                                    result: Err(error.to_string()),
                                    last: true,
                                });
                                break;
                            }
                        }
                    }
                },
                registration,
            )
            .await;
        });
        self.refresh(cx, false);
    }
    fn refresh(&mut self, cx: &mut Cx, scroll_top: bool) {
        if !self.ready { self.update_status(cx);return; }
        let cutoff = self
            .room
            .as_ref()
            .and_then(|r| super::chat_actions::cleared_through(r.room_id()));
        if scroll_top {
            self.results_before = None;
            self.results_back.clear();
            self.matches.clear();
        }
        self.results_more = false;
        let before = self.results_before.clone();
        let revision = self.query_revision.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let current = self.query_revision.clone();
        let index = self.index.clone();
        let repair_index = self.repair_index.clone();
        let query = self.text_input(cx, ids!(history_query)).text();
        let filter = self.filter;
        let request = self.request;
        let epoch = self.epoch;
        if let (Some(owner), Some(client), Some(room)) = (self.owner.clone(), get_client(), self.room.clone()) {
            spawn_async_task(async move {
                let result = tokio::task::spawn_blocking(move || -> anyhow::Result<HistorySearchResult> {
                    crate::history_store::ensure_healthy()?;
                    if current.load(Ordering::Relaxed) != revision { return Ok(None) }
                    let mut slot = index.lock().unwrap();
                    if slot.is_none() { *slot = Some(crate::history_store::open(&client, room.room_id())?); }
                    let index = slot.as_mut().unwrap();
                    if repair_index.swap(false, Ordering::Relaxed) { index.reset_stale()?; }
                    if current.load(Ordering::Relaxed) != revision { return Ok(None) }
                    let filter = match filter { HistoryFilter::Messages=>0, HistoryFilter::Media=>1, HistoryFilter::Files=>2, HistoryFilter::Audio=>3 };
                    let messages = index.search(&query, filter, cutoff, before.as_ref().map(|p|(p.0,p.1.as_str())),
                        || current.load(Ordering::Relaxed) != revision)?;
                    let messages = messages.into_iter().map(|m| Ok(HistoryMessage {
                        id: m.id.try_into()?, sender: m.sender.try_into()?, name: m.name,
                        timestamp: m.timestamp, content: serde_json::from_value::<RoomMessageEventContent>(m.content)?.msgtype,
                    })).collect::<anyhow::Result<Vec<_>>>()?;
                    Ok((current.load(Ordering::Relaxed) == revision).then_some((messages,index.stats()?)))
                }).await;
                let result = result.map_err(|e|e.to_string()).and_then(|r|r.map_err(|e|e.to_string()));
                let (mut messages, error, stats) = match result {
                    Ok(Some((messages,stats))) => (messages,None,Some(stats)),
                    Ok(None) => return,
                    Err(error) => (Vec::new(),Some(error),None),
                };
                let has_more = messages.len() > 500;
                messages.truncate(500);
                if crate::account_session::epoch() == epoch {
                    Cx::post_action(HistoryResults { owner, epoch, request, revision, messages, has_more, error, stats });
                }
            });
        }
        self.update_status(cx);
        if scroll_top {
            self.portal_list(cx, ids!(results))
                .set_first_id_and_scroll(0, 0.0);
        }
    }
    fn update_status(&mut self, cx: &mut Cx) {
        self.button(cx, ids!(results_next)).set_enabled(cx, self.results_more);
        self.button(cx, ids!(results_previous)).set_enabled(cx, !self.results_back.is_empty());
        self.view(cx, ids!(result_navigation)).set_visible(cx, self.results_more || !self.results_back.is_empty());
        let status = if let Some(error) = &self.error {
            crate::i18n::format("Could not load history: {error}. Retry to continue.", &[("error", (error).to_string())])
        } else {
            crate::i18n::format("{0} result{1} · {2} messages searched. {3}{4}", &[("0", (self.matches.len()).to_string()), ("1", (crate::i18n::plural_suffix(self.matches.len())).to_string()), ("2", (self.searched).to_string()), ("3", (if self.busy {
                    crate::i18n::tr("Searching history…")
                } else if self.exhausted {
                    crate::i18n::tr("All available history checked.")
                } else {
                    crate::i18n::tr("Older history is available.")
                }).to_string()), ("4", (if self.locked > 0 {
                    crate::i18n::format(" {0} encrypted messages are unavailable on this device.", &[("0", (self.locked).to_string())])
                } else {
                    String::new()
                }).to_string())])
        };
        self.label(cx, ids!(history_status)).set_text(cx, &status);
        self.button(cx, ids!(history_more)).set_text(
            cx,
            if self.busy {
                crate::i18n::tr("Stop")
            } else if self.error.is_some() {
                crate::i18n::tr("Retry")
            } else {
                crate::i18n::tr("Load Older")
            },
        );
        self.button(cx, ids!(history_more))
            .set_visible(cx, self.busy || !self.exhausted);
        self.view(cx, ids!(browser))
            .set_visible(cx, self.detail.is_none());
        self.view(cx, ids!(detail))
            .set_visible(cx, self.detail.is_some());
        self.label(cx, ids!(header.title)).set_text(
            cx,
            if self.detail.is_some() {
                crate::i18n::tr("Message Details")
            } else if self.filter == HistoryFilter::Messages {
                crate::i18n::tr("Search Chat History")
            } else {
                crate::i18n::tr("Shared Attachments")
            },
        );
        for (id, filter, label) in [
            (ids!(all_filter), HistoryFilter::Messages, crate::i18n::tr("All")),
            (ids!(media_filter), HistoryFilter::Media, crate::i18n::tr("Media")),
            (ids!(file_filter), HistoryFilter::Files, crate::i18n::tr("Files")),
            (ids!(audio_filter), HistoryFilter::Audio, crate::i18n::tr("Audio")),
        ] {
            self.button(cx, id).set_text(
                cx,
                &format!("{}{label}", if self.filter == filter { "✓ " } else { "" }),
            );
        }
        self.redraw(cx);
    }
    fn show_detail(&mut self, cx: &mut Cx, message: HistoryMessage) {
        self.label(cx, ids!(detail_metadata))
            .set_text(cx, &message.metadata());
        self.label(cx, ids!(detail_body))
            .set_text(cx, message.content.body());
        self.button(cx, ids!(post_text_to_moments)).set_visible(cx,
            current_user_id().as_ref() == Some(&message.sender)
                && matches!(message.content, MessageType::Text(_)));
        let attachment = message.attachment();
        self.view(cx, ids!(attachment_actions))
            .set_visible(cx, attachment.is_some());
        let hint = attachment
            .as_ref()
            .map(|a| {
                crate::i18n::format("{0}{1} · Open the original message to use its media controls.", &[("0", (a.filename).to_string()), ("1", (a.size
                        .map(|size| crate::i18n::format(" · {size} bytes", &[("size", (size).to_string())]))
                        .unwrap_or_default()).to_string())])
            })
            .unwrap_or_default();
        self.label(cx, ids!(detail_hint)).set_text(cx, &hint);
        self.detail = Some(message);
        self.refresh(cx, false);
    }
    fn back(&mut self, cx: &mut Cx) {
        if self.detail.take().is_some() {
            self.refresh(cx, false);
        } else {
            cx.action(RoomHistoryAction::Close);
        }
    }
}

fn next_request() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Replay from an index-owned cursor, independent of the SDK's committed sync
/// token. A crash between SDK sync and the index watcher cannot lose a redaction.
pub(crate) async fn catch_up_history(
    client: &matrix_sdk::Client,
    room: &matrix_sdk::Room,
    slot: &Arc<Mutex<Option<hagency_history::Index>>>,
    repair: &Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let checker=client.clone();
    let epoch=crate::account_session::epoch();
    catch_up_history_checked(client,room,slot,repair,move ||crate::history_store::ensure_account(&checker,epoch)).await
}

pub(crate) async fn catch_up_history_checked(
    client: &matrix_sdk::Client,
    room: &matrix_sdk::Room,
    slot: &Arc<Mutex<Option<hagency_history::Index>>>,
    repair: &Arc<AtomicBool>,
    current: impl Fn()->anyhow::Result<()> + Send + Sync + 'static,
) -> anyhow::Result<()> {
    // Release the exact index gate between committed pages. Foreground catchup
    // remains complete before search, while another room can make progress.
    let current=Arc::new(current);
    loop {
        let checker=current.clone();
        if !catch_up_history_step_checked(client,room,slot,repair,move ||checker()).await? {break;}
        tokio::task::yield_now().await;
    }
    Ok(())
}

/// Commit at most one ordered /messages page. True means more work may remain;
/// the next turn re-reads the index-owned cursor, never the SDK sync token.
pub(crate) async fn catch_up_history_step_checked(
    client: &matrix_sdk::Client,
    room: &matrix_sdk::Room,
    slot: &Arc<Mutex<Option<hagency_history::Index>>>,
    repair: &Arc<AtomicBool>,
    current: impl Fn()->anyhow::Result<()> + Send + Sync + 'static,
) -> anyhow::Result<bool> {
    let guard=crate::history_store::history_gate(client,room.room_id()).lock_owned().await;
    let current=Arc::new(current);
    current()?;
    let worker_slot=slot.clone();
    let worker_client=client.clone();
    let room_id=room.room_id().to_owned();
    let repair=repair.clone();
    let checker=current.clone();
    let (mut coverage,generation,guard)=tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        // The original operation owns the guard through open/reset and returns
        // it only after real IO completes. Cancelling its observer cannot free it.
        let mut slot=worker_slot.lock().unwrap();
        checker()?;
        if slot.is_none() {*slot=Some(crate::history_store::open(&worker_client,&room_id)?);}
        let index=slot.as_mut().unwrap();
        if repair.swap(false,Ordering::Relaxed) {index.reset_stale()?;}
        Ok((index.coverage()?,index.generation()?,guard))
    }).await??;
    current()?;
    let Some(cursor)=coverage.head_cursor.clone() else {return Ok(false)};
    let mut options=MessagesOptions::new(ruma::api::Direction::Forward);
    options.from=Some(cursor.clone());
    options.limit=ruma::uint!(100);
    let response=room.messages(options).await?;
    current()?;
    let done=response.end.is_none() || response.end.as_ref()==Some(&cursor);
    let events=response.chunk.iter().map(|e|crate::history_store::event(e,None)).collect::<anyhow::Result<Vec<_>>>()?.into_iter().flatten().collect::<Vec<_>>();
    let previous=coverage.clone();
    coverage.head_cursor=response.end.or(Some(cursor));
    let next=coverage;
    let worker_slot=slot.clone();
    let checker=current.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        // Recheck after queue/slot waits, immediately before the original SQLite
        // write. An already-entered transaction can finish only in its old index;
        // it is not a cancellable action or an authorization for the next account.
        let _guard=guard;
        let mut slot=worker_slot.lock().unwrap();
        checker()?;
        slot.as_mut().unwrap().insert_page(&events,&previous,&next,&generation)
    }).await??;
    current()?;
    Ok(!done)
}

impl Widget for RoomHistoryPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.owner.is_none() {
            return;
        }
        if self.owner != current_user_id() || self.epoch != crate::account_session::epoch() {
            self.reset();
            cx.action(RoomHistoryAction::Close);
            return;
        }
        if event.back_pressed()
            || matches!(
                event,
                Event::KeyDown(KeyEvent {
                    key_code: KeyCode::Escape,
                    ..
                })
            )
            || matches!(event, Event::Scroll(e) if self.back_swipe.update(e))
        {
            self.back(cx);
            return;
        }
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::Signal) {
            if crate::history_store::ensure_healthy().is_err()
                && self.error.as_deref() != Some("History index updates failed; reconnect before searching") {
                self.matches.clear();self.detail=None;
                self.query_revision.fetch_add(1,Ordering::Relaxed);
                self.error=Some("History index updates failed; reconnect before searching".into());
                self.update_status(cx);
                self.redraw(cx);
            }
        }
        if let Event::Actions(actions) = event {
            for action in actions {
                if let Some(changed)=action.downcast_ref::<crate::history_store::HistoryStoreChanged>()
                    && changed.room.as_ref().is_none_or(|room|self.room.as_ref().is_some_and(|r|r.room_id()==room))
                    && self.epoch==changed.epoch {
                    self.matches.clear();self.detail=None;
                    self.refresh(cx,false);
                    continue;
                }
                if let Some(result) = action.downcast_ref::<HistoryResults>() {
                    if self.owner.as_ref() == Some(&result.owner) && self.request == result.request
                        && self.epoch == result.epoch && self.epoch == crate::account_session::epoch()
                        && self.query_revision.load(Ordering::Relaxed) == result.revision {
                        self.error = result.error.clone();
                        if let Some((searched,locked))=result.stats {self.searched=searched;self.locked=locked;}
                        self.matches = result.messages.clone();
                        self.results_more = result.has_more;
                        self.update_status(cx);
                        self.redraw(cx);
                    }
                    continue;
                }
                if let Some(page) = action.downcast_ref::<HistoryPage>() {
                    if self.owner.as_ref() != Some(&page.owner) || self.request != page.request
                        || self.epoch != page.epoch || self.epoch != crate::account_session::epoch() {
                        continue;
                    }
                    match &page.result {
                        Ok((searched, locked, cursor, exhausted)) => {
                            self.ready = true;
                            self.searched = *searched;
                            self.locked = *locked;
                            self.cursor = cursor.clone();
                            self.exhausted = *exhausted;
                        }
                        Err(error) => self.error = Some(error.clone()),
                    }
                    if page.last {
                        self.busy = false;
                        self.abort = None;
                    }
                    self.refresh(cx, false);
                }
                if action
                    .downcast_ref::<super::chat_actions::ChatVisibilityChanged>()
                    .is_some()
                {
                    self.detail = None;
                    self.refresh(cx, false);
                }
            }
            if self.button(cx, ids!(header.back)).clicked(actions) {
                self.back(cx);
                return;
            }
            if self.detail.is_some() {
                if self.button(cx, ids!(post_text_to_moments)).clicked(actions) {
                    if let Some(message) = &self.detail {
                        cx.action(RoomHistoryAction::Close);
                        cx.action(crate::moments::ui::MomentsAction::Compose {text: message.content.body().to_owned()});
                    }
                }
                if self.button(cx, ids!(view_in_chat)).clicked(actions) {
                    if let (Some(room), Some(message)) = (&self.room, &self.detail) {
                        cx.action(RoomHistoryAction::Jump {
                            room: room.clone(),
                            event: message.id.clone(),
                        });
                        cx.action(RoomHistoryAction::Close);
                    }
                }
                if let Some(attachment) = self.detail.as_ref().and_then(|m| m.attachment()) {
                    if self.button(cx, ids!(download_attachment)).clicked(actions) {
                        start_attachment_download(attachment.clone(), None);
                    }
                    if self.button(cx, ids!(share_attachment)).clicked(actions) {
                        start_attachment_share(attachment, None);
                    }
                }
                return;
            }
            let mut paged = false;
            if self.button(cx, ids!(results_next)).clicked(actions) && self.results_more {
                if let Some(last) = self.matches.last() {
                    if self.results_back.len() == 100 { self.results_back.remove(0); }
                    self.results_back.push(self.results_before.take());
                    self.results_before = Some((last.timestamp, last.id.clone()));
                    paged = true;
                }
            } else if self.button(cx, ids!(results_previous)).clicked(actions) {
                if let Some(before) = self.results_back.pop() {
                    self.results_before = before;
                    paged = true;
                }
            }
            if paged {
                self.portal_list(cx, ids!(results)).set_first_id_and_scroll(0, 0.0);
                self.refresh(cx, false);
            }
            if self
                .text_input(cx, ids!(history_query))
                .changed(actions)
                .is_some()
            {
                self.refresh(cx, true);
            }
            if self
                .text_input(cx, ids!(history_query))
                .returned(actions)
                .is_some()
                || self.button(cx, ids!(search_history)).clicked(actions)
            {
                self.cancel();
                self.error = None;
                self.repair_index.store(true, Ordering::Relaxed);
                self.ready = false;
                self.matches.clear();
                self.searched = 0;
                self.locked = 0;
                self.cursor = None;
                self.exhausted = false;
                self.refresh(cx, true);
                self.load(cx, 10);
            }
            if self.button(cx, ids!(history_more)).clicked(actions) {
                if self.busy {
                    self.cancel();
                    self.refresh(cx, false);
                } else {
                    if self.error.is_some() {
                        self.repair_index.store(true,Ordering::Relaxed);
                        self.ready=false;self.cursor=None;self.exhausted=false;
                    }
                    self.load(cx, 10);
                }
            }
            for (id, filter) in [
                (ids!(all_filter), HistoryFilter::Messages),
                (ids!(media_filter), HistoryFilter::Media),
                (ids!(file_filter), HistoryFilter::Files),
                (ids!(audio_filter), HistoryFilter::Audio),
            ] {
                if self.button(cx, id).clicked(actions) {
                    self.filter = filter;
                    self.refresh(cx, true);
                }
            }
            let list = self.portal_list(cx, ids!(results));
            for (index, row) in list.items_with_actions(actions) {
                let clicked = actions.iter().any(|a| {
                    matches!(
                        a.as_widget_action().widget_uid_eq(row.widget_uid()).cast(),
                        NavigationBarButtonAction::Clicked
                    )
                }) || actions.iter().any(|a| {
                    matches!(
                        a.as_widget_action()
                            .widget_uid_eq(row.text_or_image(cx, ids!(thumbnail)).widget_uid())
                            .cast(),
                        TextOrImageAction::Clicked(_)
                    )
                });
                if clicked && !list.was_scrolling() {
                    if let Some(message) = self.matches.get(index).cloned() {
                        self.show_detail(cx, message);
                    }
                    break;
                }
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.owner.is_none() {
            return DrawStep::done();
        }
        if self.owner != current_user_id() || self.epoch != crate::account_session::epoch() {
            self.reset();
            return DrawStep::done();
        }
        self.draw_content(cx, scope, walk)
    }
}

impl RoomHistoryPanel {
    fn draw_content(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(message) = self.detail.clone() {
            let preview = self.text_or_image(cx, ids!(preview));
            preview.set_visible(cx, matches!(message.content, MessageType::Image(_)));
            if let MessageType::Image(image) = &message.content {
                if let Some(cache) = self.media.as_mut() {
                    match cache.try_get_media_or_fetch(&image.source, MediaFormat::File) {
                        (MediaCacheEntry::Loaded(data), format) => {
                            let variant = if matches!(format, MediaFormat::File) {"full"} else {"thumb"};
                            let key = format!("{}#history-{variant}", media_source_mxc(&image.source));
                            if preview.show_image(cx, Some(image.source.clone()), |cx, img| {
                                crate::utils::load_image_with_cache_key(&img, cx, std::path::Path::new(&key), data)
                                    .map(|()| img.size_in_pixels(cx).unwrap_or_default())
                            }).is_err() {preview.show_text(cx, crate::i18n::tr("Preview unavailable. Download the image to view it."));}
                        }
                        (MediaCacheEntry::Requested, _) => preview.show_text(cx, crate::i18n::tr("Loading original photo…")),
                        (MediaCacheEntry::Failed(_), _) => preview.show_text(cx, crate::i18n::tr("Could not load the photo. Download it or reopen to retry.")),
                    }
                }
            }
        }
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = step.borrow_mut::<PortalList>() else {
                continue;
            };
            list.set_item_range(cx, 0, self.matches.len());
            while let Some(index) = list.next_visible_item(cx) {
                let Some(message) = self.matches.get(index) else {
                    continue;
                };
                let row = list.item(cx, index, id!(Entry));
                row.label(cx, ids!(metadata))
                    .set_text(cx, &message.metadata());
                row.label(cx, ids!(body))
                    .set_text(cx, message.content.body());
                let preview = row.text_or_image(cx, ids!(thumbnail));
                preview.set_visible(cx, matches!(message.content, MessageType::Image(_)));
                if let MessageType::Image(image) = &message.content {
                    if let Some(cache) = self.media.as_mut() {
                        super::room_screen::populate_image_message_content(
                            cx,
                            &preview,
                            image.info.as_deref(),
                            image.source.clone(),
                            image.body.as_str(),
                            cache,
                        );
                    }
                }
                row.draw_all(cx, scope);
            }
        }
        DrawStep::done()
    }
}
impl RoomHistoryPanelRef {
    pub fn action(&self, cx: &mut Cx, modal: ModalRef, action: &RoomHistoryAction) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        match action {
            RoomHistoryAction::Close => {
                inner.reset();
                modal.close(cx);
            }
            RoomHistoryAction::Open { room, filter } => {
                inner.reset();
                inner.owner = current_user_id();
                inner.epoch = crate::account_session::epoch();
                inner.room = Some(room.clone());
                inner.filter = *filter;
                inner.media = Some(MediaCache::new(None));
                inner.text_input(cx, ids!(history_query)).set_text(cx, "");
                inner
                    .label(cx, ids!(room_name))
                    .set_text(cx, &room.display());
                inner.refresh(cx, true);
                modal.open(cx);
                inner.load(cx, 1);
            }
            RoomHistoryAction::Jump { .. } => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paged_history_controls_and_virtual_rows_fit_narrow_and_wide_windows() {
        for width in [390.0, 1280.0] {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            let mut panel = cx.with_vm(|vm| {
                makepad_widgets::script_mod(vm);
                crate::app::register_widgets(vm);
                vm.bx.captured_errors = Some(Vec::new());
                let value = script_eval!(vm, {mod.widgets.RoomHistoryPanel {}});
                RoomHistoryPanel::script_from_value(vm, value)
            });
            let mut index = HistoryIndex::default();
            for n in 0..1000 { insert(&mut index, message(&format!("$event{n}"), "中文 history", n)); }
            panel.matches = index.results_page("", HistoryFilter::Messages, None, None);
            panel.matches.truncate(500);
            panel.results_more = true;
            panel.update_status(&mut cx);
            let pass = DrawPass::new(&mut cx);
            pass.set_size(&mut cx, dvec2(width, 800.0));
            let mut list = DrawList2d::new(&mut cx);
            let overlay = Overlay { draw_list: DrawList::new(&mut cx) };
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            list.begin_always(&mut cx2d);
            overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(dvec2(width, 800.0), Layout::flow_overlay());
            let _ = panel.draw_content(&mut cx2d, &mut Scope::with_data(&mut crate::app::AppState::default()),
                Walk { width: Size::Fixed(width), height: Size::Fixed(800.0), ..Walk::default() });
            overlay.end(&mut cx2d);
            cx2d.end_pass_sized_turtle();
            list.end(&mut cx2d);
            cx2d.end_pass(&pass);
            drop(cx2d);
            drop(draw);
            let next = panel.button(&cx, ids!(results_next)).area().rect(&cx);
            assert!(next.size.x > 0.0 && next.pos.x + next.size.x <= width + 1.0, "{next:?}");
            assert!(panel.portal_list(&cx, ids!(results)).visible_items() < 20);
            cx.with_vm(|vm| assert!(vm.take_errors().is_empty()));
        }
    }
    fn message(id: &str, body: &str, time: u64) -> serde_json::Value {
        serde_json::json!({"type":"m.room.message", "event_id":id, "sender":"@alice:example.org", "origin_server_ts":time, "content":{"msgtype":"m.text", "body":body}})
    }
    fn insert(index: &mut HistoryIndex, value: serde_json::Value) {
        index.insert_value(value, &BTreeMap::new(), false);
    }
    #[test]
    fn result_pages_are_bounded_complete_and_stable_at_equal_timestamps() {
        let mut index = HistoryIndex::default();
        for n in 0..1205 { insert(&mut index, message(&format!("$event{n:05}"), "中文", 100)); }
        let mut cursor = None;
        let mut found = BTreeSet::new();
        loop {
            let mut page = index.results_page("中文", HistoryFilter::Messages, None, cursor.as_ref());
            let more = page.len() > 500;
            page.truncate(500);
            for message in &page { assert!(found.insert(message.id.clone())); }
            if !more { break }
            let last = page.last().unwrap();
            cursor = Some((last.timestamp, last.id.clone()));
        }
        assert_eq!(found.len(), 1205);
    }
    #[test]
    fn cancelled_search_stops_scanning_and_never_returns_partial_results() {
        let mut index = HistoryIndex::default();
        for n in 0..1000 { insert(&mut index, message(&format!("$event{n}"), "中文", n)); }
        let calls = std::cell::Cell::new(0);
        let results = index.results_page_checked("中文", HistoryFilter::Messages, None, None, || {
            calls.set(calls.get() + 1);
            calls.get() == 2
        });
        assert!(results.is_empty());
        assert_eq!(calls.get(), 2);
    }
    #[test]
    fn decryption_and_overlapping_redacted_pages_update_existing_ids() {
        let mut index = HistoryIndex::default();
        let event = message("$event", "recovered", 100);
        index.insert_value(event.clone(), &BTreeMap::new(), true);
        assert_eq!(index.locked, 1);
        insert(&mut index, event.clone());
        assert_eq!(index.locked, 0);
        assert_eq!(index.results("recovered", HistoryFilter::Messages, None).len(), 1);
        let mut redacted = event;
        redacted["unsigned"]["redacted_because"] = serde_json::json!({"type":"m.room.redaction"});
        insert(&mut index, redacted);
        assert!(index.results("", HistoryFilter::Messages, None).is_empty());
    }
    #[test]
    fn backwards_pages_apply_latest_authorized_edit_and_deduplicate() {
        let mut index = HistoryIndex::default();
        let mut edit = message("$edit", "* corrected", 200);
        edit["content"]["m.relates_to"] =
            serde_json::json!({"rel_type":"m.replace","event_id":"$original"});
        edit["content"]["m.new_content"] =
            serde_json::json!({"msgtype":"m.text","body":"Corrected 中文"});
        insert(&mut index, edit.clone());
        insert(&mut index, edit.clone());
        let mut forged = edit;
        forged["event_id"] = "$forged".into();
        forged["sender"] = "@mallory:example.org".into();
        forged["origin_server_ts"] = 300.into();
        forged["content"]["m.new_content"]["body"] = "forged".into();
        insert(&mut index, forged);
        insert(&mut index, message("$original", "obsolete", 100));
        assert_eq!(
            index
                .results("CORRECTED 中文", HistoryFilter::Messages, None)
                .len(),
            1
        );
        assert!(
            index
                .results("obsolete", HistoryFilter::Messages, None)
                .is_empty()
        );
        assert!(
            index
                .results("forged", HistoryFilter::Messages, None)
                .is_empty()
        );
        assert_eq!(index.seen.len(), 3);
    }
    #[test]
    fn redacted_edits_revert_and_redacted_messages_disappear() {
        let mut index = HistoryIndex::default();
        insert(&mut index, message("$original", "original", 100));
        let mut edit = message("$edit", "* edited", 200);
        edit["content"]["m.relates_to"] =
            serde_json::json!({"rel_type":"m.replace","event_id":"$original"});
        edit["content"]["m.new_content"] = serde_json::json!({"msgtype":"m.text","body":"edited"});
        insert(&mut index, edit);
        insert(
            &mut index,
            serde_json::json!({"type":"m.room.redaction", "event_id":"$redact_edit", "content":{"redacts":"$edit"}}),
        );
        assert_eq!(
            index
                .results("original", HistoryFilter::Messages, None)
                .len(),
            1
        );
        insert(
            &mut index,
            serde_json::json!({"type":"m.room.redaction", "event_id":"$redact_original", "redacts":"$original"}),
        );
        assert!(index.results("", HistoryFilter::Messages, None).is_empty());
    }
    #[test]
    fn attachment_filters_filename_search_and_clear_cutoff_apply_together() {
        let mut index = HistoryIndex::default();
        for (id, kind, time) in [
            ("$image", "m.image", 100),
            ("$video", "m.video", 110),
            ("$file", "m.file", 120),
            ("$audio", "m.audio", 130),
        ] {
            let mut value = message(id, "Caption", time);
            value["content"] = serde_json::json!({"msgtype":kind, "body":"Caption", "filename":"项目.PDF", "url":"mxc://example.org/data"});
            insert(&mut index, value);
        }
        assert_eq!(index.results("", HistoryFilter::Media, None).len(), 2);
        assert_eq!(
            index.results("项目.pdf", HistoryFilter::Files, None).len(),
            1
        );
        assert_eq!(index.results("", HistoryFilter::Audio, None).len(), 1);
        assert!(
            index
                .results("", HistoryFilter::Media, Some(115))
                .is_empty()
        );
        assert!(
            index
                .results("", HistoryFilter::Files, Some(120))
                .is_empty()
        );
        assert_eq!(index.results("", HistoryFilter::Audio, Some(120)).len(), 1);
    }
    #[test]
    fn undecryptable_and_redacted_bodies_are_not_searchable() {
        let mut index = HistoryIndex::default();
        index.insert_value(message("$locked", "secret", 100), &BTreeMap::new(), true);
        let mut redacted = message("$redacted", "removed", 101);
        redacted["unsigned"]["redacted_because"] = serde_json::json!({});
        insert(&mut index, redacted);
        assert_eq!(index.locked, 1);
        assert!(index.results("", HistoryFilter::Messages, None).is_empty());
    }
}
