//! V3 observations retain the original lifetime and all prior calls. Descriptive
//! reads are never spend authority: every claim/issue repeats the chain checks.
use super::*;

#[derive(Debug)]
#[non_exhaustive]
pub struct SharedBudgetSuccessorSelection {
    pub original: SharedRuntimeCommand,
    pub predecessor: SharedAttemptPredecessor,
    pub execution_budget: SharedExecutionBudget,
    pub budget: SharedSuccessorBudget,
}

fn snapshot_actual(
    db: &Connection,
    e: &VerifiedManagementEnrollment,
    original: &SharedRuntimeCommand,
) -> Result<SharedBudgetSuccessorSelection, Error> {
    original.validate(&e.profile.account)?;
    let binding = authority::original(db, e, &original.agent_reservation_id, false)?;
    let original_digest = canonical::digest(&serde_json::to_value(original)?)?;
    let (profile, saved, incarnation, provider, payload): (String,String,String,String,String) = db.query_row(
        "SELECT profile,command_digest,incarnation,provider_digest,command_payload FROM owner_shared_runtime_lifetimes WHERE child_id=?1 AND assignment_id=?2",
        params![original.agent_reservation_id,original.assignment_id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    )?;
    if profile != e.profile.digest
        || saved != original_digest
        || binding.digest != original.reserve_command_digest
        || provider != binding.provider
        || canonical::digest(&serde_json::from_str::<serde_json::Value>(&payload)?)?
            != original_digest
    {
        return Err(Error::LocalAuthority);
    }
    let latest:Option<(String,String,String,String,String,u64,String)> = db.query_row(
        "SELECT id,profile,command_digest,incarnation,provider_digest,process_attempt,command_payload FROM owner_shared_successor_attempts WHERE child_id=?1 ORDER BY process_attempt DESC LIMIT 1",
        [&original.agent_reservation_id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)),
    ).optional()?;
    let (id, digest, incarnation, number, stop_payload, stop_digest, stop_revision, stop) =
        if let Some((id, p, digest, incarnation, saved_provider, number, payload)) = latest {
            let value: serde_json::Value = serde_json::from_str(&payload)?;
            if p != profile || saved_provider != provider || canonical::digest(&value)? != digest {
                return Err(Error::Conflict);
            }
            let (stop_payload,stop_digest,stop_revision):(String,String,u64)=db.query_row(
            "SELECT payload,digest,ledger_revision FROM owner_shared_successor_observations WHERE attempt_id=?1 AND json_extract(payload,'$.state')='stopped' ORDER BY ledger_revision DESC LIMIT 1",
            [&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional()?.ok_or(Error::OutcomeUnknown)?;
            let evidence = match value.get("version").and_then(serde_json::Value::as_u64) {
                Some(2) => {
                    let c: SharedAttemptCommand = serde_json::from_value(value)?;
                    c.validate(&e.profile.account)?;
                    if c.command_id != id
                        || c.agent_reservation_id != original.agent_reservation_id
                        || c.original_runtime_digest != original_digest
                        || c.process_attempt != number
                    {
                        return Err(Error::Conflict);
                    }
                    let r: SharedAttemptProcessReceipt = serde_json::from_str(&stop_payload)?;
                    r.validate_for(&c)?;
                    (r.state, r.process_digest, r.ledger_revision)
                }
                Some(3) => {
                    let c: SharedBudgetAttemptCommand = serde_json::from_value(value)?;
                    c.validate(&e.profile.account)?;
                    if c.command_id != id
                        || c.agent_reservation_id != original.agent_reservation_id
                        || c.original_runtime_digest != original_digest
                        || c.process_attempt != number
                    {
                        return Err(Error::Conflict);
                    }
                    // Historical selected heads remain verifiable after a later
                    // delta, but never become the fresh budget pin for this turn.
                    super::super::budget::execution::resolve(
                        db,
                        &c.agent_reservation_id,
                        Some(&c.execution_budget),
                        false,
                    )?;
                    let r: SharedBudgetAttemptProcessReceipt = serde_json::from_str(&stop_payload)?;
                    r.validate_for(&c)?;
                    (r.state, r.process_digest, r.ledger_revision)
                }
                _ => return Err(Error::LocalAuthority),
            };
            (
                id,
                digest,
                incarnation,
                number,
                stop_payload,
                stop_digest,
                stop_revision,
                evidence,
            )
        } else {
            let (payload,digest,revision):(String,String,u64)=db.query_row(
            "SELECT payload,digest,ledger_revision FROM owner_shared_process_observations WHERE child_id=?1 AND json_extract(payload,'$.state')='stopped' ORDER BY ledger_revision DESC LIMIT 1",
            [&original.agent_reservation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional()?.ok_or(Error::OutcomeUnknown)?;
            let r: SharedProcessReceipt = serde_json::from_str(&payload)?;
            r.validate_for(original)?;
            (
                original.command_id.clone(),
                original_digest,
                incarnation,
                1,
                payload,
                digest,
                revision,
                (r.state, r.process_digest, r.ledger_revision),
            )
        };
    let process = canonical::digest(
        &json!({"commandDigest":digest,"incarnation":incarnation,"providerDigest":provider}),
    )?;
    if stop.0 != "stopped"
        || stop.1.as_deref() != Some(&process)
        || stop.2 != stop_revision
        || canonical::digest(&serde_json::from_str::<serde_json::Value>(&stop_payload)?)?
            != stop_digest
    {
        return Err(Error::LocalAuthority);
    }
    let (execution_budget, applied_revision) =
        super::super::budget::execution::resolve(db, &original.agent_reservation_id, None, true)?;
    let assignment = canonical::digest(
        &json!({"child":original.agent_reservation_id,"assignmentId":original.assignment_id,"assignmentGeneration":1,"commandDigest":binding.digest,"providerDigest":provider}),
    )?;
    let calls = successor::calls(db, &original.agent_reservation_id, &assignment)?;
    let revision: u64 = db.query_row(
        "SELECT revision FROM owner_resource_identity WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if revision < stop_revision || revision < applied_revision {
        return Err(Error::LocalAuthority);
    }
    let ceiling = execution_budget.effective_ceiling_tokens;
    let remaining = ceiling.saturating_sub(calls.retained);
    let state = if calls.unrepresentable {
        SharedSuccessorBudgetState::Unrepresentable
    } else if remaining == 0 {
        SharedSuccessorBudgetState::Exhausted
    } else {
        SharedSuccessorBudgetState::Available
    };
    Ok(SharedBudgetSuccessorSelection {
        original: original.clone(),
        predecessor: SharedAttemptPredecessor {
            command_id: id,
            command_digest: digest.clone(),
            process_attempt: number,
            stopped_receipt_digest: stop_digest.clone(),
            process_digest: process,
        },
        execution_budget: execution_budget.clone(),
        budget: SharedSuccessorBudget {
            observed_ledger_revision: revision,
            predecessor_command_digest: digest.clone(),
            stopped_receipt_digest: stop_digest.clone(),
            allocated_tokens: ceiling,
            known_consumed_tokens: calls.consumed,
            retained_tokens: calls.retained,
            remaining_tokens: if calls.unrepresentable { 0 } else { remaining },
            usage_complete: calls.complete,
            state,
            liability_digest: canonical::digest(
                &json!({"version":3,"child":original.agent_reservation_id,"predecessorCommandDigest":digest,"stoppedReceiptDigest":stop_digest,"executionBudget":execution_budget,"callsDigest":calls.digest}),
            )?,
        },
    })
}
impl OwnerResourceRegistry {
    pub async fn shared_budget_successor_selection(
        &self,
        e: VerifiedManagementEnrollment,
        original: SharedRuntimeCommand,
    ) -> Result<SharedBudgetSuccessorSelection, Error> {
        e.check()?;
        original.validate(&e.profile.account)?;
        let profile = e.profile.clone();
        let expires = e.expires;
        let registry = self.0.clone();
        let value = self
            .0
            .store
            .call(128 * 1024, move |db| {
                e.check()?;
                after(&e.profile, &registry, expires)?;
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Deferred)?;
                let value = snapshot(&tx, &e, &original)?;
                e.check()?;
                after(&e.profile, &registry, expires)?;
                tx.commit()?;
                Ok(value)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(value)
    }
}

pub(super) fn snapshot(db:&Connection,e:&VerifiedManagementEnrollment,original:&SharedRuntimeCommand)->Result<SharedBudgetSuccessorSelection,Error>{
    let mut value=snapshot_actual(db,e,original)?;
    if let Some((c,r))=budget_unstarted::head(db,&e.profile.digest,original)? {
        let digest=canonical::digest(&serde_json::to_value(&c)?)?;
        let receipt=canonical::digest(&serde_json::to_value(&r)?)?;
        if value.budget.observed_ledger_revision<r.ledger_revision{return Err(Error::Conflict)}
        value.predecessor=SharedAttemptPredecessor{command_id:c.command_id,command_digest:digest.clone(),process_attempt:c.process_attempt,stopped_receipt_digest:receipt.clone(),process_digest:String::new()};
        value.budget.predecessor_command_digest=digest.clone();value.budget.stopped_receipt_digest=receipt.clone();
        let binding=authority::original(db,e,&original.agent_reservation_id,false)?;
        let assignment=canonical::digest(&json!({"child":original.agent_reservation_id,"assignmentId":original.assignment_id,"assignmentGeneration":1,"commandDigest":binding.digest,"providerDigest":binding.provider}))?;
        let calls=successor::calls(db,&original.agent_reservation_id,&assignment)?;
        value.budget.liability_digest=canonical::digest(&json!({"version":3,"child":original.agent_reservation_id,"predecessorCommandDigest":digest,"stoppedReceiptDigest":receipt,"executionBudget":value.execution_budget,"callsDigest":calls.digest}))?;
    }
    Ok(value)
}
