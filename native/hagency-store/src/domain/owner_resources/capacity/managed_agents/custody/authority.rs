use super::*;

pub(super) struct Binding {
    pub command: ManagedAgentReserveCommand,
    pub digest: String,
    pub provider: String,
    pub allocated: u64,
}
pub(super) fn original(
    db: &Connection,
    e: &VerifiedManagementEnrollment,
    child: &str,
    cleanup: bool,
) -> Result<Binding, Error> {
    let row:Option<(String,String,String,u64,bool)>=db.query_row("SELECT profile,original_command,original_command_digest,allocated_tokens,permanent_fence FROM owner_managed_agents WHERE id=?1",[child],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let (profile, command, digest, allocated, fenced) = row.ok_or(Error::LocalAuthority)?;
    let c: ManagedAgentReserveCommand = serde_json::from_str(&command)?;
    let original;
    let g = if let Some(grant) = &c.pool_grant {
        c.validate(&e.profile.account)?;
        let stored: Option<String> = db.query_row("SELECT pool_grant_id FROM owner_managed_agents WHERE id=?1", [child], |r| r.get(0))?;
        if stored.as_deref() != Some(&grant.grant_id) { return Err(Error::LocalAuthority); }
        grant
    } else {
    let project: String = db.query_row(
        "SELECT reserve_command FROM owner_project_authorities WHERE allocation_id=?1",
        [&c.allocation_id],
        |r| r.get(0),
    )?;
    original = serde_json::from_str::<AllocationCommand>(&project)?;
    &original.grant_receipt
    };
    if profile != e.profile.digest
        || canonical::digest(&serde_json::to_value(&c)?)? != digest
        || !e.matches_cleanup(g)
        || e.executor_generation < g.executor_generation
    {
        return Err(Error::LocalAuthority);
    }
    let (stored, active, parent_mode): (String, bool, String) = db.query_row(
        "SELECT receipt,active,accounting_mode FROM owner_capacity_grants WHERE id=?1",
        [&g.grant_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if canonical::digest(&serde_json::from_str::<serde_json::Value>(&stored)?)?
        != c.grant_receipt_digest
    {
        return Err(Error::Conflict);
    }
    let resource = read_resource(db, &c.resource_id)?;
    let provider:String=db.query_row("SELECT provider_digest FROM owner_resource_accounts WHERE seat_id=?1 AND identity_kind='verified_provider'",[&resource.seat_id],|r|r.get(0))?;
    if !cleanup {
        let (state, mode): (String, String) = if c.pool_grant.is_some() {
            ("reserved".into(), MANAGED.into())
        } else { db.query_row(
            "SELECT state,accounting_mode FROM owner_project_reservations WHERE id=?1",
            [&c.allocation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )? };
        let now = graphs::now_ms()?;
        if fenced
            || !active
            || state != "reserved"
            || mode != MANAGED
            || parent_mode != MANAGED
            || !e.matches(g)
            || resource_revision(db, &c.resource_id)? != c.resource_revision
            || now < g.period_start_ms
            || now >= g.period_end_ms
            || now >= g.delegation_expires_at_ms
            || !hagency_core::qualification::qualifies(
                &resource.profile(),
                &c.definition.capability,
                None,
            )
        {
            return Err(Error::LocalAuthority);
        }
        authorize_resource(db, &e.profile, &c.resource_id)?;
    }
    Ok(Binding {
        command: c,
        digest,
        provider,
        allocated,
    })
}
pub(super) fn cleanup(db: &Connection, a: &ManagedCleanupAuthority) -> Result<Binding, Error> {
    original(db, &a.enrollment, &a.child, true)
}
pub(super) fn assignment(
    db: &Connection,
    a: &ManagedAssignmentAuthority,
    insert: bool,
) -> Result<(Binding, String), Error> {
    let b = original(db, &a.enrollment, &a.child, false)?;
    let digest = canonical::digest(
        &json!({"child":a.child,"assignmentId":a.assignment,"assignmentGeneration":a.generation,"commandDigest":b.digest,"providerDigest":b.provider}),
    )?;
    let stored:Option<(String,u64,String,String)>=db.query_row("SELECT assignment_id,assignment_generation,binding_digest,provider_digest FROM owner_managed_assignment_pins WHERE child_id=?1",[&a.child],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    if let Some((id, generation, binding, provider)) = stored {
        if id != a.assignment
            || generation != a.generation
            || binding != digest
            || provider != b.provider
        {
            return Err(Error::Conflict);
        }
    } else if insert {
        db.execute(
            "INSERT INTO owner_managed_assignment_pins VALUES(?1,?2,?3,?4,?5)",
            params![a.child, a.assignment, a.generation, digest, b.provider],
        )?;
    } else {
        return Err(Error::LocalAuthority);
    }
    Ok((b, digest))
}
pub(super) fn call_budget(db: &Connection, child: &str) -> Result<(u64, bool), Error> {
    let mut q=db.prepare("SELECT state,reserved_tokens,known_consumed_tokens,usage_complete,unrepresentable_usage FROM owner_managed_calls WHERE child_id=?1")?;
    let mut rows = q.query([child])?;
    let mut held = 0_u64;
    let mut unknown = false;
    let mut count = 0;
    while let Some(r) = rows.next()? {
        if count >= 100_000 {
            return Err(Error::Capacity);
        }
        count += 1;
        let state: String = r.get(0)?;
        let reserved: u64 = r.get(1)?;
        let consumed: u64 = r.get(2)?;
        let complete: bool = r.get(3)?;
        let overflow: bool = r.get(4)?;
        let open = matches!(state.as_str(), "prepared" | "issued" | "unknown");
        let value = if open {
            consumed.max(reserved)
        } else {
            consumed
        };
        held = held
            .checked_add(value)
            .filter(|n| *n <= JSON_SAFE_MAX)
            .ok_or(Error::Capacity)?;
        unknown |= overflow
            || matches!(state.as_str(), "issued" | "unknown")
            || (state == "settled" && !complete);
    }
    Ok((held, unknown))
}
