//! Authenticated, bounded discovery of the owner's private approval rooms.
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use makepad_widgets::*;
use matrix_sdk::{
    RoomState,
    ruma::{OwnedRoomId, OwnedUserId},
};
use futures_util::{stream, StreamExt};
use super::{
    approval::{Namespace, current_unix_time_millis},
    approval_state::ApprovalMarkerIndex,
};
use crate::{
    sliding_sync::{get_client, current_user_id, spawn_async_task},
    utils::RoomNameId,
};

type MarkerIndexes = HashMap<&'static str, ApprovalMarkerIndex>;

#[derive(Clone, Debug)]
pub enum ApprovalInboxAction {
    Open { project: Option<OwnedRoomId> },
    Close,
    Saved,
}
#[derive(Clone, Debug)]
struct Candidate {
    room: RoomNameId,
    agents: String,
}
#[derive(Debug)]
struct Discovered {
    owner: OwnedUserId,
    request: u64,
    candidates: Vec<Candidate>,
    failed: usize,
    scanned: usize,
    indexes: MarkerIndexes,
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    mod.widgets.ApprovalInbox = #(ApprovalInbox::register_widget(vm)) {
        ..mod.widgets.SolidView
        width: Fill height: Fill flow: Down spacing: 16 padding: Inset{top: 20 left: 24 right: 24 bottom: 20} draw_bg.color: mod.widgets.RINX_PAGE
        View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 12 align: Align{y: 0.5}
            close := RinxBackButton {}
            RinxPageTitle {text: #(crate::i18n::tr("Agent approval chats")) i18n_text: "Agent approval chats"}
        }
        explanation := RinxLabel {width: Fill height: Fit flow: Flow.Right{wrap: true} draw_text.color: mod.widgets.RINX_MUTED text: #(crate::i18n::tr("Your Agents use private approval chats to ask for permission before carrying out actions that require your approval. Open a chat to review and respond. These chats belong to your account, across Rooms and Projects. This page lists chats, not pending requests.")) i18n_text: "Your Agents use private approval chats to ask for permission before carrying out actions that require your approval. Open a chat to review and respond. These chats belong to your account, across Rooms and Projects. This page lists chats, not pending requests."}
        refresh := RinxButton {text: #(crate::i18n::tr("Refresh approval chats")) i18n_text: "Refresh approval chats"}
        status := RinxLabel {width: Fill height: Fit flow: Flow.Right{wrap: true} draw_text.color: mod.widgets.RINX_INK}
        empty_state := RoundedView {
            visible: false width: Fill{max: 880} height: Fit flow: Down padding: 20 spacing: 12
            draw_bg +: {color: mod.widgets.RINX_SURFACE border_color: mod.widgets.RINX_BORDER border_size: 1 border_radius: mod.widgets.RINX_CONTROL_RADIUS}
            empty_title := RinxLabel {width: Fill height: Fit flow: Flow.Right{wrap: true} text: #(crate::i18n::tr("No approval chats found")) i18n_text: "No approval chats found" draw_text.text_style: theme.font_bold {font_size: mod.widgets.RINX_HEADING_SIZE}}
            empty_description := RinxLabel {width: Fill height: Fit flow: Flow.Right{wrap: true} draw_text.color: mod.widgets.RINX_MUTED}
            empty_help := RinxHint {width: Fill height: Fit flow: Flow.Right{wrap: true} text: #(crate::i18n::tr("Only private approval chats you have joined and that are verified for your account appear here. Ordinary group chats do not appear. If you receive an invitation to an Agent approval chat, join it and refresh.")) i18n_text: "Only private approval chats you have joined and that are verified for your account appear here. Ordinary group chats do not appear. If you receive an invitation to an Agent approval chat, join it and refresh."}
        }
        rooms := PortalList {width: Fill height: Fill
            Row := View {width: Fill height: Fit flow: Down padding: 8 spacing: 8
                title := RinxLabel {width: Fill height: Fit flow: Flow.Right{wrap: true} draw_text.color: mod.widgets.RINX_INK}
                detail := RinxLabel {width: Fill height: Fit flow: Flow.Right{wrap: true} draw_text.color: mod.widgets.RINX_MUTED}
                open := RobrixNeutralIconButton {text: #(crate::i18n::tr("Open approvals")) i18n_text: "Open approvals"}
            }
        }
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct ApprovalInbox {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    owner: Option<OwnedUserId>,
    #[rust]
    request: u64,
    #[rust]
    active: bool,
    #[rust]
    project: Option<OwnedRoomId>,
    #[rust]
    cancelled: Option<Arc<AtomicBool>>,
    #[rust]
    candidates: Vec<Candidate>,
    #[rust]
    marker_client: Option<matrix_sdk::Client>,
    #[rust]
    indexes: MarkerIndexes,
    #[rust]
    validated_at: u64,
    #[rust]
    summary: Option<(usize, usize)>,
    #[rust]
    loading: bool,
}
fn nonce() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
impl ApprovalInbox {
    fn sync_status(&mut self, cx: &mut Cx) {
        let status = if self.loading {
            crate::i18n::tr("Checking approval chats…").to_owned()
        } else if let Some((scanned, failed)) = self.summary {
            crate::i18n::format(
                "Found {count} approval chats. Checked {scanned} rooms; {failed} checks unavailable.",
                &[("count", self.candidates.len().to_string()), ("scanned", scanned.to_string()), ("failed", failed.to_string())],
            )
        } else {
            crate::i18n::tr("Sign in to check your approval chats.").to_owned()
        };
        self.view.label(cx, ids!(status)).set_text(cx, &status);
        self.view.button(cx, ids!(refresh)).set_text(cx, crate::i18n::tr(if self.loading {"Checking approval chats…"} else {"Refresh approval chats"}));
        self.view.button(cx, ids!(refresh)).set_enabled(cx, !self.loading && self.owner.is_some());
        let empty = !self.loading && self.summary.is_some() && self.candidates.is_empty();
        self.view.view(cx, ids!(empty_state)).set_visible(cx, empty);
        self.view.portal_list(cx, ids!(rooms)).set_visible(cx, !self.candidates.is_empty());
        let incomplete = self.summary.is_some_and(|(_, failed)| failed > 0);
        self.view.label(cx, ids!(empty_title)).set_text(cx, crate::i18n::tr(if incomplete {"No verified approval chats available"} else {"No approval chats found"}));
        self.view.label(cx, ids!(empty_description)).set_text(cx, crate::i18n::tr(if incomplete {
            "Some rooms could not be checked. Refresh to try again; unchecked rooms may contain approvals."
        } else if self.project.is_some() {
            "No verified approval chats were found for this project. This does not mean an Agent failed to start."
        } else {
            "You have not joined any verified approval chats yet. This does not mean an Agent failed to start."
        }));
        self.view.redraw(cx);
    }
    fn cancel(&mut self) {
        if let Some(cancelled) = self.cancelled.take() {
            cancelled.store(true, Ordering::Relaxed);
        }
    }
    fn load(&mut self, cx: &mut Cx) {
        self.cancel();
        self.candidates.clear();
        self.request = nonce();
        self.summary = None;
        self.loading = false;
        self.owner = current_user_id();
        let Some(client) = get_client() else { self.sync_status(cx); return };
        let Some(owner) = client.user_id().map(ToOwned::to_owned) else {
            return;
        };
        let request = self.request;
        self.owner = Some(owner.clone());
        if !self
            .marker_client
            .as_ref()
            .is_some_and(crate::matrix_context::is_current)
        {
            self.indexes.clear();
        }
        self.marker_client = Some(client.clone());
        let mut indexes = self.indexes.clone();
        for index in indexes.values_mut() {
            index.begin_revalidation();
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancelled = Some(cancelled.clone());
        let project = self.project.clone();
        self.loading = true;
        self.sync_status(cx);
        spawn_async_task(async move {
            let rooms: Vec<_> = client
                .rooms()
                .into_iter()
                .filter(|r| r.state() == RoomState::Joined && !r.is_space())
                .collect();
            let joined: HashSet<_> = rooms.iter().map(|r| r.room_id().to_string()).collect();
            for index in indexes.values_mut() {
                index.retain_joined(owner.as_str(), &joined);
            }
            let overflow = rooms.len() > 4096;
            let mut failed = usize::from(overflow);
            let mut scanned = 0;
            let started = tokio::time::Instant::now();
            let mut responses=stream::iter(rooms.into_iter().take(4096).map(|room| {
                let client=client.clone(); let cancelled=cancelled.clone();
                async move {
                    if cancelled.load(Ordering::Relaxed) || !crate::matrix_context::is_current(&client) {return (room,None);}
                    let response=tokio::time::timeout(Duration::from_secs(10), client.send(matrix_sdk::ruma::api::client::state::get_state_events::v3::Request::new(room.room_id().to_owned())).with_request_config(matrix_sdk::config::RequestConfig::default().retry_limit(0))).await;
                    (room,response.ok().and_then(Result::ok))
                }
            })).buffer_unordered(2);
            loop {
                let next =
                    tokio::time::timeout_at(started + Duration::from_secs(60), responses.next())
                        .await;
                let (room, response) = match next {
                    Ok(Some(v)) => v,
                    Ok(None) => break,
                    Err(_) => {
                        failed += 1;
                        break;
                    }
                };
                if cancelled.load(Ordering::Relaxed) || !crate::matrix_context::is_current(&client)
                {
                    return;
                }
                scanned += 1;
                let Some(response) = response else {
                    failed += 1;
                    continue;
                };
                if room.state() != RoomState::Joined {
                    continue;
                }
                for namespace in Namespace::ALL {
                    let selection = super::marker_selection::select_approval_marker_raw_state(
                        response.room_state.iter(),
                        namespace,
                    );
                    let index = indexes.entry(namespace.base()).or_default();
                    if selection.observed_v2 {
                        let _ = index.observe_v2(owner.as_str(), room.room_id().as_str());
                    }
                    match selection.marker {
                        Ok(Some(event)) => {
                            let text = |key| {
                                event
                                    .get(key)
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or_default()
                            };
                            if index
                                .ingest_marker(
                                    owner.as_str(),
                                    room.room_id().as_str(),
                                    text("event_id"),
                                    text("type"),
                                    text("sender"),
                                    text("state_key"),
                                    &event["content"],
                                    current_unix_time_millis(),
                                )
                                .is_err()
                            {
                                failed += 1;
                            }
                        }
                        Err(()) => {
                            failed += 1;
                        }
                        Ok(None) => {}
                    }
                }
            }
            if cancelled.load(Ordering::Relaxed) || !crate::matrix_context::is_current(&client) {
                return;
            }
            let mut by_room: HashMap<String, Vec<String>> = HashMap::new();
            for index in indexes.values() {
                for (room, agent, project_id) in index.candidates(
                    owner.as_str(),
                    project.as_deref().map(|p| p.as_str()),
                    &joined,
                    current_unix_time_millis(),
                ) {
                    by_room
                        .entry(room)
                        .or_default()
                        .push(format!("{agent} · {project_id}"));
                }
            }
            let mut candidates = Vec::new();
            for (id, mut agents) in by_room {
                if let Ok(id) = OwnedRoomId::try_from(id)
                    && let Some(room) = client.get_room(&id)
                    && room.state() == RoomState::Joined
                {
                    agents.sort();
                    agents.dedup();
                    candidates.push(Candidate {
                        room: RoomNameId::new(
                            matrix_sdk::RoomDisplayName::Named(
                                room.name().unwrap_or_else(|| id.to_string()),
                            ),
                            id,
                        ),
                        agents: agents.join("\n"),
                    });
                }
            }
            candidates.sort_by_cached_key(|c| c.room.display().to_lowercase());
            Cx::post_action(Discovered {
                owner,
                request,
                candidates,
                failed,
                scanned,
                indexes,
            });
        });
    }
}
impl Widget for ApprovalInbox {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::ScriptReapply | Event::LiveEdit) && self.active {
            self.sync_status(cx);
        }
        if !self.active {
            return;
        }
        if current_user_id() != self.owner {
            self.cancel();
            cx.action(ApprovalInboxAction::Close);
            return;
        }
        if matches!(event, Event::KeyUp(key) if key.key_code == KeyCode::Escape) {
            cx.action(ApprovalInboxAction::Close);
            return;
        }
        let Event::Actions(actions) = event else {
            return;
        };
        if self.view.button(cx, ids!(close)).clicked(actions) {
            cx.action(ApprovalInboxAction::Close);
        }
        if self.view.button(cx, ids!(refresh)).clicked(actions) {
            self.load(cx);
        }
        for action in actions {
            if let Some(result) = action.downcast_ref::<Discovered>()
                && self.owner.as_ref() == Some(&result.owner)
                && self.request == result.request
            {
                self.candidates = result.candidates.clone();
                self.indexes = result.indexes.clone();
                self.validated_at = current_unix_time_millis();
                if let Some(app) = scope.data.get_mut::<crate::app::AppState>() {
                    app.approval_markers = self
                        .indexes
                        .get(Namespace::DEFAULT.base())
                        .cloned()
                        .unwrap_or_default();
                    app.approval_markers_by_namespace = self
                        .indexes
                        .iter()
                        .filter(|(ns, _)| **ns != Namespace::DEFAULT.base())
                        .map(|(ns, index)| (ns.to_string(), index.clone()))
                        .collect();
                    cx.action(ApprovalInboxAction::Saved);
                }
                self.summary = Some((result.scanned, result.failed));
                self.loading = false;
                self.sync_status(cx);
            }
        }
        for (index, item) in self
            .view
            .portal_list(cx, ids!(rooms))
            .items_with_actions(actions)
        {
            if item.button(cx, ids!(open)).clicked(actions)
                && let Some(candidate) = self.candidates.get(index)
            {
                if current_unix_time_millis().saturating_sub(self.validated_at)
                    >= super::approval_state::MARKER_FRESHNESS_MS
                    || !self
                        .marker_client
                        .as_ref()
                        .is_some_and(crate::matrix_context::is_current)
                {
                    self.load(cx);
                    return;
                }
                if get_client()
                    .and_then(|c| c.get_room(candidate.room.room_id()))
                    .is_some_and(|r| r.state() == RoomState::Joined)
                {
                    cx.action(ApprovalInboxAction::Close);
                    cx.action(crate::app::AppStateAction::NavigateToRoom {
                        room_to_close: None,
                        destination_room: crate::room::BasicRoomDetails::RoomId(
                            candidate.room.clone(),
                        ),
                    });
                }
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, self.candidates.len());
                while let Some(index) = list.next_visible_item(cx) {
                    if let Some(candidate) = self.candidates.get(index) {
                        let item = list.item(cx, index, id!(Row));
                        item.label(cx, ids!(title))
                            .set_text(cx, &candidate.room.display());
                        item.label(cx, ids!(detail)).set_text(
                            cx,
                            &format!("{}\n{}", candidate.room.room_id(), candidate.agents),
                        );
                        item.draw_all(cx, scope);
                    }
                }
            }
        }
        DrawStep::done()
    }
}
impl ApprovalInboxRef {
    pub fn action(
        &self,
        cx: &mut Cx,
        modal: ModalRef,
        action: &ApprovalInboxAction,
        app: &crate::app::AppState,
    ) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        match action {
            ApprovalInboxAction::Open { project } => {
                inner.indexes = Namespace::ALL
                    .into_iter()
                    .map(|ns| {
                        (
                            ns.base(),
                            if ns == Namespace::DEFAULT {
                                app.approval_markers.clone()
                            } else {
                                app.approval_markers_by_namespace
                                    .get(ns.base())
                                    .cloned()
                                    .unwrap_or_default()
                            },
                        )
                    })
                    .collect();
                inner.marker_client = get_client();
                inner.active = true;
                inner.project = project.clone();
                inner.load(cx);
                modal.open(cx);
            }
            ApprovalInboxAction::Close => {
                inner.active = false;
                inner.cancel();
                inner.marker_client = None;
                inner.indexes.clear();
                inner.candidates.clear();
                inner.loading = false;
                inner.summary = None;
                inner.request = nonce();
                modal.close(cx);
            }
            ApprovalInboxAction::Saved => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_partial_and_loading_states_explain_what_was_checked() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut inbox = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.ApprovalInbox {}});
            let inbox = ApprovalInbox::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            inbox
        });
        inbox.owner = Some(matrix_sdk::ruma::user_id!("@owner:example.org").to_owned());
        inbox.summary = Some((5, 0));
        inbox.sync_status(&mut cx);
        assert!(inbox.view.view(&cx, ids!(empty_state)).visible());
        assert_eq!(inbox.view.label(&cx, ids!(empty_title)).text(), "No approval chats found");
        assert!(inbox.view.label(&cx, ids!(empty_description)).text().contains("does not mean an Agent failed to start"));
        inbox.summary = Some((5, 2));
        inbox.sync_status(&mut cx);
        assert_eq!(inbox.view.label(&cx, ids!(empty_title)).text(), "No verified approval chats available");
        assert!(inbox.view.label(&cx, ids!(empty_description)).text().contains("unchecked rooms may contain approvals"));
        inbox.loading = true;
        inbox.sync_status(&mut cx);
        assert!(!inbox.view.view(&cx, ids!(empty_state)).visible());
        assert_eq!(inbox.view.button(&cx, ids!(refresh)).text(), "Checking approval chats…");
        inbox.loading = false;
        inbox.summary = Some((5, 0));
        inbox.sync_status(&mut cx);
        {
            use makepad_widgets::makepad_draw::cx_draw::CxDraw;
            let pass = DrawPass::new(&mut cx);
            pass.set_size(&mut cx, dvec2(310.0, 760.0));
            let mut list = DrawList2d::new(&mut cx);
            let overlay = Overlay {draw_list: DrawList::new(&mut cx)};
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            list.begin_always(&mut cx2d);
            overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(dvec2(310.0, 760.0), Layout::flow_overlay());
            inbox.draw_walk_all(&mut cx2d, &mut Scope::empty(), Walk::fill());
            cx2d.end_pass_sized_turtle();
            overlay.end(&mut cx2d);
            list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }
        let help = inbox.view.label(&cx, ids!(empty_help)).area().rect(&cx);
        let explanation = inbox.view.label(&cx, ids!(explanation)).area().rect(&cx);
        assert!(explanation.size.y > 30.0 && explanation.pos.x + explanation.size.x <= 310.0,
            "account-wide approval guidance must wrap within the page: {explanation:?}");
        assert!(help.size.y > 30.0, "long empty-state guidance must wrap on narrow pages: {help:?}");
        assert!(help.pos.x + help.size.x <= 310.0);
        let cancelled = Arc::new(AtomicBool::new(false));
        inbox.cancelled = Some(cancelled.clone());
        inbox.cancel();
        assert!(cancelled.load(Ordering::Relaxed));
    }
}
