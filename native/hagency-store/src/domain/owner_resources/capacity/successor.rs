//! Owner-confirmed v3 successor grants: an amendment of this month's chain
//! head or a renewal for the next UTC month. A successor is its own grant row
//! and immutable receipt; the predecessor's receipt is never edited.
use super::*;
use succession::State;

pub(super) struct Plan {
    kind: String,
    predecessor: NativeGrantReceipt,
    predecessor_digest: String,
}

/// Public descriptive state of one successor grant. Not authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityGrantSuccession {
    pub grant_id: String,
    pub predecessor_grant_id: String,
    pub kind: String,
    /// `pending`, `effective`, `rejected` or `cancelled`.
    pub state: String,
    /// 0 while pending.
    pub effective_from_ms: u64,
}
impl CapacityGrantSuccession {
    fn of(row: succession::Row) -> Self {
        Self {
            grant_id: row.grant_id,
            predecessor_grant_id: row.predecessor_grant_id,
            kind: row.kind,
            state: row.state.as_str().into(),
            effective_from_ms: row.effective_from_ms,
        }
    }
}

/// Read-only facts for a native successor form. Opening the form creates
/// nothing; the writer rechecks every value in its own transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityGrantSuccessionFacts {
    pub kind: String,
    pub predecessor_grant_id: String,
    pub period_key: String,
    pub period_start_ms: u64,
    pub period_end_ms: u64,
    /// When this kind may be confirmed, by the native UTC clock.
    pub window_opens_at_ms: u64,
    pub window_closes_at_ms: u64,
    /// Live Agents that keep running under the successor.
    pub carried_children: u64,
    /// The hold the successor must cover: for an amendment every hold of
    /// this month's chain, for a renewal the live carried Agents' holds.
    pub floor_tokens: u64,
    /// The largest successor amount that fits; `None` when unknown.
    pub max_tokens: Option<u64>,
}

/// What governs a chain now: its effective grant and that grant's single
/// live successor, if any. Descriptive only.
#[derive(Clone, Debug)]
pub struct CapacityGrantChainHead {
    pub effective: NativeGrantReceipt,
    /// Active and not canonically closed natively.
    pub open: bool,
    pub successor: Option<CapacityGrantSuccession>,
}

fn kind(value: &str) -> Result<&'static str, Error> {
    match value {
        SUCCESSION_AMENDMENT => Ok(SUCCESSION_AMENDMENT),
        SUCCESSION_RENEWAL => Ok(SUCCESSION_RENEWAL),
        _ => Err(Error::LocalAuthority),
    }
}
fn checked_add(a: u64, b: u64) -> Result<u64, Error> {
    a.checked_add(b)
        .filter(|v| *v <= JSON_SAFE_MAX)
        .ok_or(Error::Capacity)
}
/// The 1a rule for a grant whose month has ended.
fn ended(value: &managed_agents::Liability) -> u64 {
    if value.complete {
        value.unresolved
    } else {
        value.retained
    }
}

/// The successor's period and the window it may be confirmed in.
fn target(kind: &str, predecessor: &NativeGrantReceipt) -> Result<(OwnerGrantPeriod, u64, u64), Error> {
    let own = OwnerGrantPeriod::of_receipt(predecessor)?;
    Ok(if kind == SUCCESSION_RENEWAL {
        let end = own.period_end_ms;
        (own.next()?, end.saturating_sub(RENEWAL_WINDOW_MS), end)
    } else {
        let (start, end) = (own.period_start_ms, own.period_end_ms);
        (own, start, end)
    })
}

/// The floor the successor must cover and how the chain's commitment at its
/// evaluation time changes (section 3.5 with the same-period floor): an
/// amendment heads this month with `max(A_s, floor)` and its predecessors
/// count nothing; a renewal adds `max(A_s, carried)` to next month while the
/// fenced remainder keeps its liability in the ended month.
struct Transition {
    floor: u64,
    carried_children: u64,
    /// The chain's commitment at `at` today.
    current: u64,
    /// The part of the commitment at `at` that does not depend on `A_s`.
    after_without_successor: u64,
    at: u64,
}
fn transition(
    db: &Connection,
    kind: &str,
    predecessor: &NativeGrantReceipt,
    predecessor_tokens: u64,
    period: &OwnerGrantPeriod,
    now: u64,
) -> Result<Transition, Error> {
    let id = &predecessor.grant_id;
    if kind == SUCCESSION_AMENDMENT {
        let floor = managed_agents::period_floor(db, id, now)?;
        // The successor heads this month with max(A_s, floor); the grants it
        // supersedes count nothing once it is effective.
        Ok(Transition {
            floor: floor.total.retained,
            carried_children: floor.moving_children,
            current: predecessor_tokens.max(floor.total.retained),
            after_without_successor: 0,
            at: now,
        })
    } else {
        // Next month: only live Agents move; fenced ones stay in the ended
        // month and keep its unresolved liability there.
        let at = period.period_start_ms;
        let before = managed_agents::accounted_floor(db, id, at)?;
        let live = managed_agents::accounted_floor(db, id, now)?;
        Ok(Transition {
            floor: live.moving.retained,
            carried_children: live.moving_children,
            current: ended(&before.total),
            after_without_successor: ended(&before.staying),
            at,
        })
    }
}

impl OwnerResourceRegistry {
    /// Native-only. Prepare an owner-confirmed successor of `predecessor`, the
    /// pool's current chain head. Every check is repeated by the writer.
    pub fn prepare_capacity_grant_successor(
        &self,
        enrollment: VerifiedManagementEnrollment,
        predecessor: NativeGrantReceipt,
        kind: &str,
        selection: OwnerGrantSelection,
        expires: Instant,
    ) -> Result<OwnerGrantConsent, Error> {
        self.prepare_capacity_grant_successor_at(
            enrollment,
            predecessor,
            kind,
            selection,
            expires,
            graphs::now_ms()?,
        )
    }

    pub(super) fn prepare_capacity_grant_successor_at(
        &self,
        enrollment: VerifiedManagementEnrollment,
        predecessor: NativeGrantReceipt,
        kind: &str,
        selection: OwnerGrantSelection,
        expires: Instant,
        now: u64,
    ) -> Result<OwnerGrantConsent, Error> {
        let kind = self::kind(kind)?;
        if !matches!(predecessor.version, 2 | 3)
            || !predecessor.supported_shape()
            || predecessor.capability != CAPABILITY
            || !enrollment.matches(&predecessor)
            || selection.coordinator_mxid != predecessor.coordinator_mxid
            || selection.allow_self_approval != predecessor.allow_self_approval
            || selection.grant_id == predecessor.grant_id
            || predecessor
                .eligible_managers
                .iter()
                .any(|m| selection.eligible_managers.binary_search(m).is_err())
        {
            return Err(Error::LocalAuthority);
        }
        let (period, opens, closes) = target(kind, &predecessor)?;
        if now < opens || now >= closes {
            return Err(Error::LocalAuthority);
        }
        let predecessor_digest = canonical::digest(&serde_json::to_value(&predecessor)?)?;
        let mut consent = self.prepare_capacity_grant_at(enrollment, selection, expires, now)?;
        consent.period = period;
        consent.succession = Some(Plan {
            kind: kind.into(),
            predecessor,
            predecessor_digest,
        });
        consent.check()?;
        Ok(consent)
    }

    /// The pool's governing grant now for the chain containing `grant_id`,
    /// with its live successor. Descriptive only; never authority.
    pub async fn capacity_grant_chain_head(
        &self,
        profile: ProfileAuthority,
        executor_device_id: String,
        grant_id: String,
    ) -> Result<CapacityGrantChainHead, Error> {
        identifier(&executor_device_id, 128)?;
        identifier(&grant_id, 128)?;
        self.read(profile, move |db, profile| {
            owned(db, profile, &executor_device_id, &grant_id)?;
            let now = graphs::now_ms()?;
            // Start at the chain root so a pending or void grant resolves to
            // the grant that actually governs now.
            let root = succession::lineage(db, &grant_id)?
                .pop()
                .ok_or(Error::Schema)?;
            let head = succession::effective_at(db, &root, now)?;
            let effective = owned(db, profile, &executor_device_id, &head)?;
            let (active, closed): (bool, bool) = db.query_row(
                "SELECT g.active,EXISTS(SELECT 1 FROM owner_parent_settlements WHERE grant_id=g.id) FROM owner_capacity_grants g WHERE g.id=?1",
                [&head],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok(CapacityGrantChainHead {
                effective,
                open: active && !closed,
                successor: succession::live_successor(db, &head)?.map(CapacityGrantSuccession::of),
            })
        })
        .await
    }

    /// This grant's own succession state; `None` for a chain root.
    pub async fn capacity_grant_succession(
        &self,
        profile: ProfileAuthority,
        executor_device_id: String,
        grant_id: String,
    ) -> Result<Option<CapacityGrantSuccession>, Error> {
        identifier(&executor_device_id, 128)?;
        identifier(&grant_id, 128)?;
        self.read(profile, move |db, profile| {
            owned(db, profile, &executor_device_id, &grant_id)?;
            Ok(succession::row(db, &grant_id)?.map(CapacityGrantSuccession::of))
        })
        .await
    }

    /// Facts for the native successor form of `predecessor`.
    pub async fn capacity_grant_succession_facts(
        &self,
        profile: ProfileAuthority,
        predecessor: NativeGrantReceipt,
        kind: String,
    ) -> Result<CapacityGrantSuccessionFacts, Error> {
        let kind = self::kind(&kind)?;
        identifier(&predecessor.grant_id, 128)?;
        self.read(profile, move |db, profile| {
            let stored = owned(db, profile, &predecessor.executor_device_id, &predecessor.grant_id)?;
            if canonical::digest(&serde_json::to_value(&stored)?)?
                != canonical::digest(&serde_json::to_value(&predecessor)?)?
            {
                return Err(Error::LocalAuthority);
            }
            let now = graphs::now_ms()?;
            let (period, opens, closes) = target(kind, &predecessor)?;
            let tokens: u64 = db.query_row(
                "SELECT COALESCE(p.retained_tokens,g.allocated_tokens) FROM owner_capacity_grants g LEFT JOIN owner_parent_settlements p ON p.grant_id=g.id WHERE g.id=?1",
                [&predecessor.grant_id],
                |r| r.get(0),
            )?;
            let t = transition(db, kind, &predecessor, tokens, &period, now)?;
            let resource = read_resource(db, &predecessor.resource_id)?;
            let chain = succession::chain(db, &predecessor.grant_id)?;
            let remaining = crate::domain::successor_remaining(db, &resource, t.at, &chain)?;
            // need(A) = after_without_successor + max(A, floor) - current
            //         <= remaining  <=>  A <= remaining + current - after.
            let max_tokens = remaining.map(|remaining| {
                remaining
                    .saturating_add(t.current)
                    .saturating_sub(t.after_without_successor)
                    .min(JSON_SAFE_MAX)
            });
            Ok(CapacityGrantSuccessionFacts {
                kind: kind.into(),
                predecessor_grant_id: predecessor.grant_id.clone(),
                period_key: period.period_key.clone(),
                period_start_ms: period.period_start_ms,
                period_end_ms: period.period_end_ms,
                window_opens_at_ms: opens,
                window_closes_at_ms: closes,
                carried_children: t.carried_children,
                floor_tokens: t.floor,
                max_tokens,
            })
        })
        .await
    }

    /// The server accepted this successor (an authenticated publication
    /// reply or recovery observed it). One transaction makes it effective:
    /// an amendment from now (the flip), a renewal from its period start.
    pub async fn confirm_capacity_grant_succession(
        &self,
        profile: ProfileAuthority,
        executor_device_id: String,
        grant_id: String,
    ) -> Result<CapacityGrantSuccession, Error> {
        self.decide(profile, executor_device_id, grant_id, true).await
    }

    /// A definitive, audited server rejection before any effect. The
    /// successor never held capacity and its predecessor stays the head.
    pub async fn reject_capacity_grant_succession(
        &self,
        profile: ProfileAuthority,
        executor_device_id: String,
        grant_id: String,
    ) -> Result<CapacityGrantSuccession, Error> {
        self.decide(profile, executor_device_id, grant_id, false).await
    }

    async fn decide(
        &self,
        profile: ProfileAuthority,
        executor_device_id: String,
        grant_id: String,
        accept: bool,
    ) -> Result<CapacityGrantSuccession, Error> {
        identifier(&executor_device_id, 128)?;
        identifier(&grant_id, 128)?;
        profile.check()?;
        let caller = profile.clone();
        let fence = self.0.clone();
        let result = self
            .0
            .store
            .call(8192, move |db| {
                profile.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                let receipt = owned(&tx, &profile, &executor_device_id, &grant_id)?;
                let row = succession::row(&tx, &grant_id)?.ok_or(Error::Conflict)?;
                let now = graphs::now_ms()?;
                let row = match (row.state, accept) {
                    (State::Pending, true) => {
                        confirm(&tx, &row, &receipt, now)?;
                        succession::row(&tx, &grant_id)?.ok_or(Error::Schema)?
                    }
                    (State::Pending, false) => {
                        tx.execute(
                            "UPDATE owner_capacity_grant_successions SET state='rejected',decided_at_ms=?2 WHERE grant_id=?1",
                            params![grant_id, now.max(1)],
                        )?;
                        succession::row(&tx, &grant_id)?.ok_or(Error::Schema)?
                    }
                    // Exact repeats are idempotent; nothing reverses a decision.
                    (State::Effective, true) | (State::Rejected, false) => row,
                    // A cancelled renewal was accepted earlier, then withdrawn.
                    (State::Cancelled, true) => row,
                    _ => return Err(Error::Conflict),
                };
                profile.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                tx.commit()?;
                Ok(CapacityGrantSuccession::of(row))
            })
            .await?;
        caller.check()?;
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::OutcomeUnknown);
        }
        Ok(result)
    }

    async fn read<T: Send + 'static>(
        &self,
        profile: ProfileAuthority,
        f: impl FnOnce(&Connection, &ProfileAuthority) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        profile.check()?;
        let caller = profile.clone();
        let fence = self.0.clone();
        let result = self
            .0
            .store
            .call(16 * 1024, move |db| {
                profile.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Deferred)?;
                let value = f(&tx, &profile)?;
                profile.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                tx.commit()?;
                Ok(value)
            })
            .await?;
        caller.check()?;
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::LocalAuthority);
        }
        Ok(result)
    }
}

/// The receipt of a grant this profile and executor device own.
fn owned(
    db: &Connection,
    profile: &ProfileAuthority,
    device: &str,
    grant: &str,
) -> Result<NativeGrantReceipt, Error> {
    let (owner, raw): (String, String) = db
        .query_row(
            "SELECT profile,receipt FROM owner_capacity_grants WHERE id=?1",
            [grant],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or(Error::NotFound)?;
    if raw.len() > 12 * 1024 {
        return Err(Error::Capacity);
    }
    let receipt: NativeGrantReceipt = serde_json::from_str(&raw)?;
    if owner != profile.digest
        || receipt.owner_mxid != profile.account
        || receipt.executor_device_id != device
        || receipt.grant_id != grant
    {
        return Err(Error::LocalAuthority);
    }
    Ok(receipt)
}

/// `pending -> effective`. The successor inherits the managed accounting mode
/// of its predecessor so custody of carried children keeps working.
fn confirm(
    db: &Connection,
    row: &succession::Row,
    receipt: &NativeGrantReceipt,
    now: u64,
) -> Result<(), Error> {
    let from = if row.renewal() {
        receipt.period_start_ms
    } else {
        now.max(1)
    };
    db.execute(
        "UPDATE owner_capacity_grant_successions SET state='effective',effective_from_ms=?2,decided_at_ms=?3 WHERE grant_id=?1 AND state='pending'",
        params![row.grant_id, from, now.max(1)],
    )?;
    db.execute(
        "UPDATE owner_capacity_grants SET accounting_mode=?2 WHERE id=?1 AND (SELECT accounting_mode FROM owner_capacity_grants WHERE id=?3)=?2",
        params![row.grant_id, managed_agents::MANAGED, row.predecessor_grant_id],
    )?;
    Ok(())
}

/// A canonical closure of a successor arrived from the server, which holds
/// only successors it accepted. A renewal that never started is cancelled,
/// freeing its predecessor's slot; any other pending successor becomes
/// effective first so its children's liability belongs to this closure.
pub(super) fn settle_on_close(
    db: &Connection,
    grant: &NativeGrantReceipt,
    now: u64,
) -> Result<(), Error> {
    let Some(row) = succession::row(db, &grant.grant_id)? else {
        return Ok(());
    };
    let unstarted = row.renewal() && grant.period_start_ms > now;
    match row.state {
        State::Pending | State::Effective if unstarted => {
            db.execute(
                "UPDATE owner_capacity_grant_successions SET state='cancelled',decided_at_ms=COALESCE(decided_at_ms,?2) WHERE grant_id=?1",
                params![row.grant_id, now.max(1)],
            )?;
        }
        State::Pending => confirm(db, &row, grant, now)?,
        _ => {}
    }
    Ok(())
}

impl DomainRepository {
    pub(super) fn owner_capacity_grant_successor_with_clock(
        &mut self,
        consent: OwnerGrantConsent,
        mut now_ms: impl FnMut() -> Result<u64, Error>,
    ) -> Result<NativeGrantReceipt, Error> {
        consent.check()?;
        let e = &consent.enrollment;
        let s = &consent.selection;
        let plan = consent.succession.as_ref().ok_or(Error::LocalAuthority)?;
        let p = &plan.predecessor;
        let mut input = json!({"operation":"owner.capacity.grant.succeed","profile":e.profile.digest,"enrollment":e.enrollment_id,"contribution":e.contribution_id,"resource":e.resource_id,"resourceRevision":e.resource_revision,"device":e.executor_device_id,"generation":e.executor_generation,"selection":s,"period":consent.period,"kind":plan.kind,"predecessorGrantId":p.grant_id,"predecessorReceiptDigest":plan.predecessor_digest});
        if let Some(request) = &e.admission_request_id {
            input["admissionRequestId"] = json!(request);
        }
        let digest = canonical::digest(&input)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(old) = receipt(
            &tx,
            &e.profile.digest,
            &e.executor_device_id,
            &s.command_id,
            &digest,
        )? {
            consent.check()?;
            return Ok(old);
        }
        authorize_resource(&tx, &e.profile, &e.resource_id)?;
        if let Some(binding) = &e.verified_resource {
            binding.check_canonical(&tx, consent.registry.id())?;
        }
        if resource_revision(&tx, &e.resource_id)? != e.resource_revision {
            return Err(Error::Generation);
        }
        let resource = read_resource(&tx, &e.resource_id)?;
        let now = now_ms()?;
        let (period, opens, closes) = target(&plan.kind, p)?;
        if period != consent.period || now < opens || now >= closes {
            return Err(Error::LocalAuthority);
        }
        // The predecessor is the exact stored receipt, open, and the chain
        // head governing now: effective itself, with no live successor.
        let stored: Option<(String, String, bool, String, u64)> = tx
            .query_row(
                "SELECT g.profile,g.receipt,g.active AND NOT EXISTS(SELECT 1 FROM owner_parent_settlements WHERE grant_id=g.id),g.accounting_mode,COALESCE(p.retained_tokens,g.allocated_tokens) FROM owner_capacity_grants g LEFT JOIN owner_parent_settlements p ON p.grant_id=g.id WHERE g.id=?1",
                [&p.grant_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((owner, raw, open, mode, tokens)) = stored else {
            return Err(Error::OwnerGrantReviewChanged);
        };
        if owner != e.profile.digest
            || canonical::digest(&serde_json::from_str::<serde_json::Value>(&raw)?)?
                != plan.predecessor_digest
            || !e.matches(p)
        {
            return Err(Error::LocalAuthority);
        }
        let own = succession::row(&tx, &p.grant_id)?;
        if !open
            || !matches!(mode.as_str(), parent::RESERVATION_ONLY | managed_agents::MANAGED)
            || own
                .as_ref()
                .is_some_and(|r| r.state != State::Effective || r.effective_from_ms > now)
            || succession::live_successor(&tx, &p.grant_id)?.is_some()
            || now < p.period_start_ms
            || now >= p.period_end_ms
            || now >= p.delegation_expires_at_ms
        {
            return Err(Error::OwnerGrantReviewChanged);
        }
        if let Some(reviewed) = &consent.reviewed_budget {
            let current = super::super::budget_at(&tx, &resource, None, false, now)?;
            if &reviewed_budget_digest(&current)? != reviewed {
                return Err(Error::OwnerGrantReviewChanged);
            }
        }
        if s.delegation_expires_at_ms <= now.max(period.period_start_ms)
            || s.delegation_expires_at_ms > now.saturating_add(366 * 24 * 60 * 60 * 1000)
        {
            return Err(Error::LocalAuthority);
        }
        if serde_json::to_value(
            resource
                .ceiling
                .as_ref()
                .ok_or(Error::NoCeiling)?
                .period
                .clone(),
        )? != json!("monthly")
        {
            return Err(Error::NoCeiling);
        }
        // Floor and headroom. An amendment's hold must cover every hold of
        // this month's chain; a renewal's must cover the live Agents it carries.
        let t = transition(&tx, &plan.kind, p, tokens, &period, now)?;
        if s.allocated_tokens < t.floor.max(1) {
            return Err(Error::InsufficientCapacity);
        }
        let chain = succession::chain(&tx, &p.grant_id)?;
        let after = checked_add(t.after_without_successor, s.allocated_tokens.max(t.floor))?;
        let need = after.saturating_sub(t.current);
        let remaining = crate::domain::successor_remaining(&tx, &resource, t.at, &chain)?
            .ok_or(Error::NoCeiling)?;
        if remaining < need {
            return Err(Error::InsufficientCapacity);
        }
        let ledger_revision = revision(&tx)?;
        if ledger_revision <= p.ledger_revision {
            return Err(Error::Conflict);
        }
        let result = NativeGrantReceipt {
            version: 3,
            capability: CAPABILITY.into(),
            admission_request_id: e.admission_request_id.clone(),
            enrollment_id: e.enrollment_id.clone(),
            grant_id: s.grant_id.clone(),
            grant_revision: p.grant_revision.checked_add(1).ok_or(Error::Capacity)?,
            contribution_id: e.contribution_id.clone(),
            resource_id: e.resource_id.clone(),
            resource_revision: e.resource_revision.clone(),
            executor_device_id: e.executor_device_id.clone(),
            executor_generation: e.executor_generation,
            owner_mxid: e.profile.account.clone(),
            coordinator_mxid: s.coordinator_mxid.clone(),
            delegation_revision: 1,
            delegation_expires_at_ms: s.delegation_expires_at_ms,
            allow_self_approval: s.allow_self_approval,
            eligible_managers: s.eligible_managers.clone(),
            allocated_tokens: s.allocated_tokens,
            period_key: period.period_key.clone(),
            period_start_ms: period.period_start_ms,
            period_end_ms: period.period_end_ms,
            ledger_revision,
            succession: Some(GrantSuccession {
                kind: plan.kind.clone(),
                predecessor_grant_id: p.grant_id.clone(),
                predecessor_receipt_digest: plan.predecessor_digest.clone(),
                carried_children: t.carried_children,
                carried_retained_tokens: t.floor,
            }),
        };
        if !result.supported_shape() || serde_json::to_vec(&result)?.len() > 12 * 1024 {
            return Err(Error::Capacity);
        }
        super::provenance::offered(&tx, consent.registry.id(), &e.profile.digest, &e.resource_id, &s.command_id, &e.resource_revision, super::provenance::OfferKind::CapacityGrant)?;
        bounded_row(&tx, "owner_capacity_grants", "id", &s.grant_id, 2048)?;
        tx.execute(
            "INSERT INTO owner_capacity_grants(id,profile,resource_id,preset_id,seat_id,allocated_tokens,receipt,active,accounting_mode) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8)",
            params![
                s.grant_id,
                e.profile.digest,
                e.resource_id,
                resource.preset_id,
                resource.seat_id,
                s.allocated_tokens,
                serde_json::to_string(&result)?,
                mode
            ],
        )?;
        tx.execute(
            "INSERT INTO owner_capacity_grant_successions(grant_id,predecessor_grant_id,profile,kind,state,effective_from_ms,command_id,created_at_ms) VALUES(?1,?2,?3,?4,'pending',0,?5,?6)",
            params![s.grant_id, p.grant_id, e.profile.digest, plan.kind, s.command_id, now.max(1)],
        )?;
        save(
            &tx,
            &e.profile.digest,
            &e.executor_device_id,
            &s.command_id,
            &digest,
            &result,
        )?;
        // A clock that crossed the window or month during the transaction
        // rolls everything back.
        let later = now_ms()?;
        if later < opens || later >= closes {
            return Err(Error::LocalAuthority);
        }
        consent.check()?;
        tx.commit()?;
        consent.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
