//! An authenticated terminal state for an exact saved v2 offer (ADR-193). It
//! only releases the offer's FIFO position: no work, call, start, process or
//! capacity history changes, and its event is never offered or started again.
//! Same-installation cleanup authority suffices, so an offer older than a
//! day can still end; the server's state is the only input that is trusted.
use super::*;

/// Budget blocks end an event here only after a retry (ADR-193 amendment):
/// the original start's own block is its saved decision.
const TERMINAL: [&str; 5] = [
    "expired",
    "cancelled",
    "head_changed",
    "budget_blocked",
    "usage_unrepresentable",
];

/// Version-neutral idempotency check over a retained terminal payload.
pub(in super::super) fn same_terminal(saved: &str, offer: &SharedDemandOffer, state: &str) -> Result<bool, Error> {
    let saved: serde_json::Value = serde_json::from_str(saved)?;
    let old: SharedDemandOffer = serde_json::from_value(saved["offer"].clone())?;
    Ok(old.digest()? == offer.digest()? && saved["state"] == state)
}

impl OwnerResourceRegistry {
    pub async fn retain_shared_demand_terminal(
        &self,
        authority: ManagedCleanupAuthority,
        offer: SharedDemandOffer,
        state: String,
        runtime_command_id: Option<String>,
    ) -> Result<(), Error> {
        authority.check(&self.0)?;
        offer.validate()?;
        if !TERMINAL.contains(&state.as_str()) {
            return Err(Error::LocalAuthority);
        }
        if let Some(id) = &runtime_command_id {
            identifier(id, 128)?;
        }
        let profile = authority.enrollment.profile.clone();
        let expires = authority.deadline();
        let registry = self.0.clone();
        self.0.store.call(64 * 1024, move |db| {
            authority.check(&registry)?;
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            authority::cleanup(&tx, &authority)?;
            let Some(old) = journal::load(&tx, &authority.enrollment.profile.digest, &authority.child, &offer.demand_id)? else {
                authority.check(&registry)?;
                return Ok(());
            };
            if old.offer.digest()? != offer.digest()? {
                return Err(Error::Conflict);
            }
            let saved: Option<String> = tx.query_row(
                "SELECT payload FROM owner_shared_budget_demand_terminals WHERE demand_id=?1",
                [&offer.demand_id], |r| r.get(0),
            ).optional()?;
            if let Some(saved) = saved {
                authority.check(&registry)?;
                return if same_terminal(&saved, &offer, &state)? { Ok(()) } else { Err(Error::Conflict) };
            }
            let value = json!({"version":1,"offer":offer,"demandDigest":offer.digest()?,"state":state,"runtimeCommandId":runtime_command_id});
            let revision = revision(&tx)?;
            tx.execute(
                "INSERT INTO owner_shared_budget_demand_terminals VALUES(?1,?2,?3,?4)",
                params![offer.demand_id, canonical::encode(&value)?, canonical::digest(&value)?, revision],
            )?;
            authority.check(&registry)?;
            tx.commit()?;
            authority.check(&registry).map_err(|_| Error::OutcomeUnknown)
        }).await?;
        after(&profile, &self.0, expires)
    }
}
