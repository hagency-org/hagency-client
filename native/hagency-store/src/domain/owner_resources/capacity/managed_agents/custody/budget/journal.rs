use super::*;

#[derive(Clone, Debug)]
pub(super) struct Head {
    pub revision: u64,
    pub digest: String,
    pub ceiling: u64,
    pub original: ManagedAgentReservationReceipt,
}
/// Validate the entire bounded append-only chain against the original private receipt.
/// Neither a cached ceiling nor a server projection is a source of native credit.
pub(super) fn head(db: &Connection, child: &str) -> Result<Head, Error> {
    let (profile,payload,command_digest,allocated):(String,String,String,u64)=db.query_row(
        "SELECT profile,original_command,original_command_digest,allocated_tokens FROM owner_managed_agents WHERE id=?1",[child],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let c: ManagedAgentReserveCommand = serde_json::from_str(&payload)?;
    c.validate(&c.manager_mxid)?;
    if digest(&c)? != command_digest || allocated != c.requested_tokens {
        return Err(Error::Conflict);
    }
    let original: ManagedAgentReservationReceipt = receipt(
        db,
        &profile,
        &c.executor_device_id,
        &c.command_id,
        &command_digest,
    )?
    .ok_or(Error::OutcomeUnknown)?;
    if original.version != 1
        || original.state != "reserved"
        || original.command_digest != command_digest
        || original.command_id != c.command_id
        || original.agent_reservation_id.as_deref() != Some(child)
        || original.request_id != c.request_id
        || original.agent_id != c.proposed_agent_id
        || original.allocation_id != c.allocation_id
        || original.definition_digest != c.definition_digest
        || original.period_key != c.period_key
        || original.allocated_tokens != allocated
        || original.retained_tokens != allocated
        || original.budget_revision != 1
        || original.reason.is_some()
    {
        return Err(Error::Conflict);
    }
    let original_digest = digest(&original)?;
    let mut head = Head {
        revision: 1,
        digest: initial_budget_digest(&original)?,
        ceiling: allocated,
        original,
    };
    let mut q=db.prepare("SELECT d.command_id,d.budget_revision,d.previous_digest,d.budget_digest,d.applied_tokens,d.effective_ceiling_tokens,d.ledger_revision,c.profile,c.command_digest,c.command_payload,c.receipt_payload,c.receipt_digest,c.executor_device_id,c.ledger_revision FROM owner_managed_budget_deltas d JOIN owner_managed_budget_commands c ON c.command_id=d.command_id AND c.child_id=d.child_id WHERE d.child_id=?1 ORDER BY d.budget_revision")?;
    let mut rows = q.query([child])?;
    let mut count = 0;
    let mut last_revision = head.original.ledger_revision;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > 1024 {
            return Err(Error::Capacity);
        }
        let command: AgentBudgetCommand = serde_json::from_str(&row.get::<_, String>(9)?)?;
        command.validate(&c.manager_mxid)?;
        let value: AgentBudgetReceipt = serde_json::from_str(&row.get::<_, String>(10)?)?;
        value.validate_for(&command)?;
        if row.get::<_, String>(0)? != command.command_id
            || row.get::<_, String>(7)? != profile
            || row.get::<_, String>(8)? != digest(&command)?
            || row.get::<_, String>(11)? != digest(&value)?
            || row.get::<_, String>(12)? != c.executor_device_id
            || command.reserve_command_digest != command_digest
            || command.reserve_receipt_digest != original_digest
            || command.child()? != child
            || command.expected_budget_revision != head.revision
            || command.expected_budget_digest != head.digest
            || command.expected_ceiling_tokens != head.ceiling
            || value.state != "applied"
            || row.get::<_, u64>(1)? != value.budget_revision
            || row.get::<_, String>(2)? != head.digest
            || row.get::<_, String>(3)? != value.budget_digest
            || row.get::<_, u64>(4)? != value.applied_delta_tokens
            || row.get::<_, u64>(5)? != value.effective_ceiling_tokens
            || row.get::<_, u64>(6)? != value.ledger_revision
            || row.get::<_, u64>(13)? != value.ledger_revision
            || value.ledger_revision <= last_revision
        {
            return Err(Error::Conflict);
        }
        last_revision = value.ledger_revision;
        head.revision = value.budget_revision;
        head.digest = value.budget_digest;
        head.ceiling = value.effective_ceiling_tokens;
    }
    // An applied command without its matching chain node is corruption, never zero credit.
    let orphan:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_managed_budget_commands c WHERE c.child_id=?1 AND json_extract(c.receipt_payload,'$.state')='applied' AND NOT EXISTS(SELECT 1 FROM owner_managed_budget_deltas d WHERE d.command_id=c.command_id AND d.child_id=c.child_id) UNION ALL SELECT 1 FROM owner_managed_budget_deltas d WHERE d.child_id=?1 AND NOT EXISTS(SELECT 1 FROM owner_managed_budget_commands c WHERE c.command_id=d.command_id AND c.child_id=d.child_id))",[child],|r|r.get(0))?;
    if orphan {
        return Err(Error::Conflict);
    }
    Ok(head)
}
pub(super) fn saved(
    db: &Connection,
    e: &VerifiedManagementEnrollment,
    c: &AgentBudgetCommand,
) -> Result<Option<AgentBudgetReceipt>, Error> {
    let old:Option<(String,String,String,String,String,String)>=db.query_row("SELECT profile,executor_device_id,command_digest,command_payload,receipt_payload,receipt_digest FROM owner_managed_budget_commands WHERE command_id=?1",[&c.command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    let Some((profile, device, d, payload, result, result_digest)) = old else {
        return Ok(None);
    };
    if profile != e.profile.digest || device != e.executor_device_id {
        return Err(Error::LocalAuthority);
    }
    if d != digest(c)? || canonical::encode(&serde_json::to_value(c)?)? != payload {
        return Err(Error::Conflict);
    }
    let value: AgentBudgetReceipt = serde_json::from_str(&result)?;
    value.validate_for(c)?;
    if digest(&value)? != result_digest {
        return Err(Error::Conflict);
    }
    Ok(Some(value))
}
#[cfg(feature = "agent-budget")]
pub(super) fn save_disposition(
    db: &Connection,
    e: &VerifiedManagementEnrollment,
    c: &AgentBudgetCommand,
    r: &AgentBudgetReceipt,
) -> Result<(), Error> {
    r.validate_for(c)?;
    bounded_row(
        db,
        "owner_managed_budget_commands",
        "command_id",
        &c.command_id,
        100_000,
    )?;
    db.execute(
        "INSERT INTO owner_managed_budget_commands VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            c.command_id,
            c.child()?,
            e.profile.digest,
            e.executor_device_id,
            c.top_up_id,
            digest(c)?,
            canonical::encode(&serde_json::to_value(c)?)?,
            canonical::encode(&serde_json::to_value(r)?)?,
            digest(r)?,
            r.ledger_revision
        ],
    )?;
    if r.state == "applied" {
        db.execute(
            "INSERT INTO owner_managed_budget_deltas VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                c.command_id,
                c.child()?,
                r.budget_revision,
                r.previous_budget_digest,
                r.budget_digest,
                r.applied_delta_tokens,
                r.effective_ceiling_tokens,
                r.ledger_revision
            ],
        )?;
    }
    Ok(())
}
