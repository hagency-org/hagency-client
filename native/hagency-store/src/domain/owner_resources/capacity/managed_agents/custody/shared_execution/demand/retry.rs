//! ADR-193 amendment: an event whose attempt provably never started runs again
//! in place. The demand, its offer and its FIFO place stay. A retry is one new
//! start intent, whose id derives from the demand and the retry number, that
//! names the demand's never-started attempt. Proof is local and immutable:
//! every earlier start of the event has an unclaimed fence and no claim.
//! Nothing here starts a process, a call or a model turn.
use super::*;

/// Retries per event; the server enforces the same cap.
pub const MAX_DEMAND_RETRIES: u64 = 3;

/// The start intent id of retry `n` of `demand_id`: `sdr<n>_<demand id>`.
pub(in super::super) fn start_id(demand_id: &str, n: u64) -> Result<String, Error> {
    if !(1..=MAX_DEMAND_RETRIES).contains(&n) {
        return Err(Error::LocalAuthority);
    }
    identifier(demand_id, 120)?;
    let id = format!("sdr{n}_{demand_id}");
    identifier(&id, 128)?;
    Ok(id)
}

/// The demand and retry number named by a retry start intent id.
pub(in super::super) fn parse(start_intent_id: &str) -> Option<(String, u64)> {
    let (n, demand) = start_intent_id.strip_prefix("sdr")?.split_once('_')?;
    let number: u64 = n.parse().ok()?;
    ((1..=MAX_DEMAND_RETRIES).contains(&number) && number.to_string() == n && !demand.is_empty())
        .then(|| (demand.to_owned(), number))
}

/// The v2 demand that started `start_intent_id` and which start it was: 0 for
/// the original start, `n` for retry `n`.
pub(in super::super) fn demand_for_start(
    db: &Connection,
    start_intent_id: &str,
) -> Result<Option<(String, u64)>, Error> {
    let original: Option<String> = db
        .query_row(
            "SELECT demand_id FROM owner_shared_demand_decisions WHERE start_intent_id=?1",
            [start_intent_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(original.map(|id| (id, 0)).or_else(|| parse(start_intent_id)))
}

/// The saved start input of start `n` of a saved demand.
pub(in super::super) fn saved_start(
    db: &Connection,
    profile: &str,
    child: &str,
    saved: &SharedDemandIntent,
    n: u64,
) -> Result<SharedAttemptStartInput, Error> {
    let first = saved.input.as_ref().ok_or(Error::Conflict)?;
    if n == 0 {
        return Ok(first.start.clone());
    }
    let id = start_id(&saved.offer.demand_id, n)?;
    let row: (String, String, String, String) = db.query_row(
        "SELECT profile,child_id,input_payload,input_digest FROM owner_shared_start_intents WHERE id=?1",
        [&id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let input: SharedAttemptStartInput = serde_json::from_str(&row.2)?;
    if row.0 != profile
        || row.1 != child
        || input.start_intent_id != id
        || input.assignment_id != saved.offer.assignment_id
        || canonical::digest(&serde_json::to_value(&input)?)? != row.3
    {
        return Err(Error::Conflict);
    }
    Ok(input)
}

/// Every earlier start of the event is provably unstarted on this
/// installation, and the latest one is the never-started head. Returns that
/// head's command id.
fn proven_unstarted(
    db: &Connection,
    profile: &str,
    original: &SharedRuntimeCommand,
    saved: &SharedDemandIntent,
    n: u64,
) -> Result<String, Error> {
    let child = &original.agent_reservation_id;
    let mut previous = String::new();
    for k in 0..n {
        let start = saved_start(db, profile, child, saved, k)?;
        let claimed: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM owner_shared_successor_attempts WHERE start_intent_id=?1) OR EXISTS(SELECT 1 FROM owner_shared_successor_fences WHERE start_intent_id=?1 AND claimed_revision IS NOT NULL)",
            [&start.start_intent_id],
            |r| r.get(0),
        )?;
        if claimed {
            // A claimed attempt may have run: never another turn for this event.
            return Err(Error::LocalAuthority);
        }
        previous = db
            .query_row(
                "SELECT command_id FROM owner_shared_successor_fences WHERE start_intent_id=?1 AND child_id=?2 AND profile=?3 AND claimed_revision IS NULL",
                params![start.start_intent_id, child, profile],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(Error::Conflict)?;
    }
    let head = attempt_unstarted::head(db, profile, original)?.ok_or(Error::Conflict)?;
    if head.command_id() != previous {
        return Err(Error::Conflict);
    }
    Ok(previous)
}

/// A retry's next step: its start request, or the budget that blocks it.
pub enum SharedDemandRetry {
    Start(SharedDemandSubmission),
    Blocked(SharedDemandBudget),
}

impl OwnerResourceRegistry {
    /// Create or recover retry `n` of a saved demand whose earlier starts all
    /// provably never started here. A replay returns the same start intent; no
    /// process, call or model turn is started.
    pub async fn prepare_shared_demand_retry(
        &self,
        proof: VerifiedSharedDemand,
        original: SharedRuntimeCommand,
        predecessor: Option<SharedAttemptCommand>,
        n: u64,
    ) -> Result<SharedDemandRetry, Error> {
        if !cfg!(feature = "shared-intake") {
            return Err(Error::Unqualified);
        }
        proof.check()?;
        original.validate(&proof.enrollment.profile.account)?;
        let id = start_id(&proof.offer.demand_id, n)?;
        let profile = proof.enrollment.profile.clone();
        let p = profile.clone();
        let expires = proof.expires;
        let registry = self.0.clone();
        let result = self.0.store.call(64 * 1024, move |db| {
            proof.check()?;
            if registry.closed.load(Ordering::Acquire) {
                return Err(Error::LocalAuthority);
            }
            let e = &proof.enrollment;
            let child = original.agent_reservation_id.clone();
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let b = authority::original(&tx, e, &child, false)?;
            if b.digest != original.reserve_command_digest
                || proof.offer.assignment_id != original.assignment_id
                || original.reserve_command.executor_device_id != e.executor_device_id
                || original.reserve_command.executor_generation != e.executor_generation
            {
                return Err(Error::LocalAuthority);
            }
            let saved = journal::load(&tx, &profile.digest, &child, &proof.offer.demand_id)?
                .ok_or(Error::LocalAuthority)?;
            if saved.offer.digest()? != proof.offer.digest()? {
                return Err(Error::Conflict);
            }
            journal::control(&tx, &profile.digest, &child, proof.offer.control_revision)?;
            let finished: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM owner_shared_demand_bindings WHERE demand_id=?1) OR EXISTS(SELECT 1 FROM owner_shared_budget_demand_terminals WHERE demand_id=?1)",
                [&proof.offer.demand_id],
                |r| r.get(0),
            )?;
            if finished {
                return Err(Error::LocalAuthority);
            }
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM owner_shared_start_intents WHERE id=?1)",
                [&id],
                |r| r.get(0),
            )?;
            let start = if exists {
                // Replay of the same retry: its saved bytes, never a new start.
                saved_start(&tx, &profile.digest, &child, &saved, n)?
            } else {
                let head = proven_unstarted(&tx, &profile.digest, &original, &saved, n)?;
                let Some(c) = predecessor.as_ref().filter(|c| c.command_id == head) else {
                    return Err(Error::Conflict);
                };
                // No other start (manual or another event) names this head.
                let competing: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM owner_shared_start_intents WHERE child_id=?1 AND predecessor_digest=?2 AND id<>?3)",
                    params![child, canonical::digest(&serde_json::to_value(c)?)?, id],
                    |r| r.get(0),
                )?;
                if competing {
                    return Err(Error::Conflict);
                }
                // Earlier retries exist (checked above); no later one may.
                for k in (n + 1)..=MAX_DEMAND_RETRIES {
                    let later: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM owner_shared_start_intents WHERE id=?1)",
                        [start_id(&proof.offer.demand_id, k)?],
                        |r| r.get(0),
                    )?;
                    if later {
                        return Err(Error::Conflict);
                    }
                }
                let budget = match successor::snapshot_for_head(&tx, e, &original, predecessor.as_ref()) {
                    Ok(b) => Some(b),
                    Err(Error::Capacity) => None,
                    Err(e) => return Err(e),
                };
                if budget.as_ref().is_none_or(|b| b.state != SharedSuccessorBudgetState::Available) {
                    let description = budget
                        .as_ref()
                        .map(SharedDemandBudget::from)
                        .unwrap_or_else(SharedDemandBudget::unavailable);
                    description.validate()?;
                    proof.check()?;
                    return Ok(SharedDemandRetry::Blocked(description));
                }
                start_intent::prepare_in_transaction(&tx, e, original.clone(), predecessor.clone(), id)?.input
            };
            let input = SharedDemandStartInput {
                demand_digest: proof.offer.digest()?,
                expected_control_revision: proof.offer.control_revision,
                start,
                retry: Some(n),
            };
            input.validate_for(&proof.offer)?;
            proof.check()?;
            tx.commit()?;
            proof.check().map_err(|_| Error::OutcomeUnknown)?;
            Ok(SharedDemandRetry::Start(SharedDemandSubmission::new(
                registry, proof, original, input,
            )))
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }

    /// The saved v2 demand that started `command` and which start that was
    /// (0 original, `n` retry `n`). Descriptive only, under cleanup authority.
    pub async fn shared_demand_for_attempt(
        &self,
        authority: ManagedCleanupAuthority,
        command: SharedAttemptCommand,
    ) -> Result<Option<(SharedDemandOffer, u64)>, Error> {
        authority.check(&self.0)?;
        command.validate(&authority.enrollment.profile.account)?;
        if authority.child != command.agent_reservation_id {
            return Err(Error::LocalAuthority);
        }
        let profile = authority.enrollment.profile.clone();
        let expires = authority.deadline();
        let registry = self.0.clone();
        let result = self.0.store.call(32 * 1024, move |db| {
            authority.check(&registry)?;
            authority::cleanup(&db.db, &authority)?;
            let Some((id, n)) = demand_for_start(&db.db, &command.start_intent_id)? else {
                return Ok(None);
            };
            let profile = &authority.enrollment.profile.digest;
            let Some(saved) = journal::load(&db.db, profile, &authority.child, &id)? else {
                return Ok(None);
            };
            let input = saved_start(&db.db, profile, &authority.child, &saved, n)?;
            SharedAttemptStartIntent {
                input,
                original: command.original_runtime_command.clone(),
            }
            .matches_command(&command)?;
            authority.check(&registry)?;
            Ok(Some((saved.offer, n)))
        }).await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
}
