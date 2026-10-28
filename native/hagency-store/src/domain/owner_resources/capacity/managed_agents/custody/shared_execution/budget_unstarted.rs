//! Permanent non-issuance evidence. This never fabricates a process identity,
//! releases a model-call hold, or permits the failed command to be issued again.
use super::*;

pub(super) fn load(
    db: &Connection,
    profile: &str,
    c: &SharedBudgetAttemptCommand,
) -> Result<Option<SharedBudgetAttemptProcessReceipt>, Error> {
    let row: Option<(String, String, String, String, u64, Option<u64>, String, String, u64)> = db.query_row(
        "SELECT f.profile,f.command_payload,f.command_digest,f.child_id,f.fence_revision,f.claimed_revision,u.payload,u.digest,u.ledger_revision FROM owner_shared_unstarted_attempts u JOIN owner_shared_successor_fences f ON f.command_id=u.command_id WHERE u.command_id=?1",
        [&c.command_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?)),
    ).optional()?;
    let Some((saved_profile, command, digest, child, fence, claimed, payload, receipt_digest, revision))=row else {return Ok(None)};
    let receipt: SharedBudgetAttemptProcessReceipt=serde_json::from_str(&payload)?;
    receipt.validate_for(c)?;
    let actual: bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_successor_attempts WHERE id=?1)",[&c.command_id],|r|r.get(0))?;
    if saved_profile!=profile || child!=c.agent_reservation_id || claimed.is_some() || actual
        || canonical::digest(&serde_json::from_str::<serde_json::Value>(&command)?)?!=digest
        || canonical::digest(&serde_json::to_value(c)?)?!=digest
        || canonical::digest(&serde_json::to_value(&receipt)?)?!=receipt_digest
        || receipt.state!="not_started" || receipt.ledger_revision!=revision || revision<=fence {
        return Err(Error::Conflict);
    }
    Ok(Some(receipt))
}

/// Highest acknowledged non-issuance head after the last actual process.
pub(super) fn head(db:&Connection,profile:&str,original:&SharedRuntimeCommand)->Result<Option<(SharedBudgetAttemptCommand,SharedBudgetAttemptProcessReceipt)>,Error>{
    let payload:Option<String>=db.query_row("SELECT f.command_payload FROM owner_shared_successor_fences f JOIN owner_shared_unstarted_attempts u ON u.command_id=f.command_id WHERE f.child_id=?1 AND f.process_attempt>coalesce((SELECT max(process_attempt) FROM owner_shared_successor_attempts WHERE child_id=?1),1) ORDER BY f.process_attempt DESC LIMIT 1",[&original.agent_reservation_id],|r|r.get(0)).optional()?;
    let Some(payload)=payload else{return Ok(None)};
    let c:SharedBudgetAttemptCommand=serde_json::from_str(&payload)?;
    c.validate(&original.reserve_command.manager_mxid)?;
    if c.agent_reservation_id!=original.agent_reservation_id || c.original_runtime_digest!=canonical::digest(&serde_json::to_value(original)?)? {return Err(Error::Conflict)}
    let r=load(db,profile,&c)?.ok_or(Error::Conflict)?;
    Ok(Some((c,r)))
}

impl OwnerResourceRegistry {
    /// Returns None if one-shot custody ever existed. Only an immutable,
    /// matching, unclaimed fence can yield a not_started receipt.
    pub async fn record_unstarted_shared_budget_attempt(
        &self, authority:ManagedCleanupAuthority, command:SharedBudgetAttemptCommand,
    )->Result<Option<SharedBudgetAttemptProcessReceipt>,Error>{
        authority.check(&self.0)?;
        command.validate(&authority.enrollment.profile.account)?;
        if authority.child!=command.agent_reservation_id{return Err(Error::LocalAuthority)}
        let profile=authority.enrollment.profile.clone();let expires=authority.deadline();let registry=self.0.clone();
        let value=self.0.store.call(128*1024,move|db|{
            authority.check(&registry)?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let original=authority::cleanup(&tx,&authority)?;
            if original.digest!=command.original_runtime_command.reserve_command_digest{return Err(Error::Conflict)}
            if let Some(saved)=load(&tx,&authority.enrollment.profile.digest,&command)?{return Ok(Some(saved))}
            let fence:Option<(String,String,Option<u64>)>=tx.query_row("SELECT profile,command_digest,claimed_revision FROM owner_shared_successor_fences WHERE command_id=?1",[&command.command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let Some((p,digest,claimed))=fence else{return Err(Error::LocalAuthority)};
            if p!=authority.enrollment.profile.digest || digest!=canonical::digest(&serde_json::to_value(&command)?)? {return Err(Error::Conflict)}
            let issued:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_successor_attempts WHERE id=?1 OR (child_id=?2 AND process_attempt=?3))",params![command.command_id,command.agent_reservation_id,command.process_attempt],|r|r.get(0))?;
            if claimed.is_some() || issued {return Ok(None)}
            // Cleanup is allowed after budget/Run changes. Validate the exact
            // retained predecessor, without manufacturing current spend authority.
            let (head,digest,_)=demand::journal::head(&tx,&p,&command.original_runtime_command)?;
            if head!=command.predecessor.command_id || digest!=command.predecessor.command_digest{return Err(Error::Conflict)}
            let revision=revision(&tx)?;
            let receipt=SharedBudgetAttemptProcessReceipt{
                version:3,observation_id:format!("sanu_{}",&canonical::digest(&json!({"command":command.command_id,"digest":canonical::digest(&serde_json::to_value(&command)?)?,"state":"not_started"}))?[..48]),
                command_id:command.command_id.clone(),command_digest:canonical::digest(&serde_json::to_value(&command)?)?,assignment_id:command.assignment_id.clone(),agent_reservation_id:command.agent_reservation_id.clone(),process_attempt:command.process_attempt,original_runtime_digest:command.original_runtime_digest.clone(),budget_revision:command.execution_budget.budget_revision,budget_digest:command.execution_budget.budget_digest.clone(),ledger_revision:revision,process_digest:None,state:"not_started".into(),policy_digest:None,
            };
            receipt.validate_for(&command)?;
            bounded_row(&tx,"owner_shared_unstarted_attempts","command_id",&command.command_id,100_000)?;
            tx.execute("INSERT INTO owner_shared_unstarted_attempts VALUES(?1,?2,?3,?4)",params![command.command_id,canonical::encode(&serde_json::to_value(&receipt)?)?,canonical::digest(&serde_json::to_value(&receipt)?)?,revision])?;
            authority.check(&registry)?;tx.commit()?;authority.check(&registry).map_err(|_|Error::OutcomeUnknown)?;
            Ok(Some(receipt))
        }).await?;
        after(&profile,&self.0,expires)?;Ok(value)
    }
}
