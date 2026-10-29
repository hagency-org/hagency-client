/// Isolated test process only: install no sync/owner/bootstrap side effects.
pub(crate) fn install_link_client(client: Option<Client>) {
    *super::CLIENT.lock().unwrap() = client;
}

use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

fn event(id: &str, timestamp: u64) -> serde_json::Value {
    serde_json::json!({"event_id":id,"sender":"@alice:example.org","origin_server_ts":timestamp,
        "type":"m.room.message","content":{"msgtype":"m.text","body":id}})
}

pub(crate) type ResponseHandler = dyn Fn(&str) -> Option<(u16, serde_json::Value)> + Send + Sync;

pub(crate) struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    pub(crate) requests: Arc<Mutex<Vec<String>>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start(context_available: bool) -> Self {
        Self::with_handler(context_available, Arc::new(|_| None))
    }
    pub(crate) fn with_handler(context_available: bool, handler: Arc<ResponseHandler>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let done = stop.clone();
        let paths = requests.clone();
        let thread = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let Ok((mut socket, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 8192];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let Ok(length) = socket.read(&mut buffer) else {
                        break;
                    };
                    if length == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..length]);
                }
                let request = String::from_utf8_lossy(&request);
                let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                paths.lock().unwrap().push(path.clone());
                let (status, body) = if let Some(response) = handler(&path) {
                    response
                } else if path.contains("/sync?") {
                    (
                        200,
                        serde_json::json!({"next_batch":"s1","rooms":{"join":{"!room:example.org":{
                            "state":{"events":[{"type":"m.room.create","state_key":"","event_id":"$create",
                                "sender":"@alice:example.org","origin_server_ts":1,"content":{"creator":"@alice:example.org","room_version":"10"}},
                                {"type":"m.room.member","state_key":"@alice:example.org","event_id":"$member","sender":"@alice:example.org",
                                "origin_server_ts":2,"content":{"membership":"join"}}]},
                            "timeline":{"events":[event("$recent",1_000_000)],"limited":false,"prev_batch":"older"}
                        }}}}),
                    )
                } else if path.contains("/context/") && context_available {
                    (
                        200,
                        serde_json::json!({"event":event("$ancient",100),"events_before":[event("$before",99)],
                        "events_after":[event("$after",101)],"state":[],"start":"ctx-start","end":"ctx-end"}),
                    )
                } else if path.contains("/members") {
                    (200, serde_json::json!({"chunk":[]}))
                } else {
                    (
                        404,
                        serde_json::json!({"errcode":"M_NOT_FOUND","error":"Unavailable"}),
                    )
                };
                let body = body.to_string();
                let _ = write!(
                    socket,
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            url,
            stop,
            requests,
            thread: Some(thread),
        }
    }
    pub(crate) async fn room(&self) -> Room {
        self.room_with_store(None).await
    }
    pub(crate) async fn room_for_links(&self) -> Room {
        self.room_configured(None,true).await
    }
    async fn room_with_store(&self, store: Option<&std::path::Path>) -> Room {
        self.room_configured(store,false).await
    }
    async fn room_configured(&self, store: Option<&std::path::Path>, threading: bool) -> Room {
        let builder = Client::builder()
            .http_client(
                matrix_sdk::reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap(),
            )
            .homeserver_url(&self.url)
            .server_versions([matrix_sdk::ruma::api::MatrixVersion::V1_0]);
        let builder = if threading {
            builder.with_threading_support(matrix_sdk::ThreadingSupport::Enabled {with_subscriptions:false})
                .server_versions([matrix_sdk::ruma::api::MatrixVersion::V1_11])
        } else {builder};
        let builder = if let Some(store) = store {
            builder.sqlite_store(store, Some("fixture-passphrase"))
        } else {
            builder
        };
        let client = builder.build().await.unwrap();
        client
            .matrix_auth()
            .restore_session(
                serde_json::from_value(serde_json::json!({
                    "access_token":"fixture","user_id":"@alice:example.org","device_id":"HISTORY"
                }))
                .unwrap(),
                Default::default(),
            )
            .await
            .unwrap();
        client.event_cache().subscribe().unwrap();
        client
            .sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::ZERO))
            .await
            .unwrap();
        let room_id = ruma::room_id!("!room:example.org");
        let (cache, _handles) = client.event_cache().room(room_id).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if cache
                    .events()
                    .await
                    .unwrap()
                    .iter()
                    .any(|event| event.event_id() == Some(ruma::event_id!("$recent")))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        client.get_room(room_id).unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

#[tokio::test]
async fn sdk_sqlite_raw_decoded_and_chunk_metadata_stay_bounded_through_live_rollovers() {
    let sequence = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let next = sequence.clone();
    let server = Server::with_handler(
        true,
        Arc::new(move |path| {
            if !path.contains("/sync?") {
                return None;
            }
            let batch = next.fetch_add(1, Ordering::Relaxed);
            if batch == 0 {
                return None;
            }
            let events: Vec<_> = (batch * 100..batch * 100 + 100)
                .map(|i| {
                    let mut e = event(&format!("$live{i}"), 1_000_000 + i);
                    e["sender"] = "@bob:example.org".into();
                    e
                })
                .collect();
            Some((
                200,
                serde_json::json!({"next_batch":format!("roll{batch}"),"rooms":{"join":{"!room:example.org":{
                    "timeline":{"events":events,"limited":false},
                    "account_data":{"events":if batch==1 {vec![serde_json::json!({"type":"m.fully_read","content":{"event_id":"$live650"}})]}else{vec![]}}
                }}}}),
            ))
        }),
    );
    let directory = std::env::temp_dir().join(format!("sdk-history-{}", uuid::Uuid::new_v4()));
    if cfg!(windows) {
        assert!(
            directory
                .to_string_lossy()
                .replace('\\', "/")
                .starts_with("D:/")
        );
    }
    std::fs::create_dir_all(&directory).unwrap();
    let room = server.room_with_store(Some(&directory)).await;
    let client = room.client();
    let timeline = room
        .timeline_builder()
        .track_read_marker_and_receipts(TimelineReadReceiptTracking::AllEvents)
        .build()
        .await
        .unwrap();
    let (cache, _handles) = client.event_cache().room(room.room_id()).await.unwrap();
    // 12,000 actual sync events and many compactions, using the real SQLite
    // store, SDK controller, local HTTP transport and cache locks.
    for batch in 1..=120 {
        client
            .sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::ZERO))
            .await
            .unwrap();
        let last: OwnedEventId = format!("$live{}", batch * 100 + 99).try_into().unwrap();
        let finished = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if timeline.latest_event_id().await.as_ref() == Some(&last) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        if finished.is_err() {
            panic!(
                "batch {batch}: latest {:?}; raw {:?}",
                timeline.latest_event_id().await,
                cache
                    .events()
                    .await
                    .unwrap()
                    .iter()
                    .filter_map(|e| e.event_id())
                    .collect::<Vec<_>>()
            );
        }
        let loaded = cache.events().await.unwrap();
        assert!(loaded.len() <= 512, "raw {}", loaded.len());
        assert!(
            loaded
                .iter()
                .map(|e| e.raw().json().get().len())
                .sum::<usize>()
                <= 8 * 1024 * 1024
        );
        assert!(
            timeline.items().await.len() <= 1025,
            "decoded vector grew beyond bounded raw window plus decorations"
        );
        if batch == 6 {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if timeline.items().await.iter().any(|item| {
                        matches!(
                            item.as_virtual(),
                            Some(matrix_sdk_ui::timeline::VirtualTimelineItem::ReadMarker)
                        )
                    }) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("compaction lost fully-read marker metadata");
        }

        if batch % 20 == 0 {
            let lock = client.event_cache_store().lock().await.unwrap();
            let guard = lock.as_clean().unwrap();
            let chunks = guard
                .load_all_chunks_metadata(matrix_sdk_base::linked_chunk::LinkedChunkId::Room(
                    room.room_id(),
                ))
                .await
                .unwrap();
            assert!(chunks.len() <= 8, "{} retained chunk links", chunks.len());
            let stored = guard
                .get_room_events(room.room_id(), None, None)
                .await
                .unwrap();
            assert!(
                stored.len() <= 576,
                "detached payloads leaked: {}",
                stored.len()
            );
        }
    }
}

#[tokio::test]
async fn rebasing_the_same_event_uses_a_fresh_context_and_failed_contexts_are_reclaimable() {
    let server = Server::start(true);
    let room = server.room().await;
    let kind = TimelineKind::MainRoom {
        room_id: room.room_id().to_owned(),
    };
    let first = build_focused_timeline(&room, &kind, &Some(ruma::event_id!("$ancient").into()))
        .await
        .unwrap();
    let second = build_focused_timeline(&room, &kind, &Some(ruma::event_id!("$ancient").into()))
        .await
        .unwrap();
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.contains("/context/"))
            .count(),
        2
    );
    drop(first);
    drop(second);
    for _ in 0..40 {
        assert!(
            build_focused_timeline(&room, &kind, &Some(ruma::event_id!("$wrong").into()))
                .await
                .is_err()
        );
    }
    assert!(
        build_focused_timeline(&room, &kind, &Some(ruma::event_id!("$ancient").into()))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn ancient_event_uses_context_without_walking_history_and_returns_to_live() {
    let server = Server::start(true);
    let room = server.room().await;
    let kind = TimelineKind::MainRoom {
        room_id: room.room_id().to_owned(),
    };
    let context =
        build_focused_timeline(&room, &kind, &Some(ruma::event_id!("$ancient").to_owned()))
            .await
            .unwrap();
    let (items, _updates) = context.subscribe().await;
    assert!(items.iter().any(|item| {
        item.as_event()
            .is_some_and(|event| event.event_id() == Some(ruma::event_id!("$ancient")))
    }));
    let live = build_focused_timeline(&room, &kind, &None).await.unwrap();
    let (items, _updates) = live.subscribe().await;
    assert!(items.iter().any(|item| {
        item.as_event()
            .is_some_and(|event| event.event_id() == Some(ruma::event_id!("$recent")))
    }));
    let paths = server.requests.lock().unwrap();
    assert_eq!(
        paths
            .iter()
            .filter(|path| path.contains("/context/"))
            .count(),
        1
    );
    assert!(
        !paths.iter().any(|path| path.contains("/messages")),
        "{paths:?}"
    );
}

#[tokio::test]
async fn missing_or_wrong_context_does_not_fall_back_to_linear_pagination() {
    for available in [false, true] {
        let server = Server::start(available);
        let room = server.room().await;
        let kind = TimelineKind::MainRoom {
            room_id: room.room_id().to_owned(),
        };
        let result =
            build_focused_timeline(&room, &kind, &Some(ruma::event_id!("$missing").to_owned()))
                .await;
        assert!(result.is_err());
        assert!(
            !server
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|path| path.contains("/messages"))
        );
    }
}

#[tokio::test]
async fn index_owned_cursor_replays_mutations_after_sdk_sync_and_survives_reopen() {
    let server = Server::with_handler(
        true,
        Arc::new(|path| {
            if !path.contains("/messages?") {
                return None;
            }
            let parameters: std::collections::HashMap<_, _> =
                url::Url::parse(&format!("http://localhost{path}"))
                    .unwrap()
                    .query_pairs()
                    .into_owned()
                    .collect();
            assert_eq!(parameters.get("dir").map(String::as_str), Some("f"));
            let from = parameters.get("from").unwrap();
            let chunk = if from == "resume" {
                vec![
                    serde_json::json!({"event_id":"$edit","sender":"@alice:example.org","origin_server_ts":3,
                "type":"m.room.message","content":{"msgtype":"m.text","body":"* corrected",
                "m.relates_to":{"rel_type":"m.replace","event_id":"$original"},
                "m.new_content":{"msgtype":"m.text","body":"corrected"}}}),
                    serde_json::json!({"event_id":"$redaction","sender":"@alice:example.org","origin_server_ts":4,
                "type":"m.room.redaction","content":{},"redacts":"$deleted"}),
                ]
            } else {
                vec![]
            };
            Some((
                200,
                serde_json::json!({"start":from,"end":"caught-up","chunk":chunk,"state":[]}),
            ))
        }),
    );
    let room = server.room().await;
    let path = std::env::temp_dir().join(format!("history-replay-{}.sqlite", uuid::Uuid::new_v4()));
    let mut index = hagency_history::Index::open(&path, &[8; 32], "fixture").unwrap();
    let events: Vec<_> = [("$original", 1), ("$deleted", 2)]
        .into_iter()
        .map(|(id, ts)| hagency_history::Event {
            value: event(id, ts),
            name: "Alice".into(),
            locked: false,
        })
        .collect();
    index
        .insert_batch(
            &events,
            Some(&hagency_history::Coverage {
                cursor: Some("older".into()),
                exhausted: false,
                head_cursor: Some("resume".into()),
            }),
        )
        .unwrap();
    // The SDK has already committed its own sync. Replay uses the independent
    // encrypted index cursor, so an interrupted prior watcher loses no mutation.
    let slot = Arc::new(std::sync::Mutex::new(Some(index)));
    crate::home::room_history::catch_up_history_checked(
        &room.client(),
        &room,
        &slot,
        &Arc::new(AtomicBool::new(false)),
        || Ok(()),
    )
    .await
    .unwrap();
    drop(slot);
    let index = hagency_history::Index::open(&path, &[8; 32], "fixture").unwrap();
    assert_eq!(index.stats().unwrap(), (1, 0));
    assert_eq!(
        index
            .search("corrected", 0, None, None, || false)
            .unwrap()
            .len(),
        1
    );
    assert!(
        index
            .search("deleted", 0, None, None, || false)
            .unwrap()
            .is_empty()
    );
    let coverage = index.coverage().unwrap();
    assert_eq!(coverage.cursor.as_deref(), Some("older"));
    assert_eq!(coverage.head_cursor.as_deref(), Some("caught-up"));
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.contains("/messages?"))
            .count(),
        2
    );
}

#[tokio::test]
async fn million_and_ten_million_logical_history_random_contexts_stay_bounded() {
    // Logical server history cardinality is independent of the actual ten
    // million encrypted on-disk stress test. Only requested windows are served.
    for total in [1_000_000usize, 10_000_000] {
        let server = Server::with_handler(
            true,
            Arc::new(move |path| {
                if !path.contains("/context/") {
                    return None;
                }
                let id = path
                    .split("/context/")
                    .nth(1)
                    .unwrap()
                    .split('?')
                    .next()
                    .unwrap()
                    .replace("%24", "$");
                let target: usize = id.strip_prefix("$e").unwrap().parse().unwrap();
                assert!(target < total);
                let before: Vec<_> = (target.saturating_sub(50)..target)
                    .map(|i| event(&format!("$e{i}"), i as u64))
                    .collect();
                let after: Vec<_> = (target + 1..(target + 51).min(total))
                    .map(|i| event(&format!("$e{i}"), i as u64))
                    .collect();
                Some((
                    200,
                    serde_json::json!({"event":event(&id,target as u64),"events_before":before,"events_after":after,
                "state":[],"start":format!("before{target}"),"end":format!("after{target}")}),
                ))
            }),
        );
        let room = server.room().await;
        let kind = TimelineKind::MainRoom {
            room_id: room.room_id().to_owned(),
        };
        let started = std::time::Instant::now();
        for step in 0..40 {
            let target = (step * 2654435761usize) % total;
            let id: OwnedEventId = format!("$e{target}").try_into().unwrap();
            let timeline = build_focused_timeline(&room, &kind, &Some(id.clone()))
                .await
                .unwrap();
            let items = timeline.items().await;
            assert!(items.len() <= 205);
            assert!(
                items
                    .iter()
                    .any(|i| i.as_event().is_some_and(|e| e.event_id() == Some(&id)))
            );
            drop(items);
            drop(timeline);
        }
        let paths = server.requests.lock().unwrap();
        assert_eq!(paths.iter().filter(|p| p.contains("/context/")).count(), 40);
        assert!(!paths.iter().any(|p| p.contains("/messages")));
        eprintln!(
            "LOGICAL history={total} random_contexts=40 elapsed_ms={:.3} max_context_events=101",
            started.elapsed().as_secs_f64() * 1000.
        );
    }
}

#[tokio::test]
async fn sdk_room_cache_rejects_all_active_saturation_and_reclaims_closed_rooms() {
    let sequence = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let next = sequence.clone();
    let server = Server::with_handler(
        true,
        Arc::new(move |path| {
            if !path.contains("/sync?") {
                return None;
            }
            let stage = next.fetch_add(1, Ordering::Relaxed);
            if stage == 0 {
                return None;
            }
            if stage >= 2 {
                return Some((
                    200,
                    serde_json::json!({"next_batch":"barrier","rooms":{"join":{"!room:example.org":{
                        "timeline":{"events":[event("$barrier",10)],"limited":false}
                    }}}}),
                ));
            }
            let mut joined = serde_json::Map::new();
            for i in 0..40 {
                joined.insert(format!("!bounded{i}:example.org"),serde_json::json!({
                "state":{"events":[{"type":"m.room.create","state_key":"","event_id":format!("$create{i}"),
                    "sender":"@alice:example.org","origin_server_ts":1,"content":{"creator":"@alice:example.org","room_version":"10"}},
                    {"type":"m.room.member","state_key":"@alice:example.org","event_id":format!("$member{i}"),
                    "sender":"@alice:example.org","origin_server_ts":2,"content":{"membership":"join"}}]},
                "timeline":{"events":[event(&format!("$recent{i}"),3)],"limited":false,"prev_batch":"older"}}));
            }
            Some((
                200,
                serde_json::json!({"next_batch":"rooms40","rooms":{"join":joined}}),
            ))
        }),
    );
    let room = server.room().await;
    let client = room.client();
    let (barrier, _handles) = client.event_cache().room(room.room_id()).await.unwrap();
    client
        .sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::ZERO))
        .await
        .unwrap();
    client
        .sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::ZERO))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if barrier
                .events()
                .await
                .unwrap()
                .iter()
                .any(|e| e.event_id() == Some(ruma::event_id!("$barrier")))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("SDK background sync did not reach the admission-test barrier");
    let mut held = Vec::new();
    let mut rejected = None;
    for i in 0..40 {
        let id: OwnedRoomId = format!("!bounded{i}:example.org").try_into().unwrap();
        match client.event_cache().room(&id).await {
            Ok(cache) => held.push(cache),
            Err(matrix_sdk::event_cache::EventCacheError::HistoryWindowFull) => {
                rejected = Some(id);
                break;
            }
            Err(error) => panic!("unexpected admission error: {error}"),
        }
    }
    // SDK background processing may hold the original room while it consumes
    // the sync; public admission includes those active handles too.
    assert!(
        (30..=32).contains(&held.len()),
        "{} admitted handles",
        held.len()
    );
    let extra = rejected.expect("room cache admitted more than 32 active rooms");
    drop(held.pop());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if client.event_cache().room(&extra).await.is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a closed room was not reclaimable");
}

#[tokio::test]
async fn sdk_raw_byte_budget_compacts_large_valid_events() {
    let sequence = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let next = sequence.clone();
    let server = Server::with_handler(
        true,
        Arc::new(move |path| {
            if !path.contains("/sync?") || next.fetch_add(1, Ordering::Relaxed) == 0 {
                return None;
            }
            let events: Vec<_> = (0..100)
                .map(|i| {
                    let mut e = event(&format!("$large{i}"), 100 + i);
                    e["content"]["body"] = "a".repeat(100_000).into();
                    e
                })
                .collect();
            Some((
                200,
                serde_json::json!({"next_batch":"large-body","rooms":{"join":{"!room:example.org":{
                    "timeline":{"events":events,"limited":false}
                }}}}),
            ))
        }),
    );
    let room = server.room().await;
    let client = room.client();
    let timeline = room.timeline_builder().build().await.unwrap();
    let (cache, _handles) = client.event_cache().room(room.room_id()).await.unwrap();
    client
        .sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::ZERO))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if timeline.latest_event_id().await.as_deref() == Some(ruma::event_id!("$large99")) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let events = cache.events().await.unwrap();
    assert!(
        events.len() < 100,
        "byte budget did not compact before reaching the count limit"
    );
    assert!(
        events
            .iter()
            .map(|e| e.raw().json().get().len())
            .sum::<usize>()
            <= 8 * 1024 * 1024
    );
}

#[tokio::test]
async fn matrix_link_focus_stops_when_original_event_search_is_cancelled() {
    let target=ruma::event_id!("$linked").to_owned();
    let (requests, receiver)=watch::channel(TimelineRequest {is_timeline_open:true,
        backwards_paginate:vec![BackwardsPaginateUntilEventRequest {room_id:ruma::room_id!("!room:example.org").to_owned(),
            target_event_id:target.clone(),starting_index:0,current_tl_len:0}]});
    let held=Arc::new(());let weak=Arc::downgrade(&held);
    let build=async move {let _held=held; std::future::pending::<()>().await};
    let task=tokio::spawn(build_in_timeline_window(receiver,build,Duration::from_secs(5),Some(target)));
    requests.send_modify(|request|request.backwards_paginate.clear());
    assert!(tokio::time::timeout(Duration::from_secs(1),task).await.unwrap().unwrap().is_none());
    assert!(weak.upgrade().is_none(),"cancelling a target drops its original focused build before its network deadline");
}
