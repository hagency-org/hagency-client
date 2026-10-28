use super::*;
impl OwnerResourceRegistry {
    /// An older central protocol may serve unrelated manual work only when the
    /// exact child has no retained intake or Stop history. Absence of a remote
    /// capability never bypasses a locally known restriction.
    pub async fn require_manual_work_without_demands(
        &self,
        activity: VerifiedSharedActivity,
        work: SharedWorkItem,
    ) -> Result<(), Error> {
        activity.work(&work)?;
        if !Arc::ptr_eq(&self.0, activity.sink.registry()) {
            return Err(Error::LocalAuthority);
        }
        let p = activity.enrollment.profile.clone();
        let expires = activity.expires;
        self.0.store.call(16*1024,move|db| {
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Deferred)?;
            work::live(&tx,&activity)?;
            let known:bool=tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM owner_shared_demands WHERE child_id=?1) OR EXISTS(SELECT 1 FROM owner_shared_demand_controls WHERE child_id=?1) OR EXISTS(SELECT 1 FROM owner_shared_stop_intents WHERE child_id=?1)",
                [activity.sink.child()],|r|r.get(0))?;
            if known {return Err(Error::LocalAuthority)}
            activity.check()?; tx.commit()?; activity.check()
        }).await?;
        after(&p, &self.0, expires)
    }
    /// Pin the authenticated demand/work relation to the exact actual live
    /// process. This is not a model-call authority; normal hold/start/issue
    /// gates remain mandatory after the descriptive binding commits.
    pub async fn bind_shared_demand_work(
        &self,
        activity: VerifiedSharedActivity,
        binding: SharedDemandBinding,
        work: SharedWorkItem,
    ) -> Result<(), Error> {
        if !cfg!(feature = "shared-intake") {
            return Err(Error::Unqualified);
        }
        activity.work(&work)?;
        if !Arc::ptr_eq(&self.0, activity.sink.registry()) {
            return Err(Error::LocalAuthority);
        }
        let p = activity.enrollment.profile.clone();
        let expires = activity.expires;
        self.0.store.call(32*1024,move|db|{
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            work::live(&tx,&activity)?;
            let saved=journal::load(&tx,&activity.enrollment.profile.digest,activity.sink.child(),&binding.demand_id)?.ok_or(Error::LocalAuthority)?;
            binding.validate_for(&saved.offer,&work)?;
            journal::control(&tx,&activity.enrollment.profile.digest,activity.sink.child(),binding.control_revision)?;
            if journal::has_older(&tx,activity.sink.child(),&saved.offer)?{return Err(Error::LocalAuthority)}
            if binding.runtime_command_id!=activity.sink.command_id()||binding.runtime_command_digest!=activity.sink.command_digest(){return Err(Error::Conflict)}
            let budget_start=budget_demand::binding_start(&tx,&activity.enrollment.profile.digest,activity.sink.child(),&saved.offer,&binding.runtime_command_id,&binding.runtime_command_digest)?;
            if budget_start.is_none() { match saved.disposition {
                SharedDemandDisposition::Pending=>{
                    if binding.runtime_command_id!=saved.offer.predecessor_command_id||binding.runtime_command_digest!=saved.offer.predecessor_command_digest{return Err(Error::Conflict)}
                }
                SharedDemandDisposition::StartPrepared=>{
                    let input=saved.input.ok_or(Error::Conflict)?;
                    let id:String=tx.query_row("SELECT start_intent_id FROM owner_shared_successor_attempts WHERE id=?1 AND child_id=?2 AND command_digest=?3",params![binding.runtime_command_id,activity.sink.child(),binding.runtime_command_digest],|r|r.get(0))?;
                    if id!=input.start.start_intent_id{return Err(Error::Conflict)}
                }
                SharedDemandDisposition::BudgetBlocked|SharedDemandDisposition::HeadChanged=>return Err(Error::Capacity),
            }
            }
            let digest=canonical::digest(&serde_json::to_value(&binding)?)?;
            let old:Option<String>=tx.query_row("SELECT binding_digest FROM owner_shared_demand_bindings WHERE demand_id=?1",[&binding.demand_id],|r|r.get(0)).optional()?;
            if let Some(old)=old {
                if old!=digest{return Err(Error::Conflict)}
            }else{
                let rev=revision(&tx)?;
                tx.execute("INSERT INTO owner_shared_demand_bindings VALUES(?1,?2,?3,?4,?5)",params![binding.demand_id,work.id,canonical::encode(&serde_json::to_value(&binding)?)?,digest,rev])?;
            }
            activity.check()?;tx.commit()?;activity.check().map_err(|_|Error::OutcomeUnknown)
        }).await?;
        after(&p, &self.0, expires)
    }
}
/// Adds a restriction to already-promoted work. Old manual v1/v2 work remains
/// untouched; the isolated consumer must bind its demand before call admission.
pub(in super::super) fn check_work_control(
    db: &Connection,
    child: &str,
    w: &SharedWorkItem,
) -> Result<(), Error> {
    let row:Option<(String,String,String,String,String)>=db.query_row("SELECT d.profile,d.child_id,b.binding_payload,b.binding_digest,d.offer_payload FROM owner_shared_demand_bindings b JOIN owner_shared_demands d ON d.id=b.demand_id WHERE b.work_id=?1",[&w.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let Some((profile, saved_child, payload, digest, offer)) = row else {
        // A demand-created process cannot admit any event until its exact
        // promotion has been retained. Disabling intake never removes this fence.
        let demand_attempt: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM owner_shared_successor_attempts a JOIN owner_shared_demand_decisions d ON d.start_intent_id=a.start_intent_id WHERE a.child_id=?1 AND a.command_digest=?2) OR EXISTS(SELECT 1 FROM owner_shared_successor_attempts a JOIN owner_shared_budget_demand_decisions d ON d.start_intent_id=a.start_intent_id WHERE a.child_id=?1 AND a.command_digest=?2) OR EXISTS(SELECT 1 FROM owner_shared_demands WHERE child_id=?1 AND event_id=?3 AND json_extract(offer_payload,'$.assignmentId')=?4)",
            params![child, w.runtime_command_digest, w.event_id, w.assignment_id], |r| r.get(0),
        )?;
        return if demand_attempt {
            Err(Error::LocalAuthority)
        } else {
            Ok(())
        };
    };
    let binding: SharedDemandBinding = serde_json::from_str(&payload)?;
    let offer: SharedDemandOffer = serde_json::from_str(&offer)?;
    if saved_child != child || canonical::digest(&serde_json::to_value(&binding)?)? != digest {
        return Err(Error::Conflict);
    }
    binding.validate_for(&offer, w)?;
    journal::control(db, &profile, child, binding.control_revision)
}
