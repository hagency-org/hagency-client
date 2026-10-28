//! Descriptive applied-head resolution. The queued process/call writer must
//! repeat `resolve` in its own transaction; this read alone grants no custody.
use super::*;
use shared_execution::SharedExecutionBudget;

pub(in super::super) fn resolve(
    db: &Connection,
    child: &str,
    selected: Option<&SharedExecutionBudget>,
    current: bool,
) -> Result<(SharedExecutionBudget, u64), Error> {
    let h = journal::head(db, child)?;
    if h.revision < 2 {
        return Err(Error::LocalAuthority);
    }
    let wanted = selected.map_or(h.revision, |p| p.budget_revision);
    if let Some(p) = selected {
        p.validate()?;
    }
    if current && wanted != h.revision {
        return Err(Error::Conflict);
    }
    let row:Option<(String,String,String,String,u64)>=db.query_row(
        "SELECT c.command_id,c.command_digest,c.receipt_payload,c.receipt_digest,c.ledger_revision FROM owner_managed_budget_deltas d JOIN owner_managed_budget_commands c ON c.command_id=d.command_id AND c.child_id=d.child_id WHERE d.child_id=?1 AND d.budget_revision=?2",
        params![child,wanted],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    ).optional()?;
    let (command_id, command_digest, payload, receipt_digest, revision) =
        row.ok_or(Error::Conflict)?;
    // `head` already validated the entire bounded chain and each original
    // command/receipt/column. Never resolve a detached approval projection.
    let r: AgentBudgetReceipt = serde_json::from_str(&payload)?;
    let value = SharedExecutionBudget {
        version: 1,
        original_reserve_receipt_digest: digest(&h.original)?,
        budget_revision: r.budget_revision,
        budget_digest: r.budget_digest,
        effective_ceiling_tokens: r.effective_ceiling_tokens,
        applied_command_id: command_id,
        applied_command_digest: command_digest,
        applied_receipt_digest: receipt_digest,
    };
    value.validate()?;
    if selected.is_some_and(|p| p != &value) {
        return Err(Error::Conflict);
    }
    Ok((value, revision))
}
impl OwnerResourceRegistry {
    /// A current native-derived description only. Fresh execution separately
    /// requires exact stopped predecessor, current Run and full conserved usage.
    pub async fn managed_agent_execution_budget(
        &self,
        e: VerifiedManagementEnrollment,
        child: String,
    ) -> Result<SharedExecutionBudget, Error> {
        e.check()?;
        identifier(&child, 128)?;
        let profile = e.profile.clone();
        let expires = e.expires;
        let fence = self.0.clone();
        let value = self
            .0
            .store
            .call(8192, move |db| {
                e.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                authority::original(&db.db, &e, &child, false)?;
                let value = resolve(&db.db, &child, None, true)?.0;
                e.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                Ok(value)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(value)
    }
}
