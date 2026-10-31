//! A server promotion may commit before its local binding is saved. Once the
//! exact process has stopped, recover that relation without admitting old work.
use super::*;

fn stopped(
    db: &Connection,
    profile: &str,
    original: &SharedRuntimeCommand,
    binding: &SharedDemandBinding,
) -> Result<bool, Error> {
    if binding.runtime_command_id == original.command_id {
        if binding.runtime_command_digest != canonical::digest(&serde_json::to_value(original)?)? {
            return Err(Error::Conflict);
        }
        let payload: Option<String> = db.query_row(
            "SELECT payload FROM owner_shared_process_observations WHERE child_id=?1 AND json_extract(payload,'$.state')='stopped' ORDER BY ledger_revision DESC LIMIT 1",
            [&original.agent_reservation_id], |r| r.get(0),
        ).optional()?;
        let Some(payload) = payload else { return Ok(false) };
        serde_json::from_str::<SharedProcessReceipt>(&payload)?.validate_for(original)?;
        return Ok(true);
    }
    let (saved_profile, payload, digest): (String, String, String) = db.query_row(
        "SELECT profile,command_payload,command_digest FROM owner_shared_successor_attempts WHERE id=?1 AND child_id=?2",
        params![binding.runtime_command_id, original.agent_reservation_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let command: serde_json::Value = serde_json::from_str(&payload)?;
    if saved_profile != profile || digest != binding.runtime_command_digest
        || canonical::digest(&command)? != digest
    {
        return Err(Error::Conflict);
    }
    let payload: Option<String> = db.query_row(
        "SELECT payload FROM owner_shared_successor_observations WHERE attempt_id=?1 AND json_extract(payload,'$.state')='stopped' ORDER BY ledger_revision DESC LIMIT 1",
        [&binding.runtime_command_id], |r| r.get(0),
    ).optional()?;
    let Some(payload) = payload else { return Ok(false) };
    let original_digest = canonical::digest(&serde_json::to_value(original)?)?;
    match command.get("version").and_then(serde_json::Value::as_u64) {
        Some(2) => {
            let command: SharedAttemptCommand = serde_json::from_value(command)?;
            command.validate(&original.reserve_command.manager_mxid)?;
            if command.original_runtime_digest != original_digest { return Err(Error::Conflict) }
            serde_json::from_str::<SharedAttemptProcessReceipt>(&payload)?.validate_for(&command)?;
        }
        Some(3) => {
            let command: SharedBudgetAttemptCommand = serde_json::from_value(command)?;
            command.validate(&original.reserve_command.manager_mxid)?;
            if command.original_runtime_digest != original_digest { return Err(Error::Conflict) }
            serde_json::from_str::<SharedBudgetAttemptProcessReceipt>(&payload)?.validate_for(&command)?;
        }
        _ => return Err(Error::Conflict),
    }
    Ok(true)
}

impl OwnerResourceRegistry {
    /// Reconcile an authenticated historical promotion after a proven process
    /// stop. This writes only its exact binding: no inbox, call, start or refund.
    pub async fn recover_shared_demand_binding(
        &self,
        authority: ManagedCleanupAuthority,
        offer: SharedDemandOffer,
        binding: SharedDemandBinding,
    ) -> Result<(), Error> {
        authority.check(&self.0)?;
        binding.validate_offer(&offer)?;
        let profile = authority.enrollment.profile.clone();
        let expires = authority.deadline();
        let registry = self.0.clone();
        self.0.store.call(64 * 1024, move |db| {
            authority.check(&registry)?;
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            authority::cleanup(&tx, &authority)?;
            let saved = journal::load(&tx, &authority.enrollment.profile.digest, &authority.child, &offer.demand_id)?
                .ok_or(Error::LocalAuthority)?;
            if saved.offer.digest()? != offer.digest()? { return Err(Error::Conflict) }
            let digest = canonical::digest(&serde_json::to_value(&binding)?)?;
            if let Some(old) = tx.query_row("SELECT binding_digest FROM owner_shared_demand_bindings WHERE demand_id=?1", [&offer.demand_id], |r| r.get::<_, String>(0)).optional()? {
                return if old == digest { Ok(()) } else { Err(Error::Conflict) };
            }
            let (payload, original_digest): (String, String) = tx.query_row(
                "SELECT command_payload,command_digest FROM owner_shared_runtime_lifetimes WHERE child_id=?1 AND profile=?2",
                params![authority.child, authority.enrollment.profile.digest], |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let original: SharedRuntimeCommand = serde_json::from_str(&payload)?;
            original.validate(&authority.enrollment.profile.account)?;
            if original.agent_reservation_id != authority.child || original.assignment_id != offer.assignment_id
                || canonical::digest(&serde_json::to_value(&original)?)? != original_digest
            { return Err(Error::Conflict) }
            if !stopped(&tx, &authority.enrollment.profile.digest, &original, &binding)? { return Ok(()) }
            let budget_start = budget_demand::binding_start(&tx, &authority.enrollment.profile.digest, &authority.child, &offer, &binding.runtime_command_id, &binding.runtime_command_digest)?;
            if budget_start.is_none() {
                match saved.disposition {
                    SharedDemandDisposition::Pending => {
                        if binding.runtime_command_id != offer.predecessor_command_id || binding.runtime_command_digest != offer.predecessor_command_digest { return Err(Error::Conflict) }
                    }
                    SharedDemandDisposition::StartPrepared => {
                        let id: String = tx.query_row("SELECT start_intent_id FROM owner_shared_successor_attempts WHERE id=?1 AND child_id=?2 AND command_digest=?3", params![binding.runtime_command_id, authority.child, binding.runtime_command_digest], |r| r.get(0))?;
                        if saved.input.ok_or(Error::Conflict)?.start.start_intent_id != id { return Err(Error::Conflict) }
                    }
                    _ => return Err(Error::LocalAuthority),
                }
            }
            let revision = revision(&tx)?;
            tx.execute("INSERT INTO owner_shared_demand_bindings VALUES(?1,?2,?3,?4,?5)", params![binding.demand_id, binding.work_id, canonical::encode(&serde_json::to_value(&binding)?)?, digest, revision])?;
            authority.check(&registry)?;
            tx.commit()?;
            authority.check(&registry).map_err(|_| Error::OutcomeUnknown)
        }).await?;
        after(&profile, &self.0, expires)
    }
}
