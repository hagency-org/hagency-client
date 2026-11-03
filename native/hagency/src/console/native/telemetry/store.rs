//! Private, bounded SQLite persistence for owner telemetry.
//!
//! The host (owner daemon) chooses the directory: its private daemon
//! metadata, never a new file inside an adopted owner root. Rows hold only
//! trusted attribution IDs, normalized counts, provenance and sanitized
//! quota windows; no message text, prompts, credentials or raw provider
//! payloads. Inserts are idempotent: a row's key is a digest of its full
//! content, so re-delivery is a no-op while a conflicting payload for the
//! same identity is kept and replayed as a conflict.

use hagency_metering::dashboard::ledger::{ExecutionBinding, LedgerRecord, UsageEvent, UsageLedger, IngestOutcome};
use hagency_metering::dashboard::quota::AccountRegistration;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const STORE_FILE: &str = "owner-telemetry.sqlite3";
pub const SCHEMA_VERSION: i64 = 3;
/// Compact the event journal into a full ledger checkpoint at this size.
/// A checkpoint preserves cumulative baselines, dedup keys, and attribution.
pub const MAX_EVENT_ROWS: i64 = 200_000;
pub const MAX_BINDING_ROWS: i64 = 65_536;
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum StoreError {
    #[error("telemetry store path is not a private directory")]
    Path,
    #[error("telemetry store is unavailable")]
    Unavailable,
    #[error("telemetry store schema is unsupported")]
    Schema,
    #[error("telemetry row exceeds its bound")]
    Capacity,
}

impl From<rusqlite::Error> for StoreError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Unavailable
    }
}

pub(super) struct Store {
    db: Connection,
    path: PathBuf,
}

/// Restored state plus what was dropped while loading.
pub(super) struct Loaded {
    pub ledger: UsageLedger,
    pub accounts: Vec<AccountRegistration>,
    pub quotas: Vec<(String, String, serde_json::Value, u64)>,
    pub bindings: usize,
    pub events: usize,
    pub rejected_rows: usize,
    pub history_incomplete: bool,
}

fn key(kind: &str, payload: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    digest.update([0]);
    digest.update(payload);
    format!("{:x}", digest.finalize())
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, StoreError> {
    let bytes = serde_json::to_vec(value).map_err(|_| StoreError::Capacity)?;
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(StoreError::Capacity);
    }
    Ok(bytes)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    (bytes.len() <= MAX_PAYLOAD_BYTES)
        .then(|| serde_json::from_slice(bytes).ok())
        .flatten()
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|_| StoreError::Path)?;
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|_| StoreError::Path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(StoreError::Path);
    }
    if meta.uid() != rustix::process::geteuid().as_raw() || meta.permissions().mode() & 0o077 != 0 {
        return Err(StoreError::Path);
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_dir(dir: &Path) -> Result<(), StoreError> {
    std::fs::create_dir_all(dir).map_err(|_| StoreError::Path)
}

impl Store {
    /// Open (creating if needed) the store inside an existing or new private
    /// directory owned by this user with no group/other access.
    pub(super) fn open(dir: &Path, now_ms: u64) -> Result<Self, StoreError> {
        private_dir(dir)?;
        let path = dir.join(STORE_FILE);
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(StoreError::Path);
        }
        let file = match hagency_store::private::open(&path, true) {
            Ok(file) => file,
            Err(hagency_store::Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists =>
                hagency_store::private::open(&path, false).map_err(|_| StoreError::Path)?,
            Err(_) => return Err(StoreError::Path),
        };
        file.sync_all().map_err(|_| StoreError::Unavailable)?;
        let path = path.canonicalize().map_err(|_| StoreError::Path)?;
        let db = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(std::time::Duration::from_millis(500))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS checkpoint(singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload BLOB NOT NULL);
             CREATE TABLE IF NOT EXISTS bindings(id TEXT PRIMARY KEY, execution_id TEXT NOT NULL, payload BLOB NOT NULL, at_ms INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS usage_events(id TEXT PRIMARY KEY, execution_id TEXT NOT NULL, epoch TEXT NOT NULL, payload BLOB NOT NULL, at_ms INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS usage_events_at ON usage_events(at_ms);
             CREATE TABLE IF NOT EXISTS archives(id INTEGER PRIMARY KEY, payload BLOB NOT NULL, min_ms INTEGER NOT NULL, max_ms INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS archive_range ON archives(max_ms,min_ms);
             CREATE TABLE IF NOT EXISTS quarantined_telemetry(kind TEXT NOT NULL,id TEXT NOT NULL,payload BLOB NOT NULL,PRIMARY KEY(kind,id));
             CREATE TABLE IF NOT EXISTS archived_executions(execution_id TEXT PRIMARY KEY, archive_id INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS archived_execution_shard ON archived_executions(archive_id);
             CREATE TABLE IF NOT EXISTS accounts(account_id TEXT PRIMARY KEY, payload BLOB NOT NULL);
             CREATE TABLE IF NOT EXISTS quota(account_id TEXT NOT NULL, source TEXT NOT NULL, payload BLOB NOT NULL, at_ms INTEGER NOT NULL, PRIMARY KEY(account_id, source));",
        )?;
        let version: Option<i64> = db
            .query_row("SELECT value FROM meta WHERE key='schema'", [], |r| r.get(0))
            .optional()?;
        match version {
            None => {
                db.execute("INSERT INTO meta VALUES('schema',?1)", [SCHEMA_VERSION])?;
                db.execute("INSERT INTO meta VALUES('coverage_start_ms',?1)", [now_ms as i64])?;
            }
            Some(1 | 2) => { db.execute("UPDATE meta SET value=?1 WHERE key='schema'", [SCHEMA_VERSION])?; },
            Some(SCHEMA_VERSION) => {}
            Some(_) => return Err(StoreError::Schema),
        }
        Ok(Self { db, path })
    }

    pub(super) fn reader(&self) -> Result<Self, StoreError> {
        let db=Connection::open_with_flags(&self.path,OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX|OpenFlags::SQLITE_OPEN_NOFOLLOW)?;
        db.busy_timeout(std::time::Duration::from_millis(500))?;
        db.execute_batch("BEGIN DEFERRED")?;
        // Pin the same SQLite snapshot as the hot-ledger clone, before the
        // owner telemetry mutex is released. Rollover cannot double count.
        let _: i64=db.query_row("SELECT value FROM meta WHERE key='schema'",[],|r|r.get(0))?;
        Ok(Self{db,path:self.path.clone()})
    }
    pub(super) fn archived_count(&self) -> Result<u64, StoreError> {
        Ok(self.db.query_row("SELECT value FROM meta WHERE key='archived_count'",[],|r|r.get(0)).optional()?.unwrap_or(0))
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn coverage_start_ms(&self) -> Result<u64, StoreError> {
        let value: i64 = self
            .db
            .query_row("SELECT value FROM meta WHERE key='coverage_start_ms'", [], |r| r.get(0))?;
        u64::try_from(value).map_err(|_| StoreError::Schema)
    }

    /// Returns whether a new row was written.
    pub(super) fn put_binding(&self, binding: &ExecutionBinding, at_ms: u64) -> Result<bool, StoreError> {
        if let Some(previous) = self.archived_binding(&binding.execution_id)? {
            return if previous == *binding { Ok(false) } else { Err(StoreError::Schema) };
        }
        if let Some(parent)=binding.parent_execution_id.as_deref() {
            if let Some((id,mut ledger))=self.archive_for(parent)? {
                let added=ledger.bind_execution(binding.clone()).map_err(|_|StoreError::Schema)?;
                self.replace_archive(id,&ledger,u64::from(added))?;
                return Ok(added);
            }
        }
        let payload = encode(binding)?;
        let id = key("binding", &payload);
        let count: i64 = self.db.query_row("SELECT COUNT(*) FROM bindings", [], |r| r.get(0))?;
        if count >= MAX_BINDING_ROWS {
            return Err(StoreError::Capacity);
        }
        Ok(self.db.execute(
            "INSERT OR IGNORE INTO bindings VALUES(?1,?2,?3,?4)",
            params![id, binding.execution_id, payload, at_ms as i64],
        )? == 1)
    }

    pub(super) fn put_event(&self, event: &UsageEvent, epoch: &str) -> Result<bool, StoreError> {
        if let Some(outcome) = self.archived_event(event)? { return Ok(outcome == IngestOutcome::Applied); }
        let payload = encode(event)?;
        let id = key("usage", &payload);
        let inserted = self.db.execute(
            "INSERT OR IGNORE INTO usage_events VALUES(?1,?2,?3,?4,?5)",
            params![id, event.execution_id, epoch, payload, event.observed_at_ms as i64],
        )? == 1;
        if inserted {
            self.prune_events()?;
        }
        Ok(inserted)
    }

    fn prune_events(&self) -> Result<(), StoreError> {
        let count: i64 = self.db.query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))?;
        if count <= MAX_EVENT_ROWS {
            return Ok(());
        }
        self.checkpoint()
    }

    pub(super) fn checkpoint(&self) -> Result<(), StoreError> {
        // Keep the stream high-water marks and event-ID dedup state along
        // with historical buckets. Dropping just the earliest cumulative
        // event would re-count its whole baseline on the next restart.
        let loaded = self.load()?;
        if loaded.rejected_rows != 0 { return Err(StoreError::Schema); }
        self.replace_checkpoint(&loaded.ledger)
    }

    /// The caller holds the telemetry mutex; the checkpoint and both journals
    /// move together, preserving retained IDs and high-water marks on reopen.
    pub(super) fn replace_checkpoint(&self, ledger: &UsageLedger) -> Result<(), StoreError> {
        let checkpoint = ledger.export_json().map_err(|_| StoreError::Capacity)?;
        let tx = self.db.unchecked_transaction()?;
        tx.execute("INSERT INTO checkpoint VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET payload=excluded.payload", [checkpoint])?;
        tx.execute("DELETE FROM usage_events", [])?;
        tx.execute("DELETE FROM bindings", [])?;
        tx.commit()?;
        Ok(())
    }

    pub(super) fn mark_incomplete(&self) -> Result<(), StoreError> {
        self.db.execute("INSERT INTO meta VALUES('history_incomplete',1) ON CONFLICT(key) DO UPDATE SET value=1",[])?;Ok(())
    }
    pub(super) fn archive(&self, archived: &UsageLedger, current: &UsageLedger, quarantine:bool) -> Result<(), StoreError> {
        let shards=archived.clone().into_shards(4*1024*1024);
        let payloads=shards.iter().map(|s|s.export_json().map_err(|_|StoreError::Capacity)).collect::<Result<Vec<_>,_>>()?;
        let current_bytes = current.export_json().map_err(|_| StoreError::Capacity)?;
        let tx = self.db.unchecked_transaction()?;
        for (archived,bytes) in shards.iter().zip(payloads) {
            let (min,max)=archived.time_bounds().ok_or(StoreError::Schema)?;
            tx.execute("INSERT INTO archives(payload,min_ms,max_ms) VALUES(?1,?2,?3)", params![bytes,min.min(i64::MAX as u64),max.min(i64::MAX as u64)])?;
            let id = tx.last_insert_rowid();
            for binding in archived.bindings() { tx.execute("INSERT INTO archived_executions VALUES(?1,?2)", params![binding.execution_id,id])?; }
        }
        tx.execute("INSERT INTO meta VALUES('archived_count',?1) ON CONFLICT(key) DO UPDATE SET value=value+excluded.value",[archived.execution_count() as i64])?;
        tx.execute("INSERT INTO checkpoint VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET payload=excluded.payload", [current_bytes])?;
        if quarantine {
            tx.execute("INSERT OR IGNORE INTO quarantined_telemetry SELECT 'binding',id,payload FROM bindings",[])?;
            tx.execute("INSERT OR IGNORE INTO quarantined_telemetry SELECT 'event',id,payload FROM usage_events",[])?;
            tx.execute("INSERT INTO meta VALUES('history_incomplete',1) ON CONFLICT(key) DO UPDATE SET value=1",[])?;
        }
        tx.execute("DELETE FROM usage_events", [])?;tx.execute("DELETE FROM bindings", [])?;tx.commit()?;
        Ok(())
    }
    /// Read one shard at a time, keeping memory bounded independently of
    /// lifetime execution count. No archive is ever discarded for capacity.
    pub(super) fn visit_archives(&self, range: hagency_metering::dashboard::period::TimeRange, mut visit: impl FnMut(&UsageLedger)) -> Result<(), StoreError> {
        let mut statement = self.db.prepare("SELECT payload FROM archives WHERE max_ms>=?1 AND min_ms<?2 ORDER BY id")?;
        for row in statement.query_map(params![range.start_ms.min(i64::MAX as u64),range.end_ms.min(i64::MAX as u64)], |r| r.get::<_,Vec<u8>>(0))? {
            let ledger = UsageLedger::import_json(&row?).map_err(|_| StoreError::Schema)?;
            visit(&ledger);
        }
        Ok(())
    }
    fn archive_for(&self, execution_id: &str) -> Result<Option<(i64, UsageLedger)>, StoreError> {
        let row: Option<(i64,Vec<u8>)> = self.db.query_row("SELECT a.id,a.payload FROM archived_executions e JOIN archives a ON a.id=e.archive_id WHERE e.execution_id=?1", [execution_id], |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        row.map(|(id,bytes)| Ok((id,UsageLedger::import_json(&bytes).map_err(|_| StoreError::Schema)?))).transpose()
    }
    pub(super) fn archived_binding(&self, id: &str) -> Result<Option<ExecutionBinding>, StoreError> {
        Ok(self.archive_for(id)?.and_then(|(_,ledger)|ledger.binding(id).cloned()))
    }
    fn replace_archive(&self,id:i64,ledger:&UsageLedger,added:u64)->Result<(),StoreError> {
        let shards=ledger.clone().into_shards(4*1024*1024);
        let bytes=shards.iter().map(|s|s.export_json().map_err(|_|StoreError::Capacity)).collect::<Result<Vec<_>,_>>()?;
        let tx=self.db.unchecked_transaction()?;
        tx.execute("DELETE FROM archived_executions WHERE archive_id=?1",[id])?;
        tx.execute("DELETE FROM archives WHERE id=?1",[id])?;
        for (shard,payload) in shards.iter().zip(bytes) {
            let (min,max)=shard.time_bounds().ok_or(StoreError::Schema)?;
            tx.execute("INSERT INTO archives(payload,min_ms,max_ms) VALUES(?1,?2,?3)",params![payload,min.min(i64::MAX as u64),max.min(i64::MAX as u64)])?;
            let key=tx.last_insert_rowid();
            for binding in shard.bindings(){tx.execute("INSERT INTO archived_executions VALUES(?1,?2)",params![binding.execution_id,key])?;}
        }
        if added>0 {tx.execute("INSERT INTO meta VALUES('archived_count',?1) ON CONFLICT(key) DO UPDATE SET value=value+excluded.value",[added])?;}
        tx.commit()?;Ok(())
    }
    pub(super) fn archived_event(&self, event: &UsageEvent) -> Result<Option<IngestOutcome>, StoreError> {
        let Some((id,mut ledger)) = self.archive_for(&event.execution_id)? else { return Ok(None) };
        let outcome = ledger.ingest(event).map_err(|_| StoreError::Schema)?;
        if ledger.encoded_size()>8*1024*1024 {self.replace_archive(id,&ledger,0)?;return Ok(Some(outcome));}
        let bytes = ledger.export_json().map_err(|_| StoreError::Capacity)?;
        let (min,max)=ledger.time_bounds().ok_or(StoreError::Schema)?;
        self.db.execute("UPDATE archives SET payload=?1,min_ms=?3,max_ms=?4 WHERE id=?2", params![bytes,id,min.min(i64::MAX as u64),max.min(i64::MAX as u64)])?;
        Ok(Some(outcome))
    }

    pub(super) fn put_account(&self, account_id: &str, registration: &AccountRegistration) -> Result<(), StoreError> {
        let payload = encode(registration)?;
        self.db.execute(
            "INSERT INTO accounts VALUES(?1,?2) ON CONFLICT(account_id) DO UPDATE SET payload=excluded.payload",
            params![account_id, payload],
        )?;
        Ok(())
    }

    /// Latest sanitized quota observation per account and source family.
    pub(super) fn put_quota(
        &self,
        account_id: &str,
        source: &str,
        payload: &serde_json::Value,
        at_ms: u64,
    ) -> Result<(), StoreError> {
        // Partial Codex updates replace only one limit bucket. Keep each
        // bucket's latest observation so restoring B never discards A.
        let source = if source == super::CODEX_UPDATED {
            let bucket = payload["rateLimits"]["limitId"].as_str().unwrap_or("primary");
            format!("{source}/{bucket}")
        } else { source.to_owned() };
        let payload = encode(payload)?;
        self.db.execute(
            "INSERT INTO quota VALUES(?1,?2,?3,?4) ON CONFLICT(account_id, source)
             DO UPDATE SET payload=excluded.payload, at_ms=excluded.at_ms WHERE excluded.at_ms >= quota.at_ms",
            params![account_id, source, payload, at_ms as i64],
        )?;
        Ok(())
    }

    /// Rebuild the ledger. Rows that fail to decode or that the ledger
    /// refuses are counted, never silently converted into zero usage.
    pub(super) fn load(&self) -> Result<Loaded, StoreError> {
        let checkpoint: Option<Vec<u8>> = self.db.query_row("SELECT payload FROM checkpoint WHERE singleton=1", [], |r| r.get(0)).optional()?;
        let mut ledger = match checkpoint {
            Some(bytes) => UsageLedger::import_json(&bytes).map_err(|_| StoreError::Schema)?,
            None => UsageLedger::new(self.coverage_start_ms()?),
        };
        let mut rejected_rows = 0;
        let mut bindings = 0;
        let mut statement = self.db.prepare("SELECT payload FROM bindings ORDER BY at_ms, id")?;
        let rows = statement.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
        for row in rows {
            let applied = decode::<ExecutionBinding>(&row?)
                .is_some_and(|binding| ledger.apply(&LedgerRecord::Bind(binding)).is_ok());
            if applied {
                bindings += 1;
            } else {
                rejected_rows += 1;
            }
        }
        let mut events = 0;
        let mut statement = self.db.prepare("SELECT payload FROM usage_events ORDER BY at_ms, id")?;
        let rows = statement.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
        for row in rows {
            match decode::<UsageEvent>(&row?) {
                Some(event) if ledger.ingest(&event).is_ok() => events += 1,
                _ => rejected_rows += 1,
            }
        }
        let mut accounts = Vec::new();
        let mut statement = self.db.prepare("SELECT payload FROM accounts ORDER BY account_id")?;
        let rows = statement.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
        for row in rows {
            match decode::<AccountRegistration>(&row?) {
                Some(account) => accounts.push(account),
                None => rejected_rows += 1,
            }
        }
        let mut quotas = Vec::new();
        let mut statement = self
            .db
            .prepare("SELECT account_id, source, payload, at_ms FROM quota ORDER BY at_ms")?;
        let rows = statement.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?, r.get::<_, i64>(3)?))
        })?;
        for row in rows {
            let (account, source, payload, at) = row?;
            match (decode::<serde_json::Value>(&payload), u64::try_from(at)) {
                (Some(value), Ok(at)) => quotas.push((account, source.split('/').next().unwrap_or(&source).to_owned(), value, at)),
                _ => rejected_rows += 1,
            }
        }
        Ok(Loaded {
            ledger,
            accounts,
            quotas,
            bindings,
            events,
            rejected_rows,
            history_incomplete: rejected_rows>0 || self.db.query_row("SELECT value FROM meta WHERE key='history_incomplete'",[],|r|r.get::<_,i64>(0)).optional()?.is_some_and(|v|v!=0),
        })
    }
}
