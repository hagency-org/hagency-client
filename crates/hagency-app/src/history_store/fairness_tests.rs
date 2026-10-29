use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Notify, mpsc},
};

const LARGE_PAGES: usize = 12;

fn message(id: &str, timestamp: u64) -> serde_json::Value {
    serde_json::json!({"event_id":id,"sender":"@alice:example.org","origin_server_ts":timestamp,
        "type":"m.room.message","content":{"msgtype":"m.text","body":id}})
}

struct Fixture {
    client: Client,
    large: ruma::OwnedRoomId,
    small: ruma::OwnedRoomId,
    release: Arc<Notify>,
    started: Arc<Notify>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let large = ruma::room_id!("!large:example.org").to_owned();
        // Deliberately collide under the retired 64-shard scheme. Exact room
        // identities must still use independent gates while one HTTP page stalls.
        let shard = |room: &ruma::RoomId| blake3::hash(room.as_bytes()).as_bytes()[0] % 64;
        let small = (0..4096)
            .map(|i| format!("!small{i}:example.org").try_into().unwrap())
            .find(|room: &ruma::OwnedRoomId| shard(room) == shard(&large))
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let release = Arc::new(Notify::new());
        let started = Arc::new(Notify::new());
        let first = Arc::new(AtomicBool::new(true));
        let wake = release.clone();
        let observed = started.clone();
        let room_ids = [large.clone(), small.clone()];
        let server = tokio::spawn(async move {
            let mut requests = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let Ok((mut socket, _)) = connection else { break; };
                        let room_ids = room_ids.clone();
                        let wake = wake.clone();
                        let observed = observed.clone();
                        let first = first.clone();
                        requests.spawn(async move {
                            let mut request = Vec::new();
                            let mut buffer = [0; 4096];
                            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                                let Ok(count) = socket.read(&mut buffer).await else { return; };
                                if count == 0 { return; }
                                request.extend_from_slice(&buffer[..count]);
                            }
                            let request = String::from_utf8_lossy(&request);
                            let path = request.split_whitespace().nth(1).unwrap();
                            let body = if path.contains("/sync?") {
                                let mut joined = serde_json::Map::new();
                                for room in room_ids {
                                    joined.insert(room.to_string(), serde_json::json!({
                                        "state":{"events":[
                                            {"type":"m.room.create","state_key":"","event_id":"$create","sender":"@alice:example.org","origin_server_ts":0,"content":{"creator":"@alice:example.org","room_version":"10"}},
                                            {"type":"m.room.member","state_key":"@alice:example.org","event_id":"$member","sender":"@alice:example.org","origin_server_ts":0,"content":{"membership":"join"}}]},
                                        "timeline":{"events":[],"limited":false,"prev_batch":"older"}
                                    }));
                                }
                                serde_json::json!({"next_batch":"sdk-already-committed","rooms":{"join":joined}})
                            } else if path.contains("/messages?") {
                                let parsed = url::Url::parse(&format!("http://localhost{path}")).unwrap();
                                let args: std::collections::BTreeMap<_,_> = parsed.query_pairs().into_owned().collect();
                                assert_eq!(args.get("dir").unwrap(), "f");
                                assert_eq!(args.get("limit").unwrap(), "100");
                                let page: usize = args.get("from").unwrap().parse().unwrap();
                                let large = path.contains("large");
                                if large && page == 0 && first.swap(false, Ordering::AcqRel) {
                                    observed.notify_one();
                                    wake.notified().await;
                                }
                                let chunk = if large && page < LARGE_PAGES {
                                    (0..100).map(|i| {
                                        let n = page * 100 + i;
                                        message(&format!("$large{n}"), n as u64 + 10)
                                    }).collect::<Vec<_>>()
                                } else if !large && page == 0 {
                                    vec![
                                        serde_json::json!({"event_id":"$edit","sender":"@alice:example.org","origin_server_ts":3,
                                            "type":"m.room.message","content":{"msgtype":"m.text","body":"* corrected",
                                            "m.relates_to":{"rel_type":"m.replace","event_id":"$original"},
                                            "m.new_content":{"msgtype":"m.text","body":"corrected"}}}),
                                        serde_json::json!({"event_id":"$redaction","sender":"@alice:example.org","origin_server_ts":4,
                                            "type":"m.room.redaction","content":{},"redacts":"$deleted"}),
                                    ]
                                } else { Vec::new() };
                                let next = if chunk.is_empty() { page } else { page + 1 };
                                serde_json::json!({"start":page.to_string(),"end":next.to_string(),"chunk":chunk,"state":[]})
                            } else { serde_json::json!({"chunk":[]}) };
                            let body = body.to_string();
                            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                            let _ = socket.write_all(response.as_bytes()).await;
                        });
                    }
                    Some(_) = requests.join_next(), if !requests.is_empty() => {}
                }
            }
        });
        let client = Client::builder()
            .homeserver_url(url)
            .http_client(
                matrix_sdk::reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap(),
            )
            .server_versions([ruma::api::MatrixVersion::V1_0])
            .build()
            .await
            .unwrap();
        client.matrix_auth().restore_session(serde_json::from_value(serde_json::json!({
            "access_token":"fixture","user_id":"@alice:example.org","device_id":"FAIR-HISTORY"
        })).unwrap(), Default::default()).await.unwrap();
        client
            .sync_once(
                matrix_sdk::config::SyncSettings::default().timeout(std::time::Duration::ZERO),
            )
            .await
            .unwrap();
        assert!(client.get_room(&large).is_some());
        assert!(client.get_room(&small).is_some());
        Self {
            client,
            large,
            small,
            release,
            started,
            server,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn seeded(path: &std::path::Path, identity: &str) -> Index {
    let mut index = Index::open(path, &[8; 32], identity).unwrap();
    let events = [("$original", 1), ("$deleted", 2)]
        .into_iter()
        .map(|(id, ts)| Event {
            value: message(id, ts),
            name: "Alice".into(),
            locked: false,
        })
        .collect::<Vec<_>>();
    index
        .insert_batch(
            &events,
            Some(&hagency_history::Coverage {
                cursor: Some("older".into()),
                exhausted: false,
                head_cursor: Some("0".into()),
            }),
        )
        .unwrap();
    index
}

type Paths = Arc<std::collections::BTreeMap<ruma::OwnedRoomId, PathBuf>>;

async fn page(
    client: Client,
    room: ruma::OwnedRoomId,
    paths: Paths,
    current: Arc<AtomicBool>,
) -> Result<bool> {
    // Reopen every turn: progress must survive releasing the connection, rather
    // than relying on a retained in-memory cursor or the SDK's newer sync token.
    let file = paths[&room].clone();
    let identity = room.to_string();
    let index =
        tokio::task::spawn_blocking(move || Index::open(&file, &[8; 32], &identity)).await??;
    let slot = Arc::new(Mutex::new(Some(index)));
    let room = client.get_room(&room).unwrap();
    crate::home::room_history::catch_up_history_step_checked(
        &client,
        &room,
        &slot,
        &Arc::new(AtomicBool::new(false)),
        move || {
            ensure!(current.load(Ordering::Acquire), "account epoch retired");
            Ok(())
        },
    )
    .await
}

fn source(
    receiver: mpsc::Receiver<Vec<ruma::OwnedRoomId>>,
) -> impl futures_util::Stream<Item = Vec<ruma::OwnedRoomId>> {
    futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|batch| (batch, receiver))
    })
}

#[tokio::test]
async fn large_backlog_yields_to_small_room_edit_and_redaction_then_finishes() {
    let fixture = Fixture::new().await;
    let directory = tempfile::tempdir().unwrap();
    let paths: Paths = Arc::new(
        [
            (fixture.large.clone(), directory.path().join("large.sqlite")),
            (fixture.small.clone(), directory.path().join("small.sqlite")),
        ]
        .into_iter()
        .collect(),
    );
    for (room, path) in paths.iter() {
        drop(seeded(path, room.as_str()));
    }
    let (sender, receiver) = mpsc::channel(4);
    let client = fixture.client.clone();
    let worker_paths = paths.clone();
    let completed = Arc::new(Mutex::new(Vec::new()));
    let observed = completed.clone();
    let current = Arc::new(AtomicBool::new(true));
    let checker = current.clone();
    let progress_room = fixture.large.clone();
    let (committed_page, mut commits) = mpsc::channel(LARGE_PAGES + 1);
    let worker = tokio::spawn(drive_history_updates(
        source(receiver),
        move |room| {
            let client = client.clone();
            let paths = worker_paths.clone();
            let current = current.clone();
            let observed = observed.clone();
            let progress_room = progress_room.clone();
            let committed_page = committed_page.clone();
            async move {
                let more = page(client, room.clone(), paths, current).await?;
                // page() returns only after the real SQLite page transaction
                // has committed. A reopened index supplies every next cursor.
                if room == progress_room {
                    committed_page.send(()).await.expect("progress observer must outlive the original history worker");
                }
                if !more {
                    observed.lock().unwrap().push(room);
                }
                Ok(more)
            }
        },
        move || checker.load(Ordering::Acquire),
    ));
    sender.send(vec![fixture.large.clone()]).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.started.notified(),
    )
    .await
    .unwrap();
    // The SDK wakeup arrives during a held large-room HTTP page; it must still
    // be consumed and finish the small room before that page is released.
    sender.send(vec![fixture.small.clone(); 100]).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if completed.lock().unwrap().contains(&fixture.small) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("small room was starved behind the large-room backlog");
    assert!(!completed.lock().unwrap().contains(&fixture.large));
    let small = Index::open(&paths[&fixture.small], &[8; 32], fixture.small.as_str()).unwrap();
    assert_eq!(
        small
            .search("corrected", 0, None, None, || false)
            .unwrap()
            .len(),
        1
    );
    assert!(
        small
            .search("deleted", 0, None, None, || false)
            .unwrap()
            .is_empty()
    );
    assert_eq!(small.coverage().unwrap().cursor.as_deref(), Some("older"));
    assert_eq!(small.coverage().unwrap().head_cursor.as_deref(), Some("1"));
    drop(small);
    fixture.release.notify_one();
    drop(sender);
    // Fairness above retains its strict two-second bound. Completing twelve
    // encrypted, synchronous-FULL SQLite pages is a progress property, not a
    // five-second aggregate throughput requirement shared with parallel builds.
    // Keep the original five-second stall bound and reject extra/fake progress:
    // twelve populated pages plus the final empty page are the only 13 turns.
    let mut committed = 0;
    while tokio::time::timeout(std::time::Duration::from_secs(5), commits.recv())
        .await
        .expect("large-room catch-up made no committed progress for five seconds")
        .is_some()
    {
        committed += 1;
        assert!(committed <= LARGE_PAGES + 1, "large-room catch-up exceeded its actual page budget");
    }
    worker.await.unwrap().unwrap();
    assert_eq!(committed, LARGE_PAGES + 1);
    let large = Index::open(&paths[&fixture.large], &[8; 32], fixture.large.as_str()).unwrap();
    assert_eq!(large.stats().unwrap(), (LARGE_PAGES * 100 + 2, 0));
    assert_eq!(
        large.coverage().unwrap().head_cursor.as_deref(),
        Some(LARGE_PAGES.to_string().as_str())
    );
    assert_eq!(large.coverage().unwrap().cursor.as_deref(), Some("older"));
    assert_eq!(completed.lock().unwrap().first(), Some(&fixture.small));
}

#[tokio::test]
async fn retired_epoch_cannot_commit_the_held_page_and_original_cursor_can_resume() {
    let fixture = Fixture::new().await;
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("history.sqlite");
    drop(seeded(&file, fixture.large.as_str()));
    let paths: Paths = Arc::new(
        [(fixture.large.clone(), file.clone())]
            .into_iter()
            .collect(),
    );
    let current = Arc::new(AtomicBool::new(true));
    let client = fixture.client.clone();
    let room = fixture.large.clone();
    let worker_current = current.clone();
    let worker_paths = paths.clone();
    let worker =
        tokio::spawn(async move { page(client, room, worker_paths, worker_current).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.started.notified(),
    )
    .await
    .unwrap();
    current.store(false, Ordering::Release);
    fixture.release.notify_one();
    assert!(worker.await.unwrap().is_err());
    let index = Index::open(&file, &[8; 32], fixture.large.as_str()).unwrap();
    assert_eq!(index.stats().unwrap(), (2, 0));
    assert_eq!(index.coverage().unwrap().head_cursor.as_deref(), Some("0"));
    drop(index);
    current.store(true, Ordering::Release);
    while page(
        fixture.client.clone(),
        fixture.large.clone(),
        paths.clone(),
        current.clone(),
    )
    .await
    .unwrap()
    {}
    let reopened = Index::open(&file, &[8; 32], fixture.large.as_str()).unwrap();
    assert_eq!(reopened.stats().unwrap(), (LARGE_PAGES * 100 + 2, 0));
    assert_eq!(
        reopened.coverage().unwrap().head_cursor.as_deref(),
        Some(LARGE_PAGES.to_string().as_str())
    );
}

#[test]
fn repeated_wakeups_are_deduplicated_without_running_two_pages_for_one_room() {
    let room = ruma::room_id!("!room:example.org").to_owned();
    let other = ruma::room_id!("!other:example.org").to_owned();
    let mut queue = DirtyRooms::default();
    for _ in 0..10_000 {
        queue.enqueue(room.clone());
    }
    assert_eq!(queue.ready.len(), 1);
    assert_eq!(queue.begin(), Some(room.clone()));
    for _ in 0..10_000 {
        queue.enqueue(room.clone());
    }
    assert!(
        queue.begin().is_none(),
        "an active room must not occupy another worker"
    );
    assert_eq!(queue.again.len(), 1);
    queue.enqueue(other.clone());
    queue.finish(room.clone(), true);
    assert_eq!(queue.begin(), Some(other));
    assert_eq!(queue.begin(), Some(room));
}

#[tokio::test]
async fn cancelling_the_page_drops_its_turn_without_advancing_the_durable_cursor() {
    let fixture = Fixture::new().await;
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("history.sqlite");
    drop(seeded(&file, fixture.large.as_str()));
    let paths: Paths = Arc::new(
        [(fixture.large.clone(), file.clone())]
            .into_iter()
            .collect(),
    );
    let client = fixture.client.clone();
    let room = fixture.large.clone();
    let current = Arc::new(AtomicBool::new(true));
    let worker = tokio::spawn(async move { page(client, room, paths, current).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.started.notified(),
    )
    .await
    .unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    fixture.release.notify_one();
    let index = Index::open(&file, &[8; 32], fixture.large.as_str()).unwrap();
    assert_eq!(index.stats().unwrap(), (2, 0));
    assert_eq!(index.coverage().unwrap().head_cursor.as_deref(), Some("0"));
}

#[tokio::test]
async fn page_workers_stay_finite_and_cancelling_the_consumer_drops_all_active_pages() {
    use std::sync::atomic::AtomicUsize;
    struct Running(Arc<AtomicUsize>);
    impl Drop for Running {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (sender, receiver) = mpsc::channel(1);
    let worker_active = active.clone();
    let worker_peak = peak.clone();
    let worker_starts = starts.clone();
    let worker_release = release.clone();
    let worker = tokio::spawn(drive_history_updates(
        source(receiver),
        move |_| {
            let active = worker_active.clone();
            let peak = worker_peak.clone();
            let starts = worker_starts.clone();
            let release = worker_release.clone();
            async move {
                let _running = Running(active.clone());
                let running = active.fetch_add(1, Ordering::AcqRel) + 1;
                peak.fetch_max(running, Ordering::AcqRel);
                starts.fetch_add(1, Ordering::AcqRel);
                release.acquire().await.unwrap().forget();
                Ok(false)
            }
        },
        || true,
    ));
    sender
        .send(
            (0..50)
                .map(|i| format!("!finite{i}:example.org").try_into().unwrap())
                .collect(),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while starts.load(Ordering::Acquire) < HISTORY_PAGE_WORKERS {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(active.load(Ordering::Acquire), HISTORY_PAGE_WORKERS);
    assert_eq!(starts.load(Ordering::Acquire), HISTORY_PAGE_WORKERS);
    release.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while starts.load(Ordering::Acquire) < HISTORY_PAGE_WORKERS + 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(peak.load(Ordering::Acquire), HISTORY_PAGE_WORKERS);
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert_eq!(active.load(Ordering::Acquire), 0);
}

#[test]
fn exact_identity_gates_share_live_locks_and_discard_dead_history() {
    let mut gates = HistoryGates::default();
    let key = |i| {
        (
            "https://example.org".to_owned(),
            Some("@alice:example.org".to_owned()),
            Some("DEVICE".to_owned()),
            format!("!room{i}:example.org").try_into().unwrap(),
        )
    };
    let original = gates.get(key(0));
    let guard = original.clone().try_lock_owned().unwrap();
    drop(original);
    for i in 1..10_000 {
        let temporary = gates.get(key(i));
        assert!(gates.entries.len() <= 2, "dead history must not accumulate");
        assert!(
            gates.get(key(0)).try_lock_owned().is_err(),
            "an active original guard cannot be replaced"
        );
        drop(temporary);
    }
    drop(guard);
    let replacement = gates.get(key(10_000));
    assert_eq!(gates.entries.len(), 1);
    assert!(replacement.try_lock_owned().is_ok());
}

#[tokio::test]
async fn cancelling_initial_observer_keeps_the_gate_until_the_original_reset_finishes() {
    use std::sync::atomic::AtomicUsize;
    let fixture = Fixture::new().await;
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("history.sqlite");
    let index = seeded(&file, fixture.large.as_str());
    let original_generation = index.generation().unwrap();
    // reset_stale is intentionally a no-op for healthy indexes. Exercise the
    // actual repair contract, rather than requiring it to erase healthy history.
    Index::mark_stale(&file).unwrap();
    assert!(index.is_stale().unwrap());
    let slot = Arc::new(Mutex::new(Some(index)));
    let entered = Arc::new(Notify::new());
    let observed = entered.clone();
    let (release, waiting) = std::sync::mpsc::channel();
    let waiting = Mutex::new(waiting);
    let calls = AtomicUsize::new(0);
    let client = fixture.client.clone();
    let room = client.get_room(&fixture.large).unwrap();
    let gate = history_gate(&client, &fixture.large);
    let worker = tokio::spawn(async move {
        crate::home::room_history::catch_up_history_step_checked(
            &client,
            &room,
            &slot,
            &Arc::new(AtomicBool::new(true)),
            move || {
                if calls.fetch_add(1, Ordering::AcqRel) == 1 {
                    observed.notify_one();
                    waiting.lock().unwrap().recv().unwrap();
                }
                Ok(())
            },
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert!(gate.clone().try_lock_owned().is_err());
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert!(
        gate.clone().try_lock_owned().is_err(),
        "observer cancellation cannot release the blocking reset's original guard"
    );
    release.send(()).unwrap();
    let _finished = tokio::time::timeout(std::time::Duration::from_secs(2), gate.lock_owned())
        .await
        .unwrap();
    let index = Index::open(&file, &[8; 32], fixture.large.as_str()).unwrap();
    assert_eq!(
        index.stats().unwrap(),
        (0, 0),
        "the original reset really finished before the gate became available"
    );
    assert!(!index.is_stale().unwrap());
    assert_ne!(index.generation().unwrap(), original_generation);
    assert_eq!(index.coverage().unwrap().head_cursor, None);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Negative fixture deliberately blocks the queued SQLite slot.
async fn retired_epoch_is_rechecked_after_queued_final_write_waits_for_its_slot() {
    use std::sync::atomic::AtomicUsize;
    let fixture = Fixture::new().await;
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("history.sqlite");
    let slot = Arc::new(Mutex::new(Some(seeded(&file, fixture.large.as_str()))));
    let current = Arc::new(AtomicBool::new(true));
    let checker = current.clone();
    let observed = Arc::new(Notify::new());
    let entered = observed.clone();
    let calls = AtomicUsize::new(0);
    let client = fixture.client.clone();
    let room = client.get_room(&fixture.large).unwrap();
    let worker_slot = slot.clone();
    let worker = tokio::spawn(async move {
        crate::home::room_history::catch_up_history_step_checked(
            &client,
            &room,
            &worker_slot,
            &Arc::new(AtomicBool::new(false)),
            move || {
                ensure!(checker.load(Ordering::Acquire), "account epoch retired");
                if calls.fetch_add(1, Ordering::AcqRel) == 3 {
                    entered.notify_one();
                }
                Ok(())
            },
        )
        .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.started.notified(),
    )
    .await
    .unwrap();
    // The HTTP response can complete, but its original blocking write must wait
    // for this slot. Retire only after the pre-enqueue async check succeeded.
    let held = slot.lock().unwrap();
    fixture.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), observed.notified())
        .await
        .unwrap();
    current.store(false, Ordering::Release);
    drop(held);
    assert!(worker.await.unwrap().is_err());
    drop(slot);
    let index = Index::open(&file, &[8; 32], fixture.large.as_str()).unwrap();
    assert_eq!(index.stats().unwrap(), (2, 0));
    assert_eq!(index.coverage().unwrap().head_cursor.as_deref(), Some("0"));
}
