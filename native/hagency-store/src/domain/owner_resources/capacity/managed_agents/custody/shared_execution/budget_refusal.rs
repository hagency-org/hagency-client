//! A definitive central never-started fence plus native non-issuance may release
//! one prepared hold. A timeout, a changed balance or serialized UI data cannot.
use super::*;

/// Authenticated native transport evidence. This is deliberately not serializable
/// and grants only the exact cancellation below, never process/call authority.
pub struct VerifiedSharedBudgetWorkRefusal {
    enrollment: VerifiedManagementEnrollment,
    command: SharedBudgetAttemptCommand,
    work: SharedWorkItem,
    record: SharedBudgetWorkRefusalRecord,
    expires: Instant,
}
impl VerifiedSharedBudgetWorkRefusal {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        command: SharedBudgetAttemptCommand,
        work: SharedWorkItem,
        record: SharedBudgetWorkRefusalRecord,
        expires: Instant,
    ) -> Result<Self, Error> {
        enrollment.check()?;
        command.validate(&enrollment.profile.account)?;
        record.refusal.validate_for(&command, &work, &record.refusal.input_digest)?;
        if record.refusal_digest != canonical::digest(&serde_json::to_value(&record.refusal)?)?
            || expires <= Instant::now()
            || expires > Instant::now() + Duration::from_secs(30)
        {
            return Err(Error::LocalAuthority);
        }
        Ok(Self { expires: expires.min(enrollment.expires), enrollment, command, work, record })
    }
    fn check(&self, r: &Inner) -> Result<(), Error> {
        after(&self.enrollment.profile, r, self.expires)?;
        self.enrollment.check()
    }
}

impl OwnerResourceRegistry {
    pub async fn refuse_unissued_shared_budget_work(
        &self,
        proof: VerifiedSharedBudgetWorkRefusal,
    ) -> Result<SharedWorkReceipt, Error> {
        proof.check(&self.0)?;
        let p = proof.enrollment.profile.clone();
        let expires = proof.expires;
        let registry = self.0.clone();
        let receipt = self.0.store.call(64 * 1024, move |db| {
            proof.check(&registry)?;
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let c = &proof.command;
            let w = &proof.work;
            let original = authority::original(&tx, &proof.enrollment, &c.agent_reservation_id, true)?;
            if original.digest != c.original_runtime_command.reserve_command_digest {
                return Err(Error::Conflict);
            }
            let command_digest = canonical::digest(&serde_json::to_value(c)?)?;
            let (saved_profile, saved_digest, incarnation, provider): (String,String,String,String) = tx.query_row(
                "SELECT profile,command_digest,incarnation,provider_digest FROM owner_shared_successor_attempts WHERE id=?1 AND child_id=?2",
                params![c.command_id,c.agent_reservation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
            )?;
            let work_digest: String = tx.query_row(
                "SELECT digest FROM owner_shared_work_inbox WHERE id=?1 AND child_id=?2 AND execution_id=?3",
                params![w.id,c.agent_reservation_id,w.execution_id], |r| r.get(0),
            )?;
            if saved_profile != proof.enrollment.profile.digest || saved_digest != command_digest
                || provider != original.provider || work_digest != proof.record.refusal.work_digest
            { return Err(Error::Conflict); }
            let process_digest = canonical::digest(&json!({"commandDigest":command_digest,"incarnation":incarnation,"providerDigest":provider}))?;
            let saved: Option<(String,String)> = tx.query_row(
                "SELECT f.refusal_digest,o.payload FROM owner_shared_budget_work_refusals f JOIN owner_shared_work_observations o ON o.id=f.observation_id WHERE f.work_id=?1 AND f.command_digest=?2",
                params![w.id,command_digest], |r| Ok((r.get(0)?,r.get(1)?)),
            ).optional()?;
            if let Some((digest, payload)) = saved {
                if digest != proof.record.refusal_digest { return Err(Error::Conflict); }
                let receipt: SharedWorkReceipt = serde_json::from_str(&payload)?;
                receipt.validate_for(w, &process_digest)?;
                proof.check(&registry)?;
                return Ok(receipt);
            }
            // Historical budget remains exact after a later delta. Central has
            // permanently fenced Start; only native issue_revision proves no
            // provider effect escaped locally, under this same transaction.
            super::super::budget::execution::resolve(&tx, &c.agent_reservation_id, Some(&c.execution_budget), false)?;
            let row: (String,String,String,String,Option<u64>,u64,bool) = tx.query_row(
                "SELECT c.id,c.state,p.execution_digest,s.attempt_id,p.issue_revision,c.known_consumed_tokens,c.unrepresentable_usage FROM owner_managed_calls c JOIN owner_managed_call_pins p ON p.call_id=c.id JOIN owner_shared_successor_call_pins s ON s.call_id=c.id WHERE c.child_id=?1 AND c.execution_id=?2 AND s.work_id=?3",
                params![c.agent_reservation_id,w.execution_id,w.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)),
            )?;
            let (call_id, state, input, attempt, issued, consumed, overflow) = row;
            if !matches!(state.as_str(), "prepared" | "cancelled_before_issue") || issued.is_some()
                || input != work_digest || attempt != c.command_id || consumed != 0 || overflow
            { return Err(Error::OutcomeUnknown); }
            let contradictory: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM owner_shared_work_events WHERE work_id=?1 AND kind IN('start_confirmed','call_issued')) OR EXISTS(SELECT 1 FROM owner_shared_work_observations WHERE work_id=?1)",
                [&w.id], |r| r.get(0),
            )?;
            if contradictory { return Err(Error::OutcomeUnknown); }
            // Resolve the original saved contextual input, even if its context
            // pin was not reached before the server refused Start.
            let mode: Option<String> = tx.query_row("SELECT mode FROM owner_shared_input_modes WHERE work_id=?1", [&w.id], |r| r.get(0)).optional()?;
            let input_digest = if mode.as_deref() == Some("thread_context_v1") {
                tx.query_row("SELECT input_digest FROM owner_shared_contexts WHERE work_id=?1", [&w.id], |r| r.get::<_,String>(0))?
            } else { w.body_digest.clone() };
            proof.record.refusal.validate_for(c, w, &input_digest)?;
            tx.execute("UPDATE owner_managed_calls SET state='cancelled_before_issue',usage_complete=1 WHERE id=?1 AND state IN('prepared','cancelled_before_issue')", [&call_id])?;
            let revision = revision(&tx)?;
            let observation_id = format!("swo_{}", canonical::digest(&json!({"workId":w.id,"refusalDigest":proof.record.refusal_digest}))?);
            let receipt = SharedWorkReceipt {
                version: 1, observation_id: observation_id.clone(), work_id: w.id.clone(), work_digest,
                execution_id: w.execution_id.clone(), call_id: call_id.clone(), runtime_command_digest: command_digest.clone(),
                process_digest: process_digest.clone(), ledger_revision: revision, state: "failed".into(),
                known_consumed_tokens: 0, retained_tokens: Some(0), usage_complete: true, body: None, body_digest: None,
            };
            receipt.validate_for(w, &process_digest)?;
            bounded_row(&tx, "owner_shared_work_observations", "id", &observation_id, 1_000_000)?;
            tx.execute("INSERT INTO owner_shared_work_observations VALUES(?1,?2,?3,?4,?5)", params![observation_id,w.id,canonical::encode(&serde_json::to_value(&receipt)?)?,canonical::digest(&serde_json::to_value(&receipt)?)?,revision])?;
            bounded_row(&tx, "owner_shared_budget_work_refusals", "work_id", &w.id, 100_000)?;
            tx.execute("INSERT INTO owner_shared_budget_work_refusals VALUES(?1,?2,?3,?4,?5,?6,?7)", params![w.id,call_id,command_digest,canonical::encode(&serde_json::to_value(&proof.record.refusal)?)?,proof.record.refusal_digest,observation_id,revision])?;
            proof.check(&registry)?;
            tx.commit()?;
            proof.check(&registry).map_err(|_| Error::OutcomeUnknown)?;
            Ok(receipt)
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(receipt)
    }
}
