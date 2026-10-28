//! Read-only prerequisite for the separately versioned successor protocol.
//! This snapshot cannot issue a process, release a hold, or authorize a turn.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedSuccessorBudgetState {
    Available,
    Exhausted,
    Unrepresentable,
}

/// Native-only observation under current original enrollment authority. No
/// Deserialize or constructor: future claim/turn writers must recompute it in
/// their own transaction, never accept these numbers as capacity authority.
/// Available means child-local headroom only. It does not bypass resource/Seat
/// conservation, qualify a new process, or relax any existing v1 call gate.
#[derive(Debug)]
#[non_exhaustive]
pub struct SharedSuccessorBudget {
    pub observed_ledger_revision: u64,
    pub predecessor_command_digest: String,
    pub stopped_receipt_digest: String,
    pub allocated_tokens: u64,
    pub known_consumed_tokens: u64,
    pub retained_tokens: u64,
    pub remaining_tokens: u64,
    pub usage_complete: bool,
    pub state: SharedSuccessorBudgetState,
    pub liability_digest: String,
}

pub(super) struct Calls {
    pub(super) consumed: u64,
    pub(super) retained: u64,
    pub(super) complete: bool,
    pub(super) unrepresentable: bool,
    pub(super) digest: String,
}

fn add(value: u64, next: u64) -> Result<u64, Error> {
    value
        .checked_add(next)
        .filter(|n| *n <= JSON_SAFE_MAX)
        .ok_or(Error::Capacity)
}

// A supported incomplete call keeps at least its original hold. In particular,
// settled-with-incomplete-usage is not evidence that its reservation is free.
pub(super) fn calls(db: &Connection, child: &str, binding: &str) -> Result<Calls, Error> {
    let mut query = db.prepare(
        "SELECT c.id,c.execution_id,c.immutable_digest,c.assignment_generation,c.state,c.reserved_tokens,c.known_consumed_tokens,c.usage_complete,c.unrepresentable_usage,p.assignment_digest,p.execution_digest,p.prepare_revision,p.issue_revision FROM owner_managed_calls c LEFT JOIN owner_managed_call_pins p ON p.call_id=c.id WHERE c.child_id=?1 ORDER BY c.id",
    )?;
    let mut rows = query.query([child])?;
    let mut result = Calls {
        consumed: 0,
        retained: 0,
        complete: true,
        unrepresentable: false,
        digest: String::new(),
    };
    let mut digest = Sha256::new();
    digest.update(b"[");
    let mut count = 0;
    while let Some(row) = rows.next()? {
        if count >= 100_000 {
            return Err(Error::Capacity);
        }
        let id: String = row.get(0)?;
        let execution: String = row.get(1)?;
        let immutable: String = row.get(2)?;
        let generation: u64 = row.get(3)?;
        let state: String = row.get(4)?;
        let reserved: u64 = row.get(5)?;
        let consumed: u64 = row.get(6)?;
        let complete: bool = row.get(7)?;
        let overflow: bool = row.get(8)?;
        let assignment: Option<String> = row.get(9)?;
        let input: Option<String> = row.get(10)?;
        let prepared: Option<u64> = row.get(11)?;
        let issued: Option<u64> = row.get(12)?;
        let supported = assignment.as_deref() == Some(binding)
            && input.as_ref().is_some_and(|v| v.len() == 64)
            && prepared.is_some_and(|v| v > 0)
            && generation == 1
            && immutable
                == canonical::digest(&json!({
                    "child":child,"assignmentDigest":binding,"execution":execution,
                    "executionDigest":input,"estimatedTokens":reserved
                }))?
            && match state.as_str() {
                "prepared" | "cancelled_before_issue" => issued.is_none() && consumed == 0,
                "issued" | "unknown" | "settled" => {
                    issued.zip(prepared).is_some_and(|(i, p)| i > p)
                }
                _ => false,
            };
        let cancelled = supported && state == "cancelled_before_issue";
        let settled = supported && state == "settled" && complete && !overflow;
        let retained = if cancelled {
            0
        } else if settled {
            consumed
        } else {
            reserved.max(consumed)
        };
        result.consumed = add(result.consumed, consumed)?;
        result.retained = add(result.retained, retained)?;
        result.unrepresentable |= overflow || !supported;
        result.complete &= cancelled || settled;
        if count > 0 {
            digest.update(b",");
        }
        digest.update(
            canonical::encode(&json!({
                "id":id,"executionId":execution,"immutableDigest":immutable,
                "assignmentGeneration":generation,"state":state,"reservedTokens":reserved,
                "knownConsumedTokens":consumed,"usageComplete":complete,
                "unrepresentableUsage":overflow,"assignmentDigest":assignment,
                "executionDigest":input,"prepareRevision":prepared,"issueRevision":issued,
                "retainedTokens":retained,"supported":supported
            }))?
            .as_bytes(),
        );
        count += 1;
    }
    digest.update(b"]");
    result.digest = format!("{:x}", digest.finalize());
    Ok(result)
}

pub(super) fn snapshot(
    db: &Connection,
    enrollment: &VerifiedManagementEnrollment,
    command: &SharedRuntimeCommand,
) -> Result<SharedSuccessorBudget, Error> {
    snapshot_for_head(db, enrollment, command, None)
}
pub(super) fn snapshot_for_head(
    db: &Connection,
    enrollment: &VerifiedManagementEnrollment,
    command: &SharedRuntimeCommand,
    head: Option<&SharedAttemptCommand>,
) -> Result<SharedSuccessorBudget, Error> {
    let original = authority::original(db, enrollment, &command.agent_reservation_id, false)?;
    if original.digest != command.reserve_command_digest {
        return Err(Error::LocalAuthority);
    }
    let digest = canonical::digest(&serde_json::to_value(command)?)?;
    let row: Option<(String, String, String, String, String)> = db.query_row(
        "SELECT profile,command_digest,incarnation,provider_digest,command_payload FROM owner_shared_runtime_lifetimes WHERE child_id=?1 AND assignment_id=?2",
        params![command.agent_reservation_id,command.assignment_id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    ).optional()?;
    let (profile, saved_digest, incarnation, provider, payload) =
        row.ok_or(Error::OutcomeUnknown)?;
    if profile != enrollment.profile.digest
        || saved_digest != digest
        || canonical::digest(&serde_json::from_str::<serde_json::Value>(&payload)?)? != digest
        || provider != original.provider
    {
        return Err(Error::LocalAuthority);
    }
    let (digest, incarnation, stopped_payload, stopped_digest, stop_revision) = if let Some(head) =
        head
    {
        head.validate(&enrollment.profile.account)?;
        if head.original_runtime_digest != digest {
            return Err(Error::LocalAuthority);
        }
        let expected = canonical::digest(&serde_json::to_value(head)?)?;
        let row:Option<(String,String,String,String,u64,String)>=db.query_row("SELECT profile,command_digest,incarnation,provider_digest,process_attempt,command_payload FROM owner_shared_successor_attempts WHERE id=?1 AND child_id=?2",params![head.command_id,command.agent_reservation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
        let (saved_profile, saved, incarnation, saved_provider, number, payload) =
            row.ok_or(Error::OutcomeUnknown)?;
        let latest: u64 = db.query_row(
            "SELECT MAX(process_attempt) FROM owner_shared_successor_attempts WHERE child_id=?1",
            [&command.agent_reservation_id],
            |r| r.get(0),
        )?;
        if saved_profile != enrollment.profile.digest
            || saved != expected
            || saved_provider != provider
            || number != head.process_attempt
            || latest != number
            || canonical::digest(&serde_json::from_str::<serde_json::Value>(&payload)?)? != expected
        {
            return Err(Error::Conflict);
        }
        let stopped:Option<(String,String,u64)>=db.query_row("SELECT payload,digest,ledger_revision FROM owner_shared_successor_observations WHERE attempt_id=?1 AND json_extract(payload,'$.state')='stopped' ORDER BY ledger_revision DESC LIMIT 1",[&head.command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let (payload, digest, revision) = stopped.ok_or(Error::OutcomeUnknown)?;
        (expected, incarnation, payload, digest, revision)
    } else {
        let stopped: Option<(String,String,u64)>=db.query_row("SELECT payload,digest,ledger_revision FROM owner_shared_process_observations WHERE child_id=?1 AND json_extract(payload,'$.state')='stopped' ORDER BY ledger_revision DESC LIMIT 1",[&command.agent_reservation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let (payload, stopped_digest, revision) = stopped.ok_or(Error::OutcomeUnknown)?;
        (digest, incarnation, payload, stopped_digest, revision)
    };
    let process = canonical::digest(
        &json!({"commandDigest":digest,"incarnation":incarnation,"providerDigest":provider}),
    )?;
    let (state, observed_process, observed_revision, receipt_digest) = if let Some(head) = head {
        let stopped: SharedAttemptProcessReceipt = serde_json::from_str(&stopped_payload)?;
        stopped.validate_for(head)?;
        let receipt_digest = canonical::digest(&serde_json::to_value(&stopped)?)?;
        (
            stopped.state,
            stopped.process_digest,
            stopped.ledger_revision,
            receipt_digest,
        )
    } else {
        let stopped: SharedProcessReceipt = serde_json::from_str(&stopped_payload)?;
        stopped.validate_for(command)?;
        let receipt_digest = canonical::digest(&serde_json::to_value(&stopped)?)?;
        (
            stopped.state,
            stopped.process_digest,
            stopped.ledger_revision,
            receipt_digest,
        )
    };
    if state != "stopped"
        || observed_process.as_deref() != Some(&process)
        || observed_revision != stop_revision
        || receipt_digest != stopped_digest
    {
        return Err(Error::LocalAuthority);
    }
    let binding = canonical::digest(&json!({
        "child":command.agent_reservation_id,"assignmentId":command.assignment_id,
        "assignmentGeneration":command.assignment_generation,
        "commandDigest":command.reserve_command_digest,"providerDigest":provider
    }))?;
    let calls = calls(db, &command.agent_reservation_id, &binding)?;
    let observed_ledger_revision: u64 = db.query_row(
        "SELECT revision FROM owner_resource_identity WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if observed_ledger_revision < stop_revision {
        return Err(Error::LocalAuthority);
    }
    let remaining = original.allocated.saturating_sub(calls.retained);
    let state = if calls.unrepresentable {
        SharedSuccessorBudgetState::Unrepresentable
    } else if remaining == 0 {
        SharedSuccessorBudgetState::Exhausted
    } else {
        SharedSuccessorBudgetState::Available
    };
    Ok(SharedSuccessorBudget {
        observed_ledger_revision,
        predecessor_command_digest: digest.clone(),
        stopped_receipt_digest: stopped_digest.clone(),
        allocated_tokens: original.allocated,
        known_consumed_tokens: calls.consumed,
        retained_tokens: calls.retained,
        remaining_tokens: if calls.unrepresentable { 0 } else { remaining },
        usage_complete: calls.complete,
        state,
        liability_digest: canonical::digest(&json!({
            "child":command.agent_reservation_id,"predecessorCommandDigest":digest,
            "stoppedReceiptDigest":stopped_digest,"allocatedTokens":original.allocated,
            "callsDigest":calls.digest
        }))?,
    })
}

impl OwnerResourceRegistry {
    /// Observe an original stopped process and all prior call charges in one
    /// read snapshot. This neither qualifies a successor nor changes v1 gates.
    pub async fn shared_successor_budget(
        &self,
        enrollment: VerifiedManagementEnrollment,
        predecessor: SharedRuntimeCommand,
    ) -> Result<SharedSuccessorBudget, Error> {
        enrollment.check()?;
        predecessor.validate(&enrollment.profile.account)?;
        let profile = enrollment.profile.clone();
        let expires = enrollment
            .expires
            .min(Instant::now() + Duration::from_secs(30));
        let registry = self.0.clone();
        let value = self
            .0
            .store
            .call(32 * 1024, move |db| {
                after(&enrollment.profile, &registry, expires)?;
                enrollment.check()?;
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Deferred)?;
                let value = snapshot(&tx, &enrollment, &predecessor)?;
                enrollment.check()?;
                after(&enrollment.profile, &registry, expires)?;
                tx.commit()?;
                Ok(value)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(value)
    }
}

impl OwnerResourceRegistry {
    /// Read all child liabilities against the exact latest versioned stopped
    /// predecessor. The projection never reconstructs process custody.
    pub async fn shared_attempt_successor_budget(
        &self,
        enrollment: VerifiedManagementEnrollment,
        predecessor: SharedAttemptCommand,
    ) -> Result<SharedSuccessorBudget, Error> {
        enrollment.check()?;
        predecessor.validate(&enrollment.profile.account)?;
        let profile = enrollment.profile.clone();
        let expires = enrollment
            .expires
            .min(Instant::now() + Duration::from_secs(30));
        let registry = self.0.clone();
        let value = self
            .0
            .store
            .call(64 * 1024, move |db| {
                after(&enrollment.profile, &registry, expires)?;
                enrollment.check()?;
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Deferred)?;
                let value = snapshot_for_head(
                    &tx,
                    &enrollment,
                    &predecessor.original_runtime_command,
                    Some(&predecessor),
                )?;
                enrollment.check()?;
                after(&enrollment.profile, &registry, expires)?;
                tx.commit()?;
                Ok(value)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(value)
    }
}
