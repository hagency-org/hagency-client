//! Durable admission before effects. Pending after a crash is unknown, never
//! replayed as a second mutation. Native journals retain original command IDs.
use super::{Failure, error};
use crate::native_owner::NativeError;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::{collections::{HashMap, HashSet}, path::Path, sync::Mutex};

pub struct Journal(Mutex<Connection>, Mutex<HashSet<String>>, Mutex<HashMap<String, Result<Value, Failure>>>);
const RETRY_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCommand {
    pub request_id: String,
    pub created_at_ms: u64,
    pub retry_until_ms: u64,
}

pub enum Admission {
    New,
    Pending,
    RecoveryRequired,
    Result(Result<Value, Failure>),
}
impl Journal {
    pub fn open(path: &Path) -> Result<Self, NativeError> {
        drop(
            hagency_store::private::open(path, true)
                .or_else(|_| hagency_store::private::open(path, false))
                .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?,
        );
        let path = path
            .canonicalize()
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        let db = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS calls(id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, response TEXT); CREATE TABLE IF NOT EXISTS pending(fingerprint TEXT PRIMARY KEY, id TEXT NOT NULL);")
            .map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
        // Schema evolution is one transaction; an interrupted upgrade cannot
        // leave half of the recovery contract installed.
        let tx=db.unchecked_transaction().map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
        for (table, columns) in [
            ("pending", vec![("created_at_ms","INTEGER NOT NULL DEFAULT 0"),("owner","TEXT NOT NULL DEFAULT ''"),("retry_until_ms","INTEGER NOT NULL DEFAULT 0")]),
            ("calls", vec![("recovery_version","INTEGER NOT NULL DEFAULT 0"),("native_id","TEXT")]),
        ] {
            let known=tx.prepare(&format!("PRAGMA table_info({table})"))
                .and_then(|mut s|s.query_map([],|r|r.get::<_,String>(1))?.collect::<Result<Vec<_>,_>>())
                .map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
            for (column,kind) in columns { if !known.iter().any(|name|name==column) {
                tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"))
                    .map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
            }}
        }
        tx.commit().map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
        Ok(Self(Mutex::new(db), Mutex::new(HashSet::new()), Mutex::new(HashMap::new())))
    }
    pub fn admit(&self, id: &str, fingerprint: &str) -> Result<Admission, NativeError> {
        // If persistence failed after completion, retry only the receipt write.
        // The native operation must never be invoked a second time.
        let completed = self.2.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?.get(id).cloned();
        if let Some(result) = completed { self.finish(id, &result)?; }
        let mut db = self
            .0
            .lock()
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        let existing: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT fingerprint,response FROM calls WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        let result = match existing {
            Some((old, _)) if old != fingerprint => return Err(error(409, "request_id_reused")),
            Some((_, Some(response))) => {
                let result=serde_json::from_str(&response).map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
                if super::terminal_receipt(&result) {Admission::Result(result)} else {Admission::RecoveryRequired}
            },
            Some((_, None)) => {
                if self.1.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?.contains(id) { Admission::Pending }
                else { Admission::RecoveryRequired }
            },
            None => {
                tx.execute(
                    "INSERT INTO calls(id,fingerprint,recovery_version) VALUES(?1,?2,1)",
                    params![id, fingerprint],
                )
                .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
                Admission::New
            }
        };
        tx.commit()
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        if matches!(result, Admission::New) { self.1.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?.insert(id.into()); }
        Ok(result)
    }
    /// Legacy rows have no before-effect pin contract. New rows with no pin
    /// prove this resume did not enter native execution yet.
    pub fn native_pin(&self, id: &str) -> Result<Option<Option<String>>, NativeError> {
        self.0.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?
            .query_row("SELECT recovery_version,native_id FROM calls WHERE id=?1", [id], |r| Ok((r.get::<_,u32>(0)? != 0).then(||r.get::<_,Option<String>>(1)).transpose()?))
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))
    }
    pub fn pin_native(&self, id: &str, native: &str) -> Result<(), NativeError> {
        let n = self.0.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?
            .execute("UPDATE calls SET native_id=?1 WHERE id=?2 AND recovery_version=1 AND (native_id IS NULL OR native_id=?1)", params![native,id])
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        if n != 1 { return Err(error(409, "daemon_command_recovery_required")); }
        Ok(())
    }
    pub fn finish(&self, id: &str, result: &Result<Value, Failure>) -> Result<(), NativeError> {
        self.2.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?.insert(id.into(), result.clone());
        let raw = serde_json::to_string(result)
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        {
            let mut db=self.0.lock().map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
            let tx=db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
            let previous:Option<String>=tx.query_row("SELECT response FROM calls WHERE id=?1",[id],|r|r.get(0)).map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
            let terminal=previous.map(|raw|serde_json::from_str(&raw)).transpose().map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?.as_ref().is_some_and(super::terminal_receipt);
            if !terminal {tx.execute("UPDATE calls SET response=?1 WHERE id=?2",params![raw,id]).map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;}
            tx.commit().map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
        }
        self.1.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?.remove(id);
        self.2.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?.remove(id);
        Ok(())
    }
    #[cfg(test)]
    pub fn pending(&self, fingerprint: &str) -> Result<String, NativeError> { self.pending_for("", fingerprint) }
    pub fn pending_for(&self, owner: &str, fingerprint: &str) -> Result<String, NativeError> {
        let db = self.0.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        let id = super::super::new_request_id();
        let now = super::super::control::now_ms();
        db.execute("INSERT OR IGNORE INTO pending(fingerprint,id,created_at_ms,owner,retry_until_ms) VALUES(?1,?2,?3,?4,?5)", params![fingerprint,id,now,owner,now.saturating_add(RETRY_WINDOW_MS)])
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        // Legacy rows are tied to the caller only after its full fingerprint
        // matches; age 0 requires explicit recovery and never fresh execution.
        db.execute("UPDATE pending SET owner=?1 WHERE fingerprint=?2 AND owner=''", params![owner,fingerprint])
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        let (id, until): (String,u64) = db.query_row("SELECT id,retry_until_ms FROM pending WHERE fingerprint=?1 AND owner=?2", params![fingerprint,owner], |r| Ok((r.get(0)?,r.get(1)?)))
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        if now > until { return Err(error(409, "daemon_command_receipt_expired")); }
        Ok(id)
    }
    pub fn pending_commands(&self, owner: &str) -> Result<Vec<PendingCommand>, NativeError> {
        let db = self.0.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        db.prepare("SELECT id,created_at_ms,retry_until_ms FROM pending WHERE owner=?1 ORDER BY created_at_ms,id LIMIT 256")
            .and_then(|mut s| s.query_map([owner], |r| { let created_at_ms: u64 = r.get(1)?; Ok(PendingCommand { request_id:r.get(0)?, created_at_ms, retry_until_ms:r.get(2)? }) })?.collect())
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))
    }
    /// Explicitly reopen recovery of this original command; never mint a new ID.
    pub fn resume_pending(&self, owner: &str, id: &str) -> Result<(), NativeError> {
        let n = self.0.lock().map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?
            .execute("UPDATE pending SET retry_until_ms=?1 WHERE owner=?2 AND id=?3", params![super::super::control::now_ms().saturating_add(RETRY_WINDOW_MS),owner,id])
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        if n != 1 { return Err(error(404, "daemon_command_not_found")); }
        Ok(())
    }
    /// Used only by explicit original-command recovery entrypoints, never by
    /// a new mutation with coincidentally identical arguments.
    pub fn resume_fingerprint(&self, owner:&str, fingerprint:&str) -> Result<(),NativeError> {
        let n=self.0.lock().map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?
            .execute("UPDATE pending SET retry_until_ms=?1 WHERE owner=?2 AND fingerprint=?3",params![super::super::control::now_ms().saturating_add(RETRY_WINDOW_MS),owner,fingerprint])
            .map_err(|_|error(503,"daemon_bridge_journal_unavailable"))?;
        if n!=1 { return Err(error(404,"daemon_command_not_found")); }
        Ok(())
    }
    pub fn clear(&self, fingerprint: &str, id: &str) -> Result<(), NativeError> {
        self.0
            .lock()
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?
            .execute(
                "DELETE FROM pending WHERE fingerprint=?1 AND id=?2",
                params![fingerprint, id],
            )
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_command_survives_reopen_and_is_not_executed_twice() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("journal.sqlite3");
        let journal = Journal::open(&path).unwrap();
        let id = journal.pending("owner-package-command").unwrap();
        assert!(matches!(
            journal.admit(&id, "fingerprint").unwrap(),
            Admission::New
        ));
        drop(journal);
        let journal = Journal::open(&path).unwrap();
        assert_eq!(journal.pending("owner-package-command").unwrap(), id);
        assert!(matches!(
            journal.admit(&id, "fingerprint").unwrap(),
            Admission::RecoveryRequired
        ));
        assert_eq!(
            journal.admit(&id, "other").err().unwrap().code,
            "request_id_reused"
        );
        journal
            .finish(&id, &Ok(serde_json::json!({"originalId":"native-123"})))
            .unwrap();
        drop(journal);
        let journal = Journal::open(&path).unwrap();
        let Admission::Result(Ok(value)) = journal.admit(&id, "fingerprint").unwrap() else {
            panic!("must replay receipt")
        };
        assert_eq!(value["originalId"], "native-123");
        journal.clear("owner-package-command", "wrong-id").unwrap();
        assert_eq!(journal.pending("owner-package-command").unwrap(), id);
        journal.clear("owner-package-command", &id).unwrap();
        assert_ne!(journal.pending("owner-package-command").unwrap(), id);
    }
    #[test]
    fn receipt_write_failure_retries_only_persistence_and_expiry_keeps_original_id() {
        let temp = tempfile::tempdir().unwrap();
        let journal = Journal::open(&temp.path().join("journal")).unwrap();
        let id = journal.pending_for("@owner:test", "fingerprint").unwrap();
        assert!(matches!(journal.admit(&id,"fingerprint").unwrap(),Admission::New));
        journal.0.lock().unwrap().execute_batch("PRAGMA query_only=ON").unwrap();
        assert!(journal.finish(&id,&Ok(serde_json::json!({"original":"native-1"}))).is_err());
        journal.0.lock().unwrap().execute_batch("PRAGMA query_only=OFF").unwrap();
        let Admission::Result(Ok(value))=journal.admit(&id,"fingerprint").unwrap() else {panic!("completed effects must replay")};
        assert_eq!(value["original"],"native-1");
        journal.0.lock().unwrap().execute("UPDATE pending SET retry_until_ms=1",[]).unwrap();
        assert_eq!(journal.pending_for("@owner:test","fingerprint").unwrap_err().code,"daemon_command_receipt_expired");
        assert_eq!(journal.pending_commands("@owner:test").unwrap()[0].request_id,id);
        assert!(journal.pending_commands("@other:test").unwrap().is_empty());
        assert!(journal.resume_pending("@other:test",&id).is_err());
        journal.resume_pending("@owner:test",&id).unwrap();
        assert_eq!(journal.pending_for("@owner:test","fingerprint").unwrap(),id);
    }
    #[test]
    fn native_pin_is_immutable_and_legacy_rows_cannot_claim_before_effect_contract() {
        let temp=tempfile::tempdir().unwrap();let path=temp.path().join("journal");
        let journal=Journal::open(&path).unwrap();
        journal.admit("modern","print").unwrap();
        assert_eq!(journal.native_pin("modern").unwrap(),Some(None));
        journal.pin_native("modern","begin-original").unwrap();
        assert!(journal.pin_native("modern","replacement").is_err());
        journal.0.lock().unwrap().execute("INSERT INTO calls(id,fingerprint) VALUES('legacy','print')",[]).unwrap();
        assert_eq!(journal.native_pin("legacy").unwrap(),None);
        drop(journal);let journal=Journal::open(&path).unwrap();
        assert_eq!(journal.native_pin("modern").unwrap(),Some(Some("begin-original".into())));
        assert!(matches!(journal.admit("modern","print").unwrap(),Admission::RecoveryRequired));
    }
    #[test]
    fn recorded_native_unknown_is_reconciled_to_terminal_receipt_under_same_id() {
        let temp=tempfile::tempdir().unwrap();let path=temp.path().join("journal");let journal=Journal::open(&path).unwrap();
        let id=journal.pending_for("owner","resume").unwrap();journal.admit(&id,"scope").unwrap();journal.pin_native(&id,"original-begin").unwrap();
        journal.finish(&id,&Err(Failure::from(error(503,"management_outcome_unknown")))).unwrap();
        assert!(matches!(journal.admit(&id,"scope").unwrap(),Admission::RecoveryRequired));
        assert_eq!(journal.native_pin(&id).unwrap(),Some(Some("original-begin".into())));
        journal.finish(&id,&Ok(serde_json::json!({"commandId":"original-begin"}))).unwrap();
        assert!(matches!(journal.admit(&id,"scope").unwrap(),Admission::Result(Ok(_))));
        // A late unknown reply cannot replace the proven original result.
        journal.finish(&id,&Err(Failure::from(error(503,"management_outcome_unknown")))).unwrap();
        drop(journal);let journal=Journal::open(&path).unwrap();assert!(matches!(journal.admit(&id,"scope").unwrap(),Admission::Result(Ok(_))));
    }
    #[cfg(unix)]
    #[test]
    fn journal_refuses_a_symlink_and_public_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("journal");
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Journal::open(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let alias = temp.path().join("alias");
        symlink(&path, &alias).unwrap();
        assert!(Journal::open(&alias).is_err());
    }
}
