//! Immutable per-period credit. Only a current verified owner delivery can
//! append credit; the wall clock and descriptive budget never issue authority.
use super::*;
use hagency_core::recurring_quota::{Period, RenewalMode};
#[cfg(test)]
mod tests;

pub const AGENT_PERIOD_CAPABILITY: &str = "hagency.agent_periods.v1";
fn after(profile: &ProfileAuthority, registry: &Inner, deadline: Instant) -> Result<(), Error> {
    profile.check()?;
    if registry.closed.load(Ordering::Acquire) || Instant::now() >= deadline {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentPeriodRenewCommand {
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub reserve_command: ManagedAgentReserveCommand,
    pub reserve_command_digest: String,
    pub agent_reservation_id: String,
    pub assignment_id: String,
    pub assignment_generation: u64,
    pub current_grant: NativeGrantReceipt,
    pub current_grant_digest: String,
    pub period: Period,
    pub decision_actor_mxid: String,
    pub renewal_mode: RenewalMode,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}
impl ManagedAgentPeriodRenewCommand {
    pub fn validate(&self, owner: &str) -> Result<(), Error> {
        self.reserve_command.validate(owner)?;
        for id in [
            &self.command_id,
            &self.agent_reservation_id,
            &self.assignment_id,
        ] {
            identifier(id, 128)?;
        }
        for n in [
            self.assignment_generation,
            self.issued_at_ms,
            self.expires_at_ms,
        ] {
            exact(n)?;
        }
        let original = self
            .reserve_command
            .pool_grant
            .as_ref()
            .ok_or(Error::LocalAuthority)?;
        let g = &self.current_grant;
        let calendar = original.calendar.as_ref().ok_or(Error::LocalAuthority)?;
        calendar
            .validate_period(&self.period)
            .map_err(|_| Error::LocalAuthority)?;
        if self.version != 1
            || self.capability != AGENT_PERIOD_CAPABILITY
            || self.operation != "agent.period.renew"
            || self.assignment_generation != 1
            || self.current_grant.renewal_mode.or(self.reserve_command.renewal_mode) != Some(self.renewal_mode)
            || self.decision_actor_mxid != owner
            || original.owner_mxid != owner
            || g.owner_mxid != owner
            || g.calendar.is_none()
            || !g.supported_shape()
            || g.calendar.as_ref() != Some(calendar)
            || g.resource_id != original.resource_id
            || g.resource_revision != original.resource_revision
            || g.enrollment_id != original.enrollment_id
            || g.executor_device_id != original.executor_device_id
            || g.executor_generation != original.executor_generation
            || g.period_key != self.period.key
            || g.period_start_ms != self.period.start_ms
            || g.period_end_ms != self.period.end_ms
            || self.period.start_ms < original.period_end_ms
            || self.issued_at_ms < self.period.start_ms
            || self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms > self.period.end_ms.min(g.delegation_expires_at_ms)
            || canonical::digest(&serde_json::to_value(&self.reserve_command)?)?
                != self.reserve_command_digest
            || canonical::digest(&serde_json::to_value(g)?)? != self.current_grant_digest
            || serde_json::to_vec(self)?.len() > 32 * 1024
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentPeriodReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub request_id: String,
    pub agent_id: String,
    pub agent_reservation_id: String,
    pub period: Period,
    pub allocated_tokens: u64,
    pub ledger_revision: u64,
    pub state: String,
    pub reason: Option<String>,
}
impl ManagedAgentPeriodReceipt {
    pub fn validate_for(&self, c: &ManagedAgentPeriodRenewCommand) -> Result<(), Error> {
        exact(self.ledger_revision)?;
        if self.version != 1
            || self.ledger_revision <= c.current_grant.ledger_revision
            || self.command_id != c.command_id
            || self.command_digest != canonical::digest(&serde_json::to_value(c)?)?
            || self.request_id != c.reserve_command.request_id
            || self.agent_id != c.reserve_command.proposed_agent_id
            || self.agent_reservation_id != c.agent_reservation_id
            || self.period != c.period
            || !match self.state.as_str() {
                "authorized" => {
                    self.allocated_tokens == c.reserve_command.requested_tokens
                        && self.reason.is_none()
                }
                "refused" => {
                    self.allocated_tokens == 0
                        && self
                            .reason
                            .as_ref()
                            .is_some_and(|r| !r.is_empty() && r.len() <= 128)
                }
                _ => false,
            }
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}

/// Constructible only with a current native enrollment and authenticated lease.
pub struct VerifiedManagedAgentPeriodDelivery {
    enrollment: VerifiedManagementEnrollment,
    command: ManagedAgentPeriodRenewCommand,
    digest: String,
    expires: Instant,
}
impl VerifiedManagedAgentPeriodDelivery {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        command: ManagedAgentPeriodRenewCommand,
        command_digest: String,
        application_allowed: bool,
        lease_expires: Instant,
    ) -> Result<Self, Error> {
        enrollment.check()?;
        command.validate(&enrollment.profile.account)?;
        if !application_allowed
            || lease_expires <= Instant::now()
            || lease_expires > Instant::now() + Duration::from_secs(60)
            || canonical::digest(&serde_json::to_value(&command)?)? != command_digest
        {
            return Err(Error::LocalAuthority);
        }
        Ok(Self {
            expires: lease_expires.min(enrollment.expires),
            enrollment,
            command,
            digest: command_digest,
        })
    }
    fn check(&self, r: &Inner) -> Result<(), Error> {
        self.enrollment.check()?;
        if r.closed.load(Ordering::Acquire) || Instant::now() >= self.expires {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}

pub(super) fn recurring(c: &ManagedAgentReserveCommand) -> bool {
    c.renewal_mode.is_some()
}
pub(super) fn initial(
    db: &Connection,
    child: &str,
    c: &ManagedAgentReserveCommand,
    digest: &str,
    rev: u64,
) -> Result<(), Error> {
    if !recurring(c) {
        return Ok(());
    }
    let g = c.pool_grant.as_ref().ok_or(Error::LocalAuthority)?;
    insert(db, child, &c.command_id, digest, g, c.requested_tokens, rev)
}
fn insert(
    db: &Connection,
    child: &str,
    command: &str,
    digest: &str,
    g: &NativeGrantReceipt,
    tokens: u64,
    rev: u64,
) -> Result<(), Error> {
    let id = format!(
        "map_{}",
        &canonical::digest(&json!({"child":child,"periodKey":g.period_key}))?[..40]
    );
    db.execute("INSERT INTO owner_managed_periods(id,child_id,period_key,start_ms,end_ms,grant_id,tokens,command_id,command_digest,ledger_revision,profile,agent_id) SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,profile,agent_id FROM owner_managed_agents WHERE id=?2",
        params![id,child,g.period_key,g.period_start_ms,g.period_end_ms,g.grant_id,tokens,command,digest,rev])?;
    Ok(())
}

pub(super) struct Current {
    pub id: String,
    pub period: Period,
    pub tokens: u64,
    pub digest: String,
    pub revision: u64,
}
pub(super) fn current(db: &Connection, child: &str, now: u64) -> Result<Option<Current>, Error> {
    Ok(db.query_row("SELECT id,period_key,start_ms,end_ms,tokens,command_digest,ledger_revision FROM owner_managed_periods WHERE child_id=?1 AND start_ms<=?2 AND end_ms>?2",params![child,now],|r| Ok(Current {
        id:r.get(0)?, period:Period{key:r.get(1)?,start_ms:r.get(2)?,end_ms:r.get(3)?}, tokens:r.get(4)?,digest:r.get(5)?,revision:r.get(6)?
    })).optional()?)
}
pub(super) fn enabled(db: &Connection, child: &str) -> Result<bool, Error> {
    Ok(db.query_row("SELECT json_extract(original_command,'$.renewalMode') IS NOT NULL FROM owner_managed_agents WHERE id=?1",[child],|r|r.get(0))?)
}
/// None is legacy cumulative accounting; Some selects the sole current credit.
/// An expired periodic Agent cannot silently fall back to lifetime accounting.
pub(super) fn current_id(db: &Connection, child: &str) -> Result<Option<String>, Error> {
    if !enabled(db, child)? {
        return Ok(None);
    }
    Ok(Some(
        current(db, child, graphs::now_ms()?)?
            .ok_or(Error::LocalAuthority)?
            .id,
    ))
}
pub(super) fn pin_call(db: &Connection, child: &str, call: &str) -> Result<(), Error> {
    if let Some(period) = current_id(db, child)? {
        db.execute(
            "INSERT INTO owner_managed_call_periods(call_id,period_id) VALUES(?1,?2)",
            params![call, period],
        )?;
    }
    Ok(())
}
pub(super) fn require_call_current(db: &Connection, child: &str, call: &str) -> Result<(), Error> {
    if let Some(period) = current_id(db, child)? {
        let matched: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM owner_managed_call_periods WHERE call_id=?1 AND period_id=?2)",params![call,period],|r|r.get(0))?;
        if !matched {
            return Err(Error::LocalAuthority);
        }
    }
    Ok(())
}
pub(super) fn require_received(db: &Connection, child: &str, received: u64) -> Result<(), Error> {
    if enabled(db, child)? {
        let period = current(db, child, graphs::now_ms()?)?.ok_or(Error::LocalAuthority)?;
        if received < period.period.start_ms || received >= period.period.end_ms {
            return Err(Error::LocalAuthority);
        }
    }
    Ok(())
}

/// Current spending and prior uncertainty are distinct. Past settled usage is
/// history; past unresolved calls still reserve their *full* potential charge.
pub(super) struct Usage {
    pub consumed: u64,
    pub held: u64,
    pub unsettled: u64,
    pub uncertain: bool,
    pub prior_held: u64,
    pub complete: bool,
}
pub(super) fn usage(db: &Connection, child: &str, period: Option<&str>) -> Result<Usage, Error> {
    usage_scoped(db, child, period, true)
}
fn usage_scoped(
    db: &Connection,
    child: &str,
    period: Option<&str>,
    include_prior: bool,
) -> Result<Usage, Error> {
    let mut q = db.prepare(if include_prior {
        "SELECT c.state,c.reserved_tokens,c.known_consumed_tokens,c.usage_complete,c.unrepresentable_usage,p.period_id FROM owner_managed_calls c LEFT JOIN owner_managed_call_periods p ON p.call_id=c.id WHERE c.child_id=?1 ORDER BY c.id LIMIT 100001"
    } else {
        "SELECT c.state,c.reserved_tokens,c.known_consumed_tokens,c.usage_complete,c.unrepresentable_usage,p.period_id FROM owner_managed_call_periods p JOIN owner_managed_calls c ON c.id=p.call_id WHERE c.child_id=?1 AND p.period_id=?2 ORDER BY c.id LIMIT 100001"
    })?;
    let mut rows = if include_prior {
        q.query(params![child])?
    } else {
        q.query(params![child, period])?
    };
    let mut out = Usage {
        consumed: 0,
        held: 0,
        unsettled: 0,
        uncertain: false,
        prior_held: 0,
        complete: true,
    };
    let mut count = 0;
    while let Some(r) = rows.next()? {
        count += 1;
        if count > 100_000 {
            return Err(Error::Capacity);
        }
        let state: String = r.get(0)?;
        let known: u64 = r.get(2)?;
        let complete: bool = r.get(3)?;
        let overflow: bool = r.get(4)?;
        let pin: Option<String> = r.get(5)?;
        if pin.is_none()
            || overflow
            || (matches!(state.as_str(), "prepared" | "cancelled_before_issue") && known != 0)
        {
            return Err(Error::Capacity);
        }
        let settled = state == "settled" && complete;
        let cancelled = state == "cancelled_before_issue" && known == 0;
        // Recurring Agents hold only what is known to be spent. An open call's
        // reservation is not charged, so one interrupted run can't stop the Agent.
        let held = known;
        let add = |a: u64, b: u64| {
            a.checked_add(b)
                .filter(|n| *n <= JSON_SAFE_MAX)
                .ok_or(Error::Capacity)
        };
        if pin.as_deref() == period {
            out.complete &= settled || cancelled;
            out.consumed = add(out.consumed, known)?;
            out.held = add(out.held, held)?;
            if !settled && !cancelled {
                out.unsettled = add(out.unsettled, held)?;
                out.uncertain |= state != "prepared";
            }
        } else if !settled && !cancelled {
            out.prior_held = add(out.prior_held, held)?;
        }
    }
    Ok(out)
}

/// An amendment can move an existing period's funding. A renewal cannot move
/// historical liabilities into the new period or silently credit its Agents.
pub(super) fn funding_grant(db: &Connection, grant: &str, at: u64) -> Result<String, Error> {
    let mut head = grant.to_owned();
    for _ in 0..1024 {
        match super::super::succession::live_successor(db, &head)? {
            Some(next)
                if next.amendment()
                    && next.state == super::super::succession::State::Effective
                    && next.effective_from_ms <= at =>
            {
                head = next.grant_id
            }
            _ => return Ok(head),
        }
    }
    Err(Error::Capacity)
}

pub(super) fn grant_children(
    db: &Connection,
    grant: &str,
    at: u64,
) -> Result<Vec<(String, bool)>, Error> {
    let mut out = std::collections::BTreeSet::new();
    for ancestor in super::super::succession::lineage(db, grant)? {
        let mut q=db.prepare("SELECT DISTINCT p.child_id,a.permanent_fence FROM owner_managed_periods p JOIN owner_managed_agents a ON a.id=p.child_id WHERE p.grant_id=?1 LIMIT 100001")?;
        for row in q.query_map([&ancestor], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
        })? {
            if funding_grant(db, &ancestor, at)? == grant {
                out.insert(row?);
            }
            if out.len() > 100_000 {
                return Err(Error::Capacity);
            }
        }
    }
    Ok(out.into_iter().collect())
}

pub(super) fn grant_usage(
    db: &Connection,
    child: &str,
    grant: &str,
    at: u64,
) -> Result<(u64, u64, bool), Error> {
    let fenced: bool = db.query_row(
        "SELECT permanent_fence FROM owner_managed_agents WHERE id=?1",
        [child],
        |r| r.get(0),
    )?;
    let mut q=db.prepare("SELECT id,grant_id,start_ms,end_ms,tokens FROM owner_managed_periods WHERE child_id=?1 ORDER BY start_ms LIMIT 100001")?;
    let rows = q
        .query_map([child], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
                r.get::<_, u64>(3)?,
                r.get::<_, u64>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut held = 0_u64;
    let mut consumed = 0_u64;
    let mut complete = true;
    for (id, funder, start, end, tokens) in rows {
        if funding_grant(db, &funder, at)? != grant {
            continue;
        }
        let usage = usage_scoped(db, child, Some(&id), false)?;
        let active = at >= start && at < end;
        let mut retained = if active {
            usage.held.max(if fenced { 0 } else { tokens })
        } else {
            usage.unsettled
        };
        // Unknown processes retain their originating period even without calls.
        let unknown:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_runtime_lifetimes l WHERE l.child_id=?1 AND json_extract(l.command_payload,'$.issuedAtMs')>=?2 AND json_extract(l.command_payload,'$.issuedAtMs')<?3 AND COALESCE((SELECT json_extract(o.payload,'$.state') FROM owner_shared_process_observations o WHERE o.child_id=l.child_id ORDER BY o.ledger_revision DESC LIMIT 1),'unknown') NOT IN('qualified','stopped')) OR EXISTS(SELECT 1 FROM owner_shared_successor_attempts a WHERE a.child_id=?1 AND json_extract(a.command_payload,'$.issuedAtMs')>=?2 AND json_extract(a.command_payload,'$.issuedAtMs')<?3 AND COALESCE((SELECT json_extract(o.payload,'$.state') FROM owner_shared_successor_observations o WHERE o.attempt_id=a.id ORDER BY o.ledger_revision DESC LIMIT 1),'unknown') NOT IN('qualified','stopped'))",params![child,start,end],|r|r.get(0))?;
        if unknown {
            retained = retained.max(tokens);
        }
        // Metadata custody and a provision whose outcome is still unknown
        // also retain their originating allowance. An expired calendar is no
        // substitute for an original supervisor's stopped/failed evidence.
        let original_unknown:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_managed_runtime_lifetimes l WHERE l.child_id=?1 AND l.issued_revision IS NOT NULL AND json_extract(l.command_payload,'$.issuedAtMs')>=?2 AND json_extract(l.command_payload,'$.issuedAtMs')<?3 AND NOT EXISTS(SELECT 1 FROM owner_managed_runtime_observations o WHERE o.child_id=l.child_id AND json_extract(o.payload,'$.record.state')='stopped')) OR EXISTS(SELECT 1 FROM owner_managed_effects e JOIN owner_managed_agents a ON a.id=e.child_id WHERE e.child_id=?1 AND e.state IN('issued','unknown') AND json_extract(a.original_command,'$.poolGrant.periodStartMs')=?2)",params![child,start,end],|r|r.get(0))?;
        if original_unknown {
            retained = retained.max(tokens);
        }
        held = held
            .checked_add(retained)
            .filter(|n| *n <= JSON_SAFE_MAX)
            .ok_or(Error::Capacity)?;
        consumed = consumed
            .checked_add(if active { usage.consumed } else { 0 })
            .filter(|n| *n <= JSON_SAFE_MAX)
            .ok_or(Error::Capacity)?;
        complete &= !usage.uncertain && !unknown && !original_unknown;
    }
    Ok((held, consumed, complete))
}

fn apply(
    db: &Connection,
    d: &VerifiedManagedAgentPeriodDelivery,
    now: u64,
) -> Result<ManagedAgentPeriodReceipt, Error> {
    let e = &d.enrollment;
    let c = &d.command;
    let original = &c.reserve_command;
    let g = &c.current_grant;
    if let Some(saved) = receipt(
        db,
        &e.profile.digest,
        &e.executor_device_id,
        &c.command_id,
        &d.digest,
    )? {
        return Ok(saved);
    }
    c.validate(&e.profile.account)?;
    let stored:Option<(String,String,bool,String)>=db.query_row("SELECT profile,original_command_digest,permanent_fence,state FROM owner_managed_agents WHERE id=?1",[&c.agent_reservation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let original_grant = original.pool_grant.as_ref().ok_or(Error::LocalAuthority)?;
    let pin:Option<(String,u64)>=db.query_row("SELECT assignment_id,assignment_generation FROM owner_managed_assignment_pins WHERE child_id=?1",[&c.agent_reservation_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let saved_grant: Option<(String, String, bool)> = db
        .query_row(
            "SELECT profile,receipt,active FROM owner_capacity_grants WHERE id=?1",
            [&g.grant_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let closed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_parent_settlements WHERE grant_id=?1)",
        [&g.grant_id],
        |r| r.get(0),
    )?;
    let valid = stored
        == Some((
            e.profile.digest.clone(),
            c.reserve_command_digest.clone(),
            false,
            "reserved".into(),
        ))
        && pin.as_ref().is_none_or(|(id,generation)|id==&c.assignment_id&&*generation==c.assignment_generation)
        && e.matches(g)
        && now >= c.issued_at_ms
        && now < c.expires_at_ms
        && now >= c.period.start_ms
        && now < c.period.end_ms
        && original_grant
            .calendar
            .as_ref()
            .ok_or(Error::LocalAuthority)?
            .period_at(now)
            .map_err(|_| Error::LocalAuthority)?
            == c.period
        && g.eligible_managers.contains(&original.manager_mxid)
        && !closed
        && authorize_resource(db, &e.profile, &g.resource_id).is_ok()
        && resource_revision(db, &g.resource_id)? == g.resource_revision;
    if !valid {
        return Err(Error::LocalAuthority);
    }
    let (profile, raw, active) = saved_grant.ok_or(Error::Busy)?;
    if !active
        || profile != e.profile.digest
        || canonical::digest(&serde_json::from_str::<serde_json::Value>(&raw)?)?
            != c.current_grant_digest
    {
        return Err(Error::LocalAuthority);
    }
    let head = super::super::succession::effective_at(db, &original_grant.grant_id, now)?;
    if funding_grant(db, &g.grant_id, now)? != head {
        return Err(Error::Busy);
    }
    let (head_raw, head_active): (String, bool) = db.query_row(
        "SELECT receipt,active FROM owner_capacity_grants WHERE id=?1",
        [&head],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let governing: NativeGrantReceipt = serde_json::from_str(&head_raw)?;
    let head_closed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_parent_settlements WHERE grant_id=?1)",
        [&head],
        |r| r.get(0),
    )?;
    if !head_active
        || head_closed
        || !e.matches(&governing)
        || governing.calendar != g.calendar
        || governing.period_key != g.period_key
        || !governing.eligible_managers.contains(&original.manager_mxid)
        || now >= governing.delegation_expires_at_ms
    {
        return Err(Error::LocalAuthority);
    }
    // Same-period credit never refills, including a different owner command.
    let duplicate:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_managed_periods WHERE child_id=?1 AND (period_key=?2 OR end_ms>?3))",params![c.agent_reservation_id,c.period.key,c.period.start_ms],|r|r.get(0))?;
    if duplicate {
        return Err(Error::Conflict);
    }
    let floor = accounting::period_floor(db, &head, now)?;
    if original.requested_tokens > governing.request_limit() || original.requested_tokens
        > governing
            .allocated_tokens
            .saturating_sub(floor.total.retained)
    {
        return Err(Error::Capacity);
    }
    // Capacity/transport failures above intentionally do not save a refusal:
    // retrying the exact durable command may succeed once capacity is available.
    let rev = revision(db)?;
    if rev <= g.ledger_revision {
        return Err(Error::Conflict);
    }
    insert(
        db,
        &c.agent_reservation_id,
        &c.command_id,
        &d.digest,
        g,
        original.requested_tokens,
        rev,
    )?;
    db.execute(
        "UPDATE owner_capacity_grants SET accounting_mode=?2 WHERE id=?1",
        params![g.grant_id, MANAGED],
    )?;
    db.execute(
        "UPDATE owner_capacity_grants SET accounting_mode=?2 WHERE id=?1",
        params![head, MANAGED],
    )?;
    custody::bind_period_assignment(db,e,&c.agent_reservation_id,&c.assignment_id,c.assignment_generation)?;
    let result = ManagedAgentPeriodReceipt {
        version: 1,
        command_id: c.command_id.clone(),
        command_digest: d.digest.clone(),
        request_id: original.request_id.clone(),
        agent_id: original.proposed_agent_id.clone(),
        agent_reservation_id: c.agent_reservation_id.clone(),
        period: c.period.clone(),
        allocated_tokens: original.requested_tokens,
        ledger_revision: rev,
        state: "authorized".into(),
        reason: None,
    };
    save(
        db,
        &e.profile.digest,
        &e.executor_device_id,
        &c.command_id,
        &d.digest,
        &result,
    )?;
    Ok(result)
}

impl OwnerResourceRegistry {
    pub async fn renew_managed_agent_period(
        &self,
        d: VerifiedManagedAgentPeriodDelivery,
    ) -> Result<ManagedAgentPeriodReceipt, Error> {
        d.check(&self.0)?;
        let registry = self.0.clone();
        let profile = d.enrollment.profile.clone();
        let expires = d.expires;
        let result = self
            .0
            .store
            .call(64 * 1024, move |db| {
                d.check(&registry)?;
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                let result = apply(&tx, &d, graphs::now_ms()?)?;
                d.check(&registry)?;
                tx.commit()?;
                d.check(&registry).map_err(|_| Error::OutcomeUnknown)?;
                Ok(result)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
    pub async fn recover_managed_agent_period(
        &self,
        e: VerifiedManagementEnrollment,
        c: ManagedAgentPeriodRenewCommand,
    ) -> Result<Option<ManagedAgentPeriodReceipt>, Error> {
        e.check()?;
        c.validate(&e.profile.account)?;
        let registry = self.0.clone();
        let profile = e.profile.clone();
        let expires = e.expires;
        let result = self
            .0
            .store
            .call(64 * 1024, move |db| {
                e.check()?;
                after(&e.profile, &registry, e.expires)?;
                let original = c
                    .reserve_command
                    .pool_grant
                    .as_ref()
                    .ok_or(Error::LocalAuthority)?;
                if !e.matches_cleanup(original) {
                    return Err(Error::LocalAuthority);
                }
                let value = receipt(
                    &db.db,
                    &e.profile.digest,
                    &e.executor_device_id,
                    &c.command_id,
                    &canonical::digest(&serde_json::to_value(&c)?)?,
                )?;
                e.check()?;
                Ok(value)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
}
