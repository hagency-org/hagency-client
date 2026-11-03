//! Private, bounded SQLite persistence for owner telemetry.
//!
//! The host (owner daemon) chooses the directory: its private daemon
//! metadata, never a new file inside an adopted owner root. Rows hold only
//! trusted attribution IDs, normalized counts, provenance and sanitized
//! quota windows; no message text, prompts, credentials or raw provider
//! payloads. Inserts are idempotent: a row's key is a digest of its full
//! content, so re-delivery is a no-op while a conflicting payload for the
//! same identity is kept and replayed as a conflict.

use hagency_metering::dashboard::ledger::{ExecutionBinding, LedgerRecord, UsageEvent, UsageLedger};
use hagency_metering::dashboard::quota::AccountRegistration;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const STORE_FILE: &str = "owner-telemetry.sqlite3";
pub const SCHEMA_VERSION: i64 = 2;
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
            Some(1) => { db.execute("UPDATE meta SET value=?1 WHERE key='schema'", [SCHEMA_VERSION])?; },
            Some(SCHEMA_VERSION) => {}
            Some(_) => return Err(StoreError::Schema),
        }
        Ok(Self { db, path })
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
        let checkpoint = loaded.ledger.export_json().map_err(|_| StoreError::Capacity)?;
        let tx = self.db.unchecked_transaction()?;
        tx.execute("INSERT INTO checkpoint VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET payload=excluded.payload", [checkpoint])?;
        tx.execute("DELETE FROM usage_events", [])?;
        tx.commit()?;
        Ok(())
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
        })
    }
}
