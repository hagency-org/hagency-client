//! Account/device-bound encrypted history storage. Only call from blocking workers.
use std::{io::Read, path::PathBuf};

use anyhow::{Result, anyhow, ensure};
use hagency_history::{Event, Index};
use matrix_sdk::{Client, deserialized_responses::TimelineEvent};
use ruma::RoomId;
use zeroize::Zeroizing;

static FAILED_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

#[derive(Debug)]
pub(crate) struct HistoryStoreChanged {
    pub room: Option<ruma::OwnedRoomId>,
    pub epoch: u64,
}

pub(crate) fn ensure_healthy() -> Result<()> {
    ensure!(
        FAILED_EPOCH.load(std::sync::atomic::Ordering::Acquire) != crate::account_session::epoch(),
        "History index updates failed; reconnect before searching"
    );
    Ok(())
}

pub(crate) fn open(client: &Client, room: &RoomId) -> Result<Index> {
    ensure_healthy()?;
    crate::matrix_context::ensure_current(client)?;
    let user = client.user_id().ok_or_else(|| anyhow!("Not signed in"))?;
    let device = client
        .device_id()
        .ok_or_else(|| anyhow!("No device identity"))?;
    let file = std::fs::File::open(crate::persistence::session_file_path(user))?;
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1024 * 1024, "Session file too large");
    let mut session: crate::persistence::FullSessionPersisted = serde_json::from_slice(&bytes)?;
    ensure!(
        session.user_session.meta().user_id == user
            && session.user_session.meta().device_id == device,
        "History index session identity mismatch"
    );
    let homeserver: url::Url = session.client_session.homeserver.parse()?;
    ensure!(
        homeserver == client.homeserver(),
        "History index homeserver mismatch"
    );
    let identity = serde_json::to_string(&(
        client.homeserver().as_str(),
        user.as_str(),
        device.as_str(),
        room.as_str(),
    ))?;
    let secret = Zeroizing::new(std::mem::take(&mut session.client_session.passphrase));
    ensure!(!secret.is_empty(), "History index secret missing");
    let root_key = Zeroizing::new(blake3::derive_key(
        "hagency.history.root.v1",
        secret.as_bytes(),
    ));
    let wrapping_key =
        Zeroizing::new(*blake3::keyed_hash(&root_key, identity.as_bytes()).as_bytes());
    let path = path(client, room)?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    Index::open(&path, &wrapping_key, &identity)
}

fn directory(client: &Client) -> Result<PathBuf> {
    let user = client.user_id().ok_or_else(|| anyhow!("Not signed in"))?;
    let device = client
        .device_id()
        .ok_or_else(|| anyhow!("No device identity"))?;
    let device = blake3::hash(device.as_bytes()).to_hex().to_string();
    Ok(crate::persistence::persistent_state_dir(user)
        .join("history-search-v1")
        .join(device))
}

fn path(client: &Client, room: &RoomId) -> Result<PathBuf> {
    let room = blake3::hash(room.as_bytes()).to_hex().to_string();
    Ok(directory(client)?.join(format!("{room}.sqlite3")))
}

pub(crate) fn event(event: &TimelineEvent, name: Option<&str>) -> Result<Option<Event>> {
    let value: serde_json::Value = serde_json::from_str(event.kind.raw().json().get())?;
    if value["event_id"]
        .as_str()
        .and_then(|id| ruma::OwnedEventId::try_from(id).ok())
        .is_none()
    {
        return Ok(None);
    }
    if !event.kind.is_utd()
        && value["type"] == "m.room.message"
        && value.pointer("/unsigned/redacted_because").is_none()
    {
        let body = if value
            .pointer("/content/m.relates_to/rel_type")
            .and_then(serde_json::Value::as_str)
            == Some("m.replace")
        {
            &value["content"]["m.new_content"]
        } else {
            &value["content"]
        };
        if serde_json::from_value::<ruma::events::room::message::RoomMessageEventContent>(
            body.clone(),
        )
        .is_err()
            || value["sender"]
                .as_str()
                .and_then(|id| ruma::OwnedUserId::try_from(id).ok())
                .is_none()
        {
            return Ok(None);
        }
    }
    let name = name
        .map(str::to_owned)
        .or_else(|| value["sender"].as_str().map(str::to_owned))
        .unwrap_or_default();
    Ok(Some(Event {
        value,
        name,
        locked: event.kind.is_utd(),
    }))
}

/// Metadata-only FIFO: one queued turn per room and one dirty flag while its
/// page is in flight. Payloads and open indexes are never retained in this queue.
#[derive(Default)]
struct DirtyRooms {
    ready: std::collections::VecDeque<ruma::OwnedRoomId>,
    queued: std::collections::BTreeSet<ruma::OwnedRoomId>,
    active: std::collections::BTreeSet<ruma::OwnedRoomId>,
    again: std::collections::BTreeSet<ruma::OwnedRoomId>,
}
impl DirtyRooms {
    fn enqueue(&mut self, room: ruma::OwnedRoomId) {
        if self.active.contains(&room) {
            self.again.insert(room);
        } else if self.queued.insert(room.clone()) {
            self.ready.push_back(room);
        }
    }
    fn begin(&mut self) -> Option<ruma::OwnedRoomId> {
        let room = self.ready.pop_front()?;
        self.queued.remove(&room);
        self.active.insert(room.clone());
        Some(room)
    }
    fn finish(&mut self, room: ruma::OwnedRoomId, more: bool) {
        self.active.remove(&room);
        let dirty = self.again.remove(&room);
        if more || dirty {
            self.enqueue(room);
        }
    }
}

// The index identity is independent of the SDK sync cursor and login epoch.
// Weak entries retain no locks after their original operations have finished.
type HistoryGateKey = (String, Option<String>, Option<String>, ruma::OwnedRoomId);
type HistoryGate = std::sync::Arc<tokio::sync::Mutex<()>>;
#[derive(Default)]
struct HistoryGates {
    entries: std::collections::BTreeMap<HistoryGateKey, std::sync::Weak<tokio::sync::Mutex<()>>>,
}
impl HistoryGates {
    fn get(&mut self, key: HistoryGateKey) -> HistoryGate {
        self.entries.retain(|_, gate| gate.strong_count() != 0);
        if let Some(gate) = self.entries.get(&key).and_then(std::sync::Weak::upgrade) {
            return gate;
        }
        let gate = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        self.entries.insert(key, std::sync::Arc::downgrade(&gate));
        gate
    }
}

pub(crate) fn history_gate(client: &Client, room: &RoomId) -> HistoryGate {
    static GATES: std::sync::LazyLock<std::sync::Mutex<HistoryGates>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(HistoryGates::default()));
    GATES.lock().unwrap().get((
        client.homeserver().to_string(),
        client.user_id().map(ToString::to_string),
        client.device_id().map(ToString::to_string),
        room.to_owned(),
    ))
}

pub(crate) fn ensure_account(client: &Client, epoch: u64) -> Result<()> {
    ensure!(
        epoch == crate::account_session::epoch(),
        "History account epoch changed"
    );
    crate::matrix_context::ensure_current(client)
}

const HISTORY_PAGE_WORKERS: usize = 2;

/// Keep receiving SDK wakeups while slow pages are in flight. Each worker owns
/// one page, rather than an unbounded per-room replay. Repeated wakeups only
/// mark a room dirty; the persisted index cursor is the sole mutation source.
async fn drive_history_updates<S, F, Fut, C>(updates: S, mut step: F, current: C) -> Result<()>
where
    S: futures_util::Stream<Item = Vec<ruma::OwnedRoomId>>,
    F: FnMut(ruma::OwnedRoomId) -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
    C: Fn() -> bool,
{
    use futures_util::{StreamExt, stream::FuturesUnordered};
    futures_util::pin_mut!(updates);
    let mut rooms = DirtyRooms::default();
    let mut pages = FuturesUnordered::new();
    let mut input_open = true;
    loop {
        if !current() {
            return Ok(());
        }
        while pages.len() < HISTORY_PAGE_WORKERS {
            let Some(room) = rooms.begin() else {
                break;
            };
            let work = step(room.clone());
            pages.push(async move { (room, work.await) });
        }
        if !input_open && pages.is_empty() {
            return Ok(());
        }
        tokio::select! {
            batch = updates.next(), if input_open => {
                if !current() { return Ok(()); }
                match batch {
                    Some(batch) => for room in batch { rooms.enqueue(room); },
                    None => input_open = false,
                }
            }
            Some((room, result)) = pages.next(), if !pages.is_empty() => {
                if !current() { return Ok(()); }
                rooms.finish(room, result?);
            }
        }
    }
}

async fn update_history_page(
    client: Client,
    room_id: ruma::OwnedRoomId,
    epoch: u64,
) -> Result<bool> {
    ensure_account(&client, epoch)?;
    let file = path(&client, &room_id)?;
    if !file.exists() {
        return Ok(false);
    }
    let Some(room) = client.get_room(&room_id) else {
        return Ok(false);
    };
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let repair = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let checker = client.clone();
    let result = crate::home::room_history::catch_up_history_step_checked(
        &client,
        &room,
        &slot,
        &repair,
        move || ensure_account(&checker, epoch),
    )
    .await;
    ensure_account(&client, epoch)?;
    let more = match result {
        Ok(more) => more,
        Err(_) => {
            let guard = history_gate(&client, &room_id).lock_owned().await;
            let checker = client.clone();
            let marked = tokio::task::spawn_blocking(move || -> Result<()> {
                let _guard = guard;
                ensure_account(&checker, epoch)?;
                Index::mark_stale(&file)
            })
            .await;
            ensure_account(&client, epoch)?;
            if !matches!(marked, Ok(Ok(()))) {
                FAILED_EPOCH.store(epoch, std::sync::atomic::Ordering::Release);
                makepad_widgets::SignalToUI::set_ui_signal();
                return Err(anyhow!("Could not invalidate the lagging history index"));
            }
            false
        }
    };
    // Refresh an open search only after this room reaches its live cursor (or
    // is marked stale). Partial turns must not trigger a full UI query per page.
    if !more {
        makepad_widgets::Cx::post_action(HistoryStoreChanged {
            room: Some(room_id),
            epoch,
        });
    }
    Ok(more)
}

/// Two account-scoped page workers, never one task/connection per room. A room
/// with a large backlog yields after each committed page and rejoins the tail.
/// Lagged SDK broadcasts reschedule every joined room without skipping cursors.
pub(crate) fn start(client: Client) {
    FAILED_EPOCH.store(u64::MAX, std::sync::atomic::Ordering::Release);
    start_decryption_updates(client.clone());
    let updates = client.subscribe_to_all_room_updates();
    let epoch = crate::account_session::epoch();
    crate::account_session::spawn(async move {
        let source_client = client.clone();
        let source = futures_util::stream::unfold(updates, move |mut updates| {
            let client = source_client.clone();
            async move {
                let rooms = match updates.recv().await {
                    Ok(update) => update
                        .joined
                        .into_keys()
                        .chain(update.left.into_keys())
                        .collect(),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => client
                        .joined_rooms()
                        .into_iter()
                        .map(|room| room.room_id().to_owned())
                        .collect(),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                };
                Some((rooms, updates))
            }
        });
        let page_client = client.clone();
        // The page worker persists stale/error status before returning; fatal
        // invalidation failures retain the account-wide failed epoch latch.
        let _ = drive_history_updates(
            source,
            move |room| update_history_page(page_client.clone(), room, epoch),
            || ensure_account(&client, epoch).is_ok(),
        )
        .await;
    });
}

static DECRYPTION_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

async fn retry_locked_page(
    client: &Client,
    room: &matrix_sdk::Room,
    session: Option<&str>,
    after: Option<String>,
) -> Result<(Option<String>, std::collections::BTreeSet<String>)> {
    crate::matrix_context::ensure_current(client)?;
    let worker_client = client.clone();
    let room_id = room.room_id().to_owned();
    let session = session.map(str::to_owned);
    let (events, generation) = tokio::task::spawn_blocking(move || -> Result<_> {
        let index = open(&worker_client, &room_id)?;
        Ok((
            index.locked_page(session.as_deref(), after.as_deref())?,
            index.generation()?,
        ))
    })
    .await??;
    let last = events
        .last()
        .and_then(|e| e.value["event_id"].as_str())
        .map(str::to_owned);
    let mut decrypted = Vec::new();
    let mut sessions = std::collections::BTreeSet::new();
    for event in events {
        let Ok(raw) = ruma::serde::Raw::new(&event.value) else {
            continue;
        };
        if let Ok(resolved) = room.decrypt_event(&raw.cast_unchecked(), None).await
            && !resolved.kind.is_utd()
        {
            if let Some(session) = resolved.kind.session_id() {
                sessions.insert(session.to_owned());
            }
            if let Some(event) = self::event(&resolved, Some(&event.name))? {
                decrypted.push(event);
            }
        }
    }
    if !decrypted.is_empty() {
        crate::matrix_context::ensure_current(client)?;
        let worker_client = client.clone();
        let room_id = room.room_id().to_owned();
        tokio::task::spawn_blocking(move || -> Result<()> {
            open(&worker_client, &room_id)?.insert_for_generation(&decrypted, None, &generation)
        })
        .await??;
        crate::matrix_context::ensure_current(client)?;
        makepad_widgets::Cx::post_action(HistoryStoreChanged {
            room: Some(room.room_id().to_owned()),
            epoch: crate::account_session::epoch(),
        });
    }
    Ok((last, sessions))
}

async fn retry_sessions(
    client: &Client,
    room: &matrix_sdk::Room,
    sessions: std::collections::BTreeSet<String>,
) -> Result<()> {
    for session in sessions {
        let mut after = None;
        loop {
            let (next, _) = retry_locked_page(client, room, Some(&session), after).await?;
            if next.is_none() {
                break;
            }
            after = next;
        }
    }
    Ok(())
}

/// A cheap bounded probe on open. Resolved sessions continue in one shared
/// worker; scanning historical UTDs never holds up a search or creates N tasks.
pub(crate) fn retry_locked_on_open(client: Client, room: matrix_sdk::Room) {
    let Ok(permit) = DECRYPTION_GATE.try_acquire() else {
        return;
    };
    crate::account_session::spawn(async move {
        let _permit = permit;
        if let Ok((_, sessions)) = retry_locked_page(&client, &room, None, None).await {
            let _ = retry_sessions(&client, &room, sessions).await;
        }
    });
}

fn start_decryption_updates(client: Client) {
    use futures_util::StreamExt;
    crate::account_session::spawn(async move {
        let cache = client.event_cache();
        let reports = cache.subscribe_to_decryption_reports();
        futures_util::pin_mut!(reports);
        while let Some(report) = reports.next().await {
            if !crate::matrix_context::is_current(&client) {
                break;
            }
            let Ok(matrix_sdk::event_cache::RedecryptorReport::ResolvedUtds { room_id, events }) =
                report
            else {
                continue;
            };
            let Some(room) = client.get_room(&room_id) else {
                continue;
            };
            let Ok(file) = path(&client, &room_id) else {
                continue;
            };
            if !file.exists() {
                continue;
            }
            let Ok(_permit) = DECRYPTION_GATE.acquire().await else {
                break;
            };
            let worker_client = client.clone();
            let sessions = tokio::task::spawn_blocking(move || -> Result<_> {
                let index = open(&worker_client, &room_id)?;
                let mut sessions = std::collections::BTreeSet::new();
                for id in events {
                    if let Some(session) = index.locked_session(id.as_str())? {
                        sessions.insert(session);
                    }
                }
                Ok(sessions)
            })
            .await;
            if let Ok(Ok(sessions)) = sessions {
                let _ = retry_sessions(&client, &room, sessions).await;
            }
        }
    });
}

#[cfg(test)]
mod fairness_tests;
