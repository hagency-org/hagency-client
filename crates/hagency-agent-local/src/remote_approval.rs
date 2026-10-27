//! Durable publication and decision evidence for current owner approvals.
//!
//! This is not Matrix authentication, a room validator or an execution grant.
//! The SDK host must first check current private encrypted-room state and actual
//! verified device provenance. A deserialized message is never ObservedVerdict.
//! Only the first committed publication returns non-Clone custody; history and
//! backup restore cannot mint it. Applying a recorded decision still requires
//! the original live PendingApproval and current server/device/lease gates.
use crate::{Error, Ledger, Result, ToolProposal, audit::Phase, hash, json};
use rusqlite::{OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json as value};
pub(crate) const SCHEMA: &str = include_str!("remote_approval/schema.sql");
pub(crate) const OBJECTS: &[&str] = &[
    "remote_approval_publications",
    "retain_remote_approval_publication",
    "immutable_remote_approval_publication",
    "remote_approval_receipts",
    "retain_remote_approval_receipt",
    "immutable_remote_approval_receipt",
    "remote_approval_decisions",
    "retain_remote_approval_decision",
    "immutable_remote_approval_decision",
];
pub const MAX_REQUEST_BYTES: usize = 32 * 1024;
pub const MAX_PUBLICATIONS: usize = 4096;
pub const MAX_PUBLICATION_BYTES: usize = 8 * 1024 * 1024;
pub const REQUEST_TYPE: &str = "im.hagency.owner.approval.request.v1";
pub const VERDICT_TYPE: &str = "im.hagency.owner.approval.verdict.v1";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionDevice {
    pub id: String,
    pub generation: u64,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub owner: String,
    pub profile: String,
    pub approval_id: String,
    pub approval_room: String,
    pub execution_id: String,
    pub binding_generation: i64,
    pub dispatch_epoch: i64,
    pub execution_device: ExecutionDevice,
    pub publisher_matrix_device: String,
    pub proposal_digest: String,
    pub proposal: ToolProposal,
}
impl Request {
    pub fn validate(&self) -> Result<()> {
        for s in [
            &self.owner,
            &self.approval_id,
            &self.execution_id,
            &self.execution_device.id,
            &self.publisher_matrix_device,
        ] {
            atom(s, 255)?;
        }
        if self.binding_generation <= 0
            || self.dispatch_epoch <= 0
            || self.version != 1
            || self.execution_device.generation == 0
            || !user(&self.owner)
            || !room(&self.approval_room)
            || self.approval_room == self.proposal.scope.room
            || !digest(&self.profile)
            || !digest(&self.approval_id)
            || self.proposal_digest != hash(&self.proposal)?
            || self.proposal.expires < 0
            || json(self)?.len() > MAX_REQUEST_BYTES
        {
            return Err(Error::Invalid("remote approval request"));
        }
        self.proposal.scope.validate()?;
        Ok(())
    }
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        hash(self)
    }
    /// Sensitive full proposal, for an already checked encrypted SDK room only.
    pub fn message_content(&self) -> Result<Value> {
        self.validate()?;
        Ok(
            value!({"msgtype":REQUEST_TYPE,"body":"Owner approval requested","im.hagency.owner.approval":self}),
        )
    }
}
/// Durable write-possible custody for one original request. Not serializable,
/// cloneable, or reconstructible from a Request/Record. Drop never rearms it.
pub struct Publication {
    request: Request,
    digest: String,
    instance: std::sync::Arc<()>,
}
impl Publication {
    pub fn request(&self) -> &Request {
        &self.request
    }
    pub fn request_digest(&self) -> &str {
        &self.digest
    }
    pub fn transaction_id(&self) -> String {
        format!("hagency.remote-approval.{}", self.digest)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Verdict {
    pub approval_id: String,
    pub request_digest: String,
    pub approved: bool,
}
impl Verdict {
    pub fn message_content(&self, request_event: &str) -> Result<Value> {
        if !digest(&self.approval_id) || !digest(&self.request_digest) || !event(request_event) {
            return Err(Error::Invalid("remote approval verdict"));
        }
        Ok(
            value!({"msgtype":VERDICT_TYPE,"body":if self.approved{"Approve once"}else{"Deny"},"im.hagency.owner.approval":self,"m.relates_to":{"m.in_reply_to":{"event_id":request_event}}}),
        )
    }
}
/// Host observation constructed ONLY after actual SDK decryption, Verified
/// provenance and fresh device/key/account/room checks. Never Deserialize and
/// never infer these fields from unsigned JSON or a message's device claim.
/// This library does not itself establish any of those Matrix facts.
pub struct ObservedVerdict {
    pub observed_at: i64,
    pub sender: String,
    pub sender_device: String,
    pub sender_ed25519: String,
    pub room: String,
    pub event_id: String,
    pub request_event_id: String,
    pub verdict: Verdict,
}
/// Evidence of the durable owner decision, not a tool grant or completion.
pub struct RecordedDecision {
    approval_id: String,
    approved: bool,
}
impl RecordedDecision {
    pub fn approval_id(&self) -> &str {
        &self.approval_id
    }
    pub fn approved(&self) -> bool {
        self.approved
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub request: Request,
    pub request_digest: String,
    pub request_event_id: Option<String>,
    pub decision: Option<DecisionRecord>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionRecord {
    pub event_id: String,
    pub sender_device: String,
    pub sender_ed25519: String,
    pub approved: bool,
    pub observed_at: i64,
}
fn atom(s: &str, max: usize) -> Result<()> {
    if s.is_empty() || s.len() > max || s.chars().any(|c| c.is_control() || c.is_whitespace()) {
        Err(Error::Invalid("remote approval identity"))
    } else {
        Ok(())
    }
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn user(s: &str) -> bool {
    s.starts_with('@')
        && s[1..]
            .split_once(':')
            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
}
fn room(s: &str) -> bool {
    atom(s, 255).is_ok()
        && s.starts_with('!')
        && (s[1..]
            .split_once(':')
            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
            || s.len() == 44
                && s[1..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}
fn event(s: &str) -> bool {
    atom(s, 512).is_ok() && s.starts_with('$') && s.len() > 1
}
fn running(
    db: &rusqlite::Connection,
    request: &Request,
    device: &ExecutionDevice,
    now: i64,
) -> Result<()> {
    if now < 0 || request.proposal.expires <= now || &request.execution_device != device {
        return Err(Error::Unauthorized);
    }
    let row: Option<(String, Option<String>, String)> = db
        .query_row(
            "SELECT envelope,execution_id,state FROM local_inbox WHERE id=?",
            [&request.proposal.dispatch],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (encoded, execution, state) = row.ok_or(Error::Unauthorized)?;
    let dispatch: crate::inbox::Dispatch = serde_json::from_str(&encoded)?;
    if state != "running"
        || execution.as_deref() != Some(request.execution_id.as_str())
        || dispatch.scope() != request.proposal.scope
        || dispatch.binding_generation != request.binding_generation
        || dispatch.dispatch_epoch != Some(request.dispatch_epoch)
        || dispatch.dispatch_device_id.as_deref() != Some(device.id.as_str())
    {
        return Err(Error::Unauthorized);
    }
    for (index, scope) in request.proposal.scope.layers()?.into_iter().enumerate() {
        let revision: i64 = db
            .query_row(
                "SELECT revision FROM policies WHERE scope=?",
                [scope],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if revision != request.proposal.policy_revision[index] {
            return Err(Error::Conflict);
        }
    }
    let created: i64 = db.query_row(
        "SELECT created_at FROM approval_audit WHERE id=?",
        [&request.approval_id],
        |r| r.get(0),
    )?;
    if now < created {
        return Err(Error::Unauthorized);
    }
    Ok(())
}
impl Ledger {
    fn remote_identity(&self, owner: &str, request: &Request) -> Result<()> {
        self.owner(owner)?;
        request.validate()?;
        let profile: String =
            self.db
                .query_row("SELECT digest FROM profile_identity", [], |r| r.get(0))?;
        if request.owner != owner || request.profile != profile {
            return Err(Error::Unauthorized);
        }
        self.verify_approval_audit(owner, &request.approval_id, &request.proposal)
    }
    /// Commit before SDK publication. Existing witnesses never mint custody
    /// again, including after reopen/restore; a different request cannot replace
    /// them. The caller must own the original live PendingApproval first.
    pub fn reserve_remote_approval(
        &mut self,
        owner: &str,
        request: &Request,
        clock: impl Fn() -> i64,
    ) -> Result<Publication> {
        let initial = clock();
        let now = initial;
        self.remote_identity(owner, request)?;
        running(&self.db, request, &request.execution_device, now)?;
        if self.approval_phase(owner, &request.approval_id)? != Phase::Offered {
            return Err(Error::Conflict);
        }
        let encoded = json(request)?;
        let digest = request.digest()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = clock();
        if now < initial {
            return Err(Error::Unauthorized);
        }
        running(&tx, request, &request.execution_device, now)?;
        if crate::audit::phase(&tx, &request.approval_id)? != Phase::Offered {
            return Err(Error::Conflict);
        }
        let prior: Option<String> = tx
            .query_row(
                "SELECT request FROM remote_approval_publications WHERE approval_id=?",
                [&request.approval_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(prior) = prior {
            return Err(if prior == encoded {
                Error::Unknown
            } else {
                Error::Conflict
            });
        }
        let count: usize = tx.query_row(
            "SELECT count(*) FROM remote_approval_publications",
            [],
            |r| r.get(0),
        )?;
        if count >= MAX_PUBLICATIONS {
            return Err(Error::Invalid("remote approval capacity"));
        }
        let bytes: usize = tx.query_row("SELECT coalesce(sum(length(CAST(request AS BLOB))),0) FROM remote_approval_publications", [], |r| r.get(0))?;
        if encoded.len() > MAX_PUBLICATION_BYTES.saturating_sub(bytes) {
            return Err(Error::Invalid("remote approval byte capacity"));
        }
        tx.execute(
            "INSERT INTO remote_approval_publications VALUES (?,?,?,?)",
            (&request.approval_id, encoded, &digest, now),
        )?;
        tx.commit()?;
        Ok(Publication {
            request: request.clone(),
            digest,
            instance: self.remote_instance.clone(),
        })
    }
    fn remote_custody(&self, owner: &str, publication: &Publication) -> Result<i64> {
        if !std::sync::Arc::ptr_eq(&publication.instance, &self.remote_instance) {
            return Err(Error::Unauthorized);
        }
        self.remote_identity(owner, &publication.request)?;
        let (request, digest, created_at): (String, String, i64) = self.db.query_row(
            "SELECT request,request_digest,created_at FROM remote_approval_publications WHERE approval_id=?",
            [&publication.request.approval_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if request != json(&publication.request)? || digest != publication.digest {
            return Err(Error::Unauthorized);
        }
        Ok(created_at)
    }
    /// Actual original SDK acceptance only. A late receipt remains historical
    /// evidence after expiry/stop; it never restores decision or send authority.
    pub fn record_remote_approval_receipt(
        &mut self,
        owner: &str,
        publication: &Publication,
        event_id: &str,
        now: i64,
    ) -> Result<()> {
        let publication_at = self.remote_custody(owner, publication)?;
        if !event(event_id) || now < publication_at {
            return Err(Error::Invalid("remote approval receipt"));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<String> = tx
            .query_row(
                "SELECT event_id FROM remote_approval_receipts WHERE approval_id=?",
                [&publication.request.approval_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(prior) = prior {
            return if prior == event_id {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        tx.execute(
            "INSERT INTO remote_approval_receipts VALUES (?,?,?)",
            (&publication.request.approval_id, event_id, now),
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Record one checked remote decision atomically with the original audit.
    /// Supply the host's actual current clock, sampled again after writer waits.
    /// The host must still consume its exact live PendingApproval under fresh
    /// actual device/lease/policy checks. This result cannot execute tools.
    pub fn record_remote_approval_decision(
        &mut self,
        owner: &str,
        publication: &Publication,
        current_device: &ExecutionDevice,
        observation: &ObservedVerdict,
        clock: impl Fn() -> i64,
    ) -> Result<RecordedDecision> {
        let initial = clock();
        let now = initial;
        let publication_at = self.remote_custody(owner, publication)?;
        let request = &publication.request;
        running(&self.db, request, current_device, now)?;
        for s in [&observation.sender_device, &observation.sender_ed25519] {
            atom(s, 255)?;
        }
        if observation.observed_at < publication_at
            || observation.observed_at > now
            || now - observation.observed_at > 5
            || observation.sender != owner
            || observation.room != request.approval_room
            || observation.sender_device == request.publisher_matrix_device
            || !event(&observation.event_id)
            || observation.verdict.approval_id != request.approval_id
            || observation.verdict.request_digest != publication.digest
        {
            return Err(Error::Unauthorized);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = clock();
        if now < initial {
            return Err(Error::Unauthorized);
        }
        running(&tx, request, current_device, now)?;
        if now - observation.observed_at > 5 {
            return Err(Error::Unauthorized);
        }
        let original: Option<(String, i64)> = tx
            .query_row(
                "SELECT event_id,observed_at FROM remote_approval_receipts WHERE approval_id=?",
                [&request.approval_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if !original.as_ref().is_some_and(|(id, at)| {
            id == &observation.request_event_id && *at <= observation.observed_at
        }) || observation.event_id == observation.request_event_id
            || crate::audit::phase(&tx, &request.approval_id)? != Phase::Offered
        {
            return Err(Error::Conflict);
        }
        tx.execute(
            "INSERT INTO remote_approval_decisions VALUES (?,?,?,?,?,?)",
            (
                &request.approval_id,
                &observation.event_id,
                &observation.sender_device,
                &observation.sender_ed25519,
                observation.verdict.approved,
                now,
            ),
        )?;
        crate::audit::append(
            &tx,
            &request.approval_id,
            if observation.verdict.approved {
                Phase::OwnerAllowed
            } else {
                Phase::OwnerDenied
            },
            now,
            &value!({"origin":"verified_matrix_device","room":observation.room,"requestEventId":observation.request_event_id,"eventId":observation.event_id,"matrixDeviceId":observation.sender_device,"matrixDeviceKey":observation.sender_ed25519,"executionDeviceId":current_device.id,"executionDeviceGeneration":current_device.generation,"requestDigest":publication.digest}),
        )?;
        tx.commit()?;
        Ok(RecordedDecision {
            approval_id: request.approval_id.clone(),
            approved: observation.verdict.approved,
        })
    }
    /// Metadata only. It cannot reconstruct Publication or RecordedDecision.
    pub fn remote_approval_record(
        &self,
        owner: &str,
        agent: &str,
        approval_id: &str,
    ) -> Result<Option<Record>> {
        self.owner(owner)?;
        let stored:Option<(String,String)>=self.db.query_row("SELECT p.request,p.request_digest FROM remote_approval_publications p JOIN approval_audit a ON a.id=p.approval_id WHERE p.approval_id=? AND a.agent=?",(approval_id,agent),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((encoded, request_digest)) = stored else {
            return Ok(None);
        };
        let request: Request = serde_json::from_str(&encoded)?;
        self.remote_identity(owner, &request)?;
        if request.digest()? != request_digest {
            return Err(Error::ForeignDatabase);
        }
        let request_event_id = self
            .db
            .query_row(
                "SELECT event_id FROM remote_approval_receipts WHERE approval_id=?",
                [approval_id],
                |r| r.get(0),
            )
            .optional()?;
        let decision=self.db.query_row("SELECT event_id,device_id,device_key,approved,observed_at FROM remote_approval_decisions WHERE approval_id=?",[approval_id],|r|Ok(DecisionRecord{event_id:r.get(0)?,sender_device:r.get(1)?,sender_ed25519:r.get(2)?,approved:r.get(3)?,observed_at:r.get(4)?})).optional()?;
        Ok(Some(Record {
            request,
            request_digest,
            request_event_id,
            decision,
        }))
    }
}
#[cfg(test)]
mod tests;
