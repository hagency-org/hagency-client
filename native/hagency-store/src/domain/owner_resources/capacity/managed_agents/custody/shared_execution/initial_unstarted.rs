//! Exact non-issuance proof for an expired V4 initial command. Absence alone is
//! never a stopped process: the writer permanently tombstones that command in
//! the same transaction that proves no original custody was ever issued.
use super::*;

pub(super) fn require_not_void(db: &Connection, c: &SharedRuntimeCommand) -> Result<(), Error> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_shared_unstarted_runtimes WHERE command_id=?1)",
        [&c.command_id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}
pub(super) fn saved(
    db: &Connection,
    profile: &str,
    c: &SharedRuntimeCommand,
) -> Result<Option<SharedProcessReceipt>, Error> {
    let row:Option<(String,String,String,String,String,String,u64)>=db.query_row("SELECT profile,child_id,command_payload,command_digest,receipt,receipt_digest,ledger_revision FROM owner_shared_unstarted_runtimes WHERE command_id=?1",[&c.command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
    let Some((p, child, command, digest, payload, receipt_digest, revision)) = row else {
        return Ok(None);
    };
    let value: SharedProcessReceipt = serde_json::from_str(&payload)?;
    value.validate_for(c)?;
    if p != profile
        || child != c.agent_reservation_id
        || canonical::digest(&serde_json::from_str::<serde_json::Value>(&command)?)? != digest
        || canonical::digest(&serde_json::to_value(c)?)? != digest
        || canonical::digest(&serde_json::to_value(&value)?)? != receipt_digest
        || value.ledger_revision != revision
        || value.state != "not_started"
    {
        return Err(Error::Conflict);
    }
    Ok(Some(value))
}
impl OwnerResourceRegistry {
    /// Authenticated cleanup can prove only the exact never-issued command.
    /// None means existing/unknown issuance, which must retain its own custody.
    /// A successful tombstone survives renewal, restart and clock rollback.
    pub async fn record_unstarted_shared_runtime(
        &self,
        a: ManagedCleanupAuthority,
        c: SharedRuntimeCommand,
    ) -> Result<Option<SharedProcessReceipt>, Error> {
        a.check(&self.0)?;
        c.validate(&a.enrollment.profile.account)?;
        if a.child != c.agent_reservation_id
            || !periods::recurring(&c.reserve_command)
            || !c
                .reserve_command
                .pool_grant
                .as_ref()
                .is_some_and(|g| g.calendar.is_some() && a.enrollment.matches(g))
        {
            return Err(Error::LocalAuthority);
        }
        let profile = a.enrollment.profile.clone();
        let expires = a.deadline();
        let registry = self.0.clone();
        let result=self.0.store.call(64*1024,move|db|{
            a.check(&registry)?;let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            claim::original_command(&tx,&a.enrollment,&c)?;
            if let Some(value)=saved(&tx,&a.enrollment.profile.digest,&c)? {a.check(&registry)?;return Ok(Some(value));}
            if graphs::now_ms()?<c.expires_at_ms {return Err(Error::LocalAuthority);}
            let issued:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_runtime_lifetimes WHERE child_id=?1) OR EXISTS(SELECT 1 FROM owner_managed_runtime_lifetimes WHERE child_id=?1) OR EXISTS(SELECT 1 FROM owner_managed_effects WHERE child_id=?1 AND state!='pending') OR EXISTS(SELECT 1 FROM owner_managed_calls WHERE child_id=?1)",[&c.agent_reservation_id],|r|r.get(0))?;
            if issued {a.check(&registry)?;return Ok(None);}
            let digest=canonical::digest(&serde_json::to_value(&c)?)?;
            let revision=revision(&tx)?;
            let value=SharedProcessReceipt{version:1,observation_id:format!("srun_{}",&digest[..48]),command_id:c.command_id.clone(),command_digest:digest.clone(),assignment_id:c.assignment_id.clone(),agent_reservation_id:c.agent_reservation_id.clone(),ledger_revision:revision,process_digest:None,state:"not_started".into(),policy_digest:None};
            value.validate_for(&c)?;
            bounded_row(&tx,"owner_shared_unstarted_runtimes","command_id",&c.command_id,100_000)?;
            tx.execute("INSERT INTO owner_shared_unstarted_runtimes VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![c.command_id,c.agent_reservation_id,a.enrollment.profile.digest,canonical::encode(&serde_json::to_value(&c)?)?,digest,canonical::encode(&serde_json::to_value(&value)?)?,canonical::digest(&serde_json::to_value(&value)?)?,revision])?;
            a.check(&registry)?;tx.commit()?;a.check(&registry).map_err(|_|Error::OutcomeUnknown)?;Ok(Some(value))
        }).await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
}
