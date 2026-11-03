//! Durable admission before effects. Pending after a crash is unknown, never
//! replayed as a second mutation. Native journals retain original command IDs.
use super::{Failure, error};
use crate::native_owner::NativeError;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::{path::Path, sync::Mutex};

pub struct Journal(Mutex<Connection>);
pub enum Admission {
    New,
    Pending,
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
        Ok(Self(Mutex::new(db)))
    }
    pub fn admit(&self, id: &str, fingerprint: &str) -> Result<Admission, NativeError> {
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
            Some((_, Some(response))) => Admission::Result(
                serde_json::from_str(&response)
                    .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?,
            ),
            Some((_, None)) => Admission::Pending,
            None => {
                tx.execute(
                    "INSERT INTO calls(id,fingerprint) VALUES(?1,?2)",
                    params![id, fingerprint],
                )
                .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
                Admission::New
            }
        };
        tx.commit()
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        Ok(result)
    }
    pub fn finish(&self, id: &str, result: &Result<Value, Failure>) -> Result<(), NativeError> {
        let raw = serde_json::to_string(result)
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        self.0
            .lock()
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?
            .execute(
                "UPDATE calls SET response=?1 WHERE id=?2 AND response IS NULL",
                params![raw, id],
            )
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        Ok(())
    }
    pub fn pending(&self, fingerprint: &str) -> Result<String, NativeError> {
        let db = self
            .0
            .lock()
            .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        let id = super::super::new_request_id();
        db.execute(
            "INSERT OR IGNORE INTO pending VALUES(?1,?2)",
            params![fingerprint, id],
        )
        .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))?;
        db.query_row(
            "SELECT id FROM pending WHERE fingerprint=?1",
            [fingerprint],
            |r| r.get(0),
        )
        .map_err(|_| error(503, "daemon_bridge_journal_unavailable"))
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
            Admission::Pending
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
