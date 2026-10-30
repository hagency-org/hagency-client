use super::*;

/// Constructed only by the authenticated native executor, never by script JSON.
/// The original same-device receipt remains mandatory inside the queued writer.
pub struct VerifiedAgentBudgetDelivery {
    enrollment: VerifiedManagementEnrollment,
    command: AgentBudgetCommand,
    expires: Instant,
    cancel: bool,
}
impl VerifiedAgentBudgetDelivery {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        command: AgentBudgetCommand,
        command_digest: String,
        application_allowed: bool,
        cancellation_requested: bool,
        expires: Instant,
    ) -> Result<Self, Error> {
        enrollment.check()?;
        command.validate(&enrollment.profile.account)?;
        if command_digest != digest(&command)? {
            return Err(Error::Conflict);
        }
        if expires <= Instant::now()
            || expires > Instant::now() + Duration::from_secs(30)
            || (!application_allowed && !cancellation_requested)
        {
            return Err(Error::LocalAuthority);
        }
        let expires = expires.min(enrollment.expires);
        Ok(Self {
            enrollment,
            command,
            expires,
            cancel: cancellation_requested,
        })
    }
    fn check(&self, fence: &Inner) -> Result<(), Error> {
        self.enrollment.check()?;
        if fence.closed.load(Ordering::Acquire) || Instant::now() >= self.expires {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
fn result(
    c: &AgentBudgetCommand,
    state: &str,
    revision: u64,
    reason: Option<&str>,
) -> Result<AgentBudgetReceipt, Error> {
    let applied = state == "applied";
    let value = AgentBudgetReceipt {
        version: 1,
        command_id: c.command_id.clone(),
        command_digest: digest(c)?,
        top_up_id: c.top_up_id.clone(),
        agent_reservation_id: c.child()?.into(),
        assignment_id: c.assignment_id.clone(),
        original_reserve_receipt_digest: c.reserve_receipt_digest.clone(),
        state: state.into(),
        expected_budget_revision: c.expected_budget_revision,
        previous_budget_digest: c.expected_budget_digest.clone(),
        budget_revision: c.expected_budget_revision + u64::from(applied),
        budget_digest: if applied {
            next_budget_digest(c, revision)?
        } else {
            c.expected_budget_digest.clone()
        },
        previous_ceiling_tokens: c.expected_ceiling_tokens,
        applied_delta_tokens: if applied { c.add_tokens } else { 0 },
        effective_ceiling_tokens: c.expected_ceiling_tokens
            + if applied { c.add_tokens } else { 0 },
        ledger_revision: revision,
        period_key: c.reserve_command.period_key.clone(),
        reason: reason.map(str::to_owned),
    };
    value.validate_for(c)?;
    Ok(value)
}
fn live(db: &Connection, d: &VerifiedAgentBudgetDelivery) -> Result<Binding, Error> {
    let e = &d.enrollment;
    let c = &d.command;
    let binding = authority::original(db, e, c.child()?, false)?;
    let now = graphs::now_ms()?;
    if now < c.issued_at_ms
        || now >= c.expires_at_ms
        || now >= authority::lifetime(db, &binding, now)?
        || c.expires_at_ms > authority::lifetime(db, &binding, now)?
    {
        return Err(Error::LocalAuthority);
    }
    let expected = digest(
        &json!({"child":c.child()?,"assignmentId":c.assignment_id,"assignmentGeneration":c.assignment_generation,"commandDigest":binding.digest,"providerDigest":binding.provider}),
    )?;
    let pin:Option<(String,u64,String,String)>=db.query_row("SELECT assignment_id,assignment_generation,binding_digest,provider_digest FROM owner_managed_assignment_pins WHERE child_id=?1",[c.child()?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    if pin
        != Some((
            c.assignment_id.clone(),
            1,
            expected,
            binding.provider.clone(),
        ))
    {
        return Err(Error::LocalAuthority);
    }
    let closed:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_parent_settlements s JOIN owner_project_reservations p ON p.grant_id=s.grant_id WHERE p.id=?1)",[&binding.command.allocation_id],|r|r.get(0))?;
    if closed {
        return Err(Error::LocalAuthority);
    }
    let (_, _, _, unrepresentable) = call_liability(db, c.child()?)?;
    if unrepresentable {
        return Err(Error::Capacity);
    }
    Ok(binding)
}
fn conserved(db: &Connection, b: &Binding, owner_funded: bool) -> Result<(), Error> {
    let now = graphs::now_ms()?;
    let grant_id = if let Some(grant) = &b.command.pool_grant {
        // The grant governing the child's chain now covers its hold.
        super::super::super::super::succession::effective_at(db, &grant.grant_id, now)?
    } else {
        let (project_tokens, grant_id): (u64, String) = db.query_row(
        "SELECT requested_tokens,grant_id FROM owner_project_reservations WHERE id=?1",
        [&b.command.allocation_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if accounting::children(db, &b.command.allocation_id)?.retained > project_tokens {
            return Err(Error::Capacity);
        }
        grant_id
    };
    let grant_tokens: u64 = db.query_row(
        "SELECT allocated_tokens FROM owner_capacity_grants WHERE id=?1",
        [&grant_id],
        |r| r.get(0),
    )?;
    // Includes both direct pool Agents and any legacy project reservations.
    // Only a version-2, exact-owner approval may fund this Agent beyond the
    // original published grant. It grants no reusable pool capacity. The full
    // resulting liability (including siblings/unknown calls) is still charged
    // by the account/resource collector below in the same transaction.
    if !owner_funded {
        let floor = accounting::period_floor(db, &grant_id, now)?;
        if floor.total.retained > grant_tokens {
            return Err(Error::Capacity);
        }
        // A pending amendment or scheduled renewal must still cover the chain
        // after this change.
        if b.command.pool_grant.is_some()
            && let Some(next) = super::super::super::super::succession::live_successor(db, &grant_id)?
        {
            let successor: u64 = db.query_row(
                "SELECT allocated_tokens FROM owner_capacity_grants WHERE id=?1",
                [&next.grant_id],
                |r| r.get(0),
            )?;
            let covered = if next.amendment() {
                floor.total.retained
            } else {
                accounting::accounted_floor(db, &grant_id, now)?.moving.retained
            };
            if covered > successor {
                return Err(Error::Capacity);
            }
        }
    }
    // Shared collector rejects unbounded sibling evidence while accepting
    // bounded unknown holds already conserved inside their original parents.
    let child: String = db.query_row(
        "SELECT id FROM owner_managed_agents WHERE original_command_digest=?1",
        [&b.digest],
        |r| r.get(0),
    )?;
    shared_execution::budget_account_capacity(db, &child, b)?;
    Ok(())
}
impl OwnerResourceRegistry {
    pub async fn apply_managed_budget(
        &self,
        d: VerifiedAgentBudgetDelivery,
    ) -> Result<AgentBudgetReceipt, Error> {
        d.check(&self.0)?;
        let profile = d.enrollment.profile.clone();
        let expires = d.expires;
        let fence = self.0.clone();
        let r = self
            .0
            .store
            .call(64 * 1024, move |db| {
                d.check(&fence)?;
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                let c = &d.command;
                let e = &d.enrollment;
                // Exact original receipt/profile/device first; cleanup may be later generation.
                let binding = authority::original(&tx, e, c.child()?, true)?;
                if binding.digest != c.reserve_command_digest {
                    return Err(Error::Conflict);
                }
                let original: ManagedAgentReservationReceipt = receipt(
                    &tx,
                    &e.profile.digest,
                    &e.executor_device_id,
                    &c.reserve_command.command_id,
                    &c.reserve_command_digest,
                )?
                .ok_or(Error::OutcomeUnknown)?;
                if digest(&original)? != c.reserve_receipt_digest {
                    return Err(Error::Conflict);
                }
                if let Some(old) = journal::saved(&tx, e, c)? {
                    d.check(&fence)?;
                    return Ok(old);
                }
                let other: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM owner_managed_budget_commands WHERE top_up_id=?1)",
                    [&c.top_up_id],
                    |r| r.get(0),
                )?;
                if other {
                    return Err(Error::Conflict);
                }
                let mut reason = None;
                if !d.cancel {
                    let h = journal::head(&tx, c.child()?)?;
                    if h.revision != c.expected_budget_revision
                        || h.digest != c.expected_budget_digest
                        || h.ceiling != c.expected_ceiling_tokens
                    {
                        reason = Some("budget_changed");
                    } else if h.revision >= 1025 {
                        reason = Some("budget_history_full");
                    } else {
                        match live(&tx, &d) {
                            Ok(_) => {}
                            Err(Error::LocalAuthority) => reason = Some("authority_changed"),
                            Err(Error::Capacity) => reason = Some("usage_unrepresentable"),
                            Err(e) => return Err(e),
                        }
                    }
                }
                let value = if d.cancel {
                    let r = result(
                        c,
                        "cancelled_before_apply",
                        revision(&tx)?,
                        Some("cancelled"),
                    )?;
                    journal::save_disposition(&tx, e, c, &r)?;
                    r
                } else if let Some(reason) = reason {
                    let r = result(c, "refused", revision(&tx)?, Some(reason))?;
                    journal::save_disposition(&tx, e, c, &r)?;
                    r
                } else {
                    tx.execute_batch("SAVEPOINT proposed_budget")?;
                    let candidate = result(c, "applied", revision(&tx)?, None)?;
                    journal::save_disposition(&tx, e, c, &candidate)?;
                    match conserved(&tx, &binding, c.owner_funded) {
                        Ok(()) => {
                            tx.execute_batch("RELEASE proposed_budget")?;
                            candidate
                        }
                        Err(Error::Capacity) => {
                            tx.execute_batch(
                                "ROLLBACK TO proposed_budget; RELEASE proposed_budget",
                            )?;
                            let r = result(
                                c,
                                "refused",
                                revision(&tx)?,
                                Some("insufficient_capacity"),
                            )?;
                            journal::save_disposition(&tx, e, c, &r)?;
                            r
                        }
                        Err(e) => return Err(e),
                    }
                };
                d.check(&fence)?;
                tx.commit()?;
                d.check(&fence).map_err(|_| Error::OutcomeUnknown)?;
                Ok(value)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(r)
    }
}
