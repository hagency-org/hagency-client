//! Native phase-A budget deltas. Readable credit is not an execution capability.
use super::*;
mod types;
use types::digest;
pub use types::*;
mod journal;
#[cfg(feature = "agent-budget")]
mod writer;
#[cfg(feature = "agent-budget")]
pub use writer::VerifiedAgentBudgetDelivery;

/// Descriptive, native-derived budget. It cannot construct a call or successor.
#[derive(Debug)]
#[non_exhaustive]
pub struct AgentBudgetSnapshot {
    pub budget_revision: u64,
    pub budget_digest: String,
    pub original_ceiling_tokens: u64,
    pub effective_ceiling_tokens: u64,
    pub known_consumed_tokens: u64,
    pub retained_tokens: u64,
    pub remaining_tokens: u64,
    pub usage_complete: bool,
    pub execution_supported: bool,
}
/// Every grant/project/Seat liability collector uses this same applied chain.
/// This intentionally does not replace Binding.allocated in existing v1/v2 issuers.
pub(in super::super) fn effective_ceiling(
    db: &Connection,
    child: &str,
    original: u64,
) -> Result<u64, Error> {
    let any: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_managed_budget_commands WHERE child_id=?1 UNION ALL SELECT 1 FROM owner_managed_budget_deltas WHERE child_id=?1)",
        [child],
        |r| r.get(0),
    )?;
    if !any {
        return Ok(original);
    }
    let h = journal::head(db, child)?;
    if h.original.allocated_tokens != original {
        return Err(Error::Conflict);
    }
    Ok(h.ceiling)
}
fn call_liability(db: &Connection, child: &str) -> Result<(u64, u64, bool, bool), Error> {
    let binding: Option<String> = db
        .query_row(
            "SELECT binding_digest FROM owner_managed_assignment_pins WHERE child_id=?1",
            [child],
            |r| r.get(0),
        )
        .optional()?;
    let has_calls: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_managed_calls WHERE child_id=?1)",
        [child],
        |r| r.get(0),
    )?;
    match binding {
        Some(binding) => shared_execution::budget_liability(db, child, &binding),
        None if !has_calls => Ok((0, 0, true, false)),
        None => Err(Error::Capacity),
    }
}
// A lost process claim or an explicit unknown observation can hide an effect.
// Keep its ceiling held until the same process supplies qualified/stopped
// evidence. Do not confuse this uncertainty with a qualified idle process.
fn process_unknown(db: &Connection, child: &str) -> Result<bool, Error> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_shared_runtime_lifetimes l WHERE l.child_id=?1 AND COALESCE((SELECT json_extract(o.payload,'$.state') FROM owner_shared_process_observations o WHERE o.child_id=l.child_id ORDER BY o.ledger_revision DESC LIMIT 1),'unknown') NOT IN('qualified','stopped')) OR EXISTS(SELECT 1 FROM owner_shared_successor_attempts a WHERE a.child_id=?1 AND COALESCE((SELECT json_extract(o.payload,'$.state') FROM owner_shared_successor_observations o WHERE o.attempt_id=a.id ORDER BY o.ledger_revision DESC LIMIT 1),'unknown') NOT IN('qualified','stopped'))",
        [child], |r| r.get(0),
    )?)
}
impl OwnerResourceRegistry {
    pub async fn managed_agent_budget(
        &self,
        e: VerifiedManagementEnrollment,
        child: String,
    ) -> Result<AgentBudgetSnapshot, Error> {
        e.check()?;
        identifier(&child, 128)?;
        let profile = e.profile.clone();
        let expires = e.expires;
        let fence = self.0.clone();
        let result = self
            .0
            .store
            .call(8192, move |db| {
                e.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                authority::original(&db.db, &e, &child, true)?;
                let h = journal::head(&db.db, &child)?;
                let (known, held, complete, unrepresentable) = call_liability(&db.db, &child)?;
                if unrepresentable {
                    return Err(Error::Capacity);
                }
                let unknown = process_unknown(&db.db, &child)?;
                let held = if unknown { held.max(h.ceiling) } else { held };
                let value = AgentBudgetSnapshot {
                    budget_revision: h.revision,
                    budget_digest: h.digest,
                    original_ceiling_tokens: h.original.allocated_tokens,
                    effective_ceiling_tokens: h.ceiling,
                    known_consumed_tokens: known,
                    retained_tokens: held,
                    remaining_tokens: h.ceiling.saturating_sub(held),
                    usage_complete: complete && !unknown,
                    execution_supported: false,
                };
                e.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                Ok(value)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
    /// Observation-only recovery works after expiry/fencing under exact original cleanup identity.
    pub async fn recover_managed_budget(
        &self,
        e: VerifiedManagementEnrollment,
        c: AgentBudgetCommand,
    ) -> Result<Option<AgentBudgetReceipt>, Error> {
        e.check()?;
        c.validate(&e.profile.account)?;
        let profile = e.profile.clone();
        let expires = e.expires;
        let fence = self.0.clone();
        let result = self
            .0
            .store
            .call(32 * 1024, move |db| {
                e.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                let b = authority::original(&db.db, &e, c.child()?, true)?;
                if b.digest != c.reserve_command_digest {
                    return Err(Error::Conflict);
                }
                let result = journal::saved(&db.db, &e, &c)?;
                e.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                Ok(result)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
}

#[cfg(feature = "shared-budget-execution")]
pub(super) mod execution;

/// A settlement reports the exact applied budget chain without issuing credit.
pub(in super::super) fn settlement_head(
    db: &Connection,
    child: &str,
) -> Result<(u64, String, u64), Error> {
    let h = journal::head(db, child)?;
    Ok((h.revision, h.digest, h.ceiling))
}
