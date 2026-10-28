use super::*;
struct DecisionRow {
    state: String,
    start_id: Option<String>,
    input: Option<String>,
    digest: Option<String>,
    budget: Option<String>,
}

pub(in super::super) fn load(
    db: &Connection,
    profile: &str,
    child: &str,
    id: &str,
) -> Result<Option<SharedDemandIntent>, Error> {
    let row: Option<(String,String,String,String)> = db.query_row(
        "SELECT profile,child_id,offer_payload,offer_digest FROM owner_shared_demands WHERE id=?1", [id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((saved_profile, saved_child, payload, digest)) = row else {
        return Ok(None);
    };
    let offer: SharedDemandOffer = serde_json::from_str(&payload)?;
    if saved_profile != profile
        || saved_child != child
        || offer.demand_id != id
        || offer.digest()? != digest
    {
        return Err(Error::Conflict);
    }
    let row: Option<DecisionRow> = db.query_row(
        "SELECT disposition,start_intent_id,input_payload,input_digest,budget_payload FROM owner_shared_demand_decisions WHERE demand_id=?1", [id],
        |r| Ok(DecisionRow {state:r.get(0)?,start_id:r.get(1)?,input:r.get(2)?,digest:r.get(3)?,budget:r.get(4)?})).optional()?;
    let Some(DecisionRow {
        state,
        start_id,
        input,
        digest,
        budget,
    }) = row
    else {
        return Ok(Some(SharedDemandIntent {
            offer,
            disposition: SharedDemandDisposition::Pending,
            input: None,
            budget: None,
        }));
    };
    let disposition = match state.as_str() {
        "start_prepared" => SharedDemandDisposition::StartPrepared,
        "budget_blocked" => SharedDemandDisposition::BudgetBlocked,
        "head_changed" => SharedDemandDisposition::HeadChanged,
        _ => return Err(Error::Conflict),
    };
    let input = input.map(|payload| -> Result<SharedDemandStartInput,Error> {
        let value:SharedDemandStartInput=serde_json::from_str(&payload)?;
        value.validate_for(&offer)?;
        if Some(canonical::digest(&serde_json::to_value(&value)?)?)!=digest || Some(&value.start.start_intent_id)!=start_id.as_ref() {return Err(Error::Conflict)}
        let (p,c,payload,saved_digest):(String,String,String,String)=db.query_row("SELECT profile,child_id,input_payload,input_digest FROM owner_shared_start_intents WHERE id=?1",[&value.start.start_intent_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let saved:SharedAttemptStartInput=serde_json::from_str(&payload)?;
        if p!=profile||c!=child||canonical::digest(&serde_json::to_value(&saved)?)?!=saved_digest||canonical::digest(&serde_json::to_value(&value.start)?)?!=saved_digest {return Err(Error::Conflict)}
        Ok(value)
    }).transpose()?;
    let budget = budget
        .map(|payload| serde_json::from_str::<SharedDemandBudget>(&payload))
        .transpose()?;
    if let Some(budget) = &budget {
        budget.validate()?;
    }
    if (disposition == SharedDemandDisposition::StartPrepared) != input.is_some()
        || (disposition == SharedDemandDisposition::BudgetBlocked) != budget.is_some()
    {
        return Err(Error::Conflict);
    }
    Ok(Some(SharedDemandIntent {
        offer,
        disposition,
        input,
        budget,
    }))
}
pub(in super::super) fn control(
    db: &Connection,
    profile: &str,
    child: &str,
    revision: u64,
) -> Result<(), Error> {
    let row:Option<(String,u64,u64)>=db.query_row("SELECT profile,control_revision,stopped_through FROM owner_shared_demand_controls WHERE child_id=?1",[child],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    if row
        .as_ref()
        .is_none_or(|(p, r, s)| p != profile || *r != revision || *s >= revision)
    {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}
pub(in super::super) fn observe_control(
    db: &Connection,
    profile: &str,
    child: &str,
    revision: u64,
) -> Result<(), Error> {
    let row:Option<(String,u64,u64)>=db.query_row("SELECT profile,control_revision,stopped_through FROM owner_shared_demand_controls WHERE child_id=?1",[child],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    match row {
        None => {
            db.execute(
                "INSERT INTO owner_shared_demand_controls VALUES(?1,?2,?3,0)",
                params![child, profile, revision],
            )?;
        }
        Some((p, old, stopped)) => {
            if p != profile || revision < old || revision <= stopped {
                return Err(Error::LocalAuthority);
            }
            if revision > old {
                db.execute(
                    "UPDATE owner_shared_demand_controls SET control_revision=?2 WHERE child_id=?1",
                    params![child, revision],
                )?;
            }
        }
    }
    Ok(())
}
pub(in super::super) fn save_offer(
    db: &Connection,
    p: &VerifiedSharedDemand,
    child: &str,
    revision: u64,
) -> Result<(), Error> {
    let digest = p.offer.digest()?;
    let old: Option<(String, String, u64)> = db
        .query_row(
            "SELECT offer_digest,device_id,device_generation FROM owner_shared_demands WHERE id=?1",
            [&p.offer.demand_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((hash, device, generation)) = old {
        if hash != digest
            || device != p.enrollment.executor_device_id
            || generation != p.enrollment.executor_generation
        {
            return Err(Error::Conflict);
        }
        return Ok(());
    }
    let (total,pending):(u64,u64)=db.query_row("SELECT count(*),COALESCE(sum(NOT EXISTS(SELECT 1 FROM owner_shared_demand_decisions d WHERE d.demand_id=o.id) AND NOT EXISTS(SELECT 1 FROM owner_shared_demand_bindings b WHERE b.demand_id=o.id) AND NOT EXISTS(SELECT 1 FROM owner_shared_budget_demand_terminals t WHERE t.demand_id=o.id)),0) FROM owner_shared_demands o WHERE child_id=?1",[child],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if total >= 10_000 || pending >= 100 {
        return Err(Error::Capacity);
    }
    db.execute(
        "INSERT INTO owner_shared_demands VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            p.offer.demand_id,
            child,
            p.enrollment.profile.digest,
            p.enrollment.executor_device_id,
            p.enrollment.executor_generation,
            p.offer.event_id,
            canonical::encode(&serde_json::to_value(&p.offer)?)?,
            digest,
            revision
        ],
    )?;
    Ok(())
}
pub(in super::super) fn head(
    db: &Connection,
    profile: &str,
    original: &SharedRuntimeCommand,
) -> Result<(String, String, bool), Error> {
    #[cfg(feature="shared-budget-execution")]
    if let Some((c,_))=budget_unstarted::head(db,profile,original)? {
        return Ok((c.command_id.clone(),canonical::digest(&serde_json::to_value(&c)?)?,true));
    }
    let latest:Option<(String,String,String)>=db.query_row("SELECT profile,command_digest,command_payload FROM owner_shared_successor_attempts WHERE child_id=?1 ORDER BY process_attempt DESC LIMIT 1",[&original.agent_reservation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    if let Some((saved, digest, payload)) = latest {
        let value:serde_json::Value=serde_json::from_str(&payload)?;
        if saved != profile || canonical::digest(&value)? != digest {return Err(Error::Conflict)}
        // Idle v3 work keeps the same original offer/binding wire. Recognizing
        // its descriptive head does not permit a v2 start or upgrade in place.
        let (command_id,original_digest)=match value.get("version").and_then(serde_json::Value::as_u64) {
            Some(2)=>{let c:SharedAttemptCommand=serde_json::from_value(value)?;c.validate(&original.reserve_command.manager_mxid)?;(c.command_id,c.original_runtime_digest)},
            Some(3)=>{let c:SharedBudgetAttemptCommand=serde_json::from_value(value)?;c.validate(&original.reserve_command.manager_mxid)?;(c.command_id,c.original_runtime_digest)},
            _=>return Err(Error::LocalAuthority),
        };
        if original_digest!=canonical::digest(&serde_json::to_value(original)?)? {return Err(Error::Conflict)}
        let stopped:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_successor_observations WHERE attempt_id=?1 AND json_extract(payload,'$.state')='stopped')",[&command_id],|r|r.get(0))?;
        Ok((command_id, digest, stopped))
    } else {
        let (saved, digest): (String, String) = db.query_row(
            "SELECT profile,command_digest FROM owner_shared_runtime_lifetimes WHERE child_id=?1",
            [&original.agent_reservation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if saved != profile || digest != canonical::digest(&serde_json::to_value(original)?)? {
            return Err(Error::Conflict);
        }
        let stopped:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_process_observations WHERE child_id=?1 AND json_extract(payload,'$.state')='stopped')",[&original.agent_reservation_id],|r|r.get(0))?;
        Ok((original.command_id.clone(), digest, stopped))
    }
}
impl OwnerResourceRegistry {
    /// Persist the offered event and its original request in one transaction.
    /// No network/child launch occurs here. A replay returns saved descriptive
    /// custody before any new budget observation, never another generated ID.
    pub async fn prepare_shared_demand(
        &self,
        proof: VerifiedSharedDemand,
        original: SharedRuntimeCommand,
        predecessor: Option<SharedAttemptCommand>,
    ) -> Result<SharedDemandIntent, Error> {
        if !cfg!(feature = "shared-intake") {
            return Err(Error::Unqualified);
        }
        proof.check()?;
        original.validate(&proof.enrollment.profile.account)?;
        let profile = proof.enrollment.profile.clone();
        let p = profile.clone();
        let expires = proof.expires;
        let registry = self.0.clone();
        let result=self.0.store.call(64*1024,move|db|{
            proof.check()?;if registry.closed.load(Ordering::Acquire){return Err(Error::LocalAuthority)}
            let e=&proof.enrollment;let child=&original.agent_reservation_id;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let b=authority::original(&tx,e,child,false)?;
            if b.digest!=original.reserve_command_digest||proof.offer.assignment_id!=original.assignment_id
                ||original.reserve_command.executor_device_id!=e.executor_device_id
                ||original.reserve_command.executor_generation!=e.executor_generation{return Err(Error::LocalAuthority)}
            let old=load(&tx,&profile.digest,child,&proof.offer.demand_id)?;
            if let Some(old)=&old {
                if old.offer.digest()?!=proof.offer.digest()?{return Err(Error::Conflict)}
                if old.disposition!=SharedDemandDisposition::Pending {proof.check()?;return Ok(old.clone())}
            }
            observe_control(&tx,&profile.digest,child,proof.offer.control_revision)?;
            let (head_id,head_digest,stopped)=head(&tx,&profile.digest,&original)?;
            let changed=head_id!=proof.offer.predecessor_command_id||head_digest!=proof.offer.predecessor_command_digest;
            let pending_revision=||->Result<u64,Error>{
                if old.is_some(){Ok(tx.query_row("SELECT ledger_revision FROM owner_shared_demands WHERE id=?1",[&proof.offer.demand_id],|r|r.get(0))?)}else{revision(&tx)}
            };
            // Preserve FIFO within the current Run revision. A retained older
            // event may not be silently skipped by polling a different demand.
            let older=has_older(&tx,child,&proof.offer)?;
            let (disposition,input,budget,rev)=if older {
                (SharedDemandDisposition::Pending,None,None,pending_revision()?)
            }else if changed {
                (SharedDemandDisposition::HeadChanged,None,None,revision(&tx)?)
            }else if !stopped {
                (SharedDemandDisposition::Pending,None,None,pending_revision()?)
            }else{
                let supplied=predecessor.as_ref().map(|c| -> Result<String,Error> {Ok(canonical::digest(&serde_json::to_value(c)?)?)}).transpose()?.unwrap_or(canonical::digest(&serde_json::to_value(&original)?)?);
                if supplied!=head_digest{return Err(Error::Conflict)}
                let budget=match successor::snapshot_for_head(&tx,e,&original,predecessor.as_ref()) {
                    Ok(b)=>Some(b),Err(Error::Capacity)=>None,Err(e)=>return Err(e),
                };
                if budget.as_ref().is_none_or(|b|b.state!=SharedSuccessorBudgetState::Available) {
                    let description=budget.as_ref().map(SharedDemandBudget::from).unwrap_or_else(SharedDemandBudget::unavailable);
                    description.validate()?;
                    (SharedDemandDisposition::BudgetBlocked,None,Some(description),revision(&tx)?)
                }else{
                    let id=format!("sdstart_{}",&canonical::digest(&json!({"registry":registry.id,"profile":profile.digest,"demandId":proof.offer.demand_id,"demandDigest":proof.offer.digest()?}))?[..40]);
                    let competing:Option<String>=tx.query_row("SELECT id FROM owner_shared_start_intents WHERE child_id=?1 AND predecessor_digest=?2",params![child,head_digest],|r|r.get(0)).optional()?;
                    if competing.as_ref().is_some_and(|old|old!=&id){
                        (SharedDemandDisposition::Pending,None,None,pending_revision()?)
                    }else{
                        let saved=start_intent::prepare_in_transaction(&tx,e,original.clone(),predecessor.clone(),id)?;
                        let input=SharedDemandStartInput{demand_digest:proof.offer.digest()?,expected_control_revision:proof.offer.control_revision,start:saved.input};
                        input.validate_for(&proof.offer)?;let rev=input.start.budget_observation.observed_ledger_revision;
                        (SharedDemandDisposition::StartPrepared,Some(input),None,rev)
                    }
                }
            };
            save_offer(&tx,&proof,child,rev)?;
            if disposition!=SharedDemandDisposition::Pending {
                let state=match disposition {SharedDemandDisposition::StartPrepared=>"start_prepared",SharedDemandDisposition::BudgetBlocked=>"budget_blocked",SharedDemandDisposition::HeadChanged=>"head_changed",SharedDemandDisposition::Pending=>unreachable!()};
                tx.execute("INSERT INTO owner_shared_demand_decisions VALUES(?1,?2,?3,?4,?5,?6,?7)",params![proof.offer.demand_id,state,input.as_ref().map(|i|i.start.start_intent_id.clone()),input.as_ref().map(|i| -> Result<String,Error> {Ok(canonical::encode(&serde_json::to_value(i)?)?)}).transpose()?,input.as_ref().map(|i| -> Result<String,Error> {Ok(canonical::digest(&serde_json::to_value(i)?)?)}).transpose()?,budget.as_ref().map(serde_json::to_string).transpose()?,rev])?;
            }
            proof.check()?;tx.commit()?;proof.check().map_err(|_|Error::OutcomeUnknown)?;
            Ok(SharedDemandIntent{offer:proof.offer,disposition,input,budget})
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
    pub async fn recover_shared_demand(
        &self,
        authority: ManagedCleanupAuthority,
        id: String,
    ) -> Result<Option<SharedDemandIntent>, Error> {
        authority.check(&self.0)?;
        identifier(&id, 128)?;
        let profile = authority.enrollment.profile.clone();
        let p = profile.clone();
        let expires = authority.deadline();
        let registry = self.0.clone();
        let result = self
            .0
            .store
            .call(32 * 1024, move |db| {
                authority.check(&registry)?;
                authority::cleanup(&db.db, &authority)?;
                let value = load(&db.db, &profile.digest, &authority.child, &id)?;
                authority.check(&registry)?;
                Ok(value)
            })
            .await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
    /// Restriction only. A later fresh authenticated Run revision is needed to
    /// admit a newer offer; nothing here clears old fences or process history.
    pub async fn stop_shared_demand_revision(
        &self,
        authority: ManagedCleanupAuthority,
        revision_number: u64,
    ) -> Result<(), Error> {
        authority.check(&self.0)?;
        exact(revision_number)?;
        let p = authority.enrollment.profile.clone();
        let expires = authority.deadline();
        let registry = self.0.clone();
        self.0.store.call(4096,move|db|{
            authority.check(&registry)?;let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;authority::cleanup(&tx,&authority)?;
            tx.execute("INSERT INTO owner_shared_demand_controls VALUES(?1,?2,?3,?3) ON CONFLICT(child_id) DO UPDATE SET control_revision=max(control_revision,excluded.control_revision),stopped_through=max(stopped_through,excluded.stopped_through) WHERE profile=excluded.profile",params![authority.child,authority.enrollment.profile.digest,revision_number])?;
            let (saved_profile,stopped):(String,u64)=tx.query_row("SELECT profile,stopped_through FROM owner_shared_demand_controls WHERE child_id=?1",[&authority.child],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if saved_profile!=authority.enrollment.profile.digest||stopped<revision_number{return Err(Error::LocalAuthority)}
            revision(&tx)?;authority.check(&registry)?;tx.commit()?;authority.check(&registry).map_err(|_|Error::OutcomeUnknown)
        }).await?;
        after(&p, &self.0, expires)
    }
}

pub(in super::super) fn has_older(
    db: &Connection,
    child: &str,
    offer: &SharedDemandOffer,
) -> Result<bool, Error> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_shared_demands o WHERE o.child_id=?1 AND o.id!=?2 AND json_extract(o.offer_payload,'$.controlRevision')=?3 AND NOT EXISTS(SELECT 1 FROM owner_shared_demand_bindings b WHERE b.demand_id=o.id) AND NOT EXISTS(SELECT 1 FROM owner_shared_budget_demand_terminals t WHERE t.demand_id=o.id) AND (json_extract(o.offer_payload,'$.receivedAtMs')<?4 OR (json_extract(o.offer_payload,'$.receivedAtMs')=?4 AND o.id<?2)))",
        params![child,offer.demand_id,offer.control_revision,offer.received_at_ms],|r|r.get(0),
    )?)
}
