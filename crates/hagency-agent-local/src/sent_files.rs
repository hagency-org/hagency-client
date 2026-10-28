//! Original, frozen-byte sending. Only the first committed insertion mints
//! custody; reopened ledgers and restored snapshots expose history only.
use crate::{
    Error, Ledger, Result, Scope,
    file_delivery::{Delivery, FileLookup, FileObservation, Manifest},
    json,
};
#[cfg(unix)]
use crate::{ToolProposal, inbox::State};
use rusqlite::OptionalExtension;
use rusqlite::TransactionBehavior;
use serde::{Deserialize, Serialize};
pub const CAPABILITY_VERSION: &str = "hagency-agent-files-v1";
pub(crate) const SCHEMA: &str = include_str!("sent_files/schema.sql");
pub(crate) const HISTORY_SCHEMA: &str = include_str!("sent_files/history_schema.sql");
pub(crate) const HISTORY_OBJECTS: &[&str] = &[
    "sent_file_absence",
    "valid_sent_file_absence",
    "retain_sent_file_absence",
    "immutable_sent_file_absence",
    "reject_absent_file_update",
];
mod history;
pub use history::{Entry, Page};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Absence {
    pub observation: FileObservation,
    pub observed_at: i64,
}
pub(crate) const OBJECTS: &[&str] = &["sent_files", "retain_sent_file", "immutable_sent_file"];
#[cfg(unix)]
mod capture;
#[cfg(unix)]
pub use capture::{Captured, Sender};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub id: String,
    pub request_id: String,
    pub manifest: Manifest,
    pub remote: Option<Delivery>,
    pub audit: String,
    pub absence: Option<Absence>,
}
#[cfg(unix)]
pub struct Custody {
    captured: Captured,
    record: Record,
}
#[cfg(unix)]
impl Custody {
    pub fn record(&self) -> &Record {
        &self.record
    }
    pub fn bytes(&self) -> &[u8] {
        &self.captured.bytes
    }
    pub fn proposal(&self) -> &ToolProposal {
        &self.captured.proposal
    }
}
#[cfg(unix)]
fn running(ledger: &Ledger, scope: &Scope, dispatch: &str, execution: &str) -> Result<()> {
    let current = ledger.inbox_record(&ledger.owner, dispatch)?;
    if current.state != State::Running
        || current.execution_id.as_deref() != Some(execution)
        || current.dispatch.scope() != *scope
    {
        return Err(Error::Unauthorized);
    }
    Ok(())
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliverySummary {
    pub sent: u64,
    pub cancelled: u64,
    pub unconfirmed: u64,
    pub not_prepared: u64,
}
impl Ledger {
    /// Observed original file outcomes only. Model text is never a receipt.
    pub fn sent_file_summary(
        &self,
        scope: &Scope,
        dispatch: &str,
        execution: &str,
    ) -> Result<DeliverySummary> {
        crate::verify(&self.db, scope)?;
        let ids=self.db.prepare("SELECT id FROM sent_files WHERE scope=? AND dispatch=? AND execution=? ORDER BY id")?.query_map(rusqlite::params![json(scope)?,dispatch,execution],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut summary = DeliverySummary::default();
        for id in ids {
            let r = record(&self.db, scope, dispatch, execution, &id)?.ok_or(Error::Unknown)?;
            if r.absence.is_some() {
                summary.not_prepared += 1;
                continue;
            }
            match r.remote.as_ref().map(|r| r.state.as_str()) {
                Some("sent") => summary.sent += 1,
                Some("cancelled") => summary.cancelled += 1,
                _ => summary.unconfirmed += 1,
            };
        }
        Ok(summary)
    }
    pub fn sent_file(
        &self,
        scope: &Scope,
        dispatch: &str,
        execution: &str,
        id: &str,
    ) -> Result<Option<Record>> {
        crate::verify(&self.db, scope)?;
        crate::key(id)?;
        record(&self.db, scope, dispatch, execution, id)
    }
    /// Historical evidence only. This method never mints byte/send custody.
    pub fn observe_sent_file(
        &mut self,
        scope: &Scope,
        dispatch: &str,
        execution: &str,
        id: &str,
        receipt: Delivery,
    ) -> Result<Record> {
        crate::verify(&self.db, scope)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut record = record(&tx, scope, dispatch, execution, id)?.ok_or(Error::Unauthorized)?;
        receipt.validate(dispatch, execution, &record.request_id, &record.manifest)?;
        if record.absence.is_some() {
            return Err(Error::Conflict);
        }
        if let Some(old) = &record.remote
            && (old.id != receipt.id
                || old.created_at_ms != receipt.created_at_ms
                || old.received_bytes > receipt.received_bytes
                || old.upload_accepted && !receipt.upload_accepted
                || matches!(old.state.as_str(), "sent" | "cancelled") && old != &receipt
                || old.state == "unknown" && !matches!(receipt.state.as_str(), "unknown" | "sent")
                || state_order(&old.state) > state_order(&receipt.state))
        {
            return Err(Error::Conflict);
        }
        tx.execute(
            "UPDATE sent_files SET remote=? WHERE id=?",
            (json(&receipt)?, id),
        )?;
        tx.commit()?;
        record.remote = Some(receipt);
        Ok(record)
    }
}
fn record(
    db: &rusqlite::Connection,
    scope: &Scope,
    dispatch: &str,
    execution: &str,
    id: &str,
) -> Result<Option<Record>> {
    let row:Option<(String,String,String,Option<String>)>=db.query_row("SELECT request_id,manifest,audit,remote FROM sent_files WHERE id=? AND scope=? AND dispatch=? AND execution=?",rusqlite::params![id,json(scope)?,dispatch,execution],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    row.map(|(request_id, manifest, audit, remote)| {
        let manifest: Manifest = serde_json::from_str(&manifest)?;
        manifest.validate()?;
        let remote: Option<Delivery> = remote.map(|s| serde_json::from_str(&s)).transpose()?;
        if let Some(d) = &remote {
            d.validate(dispatch, execution, &request_id, &manifest)?;
        }
        let absence: Option<(String, i64)> = db
            .query_row(
                "SELECT proof,observed_at FROM sent_file_absence WHERE id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let absence = absence
            .map(|(proof, observed_at)| -> Result<Absence> {
                let observation: FileObservation = serde_json::from_str(&proof)?;
                observation.validate(
                    &FileLookup {
                        dispatch_id: dispatch.into(),
                        execution_id: execution.into(),
                        request_id: request_id.clone(),
                    },
                    &manifest,
                )?;
                if remote.is_some()
                    || observation.delivery.is_some()
                    || !observation.preparation_closed
                    || observed_at < 0
                {
                    return Err(Error::Conflict);
                }
                Ok(Absence {
                    observation,
                    observed_at,
                })
            })
            .transpose()?;
        Ok(Record {
            id: id.into(),
            request_id,
            manifest,
            remote,
            audit,
            absence,
        })
    })
    .transpose()
}
fn state_order(state: &str) -> u8 {
    match state {
        "staging" => 0,
        "queued" => 1,
        "uploading" => 2,
        "uploaded" => 3,
        "sending" => 4,
        "unknown" => 5,
        "sent" | "cancelled" => 6,
        _ => 255,
    }
}
#[cfg(unix)]
impl Sender {
    /// Call after current host authorization and exact owner/policy approval.
    /// Commit the possible-send witness BEFORE any Server preparation/chunk/queue.
    pub fn begin(
        &self,
        ledger: &mut Ledger,
        captured: Captured,
        audit: &str,
        now: i64,
    ) -> Result<Custody> {
        ledger.verify_approval_audit(&ledger.owner, audit, &captured.proposal)?;
        if ledger.approval_phase(&ledger.owner, audit)? != crate::audit::Phase::Applying {
            return Err(Error::Denied);
        }
        self.verify(&captured)?;
        running(
            ledger,
            &captured.proposal.scope,
            &captured.proposal.dispatch,
            &captured.execution,
        )?;
        ledger.authorize_tool(&captured.proposal, now)?;
        let tx = ledger
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sent_files WHERE id=?)",
            [&captured.id],
            |r| r.get(0),
        )?;
        if prior {
            return Err(Error::Unknown);
        }
        reject_pending(&tx, &captured)?;
        let (count,held):(u64,u64)=tx.query_row("SELECT count(*),coalesce(sum(CASE WHEN EXISTS(SELECT 1 FROM sent_file_absence a WHERE a.id=sent_files.id) OR json_extract(remote,'$.state') IN ('sent','cancelled') THEN 0 ELSE json_extract(manifest,'$.bytes') END),0) FROM sent_files",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if count >= 4096 || held.saturating_add(captured.manifest.bytes) > 128 * 1024 * 1024 {
            return Err(Error::Invalid("sent file quota"));
        }
        let request_id = format!("file_{}", captured.id);
        tx.execute("INSERT INTO sent_files(id,scope,dispatch,execution,workspace,relative_path,manifest,content_hash,request_id,audit) VALUES (?,?,?,?,?,?,?,?,?,?)",rusqlite::params![captured.id,json(&captured.proposal.scope)?,captured.proposal.dispatch,captured.execution,captured.proposal.canonical_directory,captured.relative_path,json(&captured.manifest)?,captured.manifest.sha256,request_id,audit])?;
        tx.commit()?;
        let record = Record {
            id: captured.id.clone(),
            request_id,
            manifest: captured.manifest.clone(),
            remote: None,
            audit: audit.into(),
            absence: None,
        };
        Ok(Custody { captured, record })
    }
}
#[cfg(unix)]
fn reject_pending(db: &rusqlite::Connection, c: &Captured) -> Result<()> {
    let pending:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sent_files WHERE scope=? AND dispatch=? AND execution=? AND (relative_path=? OR content_hash=?) AND (remote IS NULL OR json_extract(remote,'$.state') NOT IN ('sent','cancelled')) AND NOT EXISTS(SELECT 1 FROM sent_file_absence a WHERE a.id=sent_files.id))",rusqlite::params![json(&c.proposal.scope)?,c.proposal.dispatch,c.execution,c.relative_path,c.manifest.sha256],|r|r.get(0))?;
    if pending { Err(Error::Unknown) } else { Ok(()) }
}
#[cfg(all(test, unix))]
pub(crate) mod tests;
