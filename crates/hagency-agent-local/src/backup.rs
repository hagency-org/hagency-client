//! Complete current-schema owner ledger snapshots. Provider/Matrix login files
//! are outside this database. A checksum detects corruption, not authorship.
//! The host authenticates the profile, retains private file custody and stops all
//! local execution before restore. Restoration never overwrites populated state.
use crate::{APP_ID, Error, Ledger, Result};
use rusqlite::{Connection, MAIN_DB, backup::Backup, backup::StepResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"HAGLED1\n";
const HEADER_LIMIT: usize = 16 * 1024;
pub const MAX_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Summary {
    pub version: u32,
    pub owner: String,
    pub profile: String,
    pub created_at: i64,
    pub database_bytes: usize,
    pub database_sha256: String,
    pub bindings: u64,
    pub calls: u64,
    pub pending_calls: u64,
    pub inbox_records: u64,
    pub pending_replies: u64,
}

/// Validated snapshot, bound to the exact bytes inspected by the owner. Private
/// fields prevent a caller from substituting content after inspection.
pub struct Snapshot {
    bytes: Vec<u8>,
    summary: Summary,
    digest: String,
}
impl Snapshot {
    pub fn summary(&self) -> &Summary {
        &self.summary
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Parse and validate without creating or migrating a database on disk.
    pub fn inspect(bytes: Vec<u8>, owner: &str, profile: &str) -> Result<Self> {
        if bytes.len() > MAX_BYTES || bytes.len() < 12 || &bytes[..8] != MAGIC {
            return Err(Error::Invalid("ledger snapshot format"));
        }
        let header_len = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
        if header_len > HEADER_LIMIT || header_len > bytes.len() - 12 {
            return Err(Error::Invalid("ledger snapshot header"));
        }
        let summary: Summary = serde_json::from_slice(&bytes[12..12 + header_len])?;
        if summary.version != 1 || summary.created_at < 0 {
            return Err(Error::Invalid("ledger snapshot version or time"));
        }
        if summary.owner != owner || summary.profile != profile {
            return Err(Error::Unauthorized);
        }
        let payload = &bytes[12 + header_len..];
        if payload.len() != summary.database_bytes
            || payload.len() < 100
            || checksum(payload) != summary.database_sha256
        {
            return Err(Error::Invalid("ledger snapshot checksum"));
        }
        let db = decode(payload, true)?;
        validate(&db, owner, profile)?;
        if describe(&db, owner, profile, summary.created_at, payload)? != summary {
            return Err(Error::Invalid("ledger snapshot summary"));
        }
        let digest = checksum(&bytes);
        Ok(Self {
            bytes,
            summary,
            digest,
        })
    }
    fn database(&self) -> Result<Connection> {
        let header_len = u32::from_be_bytes(self.bytes[8..12].try_into().unwrap()) as usize;
        let db = decode(&self.bytes[12 + header_len..], false)?;
        validate(&db, &self.summary.owner, &self.summary.profile)?;
        // This additive metadata migration never invents historical dates or
        // alerts. Older version-1 snapshots retain their original call rows.
        db.execute_batch(crate::usage::SCHEMA)?;
        db.execute_batch(crate::audit::SCHEMA)?;
        db.execute_batch(crate::recovery::SCHEMA)?;
        db.execute_batch(crate::received_files::SCHEMA)?;
        db.execute_batch(crate::sent_files::SCHEMA)?;
        db.execute_batch(crate::sent_files::HISTORY_SCHEMA)?;
        db.execute_batch(crate::remote_approval::SCHEMA)?;
        db.execute_batch(crate::remote_approval::review::SCHEMA)?;
        Ok(db)
    }
}

impl Ledger {
    /// SQLite's online backup reads all tables from one committed snapshot,
    /// including WAL pages and unresolved reservations; never copies live files.
    pub fn snapshot(&self, owner: &str, profile: &str, now: i64) -> Result<Snapshot> {
        self.owner(owner)?;
        if now < 0 {
            return Err(Error::Invalid("time"));
        }
        let source = self.db.unchecked_transaction()?;
        // Freeze and bound the same read transaction before allocating its copy.
        let _: u64 = source.query_row("SELECT count(*) FROM identity", [], |r| r.get(0))?;
        let pages: u64 = source.pragma_query_value(None, "page_count", |r| r.get(0))?;
        let page_size: u64 = source.pragma_query_value(None, "page_size", |r| r.get(0))?;
        if pages.saturating_mul(page_size) > (MAX_BYTES - HEADER_LIMIT - 12) as u64 {
            return Err(Error::Invalid("ledger snapshot size"));
        }
        let mut db = Connection::open_in_memory()?;
        copy(&source, &mut db)?;
        source.rollback()?;
        // A WAL-mode page image still names sidecar journals. Materialize the
        // isolated copy in rollback-journal mode before serializing it.
        db.pragma_update(None, "journal_mode", "DELETE")?;
        db.execute_batch("VACUUM")?;
        validate(&db, owner, profile)?;
        let pages: u64 = db.pragma_query_value(None, "page_count", |r| r.get(0))?;
        let page_size: u64 = db.pragma_query_value(None, "page_size", |r| r.get(0))?;
        if pages.saturating_mul(page_size) > (MAX_BYTES - HEADER_LIMIT - 12) as u64 {
            return Err(Error::Invalid("ledger snapshot size"));
        }
        let payload = db.serialize(MAIN_DB)?;
        let summary = describe(&db, owner, profile, now, &payload)?;
        let header = serde_json::to_vec(&summary)?;
        if header.len() > HEADER_LIMIT || payload.len() + header.len() + 12 > MAX_BYTES {
            return Err(Error::Invalid("ledger snapshot size"));
        }
        let mut bytes = Vec::with_capacity(12 + header.len() + payload.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(header.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&payload);
        // Exercise the same decoder used on import, including SQLite's
        // serialized journal-mode handling, before publishing any backup.
        Snapshot::inspect(bytes, owner, profile)
    }
    /// Restore only into a freshly initialized exact-profile ledger. A live or
    /// historical populated ledger is refused: an older backup cannot erase
    /// charged usage, consumed approvals or deduplication witnesses. All source
    /// records, holds and original replies survive byte-for-byte logically.
    /// The host must stop all runtime owners and hold its management lock.
    pub fn restore_snapshot(
        &mut self,
        owner: &str,
        profile: &str,
        snapshot: &Snapshot,
    ) -> Result<()> {
        self.restore_snapshot_checked(owner, profile, snapshot, || Ok(()))
    }
    /// The host can revoke a finite authorization proof before either admission
    /// or commit. An error rolls back all imported rows.
    pub fn restore_snapshot_checked(
        &mut self,
        owner: &str,
        profile: &str,
        snapshot: &Snapshot,
        mut authorize: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        authorize()?;
        self.owner(owner)?;
        if snapshot.summary.owner != owner || snapshot.summary.profile != profile {
            return Err(Error::Unauthorized);
        }
        let source = snapshot.database()?;
        validate(&source, owner, profile)?;
        validate(&self.db, owner, profile)?;
        // BEGIN IMMEDIATE excludes another SQLite writer through validation and
        // copying. Use row insertion rather than file replacement, retaining
        // the original database handle and one atomic commit (including WAL).
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut tables = data_tables(&tx)?;
        // The final-absence INSERT guard validates its original sent-file row.
        // Keep that guard active during restore; FK deferral alone does not
        // defer triggers, so restore this dependent evidence after its witness.
        tables.sort_by_key(|table| table == "sent_file_absence");
        for table in &tables {
            let count: u64 = tx.query_row(
                &format!("SELECT count(*) FROM {}", quoted(table)),
                [],
                |r| r.get(0),
            )?;
            if count != 0 {
                return Err(Error::Conflict);
            }
        }
        // Defer FKs: table order is deterministic, not dependency order.
        tx.pragma_update(None, "defer_foreign_keys", true)?;
        for table in &tables {
            let mut select = source.prepare(&format!("SELECT * FROM {}", quoted(table)))?;
            let columns = select.column_count();
            let placeholders = vec!["?"; columns].join(",");
            let mut insert = tx.prepare(&format!(
                "INSERT INTO {} VALUES ({placeholders})",
                quoted(table)
            ))?;
            let mut rows = select.query([])?;
            while let Some(row) = rows.next()? {
                let values = (0..columns)
                    .map(|i| row.get::<_, rusqlite::types::Value>(i))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                insert.execute(rusqlite::params_from_iter(values))?;
            }
        }
        // Limits are configuration, not an execution record; replace the fresh
        // default without deleting protected identity/profile witnesses.
        let (records, bytes): (u64, u64) = source.query_row(
            "SELECT max_records,max_bytes FROM inbox_limits WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        tx.execute(
            "UPDATE inbox_limits SET max_records=?,max_bytes=? WHERE singleton=1",
            (records, bytes),
        )?;
        authorize()?;
        tx.commit()?;
        Ok(())
    }
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn copy(source: &Connection, destination: &mut Connection) -> Result<()> {
    match Backup::new(source, destination)?.step(-1)? {
        StepResult::Done => Ok(()),
        _ => Err(Error::Conflict),
    }
}
fn decode(payload: &[u8], read_only: bool) -> Result<Connection> {
    let mut db = Connection::open_in_memory()?;
    db.deserialize_read_exact(MAIN_DB, payload, payload.len(), read_only)?;
    db.pragma_update(None, "trusted_schema", false)?;
    Ok(db)
}
fn objects(db: &Connection) -> Result<Vec<(String, String, String)>> {
    Ok(db.prepare("SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name")?
        .query_map([], |r| Ok((r.get(0)?,r.get(1)?,schema_text(&r.get::<_,String>(2)?))))?
        .collect::<std::result::Result<Vec<_>,_>>()?)
}
fn schema_text(sql: &str) -> String {
    // ALTER TABLE can add whitespace. Keep quoted CHECK values and identifiers.
    let mut quote = None;
    sql.chars()
        .filter(|c| {
            if quote == Some(*c) {
                quote = None;
            } else if quote.is_none() && ['\'', '"', '`'].contains(c) {
                quote = Some(*c);
            }
            quote.is_some() || !c.is_whitespace()
        })
        .collect()
}
fn data_tables(db: &Connection) -> Result<Vec<String>> {
    Ok(db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT IN ('identity','profile_identity','inbox_limits') ORDER BY name")?
        .query_map([], |r| r.get(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
}
fn validate(db: &Connection, owner: &str, profile: &str) -> Result<()> {
    let app: i64 = db.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if app != APP_ID || version != 1 {
        return Err(Error::ForeignDatabase);
    }
    let expected = Ledger::open_scoped(":memory:", owner, profile)?;
    let stored = objects(db)?;
    let current = objects(&expected.db)?;
    let before_review: Vec<_> = current
        .iter()
        .filter(|(_, name, _)| !crate::remote_approval::review::OBJECTS.contains(&name.as_str()))
        .cloned()
        .collect();
    let before_remote: Vec<_> = before_review
        .iter()
        .filter(|(_, name, _)| !crate::remote_approval::OBJECTS.contains(&name.as_str()))
        .cloned()
        .collect();
    let before_file_history: Vec<_> = before_remote
        .iter()
        .filter(|(_, name, _)| !crate::sent_files::HISTORY_OBJECTS.contains(&name.as_str()))
        .cloned()
        .collect();
    let before_sent: Vec<_> = before_file_history
        .iter()
        .filter(|(_, name, _)| !crate::sent_files::OBJECTS.contains(&name.as_str()))
        .cloned()
        .collect();
    let before_received: Vec<_> = before_sent
        .iter()
        .filter(|(_, name, _)| !crate::received_files::OBJECTS.contains(&name.as_str()))
        .cloned()
        .collect();
    let before_recovery: Vec<_> = before_received
        .iter()
        .filter(|(_, name, _)| !crate::recovery::OBJECTS.contains(&name.as_str()))
        .cloned()
        .collect();
    let before_audit: Vec<_> = before_recovery
        .iter()
        .filter(|(_, name, _)| !crate::audit::OBJECTS.contains(&name.as_str()))
        .cloned()
        .collect();
    let legacy: Vec<_> = before_audit
        .iter()
        .filter(|(_, name, _)| {
            !matches!(
                name.as_str(),
                "call_observations"
                    | "call_observations_time"
                    | "usage_alerts"
                    | "usage_alerts_scope"
                    | "permanent_call_time"
                    | "retain_call_time"
                    | "retain_usage_alert"
                    | "immutable_usage_alert"
            )
        })
        .cloned()
        .collect();
    if stored != current
        && stored != before_review
        && stored != before_remote
        && stored != before_file_history
        && stored != before_sent
        && stored != before_received
        && stored != before_recovery
        && stored != before_audit
        && stored != legacy
    {
        return Err(Error::ForeignDatabase);
    }
    let identities: Vec<String> = db
        .prepare("SELECT owner FROM identity")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let profiles: Vec<String> = db
        .prepare("SELECT digest FROM profile_identity")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if identities != [owner] || profiles != [profile] {
        return Err(Error::Unauthorized);
    }
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    let foreign_keys = db.prepare("PRAGMA foreign_key_check")?.exists([])?;
    if integrity != "ok" || foreign_keys {
        return Err(Error::Invalid("ledger snapshot integrity"));
    }
    Ok(())
}
fn describe(
    db: &Connection,
    owner: &str,
    profile: &str,
    created_at: i64,
    payload: &[u8],
) -> Result<Summary> {
    Ok(Summary {
        version: 1,
        owner: owner.into(),
        profile: profile.into(),
        created_at,
        database_bytes: payload.len(),
        database_sha256: checksum(payload),
        bindings: db.query_row("SELECT count(*) FROM bindings", [], |r| r.get(0))?,
        calls: db.query_row("SELECT count(*) FROM calls", [], |r| r.get(0))?,
        pending_calls: db.query_row(
            "SELECT count(*) FROM calls WHERE state IN ('pending','unknown')",
            [],
            |r| r.get(0),
        )?,
        inbox_records: db.query_row("SELECT count(*) FROM local_inbox", [], |r| r.get(0))?,
        pending_replies: db.query_row(
            "SELECT count(*) FROM local_inbox WHERE state='reply_ready'",
            [],
            |r| r.get(0),
        )?,
    })
}

#[cfg(test)]
mod tests;
