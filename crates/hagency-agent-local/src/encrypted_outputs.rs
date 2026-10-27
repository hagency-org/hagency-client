//! Original sealed outbound payload; repeated observations cannot reseal or resend.
use crate::{Error, Ledger, Result};
use rusqlite::{OptionalExtension, TransactionBehavior};
use serde_json::Value;
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS encrypted_outputs (
 dispatch_id TEXT NOT NULL REFERENCES local_inbox(id), kind TEXT NOT NULL CHECK(kind IN ('reply','file')),
 request_id TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(dispatch_id,kind,request_id)
);
CREATE TRIGGER IF NOT EXISTS immutable_encrypted_output BEFORE UPDATE ON encrypted_outputs BEGIN SELECT RAISE(ABORT,'immutable_encrypted_output'); END;
CREATE TRIGGER IF NOT EXISTS retain_encrypted_output BEFORE DELETE ON encrypted_outputs BEGIN SELECT RAISE(ABORT,'encrypted_output_retained'); END;
"#;
pub(crate) const OBJECTS: &[&str] = &[
    "encrypted_outputs",
    "immutable_encrypted_output",
    "retain_encrypted_output",
];
impl Ledger {
    pub fn encrypted_output(
        &self,
        owner: &str,
        dispatch: &str,
        kind: &str,
        request: &str,
    ) -> Result<Option<Value>> {
        self.owner(owner)?;
        let value:Option<String>=self.db.query_row("SELECT payload FROM encrypted_outputs WHERE dispatch_id=? AND kind=? AND request_id=?",(dispatch,kind,request),|r|r.get(0)).optional()?;
        value.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
    }
    pub fn retain_encrypted_output(
        &mut self,
        owner: &str,
        dispatch: &str,
        kind: &str,
        request: &str,
        payload: &Value,
    ) -> Result<()> {
        self.owner(owner)?;
        crate::key(request)?;
        let encoded = crate::json(payload)?;
        if encoded.len() > 131072 {
            return Err(Error::Invalid("encrypted output capacity"));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM encrypted_inputs WHERE dispatch_id=?)",
            [dispatch],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::Conflict);
        }
        let old:Option<String>=tx.query_row("SELECT payload FROM encrypted_outputs WHERE dispatch_id=? AND kind=? AND request_id=?",(dispatch,kind,request),|r|r.get(0)).optional()?;
        if old.as_ref().is_some_and(|s| s != &encoded) {
            return Err(Error::Conflict);
        }
        if old.is_none() {
            tx.execute("INSERT INTO encrypted_outputs(dispatch_id,kind,request_id,payload) VALUES(?,?,?,?)",(dispatch,kind,request,&encoded))?;
        }
        tx.commit()?;
        Ok(())
    }
}
