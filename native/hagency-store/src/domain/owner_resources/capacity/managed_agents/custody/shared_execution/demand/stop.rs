//! Explicit owner Stop command custody. Cancellation observations never create it.
use super::*;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandStopInput {
    pub command_id: String,
    pub expected_revision: u64,
    pub mode: String,
}
pub struct SharedDemandStopIntent {
    original: SharedRuntimeCommand,
    input: SharedDemandStopInput,
}
impl SharedDemandStopIntent {
    pub fn original(&self) -> &SharedRuntimeCommand {
        &self.original
    }
    pub fn input(&self) -> &SharedDemandStopInput {
        &self.input
    }
}
fn saved(
    db: &Connection,
    profile: &ProfileAuthority,
    id: &str,
) -> Result<SharedDemandStopIntent, Error> {
    let (p,child,actor,assignment,selected,payload,digest,original,original_digest):(String,String,String,String,u64,String,String,String,String)=db.query_row("SELECT s.profile,s.child_id,s.actor_mxid,s.assignment_id,s.selected_revision,s.input_payload,s.input_digest,l.command_payload,l.command_digest FROM owner_shared_stop_intents s JOIN owner_shared_runtime_lifetimes l ON l.child_id=s.child_id AND l.profile=s.profile WHERE s.command_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?)))?;
    let original: SharedRuntimeCommand = serde_json::from_str(&original)?;
    original.validate(&profile.account)?;
    let input: SharedDemandStopInput = serde_json::from_str(&payload)?;
    if p != profile.digest
        || actor != profile.account
        || child != original.agent_reservation_id
        || assignment != original.assignment_id
        || input.command_id != id
        || input.expected_revision != selected
        || input.mode != "paused"
        || canonical::digest(&serde_json::to_value(&input)?)? != digest
        || canonical::digest(&serde_json::to_value(&original)?)? != original_digest
    {
        return Err(Error::Conflict);
    }
    Ok(SharedDemandStopIntent { original, input })
}
impl OwnerResourceRegistry {
    pub async fn prepare_shared_demand_stop(
        &self,
        a: ManagedCleanupAuthority,
        selected_revision: u64,
    ) -> Result<SharedDemandStopIntent, Error> {
        self.prepare_stop(a, selected_revision, None).await
    }
    /// Exact reviewed human command. Never substitutes another revision or ID.
    pub async fn prepare_reviewed_shared_demand_stop(
        &self,
        a: ManagedCleanupAuthority,
        input: SharedDemandStopInput,
    ) -> Result<SharedDemandStopIntent, Error> {
        identifier(&input.command_id, 128)?;
        if input.mode != "paused" {
            return Err(Error::Conflict);
        }
        self.prepare_stop(a, input.expected_revision, Some(input.command_id))
            .await
    }
    async fn prepare_stop(
        &self,
        a: ManagedCleanupAuthority,
        selected_revision: u64,
        command_id: Option<String>,
    ) -> Result<SharedDemandStopIntent, Error> {
        if !cfg!(feature = "shared-intake") {
            return Err(Error::Unqualified);
        }
        a.check(&self.0)?;
        exact(selected_revision)?;
        if selected_revision == JSON_SAFE_MAX {
            return Err(Error::LocalAuthority);
        }
        let registry = self.0.clone();
        let p = a.enrollment.profile.clone();
        let deadline = a.deadline();
        let result=self.0.store.call(8192,move|db|{
            a.check(&registry)?;let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;authority::cleanup(&tx,&a)?;
            let id=match command_id {Some(id)=>id,None=>format!("sdstop_{}",&canonical::digest(&json!({"registry":registry.id,"profile":a.enrollment.profile.digest,"child":a.child,"revision":selected_revision}))?[..40])};
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_stop_intents WHERE command_id=?1)",[&id],|r|r.get(0))?;
            if exists {
                let old=saved(&tx,&a.enrollment.profile,&id)?;
                if old.input.expected_revision!=selected_revision || old.original.agent_reservation_id!=a.child || old.original.reserve_command.executor_device_id!=a.enrollment.executor_device_id {return Err(Error::Conflict)}
            } else {
                let other:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_stop_intents WHERE profile=?1 AND child_id=?2 AND selected_revision=?3)",params![a.enrollment.profile.digest,a.child,selected_revision],|r|r.get(0))?;
                if other {return Err(Error::Conflict)}
                let (payload,digest):(String,String)=tx.query_row("SELECT command_payload,command_digest FROM owner_shared_runtime_lifetimes WHERE child_id=?1 AND profile=?2",params![a.child,a.enrollment.profile.digest],|r|Ok((r.get(0)?,r.get(1)?)))?;
                let original:SharedRuntimeCommand=serde_json::from_str(&payload)?;original.validate(&a.enrollment.profile.account)?;
                if canonical::digest(&serde_json::to_value(&original)?)?!=digest||original.reserve_command.executor_device_id!=a.enrollment.executor_device_id{return Err(Error::LocalAuthority)}
                let count:u64=tx.query_row("SELECT count(*) FROM owner_shared_stop_intents WHERE child_id=?1",[&a.child],|r|r.get(0))?;if count>=10000{return Err(Error::Capacity)}
                let input=SharedDemandStopInput{command_id:id.clone(),expected_revision:selected_revision,mode:"paused".into()};let value=serde_json::to_value(input)?;
                let rev=revision(&tx)?;
                tx.execute("INSERT INTO owner_shared_stop_intents VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![id,a.child,a.enrollment.profile.digest,a.enrollment.executor_device_id,a.enrollment.executor_generation,a.enrollment.profile.account,original.assignment_id,selected_revision,canonical::encode(&value)?,canonical::digest(&value)?,rev])?;
            }
            // The local restriction and exact outbound command commit together.
            tx.execute("INSERT INTO owner_shared_demand_controls VALUES(?1,?2,?3,?3) ON CONFLICT(child_id) DO UPDATE SET control_revision=max(control_revision,excluded.control_revision),stopped_through=max(stopped_through,excluded.stopped_through) WHERE profile=excluded.profile",params![a.child,a.enrollment.profile.digest,selected_revision])?;
            let result=saved(&tx,&a.enrollment.profile,&id)?;a.check(&registry)?;tx.commit()?;a.check(&registry).map_err(|_|Error::OutcomeUnknown)?;Ok(result)
        }).await?;
        after(&p, &self.0, deadline)?;
        Ok(result)
    }
    /// Restriction-only owner custody: no remote enrollment, grant, or execution
    /// authority is created. The immutable local lifetime proves the original
    /// account/device/assignment; the current native profile gate bounds writes.
    pub async fn prepare_local_owner_demand_stop(
        &self,
        profile: ProfileAuthority,
        device: String,
        generation: u64,
        original: SharedRuntimeCommand,
        input: SharedDemandStopInput,
    ) -> Result<SharedDemandStopIntent, Error> {
        if !cfg!(feature = "shared-intake") {
            return Err(Error::Unqualified);
        }
        profile.check()?;
        identifier(&device, 128)?;
        identifier(&input.command_id, 128)?;
        exact(generation)?;
        exact(input.expected_revision)?;
        original.validate(&profile.account)?;
        if input.mode != "paused"
            || input.expected_revision == JSON_SAFE_MAX
            || original.reserve_command.executor_device_id != device
            || original.reserve_command.executor_generation > generation
        {
            return Err(Error::LocalAuthority);
        }
        let registry = self.0.clone();
        let p = profile.clone();
        let deadline = profile.expires;
        let result = self.0.store.call(64 * 1024, move |db| {
            profile.check()?;
            if registry.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority); }
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (payload, digest): (String,String) = tx.query_row(
                "SELECT command_payload,command_digest FROM owner_shared_runtime_lifetimes WHERE child_id=?1 AND profile=?2",
                params![original.agent_reservation_id,profile.digest], |r| Ok((r.get(0)?,r.get(1)?)))?;
            let saved_original: SharedRuntimeCommand = serde_json::from_str(&payload)?;
            saved_original.validate(&profile.account)?;
            if canonical::digest(&serde_json::to_value(&saved_original)?)? != digest
                || canonical::digest(&serde_json::to_value(&original)?)? != digest
            { return Err(Error::Conflict); }
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_stop_intents WHERE command_id=?1)", [&input.command_id], |r| r.get(0))?;
            if exists {
                let old = saved(&tx, &profile, &input.command_id)?;
                if old.input.expected_revision != input.expected_revision
                    || old.original.agent_reservation_id != original.agent_reservation_id
                    || old.original.reserve_command.executor_device_id != device
                { return Err(Error::Conflict); }
            } else {
                let other: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_stop_intents WHERE profile=?1 AND child_id=?2 AND selected_revision=?3)", params![profile.digest,original.agent_reservation_id,input.expected_revision], |r| r.get(0))?;
                if other { return Err(Error::Conflict); }
                let count: u64 = tx.query_row("SELECT count(*) FROM owner_shared_stop_intents WHERE child_id=?1", [&original.agent_reservation_id], |r| r.get(0))?;
                if count >= 10000 { return Err(Error::Capacity); }
                let value = serde_json::to_value(&input)?;
                let rev = revision(&tx)?;
                tx.execute("INSERT INTO owner_shared_stop_intents VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![input.command_id,original.agent_reservation_id,profile.digest,device,generation,profile.account,original.assignment_id,input.expected_revision,canonical::encode(&value)?,canonical::digest(&value)?,rev])?;
            }
            let changed = tx.execute("INSERT INTO owner_shared_demand_controls VALUES(?1,?2,?3,?3) ON CONFLICT(child_id) DO UPDATE SET control_revision=max(control_revision,excluded.control_revision),stopped_through=max(stopped_through,excluded.stopped_through) WHERE profile=excluded.profile", params![original.agent_reservation_id,profile.digest,input.expected_revision])?;
            if changed != 1 { return Err(Error::Conflict); }
            let result = saved(&tx, &profile, &input.command_id)?;
            profile.check()?;
            if registry.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority); }
            tx.commit()?;
            profile.check().map_err(|_| Error::OutcomeUnknown)?;
            Ok(result)
        }).await?;
        after(&p, &self.0, deadline)?;
        Ok(result)
    }
    pub async fn next_shared_demand_stop(
        &self,
        p: ProfileAuthority,
        device: String,
        after_id: Option<String>,
    ) -> Result<Option<SharedDemandStopIntent>, Error> {
        p.check()?;
        identifier(&device, 128)?;
        if let Some(id) = &after_id {
            identifier(id, 128)?;
        }
        let check = p.clone();
        let deadline = p.expires;
        let result=self.0.store.call(8192,move|db|{
            p.check()?;
            let id:Option<String>=db.db.query_row("SELECT s.command_id FROM owner_shared_stop_intents s WHERE s.profile=?1 AND s.device_id=?2 AND (?3 IS NULL OR s.command_id>?3) AND NOT EXISTS(SELECT 1 FROM owner_shared_stop_receipts r WHERE r.command_id=s.command_id) ORDER BY s.command_id LIMIT 1",params![p.digest,device,after_id],|r|r.get(0)).optional()?;
            let result=id.map(|id|saved(&db.db,&p,&id)).transpose()?;p.check()?;Ok(result)
        }).await?;
        after(&check, &self.0, deadline)?;
        Ok(result)
    }
    pub async fn record_shared_demand_stop(
        &self,
        a: ManagedCleanupAuthority,
        id: String,
        applied: u64,
    ) -> Result<(), Error> {
        a.check(&self.0)?;
        identifier(&id, 128)?;
        exact(applied)?;
        let p = a.enrollment.profile.clone();
        let deadline = a.deadline();
        let registry = self.0.clone();
        self.0.store.call(8192,move|db|{
            a.check(&registry)?;let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;authority::cleanup(&tx,&a)?;
            let intent=saved(&tx,&a.enrollment.profile,&id)?;
            if intent.original.agent_reservation_id!=a.child||intent.original.reserve_command.executor_device_id!=a.enrollment.executor_device_id||applied<intent.input.expected_revision||applied>intent.input.expected_revision+1{return Err(Error::Conflict)}
            let old:Option<u64>=tx.query_row("SELECT applied_revision FROM owner_shared_stop_receipts WHERE command_id=?1",[&id],|r|r.get(0)).optional()?;
            if let Some(old)=old{if old!=applied{return Err(Error::Conflict)}}else{let rev=revision(&tx)?;tx.execute("INSERT INTO owner_shared_stop_receipts VALUES(?1,?2,?3)",params![id,applied,rev])?;}
            a.check(&registry)?;tx.commit()?;a.check(&registry).map_err(|_|Error::OutcomeUnknown)
        }).await?;
        after(&p, &self.0, deadline)
    }
}
