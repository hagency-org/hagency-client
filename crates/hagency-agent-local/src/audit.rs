//! Append-only tool approval evidence. Owner decisions, permission application
//! and observed execution results are distinct; a grant is never completion.
use crate::{Error, Ledger, Result, Scope, ToolProposal, hash, json, key, verify};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json as value};
use sha2::{Digest, Sha256};
pub(crate) const SCHEMA: &str = include_str!("audit/schema.sql");
pub(crate) const OBJECTS: &[&str] = &[
    "approval_audit",
    "approval_audit_scope",
    "retain_approval_audit",
    "immutable_approval_audit",
    "approval_audit_events",
    "approval_audit_events_id",
    "retain_approval_event",
    "immutable_approval_event",
];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Offered,
    OwnerAllowed,
    OwnerDenied,
    PolicyAllowed,
    PolicyDenied,
    Applying,
    Submitted,
    Completed,
    Failed,
    Unknown,
    Expired,
}
impl Phase {
    fn wire(self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::OwnerAllowed => "owner_allowed",
            Self::OwnerDenied => "owner_denied",
            Self::PolicyAllowed => "policy_allowed",
            Self::PolicyDenied => "policy_denied",
            Self::Applying => "applying",
            Self::Submitted => "submitted",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Expired => "expired",
        }
    }
    fn parse(text: &str) -> Result<Self> {
        serde_json::from_value(Value::String(text.into())).map_err(Error::from)
    }
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::OwnerDenied
                | Self::PolicyDenied
                | Self::Completed
                | Self::Failed
                | Self::Unknown
                | Self::Expired
        )
    }
    fn next(self, phase: Self) -> bool {
        match self {
            Self::Offered => matches!(
                phase,
                Self::OwnerAllowed
                    | Self::OwnerDenied
                    | Self::PolicyAllowed
                    | Self::PolicyDenied
                    | Self::Expired
            ),
            Self::OwnerAllowed | Self::PolicyAllowed => {
                matches!(phase, Self::Applying | Self::Expired | Self::Unknown)
            }
            Self::Applying => matches!(
                phase,
                Self::Submitted | Self::Completed | Self::Failed | Self::Unknown | Self::Expired
            ),
            Self::Submitted => matches!(phase, Self::Completed | Self::Failed | Self::Unknown),
            _ => false,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub sequence: i64,
    pub phase: Phase,
    pub observed_at: i64,
    pub detail: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: String,
    pub proposal_digest: String,
    pub scope: Scope,
    pub tool: String,
    pub created_at: i64,
    pub expires: i64,
    pub phase: Phase,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub summary: Summary,
    pub proposal: ToolProposal,
    pub events: Vec<Event>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub records: Vec<Summary>,
    pub next_cursor: Option<String>,
}
impl Ledger {
    pub fn begin_approval_audit(
        &mut self,
        owner: &str,
        proposal: &ToolProposal,
        now: i64,
    ) -> Result<String> {
        self.owner(owner)?;
        verify(&self.db, &proposal.scope)?;
        for field in [
            &proposal.dispatch,
            &proposal.tool,
            &proposal.canonical_directory,
            &proposal.risk,
        ] {
            key(field)?;
        }
        if now < 0 || proposal.expires <= now || proposal.expires - now > 3600 {
            return Err(Error::Invalid("approval audit expiry"));
        }
        let encoded = json(proposal)?;
        if encoded.len() > 1024 * 1024 {
            return Err(Error::Invalid("approval audit proposal capacity"));
        }
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| Error::Invalid("approval audit entropy"))?;
        let id = format!("{:x}", Sha256::digest(nonce));
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO approval_audit VALUES (?,?,?,?,?,?)",
            (
                &id,
                hash(proposal)?,
                encoded,
                &proposal.scope.agent,
                &proposal.dispatch,
                now,
            ),
        )?;
        append(&tx, &id, Phase::Offered, now, &value!({}))?;
        tx.commit()?;
        Ok(id)
    }
    pub fn verify_approval_audit(
        &self,
        owner: &str,
        id: &str,
        proposal: &ToolProposal,
    ) -> Result<()> {
        self.owner(owner)?;
        key(id)?;
        let stored: Option<String> = self
            .db
            .query_row(
                "SELECT proposal FROM approval_audit WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        if stored.as_deref() != Some(&json(proposal)?) {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    pub fn approval_phase(&self, owner: &str, id: &str) -> Result<Phase> {
        self.owner(owner)?;
        key(id)?;
        phase(&self.db, id)
    }
    pub fn record_approval_event(
        &mut self,
        owner: &str,
        id: &str,
        next: Phase,
        now: i64,
        detail: &Value,
    ) -> Result<()> {
        self.owner(owner)?;
        key(id)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        append(&tx, id, next, now, detail)?;
        tx.commit()?;
        Ok(())
    }
    /// Offered approvals cannot execute after expiry. Granted operations become
    /// unknown when this device no longer observes a runtime for this agent.
    /// This does not prove that a remote side effect did not happen.
    pub fn close_approval_audits(
        &mut self,
        owner: &str,
        agent: &str,
        stopped: bool,
        now: i64,
    ) -> Result<usize> {
        self.owner(owner)?;
        key(agent)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows = tx
            .prepare("SELECT a.id,a.proposal FROM approval_audit a WHERE a.agent=? AND (SELECT phase FROM approval_audit_events e WHERE e.approval_id=a.id ORDER BY sequence DESC LIMIT 1) IN ('offered','owner_allowed','policy_allowed','applying','submitted')")?
            .query_map([agent], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut changed = 0;
        for (id, encoded) in rows {
            let original = phase(&tx, &id)?;
            if original.terminal() {
                continue;
            }
            let proposal: ToolProposal = serde_json::from_str(&encoded)?;
            let next = if original == Phase::Offered && (stopped || proposal.expires <= now) {
                Some(Phase::Expired)
            } else if stopped {
                Some(Phase::Unknown)
            } else {
                None
            };
            if let Some(next) = next {
                append(
                    &tx,
                    &id,
                    next,
                    now,
                    &value!({"reason":if stopped{"runtime_not_observed"}else{"approval_expired"}}),
                )?;
                changed += 1;
            }
        }
        tx.commit()?;
        Ok(changed)
    }
    pub fn approval_history(&self, owner: &str, agent: &str, cursor: Option<&str>) -> Result<Page> {
        self.owner(owner)?;
        key(agent)?;
        let before: Option<i64> = if let Some(id) = cursor {
            key(id)?;
            Some(
                self.db
                    .query_row(
                        "SELECT created_at FROM approval_audit WHERE id=? AND agent=?",
                        (id, agent),
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or(Error::Unauthorized)?,
            )
        } else {
            None
        };
        let mut stmt=self.db.prepare("SELECT id,proposal_digest,proposal,created_at FROM approval_audit WHERE agent=?1 AND (?2 IS NULL OR created_at<?2 OR (created_at=?2 AND id<?3)) ORDER BY created_at DESC,id DESC LIMIT 101")?;
        let mut rows = stmt.query((agent, before, cursor))?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let proposal: ToolProposal = serde_json::from_str(&row.get::<_, String>(2)?)?;
            records.push(Summary {
                id: id.clone(),
                proposal_digest: row.get(1)?,
                scope: proposal.scope,
                tool: proposal.tool,
                created_at: row.get(3)?,
                expires: proposal.expires,
                phase: phase(&self.db, &id)?,
            });
        }
        let more = records.len() > 100;
        records.truncate(100);
        let next_cursor = more.then(|| records.last().unwrap().id.clone());
        Ok(Page {
            records,
            next_cursor,
        })
    }
    pub fn approval_record(&self, owner: &str, agent: &str, id: &str) -> Result<Record> {
        self.owner(owner)?;
        key(agent)?;
        key(id)?;
        let (digest,encoded,created):(String,String,i64)=self.db.query_row("SELECT proposal_digest,proposal,created_at FROM approval_audit WHERE id=? AND agent=?",(id,agent),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.ok_or(Error::Unauthorized)?;
        let proposal: ToolProposal = serde_json::from_str(&encoded)?;
        let mut stmt=self.db.prepare("SELECT sequence,phase,observed_at,detail FROM approval_audit_events WHERE approval_id=? ORDER BY sequence")?;
        let mut rows = stmt.query([id])?;
        let mut events = Vec::new();
        while let Some(row) = rows.next()? {
            events.push(Event {
                sequence: row.get(0)?,
                phase: Phase::parse(&row.get::<_, String>(1)?)?,
                observed_at: row.get(2)?,
                detail: serde_json::from_str(&row.get::<_, String>(3)?)?,
            });
        }
        let current = events.last().ok_or(Error::Conflict)?.phase;
        Ok(Record {
            summary: Summary {
                id: id.into(),
                proposal_digest: digest,
                scope: proposal.scope.clone(),
                tool: proposal.tool.clone(),
                created_at: created,
                expires: proposal.expires,
                phase: current,
            },
            proposal,
            events,
        })
    }
}
pub(crate) fn phase(db: &Connection, id: &str) -> Result<Phase> {
    Phase::parse(&db.query_row("SELECT phase FROM approval_audit_events WHERE approval_id=? ORDER BY sequence DESC LIMIT 1",[id],|r|r.get::<_,String>(0))?)
}
pub(crate) fn finalize_inbox(
    db: &Connection,
    id: &str,
    model_completed: bool,
    now: i64,
) -> Result<()> {
    let agent: String = db.query_row(
        "SELECT b.agent FROM local_inbox i JOIN bindings b ON b.id=i.binding WHERE i.id=?",
        [id],
        |r| r.get(0),
    )?;
    let rows = db
        .prepare("SELECT id,proposal FROM approval_audit WHERE agent=? AND dispatch=?")?
        .query_map((&agent, id), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (id, encoded) in rows {
        let old = phase(db, &id)?;
        if old.terminal() {
            continue;
        }
        let proposal: ToolProposal = serde_json::from_str(&encoded)?;
        let next = if old == Phase::Offered {
            Phase::Expired
        } else if model_completed
            && proposal.tool == "model.request"
            && matches!(old, Phase::Applying | Phase::Submitted)
        {
            Phase::Completed
        } else {
            Phase::Unknown
        };
        append(
            db,
            &id,
            next,
            now,
            &value!({"reason":if next==Phase::Completed{"model_reply_persisted"}else{"dispatch_ended_without_tool_receipt"}}),
        )?;
    }
    Ok(())
}
pub(crate) fn append(
    db: &Connection,
    id: &str,
    next: Phase,
    now: i64,
    detail: &Value,
) -> Result<()> {
    let detail = json(detail)?;
    if now < 0 || detail.len() > 16384 {
        return Err(Error::Invalid("approval audit event"));
    }
    if next == Phase::Offered {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM approval_audit_events WHERE approval_id=?)",
            [id],
            |r| r.get(0),
        )?;
        if exists {
            return Err(Error::Conflict);
        }
    } else {
        let old = phase(db, id)?;
        if old == next {
            return Ok(());
        }
        if !old.next(next) {
            return Err(Error::Conflict);
        }
    }
    db.execute(
        "INSERT INTO approval_audit_events(approval_id,phase,observed_at,detail) VALUES (?,?,?,?)",
        (id, next.wire(), now, detail),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
