//! Durable v3 assessment of the original Matrix event. A budget delta may
//! reassess a blocked event, but cannot rewrite an uncertain start or Run pin.
use super::*;

#[cfg(feature="shared-budget-execution")]
pub struct VerifiedSharedBudgetDemand {
    proof: VerifiedSharedDemand,
    status: SharedBudgetDemandStatus,
}
#[cfg(feature="shared-budget-execution")]
impl VerifiedSharedBudgetDemand {
    pub fn new(proof: VerifiedSharedDemand, status: SharedBudgetDemandStatus) -> Result<Self, Error> {
        proof.check()?;
        status.validate()?;
        if proof.offer.digest()? != status.demand_digest
            || !matches!(status.state.as_str(), "offered" | "start_committed" | "expired" | "cancelled" | "head_changed")
            || status.original_block.as_ref().is_some_and(|b| b.reason != "budget_blocked")
        { return Err(Error::LocalAuthority); }
        Ok(Self { proof, status })
    }
}

/// Descriptive saved request; never a process or model-call permission.
pub struct SharedBudgetDemandIntent {
    offer: SharedDemandOffer,
    disposition: SharedDemandDisposition,
    input: Option<SharedBudgetDemandStartInput>,
    block: Option<SharedBudgetDemandBlockInput>,
}
impl SharedBudgetDemandIntent {
    pub fn offer(&self) -> &SharedDemandOffer { &self.offer }
    pub fn disposition(&self) -> &SharedDemandDisposition { &self.disposition }
    pub fn input(&self) -> Option<&SharedBudgetDemandStartInput> { self.input.as_ref() }
    pub fn block(&self) -> Option<&SharedBudgetDemandBlockInput> { self.block.as_ref() }
}

type DecisionRow = (String, String, Option<String>, String, String);
fn load(db: &Connection, profile: &str, child: &str, offer: &SharedDemandOffer, head: &str) -> Result<Option<SharedBudgetDemandIntent>, Error> {
    let saved = demand::journal::load(db, profile, child, &offer.demand_id)?;
    if saved.as_ref().is_some_and(|s| s.offer().digest().ok() != offer.digest().ok()) { return Err(Error::Conflict); }
    let row: Option<DecisionRow> = db.query_row(
        "SELECT execution_budget_payload,disposition,start_intent_id,input_payload,input_digest FROM owner_shared_budget_demand_decisions WHERE demand_id=?1 AND (budget_digest=?2 OR start_intent_id IS NOT NULL) ORDER BY start_intent_id IS NULL LIMIT 1",
        params![offer.demand_id,head], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    ).optional()?;
    let Some((budget, disposition, start_id, payload, digest)) = row else { return Ok(None); };
    if saved.is_none() { return Err(Error::Conflict); }
    let budget: SharedExecutionBudget = serde_json::from_str(&budget)?;
    budget.validate()?;
    if canonical::digest(&serde_json::from_str::<serde_json::Value>(&payload)?)? != digest { return Err(Error::Conflict); }
    let (disposition,input,block) = match disposition.as_str() {
        "start_prepared" => {
            let input: SharedBudgetDemandStartInput = serde_json::from_str(&payload)?;
            input.validate_for(offer)?;
            if Some(&input.start.start_intent_id) != start_id.as_ref() || input.start.execution_budget != budget { return Err(Error::Conflict); }
            let (p,c,payload,digest):(String,String,String,String) = db.query_row(
                "SELECT profile,child_id,input_payload,input_digest FROM owner_shared_budget_start_intents WHERE id=?1", [input.start.start_intent_id.clone()],
                |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
            )?;
            if p!=profile || c!=child || canonical::digest(&serde_json::from_str::<serde_json::Value>(&payload)?)?!=digest
                || canonical::digest(&serde_json::to_value(&input.start)?)?!=digest { return Err(Error::Conflict); }
            (SharedDemandDisposition::StartPrepared,Some(input),None)
        }
        "budget_blocked" => {
            let block: SharedBudgetDemandBlockInput = serde_json::from_str(&payload)?;
            block.validate_for(offer)?;
            if start_id.is_some() || block.execution_budget != budget { return Err(Error::Conflict); }
            (SharedDemandDisposition::BudgetBlocked,None,Some(block))
        }
        _ => return Err(Error::Conflict),
    };
    Ok(Some(SharedBudgetDemandIntent { offer:offer.clone(), disposition, input, block }))
}

/// These restrictions are also compiled when new v3 issuance is disabled.
pub(super) fn control_revision(db: &Connection, profile: &str, c: &SharedBudgetAttemptCommand) -> Result<Option<u64>,Error> {
    let id:Option<String> = db.query_row("SELECT demand_id FROM owner_shared_budget_demand_decisions WHERE start_intent_id=?1", [&c.start_intent_id], |r|r.get(0)).optional()?;
    let Some(id) = id else {
        demand::require_explicit_start(db, &c.start_intent_id)?;
        demand::require_no_owner_stop(db, profile, &c.agent_reservation_id)?;
        let blocked:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_demand_controls WHERE child_id=?1 AND (profile<>?2 OR stopped_through>0))",params![c.agent_reservation_id,profile],|r|r.get(0))?;
        if blocked { return Err(Error::LocalAuthority); }
        return Ok(None);
    };
    let offer = demand::journal::load(db,profile,&c.agent_reservation_id,&id)?.ok_or(Error::Conflict)?;
    let saved=load(db,profile,&c.agent_reservation_id,offer.offer(),&c.execution_budget.budget_digest)?.ok_or(Error::Conflict)?;
    let input=saved.input.ok_or(Error::Conflict)?.start;
    let expected=SharedBudgetAttemptStartInput {
        web_search: c.web_search, tool_policy: c.tool_policy.clone(),start_intent_id:c.start_intent_id.clone(),assignment_id:c.assignment_id.clone(),predecessor:c.predecessor.clone(),execution_budget:c.execution_budget.clone(),budget_observation:c.budget_observation.clone()};
    if canonical::digest(&serde_json::to_value(input)?)?!=canonical::digest(&serde_json::to_value(expected)?)? {return Err(Error::Conflict)}
    demand::journal::control(db,profile,&c.agent_reservation_id,saved.offer.control_revision)?;
    Ok(Some(saved.offer.control_revision))
}

/// Returns Some only for a v3 demand start. Idle offers keep the old binding rules.
pub(super) fn binding_start(db:&Connection,profile:&str,child:&str,offer:&SharedDemandOffer,command_id:&str,command_digest:&str)->Result<Option<String>,Error>{
    let row:Option<(String,String)>=db.query_row("SELECT a.command_payload,a.start_intent_id FROM owner_shared_successor_attempts a JOIN owner_shared_budget_demand_decisions d ON d.start_intent_id=a.start_intent_id WHERE a.id=?1 AND a.child_id=?2 AND a.command_digest=?3 AND d.demand_id=?4",params![command_id,child,command_digest,offer.demand_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((payload,id))=row else {return Ok(None)};
    let c:SharedBudgetAttemptCommand=serde_json::from_str(&payload)?;
    if canonical::digest(&serde_json::to_value(&c)?)?!=command_digest||control_revision(db,profile,&c)?!=Some(offer.control_revision){return Err(Error::Conflict)}
    Ok(Some(id))
}

#[cfg(feature="shared-budget-execution")]
impl OwnerResourceRegistry {
    pub async fn prepare_shared_budget_demand(&self, v:VerifiedSharedBudgetDemand, original:SharedRuntimeCommand)->Result<SharedBudgetDemandIntent,Error>{
        if !cfg!(feature="shared-intake"){return Err(Error::Unqualified)}
        if !matches!(v.status.state.as_str(),"offered"|"start_committed"){return Err(Error::LocalAuthority)}
        v.proof.check()?; original.validate(&v.proof.enrollment.profile.account)?;
        let p=v.proof.enrollment.profile.clone();let expires=v.proof.expires;let registry=self.0.clone();
        let value=self.0.store.call(128*1024,move|db|{
            let proof=v.proof;let status=v.status;
            proof.check()?;after(&proof.enrollment.profile,&registry,expires)?;
            let e=&proof.enrollment;let child=&original.agent_reservation_id;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let b=authority::original(&tx,e,child,false)?;
            if b.digest!=original.reserve_command_digest||proof.offer.assignment_id!=original.assignment_id
                ||original.reserve_command.executor_device_id!=e.executor_device_id||original.reserve_command.executor_generation!=e.executor_generation{return Err(Error::LocalAuthority)}
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_budget_demand_terminals WHERE demand_id=?1)",[&proof.offer.demand_id],|r|r.get::<_,bool>(0))? {return Err(Error::LocalAuthority)}
            let old=demand::journal::load(&tx,&e.profile.digest,child,&proof.offer.demand_id)?;
            if let Some(old)=&old {
                if old.offer().digest()?!=proof.offer.digest()? {return Err(Error::Conflict)}
                if matches!(old.disposition(),SharedDemandDisposition::StartPrepared|SharedDemandDisposition::HeadChanged){return Err(Error::LocalAuthority)}
                if let Some(b)=old.budget() {
                    if b.state!="exhausted" {return Err(Error::LocalAuthority)}
                    let expected=canonical::digest(&json!({"demandDigest":proof.offer.digest()?,"expectedControlRevision":proof.offer.control_revision,"reason":"budget_blocked","budget":b.observation}))?;
                    if status.original_block.as_ref().is_none_or(|b|b.reason!="budget_blocked"||b.payload_digest!=expected){return Err(Error::Conflict)}
                }
            }
            demand::journal::observe_control(&tx,&e.profile.digest,child,proof.offer.control_revision)?;
            if let Some(saved)=load(&tx,&e.profile.digest,child,&proof.offer,&status.execution_budget.budget_digest)?{
                proof.check()?;tx.commit()?;proof.check().map_err(|_|Error::OutcomeUnknown)?;return Ok(saved)
            }
            let (head,hash,stopped)=demand::journal::head(&tx,&e.profile.digest,&original)?;
            let (applied,_)=super::super::budget::execution::resolve(&tx,child,Some(&status.execution_budget),true)?;
            let changed=head!=proof.offer.predecessor_command_id||hash!=proof.offer.predecessor_command_digest;
            let older=demand::journal::has_older(&tx,child,&proof.offer)?;
            let rev=revision(&tx)?;
            demand::journal::save_offer(&tx,&proof,child,rev)?;
            let mut result=SharedBudgetDemandIntent{offer:proof.offer.clone(),disposition:SharedDemandDisposition::Pending,input:None,block:None};
            if changed {result.disposition=SharedDemandDisposition::HeadChanged;}
            else if stopped&&!older {
                let selected=budget_snapshot::snapshot(&tx,e,&original)?;
                if selected.execution_budget!=applied {return Err(Error::Conflict)}
                if selected.budget.state!=SharedSuccessorBudgetState::Available {
                    let description=SharedDemandBudget::from(&selected.budget);
                    let block=SharedBudgetDemandBlockInput {demand_digest:proof.offer.digest()?,expected_control_revision:proof.offer.control_revision,original_block:status.original_block,execution_budget:applied.clone(),reason:if description.state=="exhausted"{"budget_blocked"}else{"usage_unrepresentable"}.into(),budget:if description.state=="exhausted"{description.observation}else{None}};
                    block.validate_for(&proof.offer)?;
                    result.disposition=SharedDemandDisposition::BudgetBlocked;result.block=Some(block);
                }else{
                    let id=format!("sbdstart_{}",&canonical::digest(&json!({"registry":registry.id,"profile":e.profile.digest,"demandDigest":proof.offer.digest()?,"budgetDigest":applied.budget_digest}))?[..40]);
                    let competing:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_start_intents WHERE child_id=?1 AND predecessor_digest=?2) OR EXISTS(SELECT 1 FROM owner_shared_budget_start_intents WHERE child_id=?1 AND predecessor_digest=?2 AND id<>?3)",params![child,hash,id],|r|r.get(0))?;
                    if !competing {
                        let start=budget_start::prepare_for_demand(&tx,e,original.clone(),id,proof.offer.control_revision)?;
                        let input=SharedBudgetDemandStartInput{demand_digest:proof.offer.digest()?,expected_control_revision:proof.offer.control_revision,original_block:status.original_block,start:start.input};
                        input.validate_for(&proof.offer)?;
                        result.disposition=SharedDemandDisposition::StartPrepared;result.input=Some(input);
                    }
                }
                if result.input.is_some()||result.block.is_some(){
                    let (state,id,payload)=if let Some(i)=&result.input {("start_prepared",Some(i.start.start_intent_id.clone()),serde_json::to_value(i)?)}else{("budget_blocked",None,serde_json::to_value(result.block.as_ref().ok_or(Error::Conflict)?)?)};
                    let rev=revision(&tx)?;
                    tx.execute("INSERT INTO owner_shared_budget_demand_decisions VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![proof.offer.demand_id,applied.budget_digest,canonical::encode(&serde_json::to_value(&applied)?)?,state,id,canonical::encode(&payload)?,canonical::digest(&payload)?,rev])?;
                }
            }
            proof.check()?;after(&e.profile,&registry,expires)?;tx.commit()?;proof.check().map_err(|_|Error::OutcomeUnknown)?;Ok(result)
        }).await?;
        after(&p,&self.0,expires)?;Ok(value)
    }
}

#[cfg(feature="shared-budget-execution")]
pub struct SharedBudgetDemandSubmission {
    proof:VerifiedSharedBudgetDemand,
    registry:Arc<Inner>,
    input:SharedBudgetDemandStartInput,
}
#[cfg(feature="shared-budget-execution")]
impl SharedBudgetDemandSubmission {
    pub fn check(&self)->Result<(),Error>{self.proof.proof.check()?;after(&self.proof.proof.enrollment.profile,&self.registry,self.proof.proof.expires)}
    pub fn input(&self)->&SharedBudgetDemandStartInput{&self.input}
}
#[cfg(feature="shared-budget-execution")]
impl OwnerResourceRegistry {
    pub async fn shared_budget_demand_submission(&self,v:VerifiedSharedBudgetDemand,original:SharedRuntimeCommand)->Result<SharedBudgetDemandSubmission,Error>{
        if !matches!(v.status.state.as_str(),"offered"|"start_committed"){return Err(Error::LocalAuthority)}
        v.proof.check()?;original.validate(&v.proof.enrollment.profile.account)?;
        let registry=self.0.clone();
        self.0.store.call(64*1024,move|db|{
            v.proof.check()?;let e=&v.proof.enrollment;let child=&original.agent_reservation_id;
            after(&e.profile,&registry,v.proof.expires)?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Deferred)?;
            if authority::original(&tx,e,child,false)?.digest!=original.reserve_command_digest||original.reserve_command.executor_generation!=e.executor_generation||original.reserve_command.executor_device_id!=e.executor_device_id{return Err(Error::LocalAuthority)}
            demand::journal::control(&tx,&e.profile.digest,child,v.proof.offer.control_revision)?;
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_budget_demand_terminals WHERE demand_id=?1)",[&v.proof.offer.demand_id],|r|r.get::<_,bool>(0))? {return Err(Error::LocalAuthority)}
            let input=load(&tx,&e.profile.digest,child,&v.proof.offer,&v.status.execution_budget.budget_digest)?.and_then(|s|s.input).ok_or(Error::LocalAuthority)?;
            if input.original_block!=v.status.original_block||input.start.execution_budget!=v.status.execution_budget{return Err(Error::Conflict)}
            // A later head cannot silently turn an old uncertain start into a
            // new request. HTTP recovery may still return its original history.
            v.proof.check()?;tx.commit()?;
            let value=SharedBudgetDemandSubmission{proof:v,registry,input};value.check()?;Ok(value)
        }).await
    }
}

#[cfg(feature="shared-budget-execution")]
impl OwnerResourceRegistry {
    /// Persist an authenticated terminal for this exact offer. It only releases
    /// FIFO queue position; no work, model-call or capacity history is changed.
    pub async fn retain_shared_budget_demand_terminal(&self,v:VerifiedSharedBudgetDemand,original:SharedRuntimeCommand)->Result<(),Error>{
        v.proof.check()?;original.validate(&v.proof.enrollment.profile.account)?;
        if !matches!(v.status.state.as_str(),"expired"|"cancelled"|"head_changed"){return Err(Error::LocalAuthority)}
        let profile=v.proof.enrollment.profile.clone();let expires=v.proof.expires;let registry=self.0.clone();
        self.0.store.call(128*1024,move|db|{
            v.proof.check()?;after(&profile,&registry,expires)?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let e=&v.proof.enrollment;
            let b=authority::original(&tx,e,&original.agent_reservation_id,false)?;
            if b.digest!=original.reserve_command_digest || v.status.offer.assignment_id!=original.assignment_id{return Err(Error::LocalAuthority)}
            let old=demand::journal::load(&tx,&e.profile.digest,&original.agent_reservation_id,&v.status.offer.demand_id)?;
            let Some(old)=old else{return Ok(())};
            if old.offer().digest()?!=v.status.demand_digest{return Err(Error::Conflict)}
            let saved:Option<String>=tx.query_row("SELECT payload FROM owner_shared_budget_demand_terminals WHERE demand_id=?1",[&v.status.offer.demand_id],|r|r.get(0)).optional()?;
            if let Some(saved)=saved {
                let previous:SharedBudgetDemandStatus=serde_json::from_str(&saved)?;
                if previous.offer.digest()?!=v.status.offer.digest()? || previous.state!=v.status.state{return Err(Error::Conflict)}
                return Ok(())
            }
            let payload=canonical::encode(&serde_json::to_value(&v.status)?)?;let digest=canonical::digest(&serde_json::to_value(&v.status)?)?;let revision=revision(&tx)?;
            tx.execute("INSERT INTO owner_shared_budget_demand_terminals VALUES(?1,?2,?3,?4)",params![v.status.offer.demand_id,payload,digest,revision])?;
            v.proof.check()?;tx.commit()?;v.proof.check().map_err(|_|Error::OutcomeUnknown)
        }).await
    }
}
