//! Current publication eligibility; immutable receipt recovery is separate.
use super::*;
impl OwnerResourceRegistry {
    /// Read-only check before a new central publication. This is not an atomic
    /// remote lease: later local closure still fences every execution writer.
    /// Historical result recovery deliberately does not require this check.
    pub async fn require_publishable_capacity_grant(
        &self,
        enrollment: VerifiedManagementEnrollment,
        receipt: NativeGrantReceipt,
    ) -> Result<(), Error> {
        enrollment.check()?;
        if !enrollment.matches(&receipt) {
            return Err(Error::LocalAuthority);
        }
        let caller = enrollment.profile.clone();
        let deadline = enrollment.expires;
        let fence = self.0.clone();
        self.0.store.call(16*1024,move |db| {
            enrollment.check()?;
            if fence.closed.load(Ordering::Acquire) {return Err(Error::LocalAuthority);}
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let row:Option<(String,String,String,String,String,bool,String)>=tx.query_row(
                "SELECT profile,resource_id,preset_id,seat_id,receipt,active,accounting_mode FROM owner_capacity_grants WHERE id=?1",
                [&receipt.grant_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
            let Some((profile,resource_id,preset,seat,stored,active,mode))=row else{return Err(Error::Conflict)};
            if profile!=enrollment.profile.digest || resource_id!=enrollment.resource_id
                || canonical::digest(&serde_json::from_str::<serde_json::Value>(&stored)?)?!=canonical::digest(&serde_json::to_value(&receipt)?)?
            {return Err(Error::LocalAuthority);}
            let closed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_parent_settlements WHERE grant_id=?1)",[&receipt.grant_id],|r|r.get(0))?;
            let now=graphs::now_ms()?;
            // A successor is publishable while it is pending (or already
            // effective, for a retry) and its predecessor is still open. A
            // renewal is published during its window, before its month starts.
            let link=super::succession::row(&tx,&receipt.grant_id)?;
            let opens=if link.as_ref().is_some_and(|l|l.renewal()) {receipt.period_start_ms.saturating_sub(RENEWAL_WINDOW_MS)} else {receipt.period_start_ms};
            if !active || closed || !matches!(mode.as_str(),parent::RESERVATION_ONLY|managed_agents::MANAGED)
                || now<opens || now>=receipt.period_end_ms || now>=receipt.delegation_expires_at_ms
                || authorize_resource(&tx,&enrollment.profile,&resource_id).is_err()
            {return Err(Error::Conflict);}
            if let Some(link)=&link {
                let open:bool=tx.query_row("SELECT g.active AND NOT EXISTS(SELECT 1 FROM owner_parent_settlements WHERE grant_id=g.id) FROM owner_capacity_grants g WHERE g.id=?1",[&link.predecessor_grant_id],|r|r.get(0))?;
                if link.state.void() || !open {return Err(Error::Conflict);}
            }
            let resource=read_resource(&tx,&resource_id)?;
            // E20 for successors: their own chain's in-flight usage is held.
            let unsettled=if link.is_some() {
                managed_agents::unsettled_resource_except(&tx,&resource,&super::succession::chain(&tx,&receipt.grant_id)?)?
            } else {managed_agents::unsettled_resource(&tx,&resource)?};
            if resource.preset_id!=preset || resource.seat_id!=seat
                || resource_publication_revision(&resource)?!=enrollment.resource_revision
                || unsettled
            {return Err(Error::Conflict);}
            if let Some(binding)=&enrollment.verified_resource {binding.check_canonical(&tx,&fence.id)?;}
            enrollment.check()?;
            if fence.closed.load(Ordering::Acquire) {return Err(Error::LocalAuthority);}
            tx.commit()?;
            Ok(())
        }).await?;
        caller.check()?;
        if Instant::now() >= deadline || self.0.closed.load(Ordering::Acquire) {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
