//! V3 original-event consumer. Recovery never invents another event/start ID.
use super::*;
use budget_demand_io::BudgetDemandAction;
async fn offer_proof(b:&Bound,r:&OwnerResourceRegistry,c:&SharedRuntimeCommand,s:&SharedBudgetDemandStatus,observed:u64)->Result<VerifiedSharedBudgetDemand,NativeError>{
    s.validate().map_err(store)?;
    if s.offer.assignment_id!=c.assignment_id{return Err(invalid())}
    let proof=VerifiedSharedDemand::new(b.enrollment(r,c,true).await?,s.offer.clone(),s.demand_digest.clone(),observed,b.scope.deadline.min(Instant::now()+Duration::from_secs(30))).map_err(store)?;
    VerifiedSharedBudgetDemand::new(proof,s.clone()).map_err(store)
}
impl SharedExecutionTransport {
    #[cfg(test)]
    pub(in crate::console::native) async fn poll_budget_demand_fixture(&self,r:&OwnerResourceRegistry,launch:super::super::managed_runtime::PreparedLaunch)->Result<SharedDemandPoll,NativeError>{
        let b=self.io.bound().await?;budget_demand_io::require_pair(&b).await?;
        let reply:Timed<hagency_store::owner_resources::shared_execution::SharedBudgetDemandPoll>=b.request(BudgetDemandAction::Poll{after:None}.into(),false).await?;
        let outcome=match reply.value.demand{Some(s)=>self.consume_budget_demand(r,s,reply.observed,Some(launch)).await,None=>Ok(SharedDemandProgress::Idle)};
        Ok(SharedDemandPoll{next_after:reply.value.next_after,outcome})
    }
    pub async fn poll_budget_demand(&self,r:&OwnerResourceRegistry,after:Option<String>)->Result<SharedDemandPoll,NativeError>{
        let b=self.io.bound().await?;budget_demand_io::require_pair(&b).await?;
        let reply:Timed<hagency_store::owner_resources::shared_execution::SharedBudgetDemandPoll>=b.request(BudgetDemandAction::Poll{after}.into(),false).await?;
        if let Some(id)=&reply.value.next_after{super::super::key(id)?;}
        let outcome=match reply.value.demand {Some(s)=>self.consume_budget_demand(r,s,reply.observed,None).await,None=>Ok(SharedDemandProgress::Idle)};
        Ok(SharedDemandPoll{next_after:reply.value.next_after,outcome})
    }
    pub async fn resume_next_budget_demand(&self,r:&OwnerResourceRegistry,after:Option<String>)->Result<Option<SharedDemandResume>,NativeError>{
        let b=self.io.bound().await?;
        let Some(saved)=r.next_shared_demand(b.scope.profile.clone(),b.scope.device.device_id().into(),after).await.map_err(store)?else{return Ok(None)};
        let id=saved.intent().offer().demand_id.clone();let expected=saved.intent().offer().digest().map_err(store)?;
        let outcome=async{
            let reply:Timed<budget_demand_io::RecoverReply>=b.request(BudgetDemandAction::Recover{id:id.clone(),digest:expected.clone()}.into(),false).await?;
            let status=reply.value.demand.ok_or_else(invalid)?;
            if status.offer.demand_id!=id||status.demand_digest!=expected{return Err(invalid())}
            self.consume_budget_demand(r,status,reply.observed,None).await
        }.await;
        Ok(Some(SharedDemandResume{demand_id:id,outcome}))
    }
    async fn consume_budget_demand(&self,r:&OwnerResourceRegistry,s:SharedBudgetDemandStatus,observed:u64,launch:Option<super::super::managed_runtime::PreparedLaunch>)->Result<SharedDemandProgress,NativeError>{
        s.validate().map_err(store)?;let id=s.offer.demand_id.clone();let b=self.io.bound().await?;
        let original=r.shared_original_selection(b.scope.profile.clone(),s.offer.assignment_id.clone()).await.map_err(store)?.ok_or_else(||failure(409,"shared_demand_original_missing"))?;
        io::binding(&b.scope,&original,false)?;
        if !matches!(s.state.as_str(),"offered"|"start_committed") {
            if matches!(s.state.as_str(),"expired"|"cancelled"|"head_changed") {
                r.retain_shared_budget_demand_terminal(offer_proof(&b,r,&original,&s,observed).await?,original.clone()).await.map_err(store)?;
            }
            if s.state=="cancelled" {
                r.stop_shared_demand_revision(b.cleanup(r,&original).await?,s.offer.control_revision).await.map_err(store)?;
                self.manager.owner.0.shared_execution.stop_assignment(&s.offer.assignment_id,s.offer.control_revision)?;
            }
            return Ok(SharedDemandProgress::Historical{demand_id:id,status:s.state});
        }
        let saved=r.prepare_shared_budget_demand(offer_proof(&b,r,&original,&s,observed).await?,original.clone()).await.map_err(store)?;
        match saved.disposition(){
            SharedDemandDisposition::Pending=>Ok(SharedDemandProgress::Pending{demand_id:id}),
            SharedDemandDisposition::HeadChanged=>Ok(SharedDemandProgress::Blocked{demand_id:id,reason:"head_changed".into()}),
            SharedDemandDisposition::BudgetBlocked=>{
                let input=saved.block().ok_or_else(invalid)?.clone();input.validate_for(saved.offer()).map_err(store)?;
                let reply:Timed<budget_demand_io::RecoverReply>=b.request(BudgetDemandAction::Block{id:id.clone(),input:Box::new(input.clone())}.into(),true).await?;
                let current=reply.value.demand.ok_or_else(invalid)?;current.validate().map_err(store)?;
                if current.demand_digest!=s.demand_digest||current.execution_budget!=input.execution_budget||current.state!=input.reason{return Err(invalid())}
                Ok(SharedDemandProgress::Blocked{demand_id:id,reason:input.reason})
            }
            SharedDemandDisposition::StartPrepared=>{
                let submission=r.shared_budget_demand_submission(offer_proof(&b,r,&original,&s,observed).await?,original).await.map_err(store)?;
                budget_demand_io::require_pair(&b).await?;submission.check().map_err(store)?;
                let progress=self.process_budget_attempt(r,budget_io::AttemptAction::DemandStart{id:id.clone(),input:submission.input().clone()},None,launch).await?;
                Ok(SharedDemandProgress::Execution{demand_id:id,progress})
            }
        }
    }
}
