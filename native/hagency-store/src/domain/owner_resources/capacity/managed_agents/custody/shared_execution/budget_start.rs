//! Persist exact v3 startup input before HTTP. Recovery never resnapshots a
//! newer budget or mints a second intent around an uncertain predecessor.
use super::*;

pub struct SharedBudgetAttemptStartIntent {
    pub(super) input: SharedBudgetAttemptStartInput,
    pub(super) original: SharedRuntimeCommand,
}
impl SharedBudgetAttemptStartIntent {
    pub fn input(&self) -> &SharedBudgetAttemptStartInput { &self.input }
    pub fn original(&self) -> &SharedRuntimeCommand { &self.original }
    pub fn matches_command(&self, c: &SharedBudgetAttemptCommand) -> Result<(), Error> {
        let expected = SharedBudgetAttemptStartInput {
        web_search: c.web_search, tool_policy: c.tool_policy.clone(),
            start_intent_id: c.start_intent_id.clone(), assignment_id: c.assignment_id.clone(),
            predecessor: c.predecessor.clone(), execution_budget: c.execution_budget.clone(),
            budget_observation: c.budget_observation.clone(),
        };
        if canonical::digest(&serde_json::to_value(&self.input)?)? != canonical::digest(&serde_json::to_value(&expected)?)?
            || c.original_runtime_digest != canonical::digest(&serde_json::to_value(&self.original)?)?
        { return Err(Error::Conflict); }
        Ok(())
    }
}

type SavedStartRow = (String, String, u64, String, String, String, String, String);

fn load(db: &Connection, e: &VerifiedManagementEnrollment, original: &SharedRuntimeCommand, id: &str) -> Result<Option<SharedBudgetAttemptStartIntent>,Error> {
    let row: Option<SavedStartRow> = db.query_row(
        "SELECT profile,device_id,device_generation,child_id,original_runtime_digest,original_runtime_payload,input_payload,input_digest FROM owner_shared_budget_start_intents WHERE id=?1",
        [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)),
    ).optional()?;
    let Some((profile,device,generation,child,original_digest,original_payload,payload,digest)) = row else { return Ok(None); };
    let input: SharedBudgetAttemptStartInput = serde_json::from_str(&payload)?;
    input.validate()?;
    let saved_original: SharedRuntimeCommand = serde_json::from_str(&original_payload)?;
    if profile != e.profile.digest || device != e.executor_device_id || generation != original.reserve_command.executor_generation
        || child != original.agent_reservation_id || original_digest != canonical::digest(&serde_json::to_value(original)?)?
        || original_digest != canonical::digest(&serde_json::to_value(&saved_original)?)?
        || input.start_intent_id != id || input.assignment_id != original.assignment_id
        || digest != canonical::digest(&serde_json::to_value(&input)?)?
    { return Err(Error::Conflict); }
    super::super::budget::execution::resolve(db, &child, Some(&input.execution_budget), false)?;
    Ok(Some(SharedBudgetAttemptStartIntent { input, original: saved_original }))
}

impl OwnerResourceRegistry {
    pub async fn prepare_shared_budget_attempt_start(
        &self, e: VerifiedManagementEnrollment, original: SharedRuntimeCommand, start_id: String,
    ) -> Result<SharedBudgetAttemptStartIntent,Error> {
        e.check()?;
        original.validate(&e.profile.account)?;
        identifier(&start_id,128)?;
        let p=e.profile.clone(); let expires=e.expires; let registry=self.0.clone();
        let value=self.0.store.call(128*1024,move |db| {
            e.check()?; after(&e.profile,&registry,e.expires)?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let b=authority::original(&tx,&e,&original.agent_reservation_id,false)?;
            if b.digest!=original.reserve_command_digest {return Err(Error::Conflict)}
            let value=prepare_in_transaction(&tx,&e,original,start_id)?;
            e.check()?;tx.commit()?;e.check().map_err(|_|Error::OutcomeUnknown)?;Ok(value)
        }).await?;
        after(&p,&self.0,expires)?;Ok(value)
    }
    pub async fn recover_shared_budget_attempt_start(
        &self, a: ManagedCleanupAuthority, original: SharedRuntimeCommand, predecessor_digest: String,
    ) -> Result<Option<SharedBudgetAttemptStartIntent>,Error> {
        a.check(&self.0)?;original.validate(&a.enrollment.profile.account)?;hash(&predecessor_digest)?;
        if a.child!=original.agent_reservation_id {return Err(Error::LocalAuthority)}
        let p=a.enrollment.profile.clone();let expires=a.deadline();let registry=self.0.clone();
        let value=self.0.store.call(64*1024,move|db| {
            a.check(&registry)?;
            if authority::cleanup(&db.db,&a)?.digest!=original.reserve_command_digest {return Err(Error::Conflict)}
            let id:Option<String>=db.db.query_row("SELECT id FROM owner_shared_budget_start_intents WHERE child_id=?1 AND predecessor_digest=?2 AND NOT EXISTS(SELECT 1 FROM owner_shared_budget_demand_decisions d WHERE d.start_intent_id=owner_shared_budget_start_intents.id)",params![a.child,predecessor_digest],|r|r.get(0)).optional()?;
            let value=id.map(|id|load(&db.db,&a.enrollment,&original,&id)).transpose()?.flatten();
            a.check(&registry)?;Ok(value)
        }).await?;
        after(&p,&self.0,expires)?;Ok(value)
    }
    pub async fn require_shared_budget_attempt_start(&self,a:ManagedCleanupAuthority,c:SharedBudgetAttemptCommand)->Result<bool,Error>{
        a.check(&self.0)?;c.validate(&a.enrollment.profile.account)?;
        if a.child!=c.agent_reservation_id {return Err(Error::LocalAuthority)}
        let p=a.enrollment.profile.clone();let expires=a.deadline();let registry=self.0.clone();
        let value=self.0.store.call(64*1024,move|db|{
            a.check(&registry)?;authority::cleanup(&db.db,&a)?;
            let value=load(&db.db,&a.enrollment,&c.original_runtime_command,&c.start_intent_id)?;
            if let Some(value)=&value{value.matches_command(&c)?;}
            a.check(&registry)?;Ok(value.is_some())
        }).await?;
        after(&p,&self.0,expires)?;Ok(value)
    }
}

pub(super) fn prepare_in_transaction(db:&Connection,e:&VerifiedManagementEnrollment,original:SharedRuntimeCommand,start_id:String)->Result<SharedBudgetAttemptStartIntent,Error>{
    demand::require_explicit_start(db,&start_id)?;
    if let Some(value)=load(db,e,&original,&start_id)? {return Ok(value)}
    demand::require_no_owner_stop(db,&e.profile.digest,&original.agent_reservation_id)?;
    prepare_new(db,e,original,start_id)
}

pub(super) fn prepare_for_demand(db:&Connection,e:&VerifiedManagementEnrollment,original:SharedRuntimeCommand,start_id:String,control_revision:u64)->Result<SharedBudgetAttemptStartIntent,Error>{
    demand::journal::control(db,&e.profile.digest,&original.agent_reservation_id,control_revision)?;
    if let Some(value)=load(db,e,&original,&start_id)? {return Ok(value)}
    prepare_new(db,e,original,start_id)
}
fn prepare_new(db:&Connection,e:&VerifiedManagementEnrollment,original:SharedRuntimeCommand,start_id:String)->Result<SharedBudgetAttemptStartIntent,Error>{
    // Advance before the snapshot, so journal insertion preserves the observed
    // global ledger revision used by the one-shot process claim.
    let rev=revision(db)?;
    let selected=budget_snapshot::snapshot(db,e,&original)?;
    if selected.budget.state!=SharedSuccessorBudgetState::Available || selected.budget.observed_ledger_revision!=rev {return Err(Error::Capacity)}
    let collision:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_start_intents WHERE id=?1 OR (child_id=?2 AND predecessor_digest=?3))",params![start_id,original.agent_reservation_id,selected.predecessor.command_digest],|r|r.get(0))?;
    if collision {return Err(Error::Conflict)}
    let input=SharedBudgetAttemptStartInput {
        tool_policy: owner_tool_policy(db, &original)?,
        web_search: owner_live_web(db, &original),
        start_intent_id:start_id.clone(),assignment_id:original.assignment_id.clone(),predecessor:selected.predecessor,
        execution_budget:selected.execution_budget,budget_observation:SharedBudgetObservation {
            observed_ledger_revision:rev,allocated_tokens:selected.budget.allocated_tokens,known_consumed_tokens:selected.budget.known_consumed_tokens,
            retained_tokens:selected.budget.retained_tokens,remaining_tokens:selected.budget.remaining_tokens,usage_complete:selected.budget.usage_complete,liability_digest:selected.budget.liability_digest,
        },
    };
    input.validate()?;
    bounded_row(db,"owner_shared_budget_start_intents","id",&start_id,100_000)?;
    db.execute("INSERT INTO owner_shared_budget_start_intents VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![start_id,original.agent_reservation_id,e.profile.digest,e.executor_device_id,e.executor_generation,input.predecessor.command_digest,canonical::digest(&serde_json::to_value(&original)?)?,canonical::encode(&serde_json::to_value(&original)?)?,canonical::encode(&serde_json::to_value(&input)?)?,canonical::digest(&serde_json::to_value(&input)?)?,rev])?;
    e.check()?;Ok(SharedBudgetAttemptStartIntent{input,original})
}
