//! The uncertainty time bound for a claimed v3 attempt; the same rule as v2
//! (ADR-193). The pinned budget head is copied from the claim, never moved.
use super::*;

impl OwnerResourceRegistry {
    pub async fn record_shared_budget_attempt_timeout(
        &self,
        authority: ManagedCleanupAuthority,
        command: SharedBudgetAttemptCommand,
    ) -> Result<Option<SharedBudgetAttemptProcessReceipt>, Error> {
        authority.check(&self.0)?;
        command.validate(&authority.enrollment.profile.account)?;
        if authority.child != command.agent_reservation_id {
            return Err(Error::LocalAuthority);
        }
        let profile = authority.enrollment.profile.clone();
        let expires = authority.deadline();
        let registry = self.0.clone();
        let value = self.0.store.call(64 * 1024, move |db| {
            authority.check(&registry)?;
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            authority::cleanup(&tx, &authority)?;
            let digest = canonical::digest(&serde_json::to_value(&command)?)?;
            let claim: Option<(String, String, String, String)> = tx.query_row(
                "SELECT profile,command_digest,incarnation,provider_digest FROM owner_shared_successor_attempts WHERE id=?1 AND child_id=?2",
                params![command.command_id, command.agent_reservation_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            ).optional()?;
            let Some((saved_profile, saved, incarnation, provider)) = claim else {
                authority.check(&registry)?;
                return Ok(None);
            };
            if saved_profile != authority.enrollment.profile.digest || saved != digest {
                return Err(Error::Conflict);
            }
            let fenced: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM owner_shared_successor_fences WHERE command_id=?1 AND command_digest=?2)",
                params![command.command_id, digest], |r| r.get(0),
            )?;
            let stopped: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM owner_shared_successor_observations WHERE attempt_id=?1 AND json_extract(payload,'$.state')='stopped')",
                [&command.command_id], |r| r.get(0),
            )?;
            if !fenced || stopped || !attempt::timeout_due(command.expires_at_ms)? {
                authority.check(&registry)?;
                return Ok(None);
            }
            let process = canonical::digest(&json!({"commandDigest":digest,"incarnation":incarnation,"providerDigest":provider}))?;
            let id = format!("sapo_{}", canonical::digest(&json!({"processDigest":process,"state":"stopped","policyDigest":null,"reason":UNCERTAIN_TIMEOUT}))?);
            let revision = revision(&tx)?;
            let receipt = SharedBudgetAttemptProcessReceipt {
                version: 3,
                observation_id: id.clone(),
                command_id: command.command_id.clone(),
                command_digest: digest,
                assignment_id: command.assignment_id.clone(),
                agent_reservation_id: command.agent_reservation_id.clone(),
                process_attempt: command.process_attempt,
                original_runtime_digest: command.original_runtime_digest.clone(),
                budget_revision: command.execution_budget.budget_revision,
                budget_digest: command.execution_budget.budget_digest.clone(),
                ledger_revision: revision,
                process_digest: Some(process),
                state: "stopped".into(),
                policy_digest: None,
                reason: Some(UNCERTAIN_TIMEOUT.into()),
            };
            receipt.validate_for(&command)?;
            bounded_row(&tx, "owner_shared_successor_observations", "id", &id, 100_000)?;
            tx.execute(
                "INSERT INTO owner_shared_successor_observations VALUES(?1,?2,?3,?4,?5)",
                params![id, command.command_id, canonical::encode(&serde_json::to_value(&receipt)?)?, canonical::digest(&serde_json::to_value(&receipt)?)?, revision],
            )?;
            observations::close_stopped_calls(&tx, &command.agent_reservation_id)?;
            authority.check(&registry)?;
            tx.commit()?;
            authority.check(&registry).map_err(|_| Error::OutcomeUnknown)?;
            Ok(Some(receipt))
        }).await?;
        after(&profile, &self.0, expires)?;
        Ok(value)
    }
}
