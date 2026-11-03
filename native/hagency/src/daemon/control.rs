//! Durable control state and command receipts.
//!
//! SQLite commits hosting intent and the exact command reply in one transaction.
//! Receipts are indexed on disk and never evicted: a delayed retry after a daemon
//! restart cannot become a new operation merely because other commands ran.
use super::protocol::{DaemonError, Reply, Request, errors};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, path::{Path, PathBuf}, sync::Mutex};

const FORMAT: &str = "hagency-owner-daemon-control";
const VERSION: u8 = 1;
/// The former JSON ring size, used only when importing pre-release state.
const LEGACY_RECEIPTS: usize = 256;
const MAX_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent { Hosting, Paused, Stopped }
impl Intent {
    fn key(self) -> &'static str { match self { Self::Hosting => "hosting", Self::Paused => "paused", Self::Stopped => "stopped" } }
    fn parse(value: &str) -> Result<Self, DaemonError> {
        match value { "hosting" => Ok(Self::Hosting), "paused" => Ok(Self::Paused), "stopped" => Ok(Self::Stopped), _ => Err(errors::STATE_UNAVAILABLE) }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt { request_id: String, fingerprint: String, recorded_at_ms: u64, reply: Reply }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved { format: String, version: u8, revision: u64, intent: Intent, receipts: VecDeque<Receipt> }

pub enum Lookup { New, Replay(Reply), Conflict, Unavailable }
struct State { db: Connection, revision: u64, intent: Intent }
pub struct ControlStore { path: PathBuf, state: Mutex<State> }
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0)
}
fn storage<T>(result: rusqlite::Result<T>) -> Result<T, DaemonError> { result.map_err(|_| errors::STATE_UNAVAILABLE) }
impl ControlStore {
    pub fn open(path: &Path) -> Result<Self, DaemonError> {
        let database = path.with_extension("sqlite3");
        let legacy: Option<Saved> = super::read_private(path, MAX_BYTES)
            .map_err(|_| errors::STATE_UNAVAILABLE)?
            .map(|raw| serde_json::from_slice(&raw).map_err(|_| errors::STATE_UNAVAILABLE)).transpose()?;
        if legacy.as_ref().is_some_and(|s| s.format != FORMAT || s.version != VERSION || s.receipts.len() > LEGACY_RECEIPTS) {
            return Err(errors::STATE_UNAVAILABLE);
        }
        // Creates/checks a 0600, owner-only, single-link non-symlink entry.
        let file = match hagency_store::private::open(&database, true) {
            Ok(file) => file,
            Err(hagency_store::Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists =>
                hagency_store::private::open(&database, false).map_err(|_| errors::STATE_UNAVAILABLE)?,
            Err(_) => return Err(errors::STATE_UNAVAILABLE),
        };
        file.sync_all().map_err(|_| errors::STATE_UNAVAILABLE)?;
        // macOS /var is a system symlink; SQLite NOFOLLOW applies to every
        // path component. Resolve it only after validating the actual file.
        let database = database.canonicalize().map_err(|_| errors::STATE_UNAVAILABLE)?;
        let mut db = storage(Connection::open_with_flags(&database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW))?;
        storage(db.busy_timeout(std::time::Duration::from_secs(2)))?;
        let version: i64 = storage(db.query_row("PRAGMA user_version", [], |r| r.get(0)))?;
        if !matches!(version, 0 | 1) { return Err(errors::STATE_UNAVAILABLE) }
        storage(db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;"))?;
        let tx = storage(db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate))?;
        storage(tx.execute_batch("CREATE TABLE IF NOT EXISTS control (singleton INTEGER PRIMARY KEY CHECK(singleton=1), revision INTEGER NOT NULL CHECK(revision>=0), intent TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS receipts (request_id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, recorded_at_ms INTEGER NOT NULL, reply TEXT NOT NULL); PRAGMA user_version=1;"))?;
        let current: Option<(u64, String)> = storage(tx.query_row("SELECT revision,intent FROM control WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?))).optional())?;
        let (revision, intent) = if let Some((revision,intent)) = current { (revision, Intent::parse(&intent)?) } else {
            let (revision,intent) = legacy.as_ref().map(|v|(v.revision,v.intent)).unwrap_or((0,Intent::Hosting));
            if let Some(saved) = legacy {
                for receipt in saved.receipts {
                    let reply = serde_json::to_string(&receipt.reply).map_err(|_| errors::STATE_UNAVAILABLE)?;
                    storage(tx.execute("INSERT INTO receipts VALUES(?1,?2,?3,?4)", params![receipt.request_id,receipt.fingerprint,receipt.recorded_at_ms,reply]))?;
                }
            }
            storage(tx.execute("INSERT INTO control VALUES(1,?1,?2)", params![revision,intent.key()]))?;
            (revision,intent)
        };
        let revision = revision.checked_add(1).filter(|v| *v <= i64::MAX as u64).ok_or(errors::STATE_UNAVAILABLE)?;
        storage(tx.execute("UPDATE control SET revision=?1 WHERE singleton=1", [revision]))?;
        storage(tx.commit())?;
        Ok(Self { path:database, state:Mutex::new(State {db,revision,intent}) })
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> { self.state.lock().unwrap_or_else(|p|p.into_inner()) }
    fn available(&self) -> bool { hagency_store::private::open(&self.path, false).is_ok() }
    pub fn revision(&self) -> u64 { self.lock().revision }
    pub fn intent(&self) -> Intent { self.lock().intent }
    pub fn lookup(&self, request: &Request) -> Lookup {
        let state = self.lock();
        if !self.available() { return Lookup::Unavailable }
        match state.db.query_row("SELECT fingerprint,reply FROM receipts WHERE request_id=?1", [&request.request_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional() {
            Ok(None) => Lookup::New,
            Ok(Some((fingerprint,reply))) if fingerprint == request.fingerprint() => serde_json::from_str(&reply).map(Lookup::Replay).unwrap_or(Lookup::Unavailable),
            Ok(Some(_)) => Lookup::Conflict,
            Err(_) => Lookup::Unavailable,
        }
    }
    pub fn commit(&self, request: &Request, intent: Option<Intent>, reply: impl FnOnce(u64)->Reply) -> Result<Reply, DaemonError> {
        let mut state = self.lock();
        if !self.available() { return Err(errors::STATE_UNAVAILABLE) }
        let next_intent = intent.unwrap_or(state.intent);
        let revision = if next_intent != state.intent { state.revision.checked_add(1).filter(|v| *v <= i64::MAX as u64).ok_or(errors::STATE_UNAVAILABLE)? } else { state.revision };
        let tx = storage(state.db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate))?;
        let known: bool = storage(tx.query_row("SELECT EXISTS(SELECT 1 FROM receipts WHERE request_id=?1)", [&request.request_id], |r| r.get(0)))?;
        if known { return Err(errors::REQUEST_ID_REUSED) }
        let reply = reply(revision);
        let raw = serde_json::to_string(&reply).map_err(|_| errors::STATE_UNAVAILABLE)?;
        if raw.len() > super::protocol::MAX_FRAME { return Err(errors::STATE_UNAVAILABLE) }
        storage(tx.execute("INSERT INTO receipts VALUES(?1,?2,?3,?4)", params![request.request_id,request.fingerprint(),now_ms(),raw]))?;
        storage(tx.execute("UPDATE control SET revision=?1,intent=?2 WHERE singleton=1", params![revision,next_intent.key()]))?;
        storage(tx.commit())?;
        state.revision = revision;
        state.intent = next_intent;
        Ok(reply)
    }
    pub fn bump(&self) -> Result<u64, DaemonError> {
        let mut state = self.lock();
        if !self.available() { return Err(errors::STATE_UNAVAILABLE) }
        let revision = state.revision.checked_add(1).filter(|v| *v <= i64::MAX as u64).ok_or(errors::STATE_UNAVAILABLE)?;
        storage(state.db.execute("UPDATE control SET revision=?1 WHERE singleton=1", [revision]))?;
        state.revision = revision;
        Ok(revision)
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::{Command, Reply};
    use super::*;

    pub(crate) fn private_dir() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        temp
    }

    fn request(id: &str, command: Command) -> Request {
        Request::new(id, Some("@o:s".into()), command)
    }

    #[test]
    fn intent_and_receipt_commit_together_and_survive_restart() {
        let temp = private_dir();
        let path = temp.path().join("control.json");
        let store = ControlStore::open(&path).unwrap();
        assert_eq!(store.revision(), 1);
        assert_eq!(store.intent(), Intent::Hosting);
        let pause = request("p1", Command::PauseHosting);
        let reply = store
            .commit(&pause, Some(Intent::Paused), |rev| {
                Reply::ok("p1", rev, serde_json::json!({"rev":rev}))
            })
            .unwrap();
        assert_eq!(reply.revision, 2);
        drop(store);
        let reopened = ControlStore::open(&path).unwrap();
        assert_eq!(reopened.intent(), Intent::Paused);
        // Each daemon start advances the revision once.
        assert_eq!(reopened.revision(), 3);
        match reopened.lookup(&pause) {
            Lookup::Replay(saved) => assert_eq!(saved, reply),
            _ => panic!("receipt lost across restart"),
        }
        let reused = request("p1", Command::StopHosting);
        assert!(matches!(reopened.lookup(&reused), Lookup::Conflict));
        assert_eq!(
            reopened
                .commit(&reused, Some(Intent::Stopped), |r| Reply::ok("p1", r, serde_json::json!({})))
                .unwrap_err(),
            errors::REQUEST_ID_REUSED
        );
        assert_eq!(reopened.intent(), Intent::Paused);
    }

    #[test]
    fn unchanged_intent_records_receipt_without_revision_change() {
        let temp = private_dir();
        let store = ControlStore::open(&temp.path().join("control.json")).unwrap();
        let before = store.revision();
        store
            .commit(&request("a", Command::ResumeHosting), Some(Intent::Hosting), |r| {
                Reply::ok("a", r, serde_json::json!({}))
            })
            .unwrap();
        assert_eq!(store.revision(), before);
        assert!(matches!(
            store.lookup(&request("a", Command::ResumeHosting)),
            Lookup::Replay(_)
        ));
    }

    #[test]
    fn delayed_receipts_survive_the_former_retention_limit_and_restart() {
        let temp = private_dir();
        let store = ControlStore::open(&temp.path().join("control.json")).unwrap();
        for i in 0..LEGACY_RECEIPTS + 3 {
            let id = format!("r{i}");
            store
                .commit(&request(&id, Command::PauseHosting), None, |r| {
                    Reply::ok(&id, r, serde_json::json!({}))
                })
                .unwrap();
        }
        assert!(matches!(
            store.lookup(&request("r0", Command::PauseHosting)),
            Lookup::Replay(_)
        ));
        assert!(matches!(
            store.lookup(&request(&format!("r{}", LEGACY_RECEIPTS + 2), Command::PauseHosting)),
            Lookup::Replay(_)
        ));
        drop(store);
        let store = ControlStore::open(&temp.path().join("control.json")).unwrap();
        assert!(matches!(store.lookup(&request("r0", Command::PauseHosting)), Lookup::Replay(_)));
        assert!(matches!(store.lookup(&request("r0", Command::StopHosting)), Lookup::Conflict));
    }

    #[test]
    fn failed_persistence_applies_nothing() {
        let temp = private_dir();
        let dir = temp.path().join("gone");
        hagency_store::private::directory(&dir).unwrap();
        let store = ControlStore::open(&dir.join("control.json")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let error = store
            .commit(&request("x", Command::StopHosting), Some(Intent::Stopped), |r| {
                Reply::ok("x", r, serde_json::json!({}))
            })
            .unwrap_err();
        assert_eq!(error, errors::STATE_UNAVAILABLE);
        assert_eq!(store.intent(), Intent::Hosting);
        assert!(matches!(
            store.lookup(&request("x", Command::StopHosting)),
            Lookup::Unavailable
        ));
    }

    #[test]
    fn corrupt_or_foreign_state_is_refused() {
        let temp = private_dir();
        let path = temp.path().join("control.json");
        hagency_store::private::replace(&path, b"{\"format\":\"other\"}").unwrap();
        assert!(ControlStore::open(&path).is_err());
    }
}
