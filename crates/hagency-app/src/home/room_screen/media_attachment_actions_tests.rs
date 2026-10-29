//! Actual SDK timeline events and native rows, without a player or native export.
use super::*;
use makepad_widgets::makepad_draw::cx_draw::CxDraw;
use matrix_sdk_ui::timeline::RoomExt;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

struct Fixture {
    origin: String,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
    requests: Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

fn request_head(reader: &mut impl Read) -> std::io::Result<(String, String)> {
    let mut request = Vec::new();
    let mut bytes = [0; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        let remaining = 8192usize.saturating_sub(request.len());
        if remaining == 0 {
            return Err(std::io::Error::other("fixture request headers exceed their limit"));
        }
        let count = match reader.read(&mut bytes[..remaining.min(1024)]) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        request.extend_from_slice(&bytes[..count]);
    }
    let request = std::str::from_utf8(&request)
        .map_err(|_| std::io::Error::other("fixture request headers are not UTF-8"))?;
    let mut line = request.lines().next().unwrap_or_default().split_whitespace();
    let method = line.next().ok_or(std::io::ErrorKind::InvalidData)?;
    let path = line.next().ok_or(std::io::ErrorKind::InvalidData)?;
    let version = line.next().ok_or(std::io::ErrorKind::InvalidData)?;
    if !version.starts_with("HTTP/1.") || line.next().is_some() {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok((method.to_owned(), path.to_owned()))
}

fn sync_target(path: &str) -> bool {
    matches!(path.split('?').next(), Some("/_matrix/client/r0/sync" | "/_matrix/client/v3/sync"))
}

impl Fixture {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = requests.clone();
        let server = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let Ok((mut socket, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                // BSD accepted sockets can inherit O_NONBLOCK. A timeout does
                // not clear it; partial headers must never select a 404 route.
                socket.set_nonblocking(false).unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                socket.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                let Ok((method, path)) = request_head(&mut socket) else { continue; };
                {
                    let mut requests = observed.lock().unwrap();
                    if requests.len() == 32 { requests.remove(0); }
                    requests.push((method.clone(), path.clone()));
                }
                let (status, body) = if method == "GET" && sync_target(&path) {
                    (200, json!({"next_batch":"media-fixture", "rooms":{"join":{"!media:test":{
                        "state":{"events":[
                            {"type":"m.room.create","state_key":"","event_id":"$create:test","sender":"@alice:test","origin_server_ts":1,
                             "content":{"creator":"@alice:test","room_version":"10"}},
                            {"type":"m.room.member","state_key":"@alice:test","event_id":"$member:test","sender":"@alice:test","origin_server_ts":2,
                             "content":{"membership":"join","displayname":"Alice"}}
                        ]},
                        "timeline":{"events":events(),"limited":false}
                    }}}}))
                } else {
                    (404, json!({"errcode":"M_NOT_FOUND","error":"fixture has no other endpoint"}))
                };
                let body = body.to_string();
                let _ = write!(socket, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        Self { origin, stop, server: Some(server), requests }
    }

    async fn items(&self) -> Vector<Arc<TimelineItem>> {
        let client = matrix_sdk::Client::builder()
            .http_client(matrix_sdk::reqwest::Client::builder().no_proxy().build().unwrap())
            .homeserver_url(&self.origin)
            .server_versions([ruma::api::MatrixVersion::V1_0])
            .build().await.unwrap();
        client.matrix_auth().restore_session(serde_json::from_value(json!({
            "access_token":"media-widget-fixture", "user_id":"@alice:test", "device_id":"MEDIA"
        })).unwrap(), Default::default()).await.unwrap();
        client.encryption().wait_for_e2ee_initialization_tasks().await;
        client.event_cache().subscribe().unwrap();
        client.sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::ZERO)).await
            .unwrap_or_else(|error| panic!("actual SDK sync failed: {error}; complete fixture requests: {:?}", self.requests.lock().unwrap()));
        let room = client.get_room(ruma::room_id!("!media:test")).unwrap();
        let timeline = room.timeline().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let items = timeline.items().await;
                if items.iter().filter(|i| i.as_event().is_some_and(|e| e.event_id().is_some_and(|id| id.as_str().starts_with("$media-")))).count() == 5 {
                    break items;
                }
                tokio::task::yield_now().await;
            }
        }).await.expect("actual SDK timeline must contain every media fixture")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.server.take().unwrap().join().unwrap();
    }
}

fn content(kind: &str, encrypted: bool) -> Value {
    let ext = if kind == "audio" { "ogg" } else { "mp4" };
    let mut value = json!({"msgtype":format!("m.{kind}"), "body":format!("sample<{kind}>.{ext}"),
        "info":{"size":321,"duration":1200,"mimetype":format!("{kind}/{ext}")}});
    if encrypted {
        value["file"] = json!({"url":format!("mxc://media.test/{kind}-encrypted"),
            "key":{"kty":"oct","key_ops":["encrypt","decrypt"],"alg":"A256CTR","k":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","ext":true},
            "iv":"AAAAAAAAAAAAAAAAAAAAAA","hashes":{"sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},"v":"v2"});
    } else {
        value["url"] = json!(format!("mxc://media.test/{kind}-plain"));
    }
    value
}
fn events() -> Vec<Value> {
    let mut events = Vec::new();
    for kind in ["audio", "video"] {
        for encrypted in [false, true] {
            events.push(json!({"event_id":format!("$media-{kind}-{encrypted}:test"),"sender":"@alice:test",
                "origin_server_ts":1000 + events.len(),"type":"m.room.message","content":content(kind,encrypted)}));
        }
    }
    events.push(json!({"event_id":"$media-text:test","sender":"@alice:test","origin_server_ts":2000,
        "type":"m.room.message","content":{"msgtype":"m.text","body":"A recycled text row"}}));
    events
}
fn fixture_item(items: &Vector<Arc<TimelineItem>>, id: &str) -> Arc<TimelineItem> {
    items.iter().find(|item| item.as_event().is_some_and(|event| event.event_id().is_some_and(|event_id| event_id.as_str() == id))).unwrap().clone()
}
fn list() -> (Cx, PortalList) {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let list = cx.with_vm(|vm| {
        makepad_widgets::script_mod(vm);
        crate::app::register_widgets(vm);
        vm.bx.captured_errors = Some(Vec::new());
        let value = script_eval!(vm, { mod.widgets.PortalList {
            Message := mod.widgets.Message {}
            CondensedMessage := mod.widgets.CondensedMessage {}
            Empty := mod.widgets.Empty {}
        }});
        let list = PortalList::script_from_value(vm, value);
        assert!(vm.take_errors().is_empty());
        list
    });
    (cx, list)
}

/// Paint through the same timeline conversion used by RoomScreen, including the
/// attachment construction and Message::set_data, rather than seeding controls.
fn paint(cx: &mut Cx, list: &mut PortalList, item: &Arc<TimelineItem>, condensed: bool, width: f64) -> WidgetRef {
    cx.redraw_all();
    let pass = DrawPass::new(cx);
    pass.set_size(cx, dvec2(width, 900.0));
    let mut draw_list = DrawList2d::new(cx);
    let event = DrawEvent::default();
    let mut draw = CxDraw::new(cx, &event);
    let mut cx2d = Cx2d::new(&mut draw);
    cx2d.begin_pass(&pass, None);
    draw_list.begin_always(&mut cx2d);
    cx2d.begin_root_turtle(dvec2(width, 900.0), Layout::flow_overlay());
    list.set_item_range(&mut cx2d, 0, 1);
    let mut result = WidgetRef::empty();
    while list.draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fixed(width, 850.0)).step().is_some() {
        while let Some(index) = list.next_visible_item(&mut cx2d) {
            // PortalList also asks for an empty sentinel after the range. Match
            // RoomScreen's real bounds check; never paint a duplicate event in
            // that sentinel or return an offscreen duplicate as the visible row.
            if index != 0 {
                list.item(&mut cx2d, index, id!(Empty));
                continue;
            }
            let event = item.as_event().unwrap();
            let TimelineItemContent::MsgLike(message) = event.content() else { panic!("message fixture"); };
            let (row, _) = populate_message_view(
                &mut cx2d, list, index, &TimelineKind::MainRoom { room_id: ruma::room_id!("!media:test").to_owned() },
                event, message, condensed.then_some(item), &mut MediaCache::new(None), &mut LinkPreviewCache::new(None),
                &HashMap::new(), &mut HashSet::new(), &UserPowerLevels::empty(), &[], &[], &HashSet::new(),
                #[cfg(feature = "agent_chat")]
                &mut ApprovalUiState::default(),
                false, false, false, ItemDrawnStatus::default(), WidgetUid(9001),
            );
            row.draw_all(&mut cx2d, &mut Scope::empty());
            result = row;
        }
    }
    cx2d.end_pass_sized_turtle();
    draw_list.end(&mut cx2d);
    cx2d.end_pass(&pass);
    result
}

fn click(cx: &mut Cx, row: &mut WidgetRef, point: DVec2) -> ActionsBuf {
    let mut output = ActionsBuf::default();
    for event in [
        Event::MouseDown(MouseDownEvent { abs: point, button: MouseButton::PRIMARY, window_id: WindowId(0, 0),
            modifiers: KeyModifiers::default(), handled: std::cell::Cell::new(Area::Empty), time: 1.0 }),
        Event::MouseUp(MouseUpEvent { abs: point, button: MouseButton::PRIMARY, window_id: WindowId(0, 0),
            modifiers: KeyModifiers::default(), time: 1.1 }),
    ] {
        let actions = cx.capture_actions(|cx| row.handle_event(cx, &event, &mut Scope::empty()));
        let forwarded = cx.capture_actions(|cx| row.handle_event(cx, &Event::Actions(actions), &mut Scope::empty()));
        output.extend(forwarded);
    }
    output
}

fn selected_attachment(actions: &Actions, share: bool) -> DownloadableAttachment {
    let attachments: Vec<_> = actions.iter().filter_map(|action| match action.as_widget_action().cast_ref::<MessageAction>() {
        MessageAction::DownloadAttachment(info) if !share => Some((*info).clone()),
        MessageAction::ShareAttachment(info) if share => Some((*info).clone()),
        _ => None,
    }).collect();
    assert_eq!(attachments.len(), 1, "one actual button click must select exactly one attachment");
    attachments.into_iter().next().unwrap()
}

#[tokio::test]
async fn media_attachment_actions_visible_plain_and_encrypted_rows_select_the_complete_original_source() {
    let fixture = Fixture::start();
    let items = fixture.items().await;
    for kind in ["audio", "video"] {
        for encrypted in [false, true] {
            let item = fixture_item(&items, &format!("$media-{kind}-{encrypted}:test"));
            for condensed in [false, true] {
                for share in [false, true] {
                    let (mut cx, mut list) = list();
                    let width = if condensed { 375.0 } else { 1100.0 };
                    let mut row = paint(&mut cx, &mut list, &item, condensed, width);
                    let html = row.html(&cx, ids!(content.message.html_view.html)).text();
                    assert!(html.contains(&format!("In-app {kind} playback is unavailable.")), "{html}");
                    assert!(html.contains("Download to play in another app, or Share the file."), "{html}");
                    assert!(html.contains(&format!("sample&lt;{kind}&gt;")), "filenames must stay escaped: {html}");
                    if kind == "audio" { assert!(!html.contains("Video playback")); }
                    assert!(row.view(&cx, ids!(content.download_section)).visible());
                    let button = row.button(&cx, if share { ids!(content.download_section.share_button) } else { ids!(content.download_section.download_button) });
                    assert!(button.visible());
                    let bounds = button.area().clipped_rect(&cx);
                    assert!(bounds.size.x > 40.0 && bounds.size.y > 20.0 && bounds.pos.x >= 0.0 && bounds.pos.x + bounds.size.x <= width + 0.5, "visible media action must fit: {bounds:?}");
                    let actions = click(&mut cx, &mut row, bounds.pos + bounds.size * 0.5);
                    let attachment = selected_attachment(&actions, share);
                    let expected: RoomMessageEventContent = serde_json::from_value(content(kind, encrypted)).unwrap();
                    let expected_source = match expected.msgtype { MessageType::Audio(audio) => audio.source, MessageType::Video(video) => video.source, _ => unreachable!() };
                    assert_eq!(serde_json::to_value(&attachment.media_source).unwrap(), serde_json::to_value(expected_source).unwrap(), "encrypted keys, IV and hashes must never be reduced to a raw MXC URL");
                    assert_eq!(attachment.filename, format!("sample<{kind}>.{}", if kind == "audio" { "ogg" } else { "mp4" }));
                    assert_eq!(attachment.size, Some(321));
                    assert_eq!(attachment.kind, if kind == "audio" { DownloadKind::Audio } else { DownloadKind::Video });
                    assert_eq!(actions.iter().filter(|action| matches!(action.as_widget_action().cast_ref::<MessageAction>(), MessageAction::DownloadAttachment(_) | MessageAction::ShareAttachment(_))).count(), 1);
                    cx.with_vm(|vm| assert!(vm.take_errors().is_empty()));
                }
            }
        }
    }
}

#[tokio::test]
async fn media_attachment_actions_recycled_text_row_hides_controls_and_rejects_old_queued_clicks() {
    let fixture = Fixture::start();
    let items = fixture.items().await;
    let text = fixture_item(&items, "$media-text:test");
    for kind in ["audio", "video"] {
        for encrypted in [false, true] {
            let item = fixture_item(&items, &format!("$media-{kind}-{encrypted}:test"));
            let (mut cx, mut list) = list();
            let row = paint(&mut cx, &mut list, &item, false, 1100.0);
            let uid = row.widget_uid();
            let stale_clicks = cx.capture_actions(|cx| {
                for button in [ids!(content.download_section.download_button), ids!(content.download_section.share_button)] {
                    cx.widget_action(row.button(cx, button).widget_uid(), ButtonAction::Clicked(KeyModifiers::default()));
                }
            });
            let recycled = paint(&mut cx, &mut list, &text, false, 1100.0);
            assert_eq!(recycled.widget_uid(), uid, "test must reuse the same actual PortalList row");
            assert!(!recycled.view(&cx, ids!(content.download_section)).visible());
            assert!(recycled.as_message().borrow().unwrap().download_info.is_none());
            let actions = cx.capture_actions(|cx| recycled.handle_event(cx, &Event::Actions(stale_clicks), &mut Scope::empty()));
            assert!(!actions.iter().any(|action| matches!(action.as_widget_action().cast_ref::<MessageAction>(), MessageAction::DownloadAttachment(_) | MessageAction::ShareAttachment(_))), "queued clicks from a recycled attachment must not export its old source");
        }
    }
}

#[test]
fn media_attachment_actions_fixture_routes_only_complete_fragmented_headers() {
    struct Fragments {
        chunks: std::collections::VecDeque<std::io::Result<Vec<u8>>>,
    }
    impl Read for Fragments {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let Some(chunk) = self.chunks.pop_front() else { return Ok(0); };
            let chunk = chunk?;
            assert!(chunk.len() <= bytes.len());
            bytes[..chunk.len()].copy_from_slice(&chunk);
            Ok(chunk.len())
        }
    }
    let mut fragmented = Fragments { chunks: [
        Err(std::io::ErrorKind::Interrupted.into()),
        Ok(b"GET ".to_vec()),
        Ok(b"/_matrix/client/r0/sync HTTP/1.1\r\nHost: localhost\r\n".to_vec()),
        Ok(b"\r\n".to_vec()),
    ].into() };
    let (method, path) = request_head(&mut fragmented).unwrap();
    assert_eq!(method, "GET");
    assert!(sync_target(&path));
    assert!(sync_target("/_matrix/client/v3/sync?timeout=0"));
    assert!(!sync_target("/_matrix/client/v3/room_keys/version"));

    let mut would_block = Fragments { chunks: [
        Ok(b"GET ".to_vec()),
        Err(std::io::ErrorKind::WouldBlock.into()),
    ].into() };
    assert_eq!(request_head(&mut would_block).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    let mut truncated = std::io::Cursor::new(b"GET /_matrix/client/r0/sync HTTP/1.1\r\n");
    assert_eq!(request_head(&mut truncated).unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn media_attachment_actions_guidance_translations_preserve_both_operations_and_media_type() {
    let english: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("../../../resources/i18n/en.json")).unwrap();
    for kind in ["audio", "video"] {
        let key = english.keys().find(|key| key.contains(&format!("In-app {kind} playback is unavailable."))).unwrap();
        let chinese = crate::i18n::text_for(crate::i18n::Language::Chinese, key);
        assert_ne!(chinese, key, "guidance must be translated");
        assert!(chinese.contains(if kind == "audio" { "音频播放" } else { "视频播放" }));
        assert!(chinese.contains("下载") && chinese.contains("分享") && chinese.contains("其他应用播放"));
        if kind == "audio" { assert!(!chinese.contains("视频播放")); }
        assert_eq!(crate::i18n::text_for(crate::i18n::Language::English, key), key);
    }
}
