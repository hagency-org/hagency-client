//! Exact original event, immutable startup retry, and Run/Stop regressions.
use super::*;
fn offer(c:&SharedRuntimeCommand,id:&str,revision:u64)->SharedDemandOffer {
    SharedDemandOffer {version:1,demand_id:id.into(),assignment_id:c.assignment_id.clone(),event_id:format!("${id}"),event_digest:"e".repeat(64),control_revision:revision,received_at_ms:graphs::now_ms().unwrap(),predecessor_command_id:c.command_id.clone(),predecessor_command_digest:digest(c)}
}
fn proof_for(f:&Fixture,o:&SharedDemandOffer)->VerifiedSharedDemand {
    VerifiedSharedDemand::new(enrollment(f),o.clone(),digest(o),graphs::now_ms().unwrap(),Instant::now()+Duration::from_secs(20)).unwrap()
}
async fn status(f:&Fixture,c:&SharedRuntimeCommand,o:&SharedDemandOffer)->SharedBudgetDemandStatus {
    let s=f.registry.shared_budget_successor_selection(enrollment(f),c.clone()).await.unwrap();
    SharedBudgetDemandStatus {offer:o.clone(),demand_digest:digest(o),state:"offered".into(),execution_budget:s.execution_budget,original_block:None,runtime_command_id:None,binding:None}
}
fn verified(f:&Fixture,s:SharedBudgetDemandStatus)->VerifiedSharedBudgetDemand {VerifiedSharedBudgetDemand::new(proof_for(f,&s.offer),s).unwrap()}
async fn prepare(f:&Fixture,c:&SharedRuntimeCommand,s:SharedBudgetDemandStatus)->Result<SharedBudgetDemandIntent,Error>{
    f.registry.prepare_shared_budget_demand(verified(f,s),c.clone()).await
}
async fn command_for(f:&Fixture,c:&SharedRuntimeCommand,input:&SharedBudgetDemandStartInput)->SharedBudgetAttemptCommand {
    let mut result=next(f,c,"demand_runtime").await;
    result.start_intent_id=input.start.start_intent_id.clone();result.predecessor=input.start.predecessor.clone();
    result.execution_budget=input.start.execution_budget.clone();result.budget_observation=input.start.budget_observation.clone();result
}
async fn count(f:&Fixture,table:&'static str)->u64 {
    f.registry.0.store.call(1024,move|db|Ok(db.db.query_row(&format!("SELECT count(*) FROM {table}"),[],|r|r.get(0))?)).await.unwrap()
}
#[tokio::test]
async fn budget_execution_v3_demand_retry_restarts_keep_exact_request_and_manual_lanes_separate(){
    let(mut f,c)=synthetic().await;let o=offer(&c,"durable_budget",1);let s=status(&f,&c,&o).await;
    let(a,b)=tokio::join!(prepare(&f,&c,s.clone()),prepare(&f,&c,s.clone()));
    let input=a.unwrap().input().unwrap().clone();assert_eq!(digest(&input),digest(b.unwrap().input().unwrap()));
    assert_eq!(count(&f,"owner_shared_budget_start_intents").await,1);
    assert!(f.registry.recover_shared_budget_attempt_start(cleanup(&f,&c,1).await,c.clone(),input.start.predecessor.command_digest.clone()).await.unwrap().is_none());
    assert!(f.registry.prepare_shared_budget_attempt_start(enrollment(&f),c.clone(),input.start.start_intent_id.clone()).await.is_err());
    f.registry.shutdown().await.unwrap();f.registry=OwnerResourceRegistry::open_root(&f._temp.path().join("owner")).unwrap();
    assert_eq!(digest(prepare(&f,&c,s).await.unwrap().input().unwrap()),digest(&input));
    increase(&f,&c,"after_uncertain_start",25).await;
    let newer=status(&f,&c,&o).await;
    assert_eq!(digest(prepare(&f,&c,newer).await.unwrap().input().unwrap()),digest(&input),"never rebase an uncertain start onto more budget");
    let command=command_for(&f,&c,&input).await;
    assert!(f.registry.claim_shared_budget_successor(delivery(&f,command)).await.is_err());
    assert_eq!(count(&f,"owner_shared_budget_start_intents").await,1);
    f.registry.shutdown().await.unwrap();
}
#[tokio::test]
async fn budget_execution_v3_demand_reassesses_original_v2_budget_block_without_rewriting_it(){
    let(f,c)=synthetic().await;
    call(&f,&c,"retained_overrun","settled",150,231,true,false).await;
    let o=offer(&c,"original_block",1);
    let old=f.registry.prepare_shared_demand(proof_for(&f,&o),c.clone(),None).await.unwrap();
    assert_eq!(*old.disposition(),SharedDemandDisposition::BudgetBlocked);
    let original_hash=digest(&json!({"demandDigest":digest(&o),"expectedControlRevision":1,"reason":"budget_blocked","budget":old.budget().unwrap().observation}));
    increase(&f,&c,"restore_capacity",100).await;
    let mut s=status(&f,&c,&o).await;
    assert!(prepare(&f,&c,s.clone()).await.is_err(),"missing original block cannot be substituted");
    s.original_block=Some(SharedOriginalBudgetBlock{reason:"budget_blocked".into(),payload_digest:original_hash});
    let input=prepare(&f,&c,s).await.unwrap().input().unwrap().clone();
    assert_eq!(input.start.budget_observation.known_consumed_tokens,231);
    assert_eq!(input.start.budget_observation.remaining_tokens,69);
    assert_eq!(input.demand_digest,digest(&o));assert_eq!(input.expected_control_revision,1);
    assert_eq!(count(&f,"owner_shared_demands").await,1);
    assert_eq!(count(&f,"owner_shared_demand_decisions").await,1);
    assert_eq!(count(&f,"owner_shared_budget_demand_decisions").await,1);
    let unchanged=f.registry.recover_shared_demand(cleanup(&f,&c,1).await,o.demand_id).await.unwrap().unwrap();
    assert_eq!(digest(unchanged.budget().unwrap()),digest(old.budget().unwrap()));
    let command=command_for(&f,&c,&input).await;
    let SharedBudgetAttemptClaim::Issued(custody)=f.registry.claim_shared_budget_successor(delivery(&f,command)).await.unwrap() else{panic!("one claim")};
    assert_eq!(custody.demand_control_revision(),Some(1));
    f.registry.shutdown().await.unwrap();
}
#[tokio::test]
async fn budget_execution_v3_demand_each_head_appends_block_then_start_for_same_event(){
    let(f,c)=synthetic().await;
    call(&f,&c,"complete_overrun","settled",150,231,true,false).await;
    let o=offer(&c,"v3_block",1);let s=status(&f,&c,&o).await;
    let blocked=prepare(&f,&c,s.clone()).await.unwrap();assert_eq!(*blocked.disposition(),SharedDemandDisposition::BudgetBlocked);
    let block_hash=digest(blocked.block().unwrap());
    assert_eq!(digest(prepare(&f,&c,s).await.unwrap().block().unwrap()),block_hash);
    increase(&f,&c,"too_small",10).await;
    assert_eq!(*prepare(&f,&c,status(&f,&c,&o).await).await.unwrap().disposition(),SharedDemandDisposition::BudgetBlocked);
    increase(&f,&c,"enough",30).await;
    let started=prepare(&f,&c,status(&f,&c,&o).await).await.unwrap();assert!(started.input().is_some());
    assert_eq!(count(&f,"owner_shared_budget_demand_decisions").await,3);
    assert_eq!(count(&f,"owner_shared_budget_start_intents").await,1);
    assert_eq!(count(&f,"owner_shared_demands").await,1);
    f.registry.shutdown().await.unwrap();
}
#[tokio::test]
async fn budget_execution_v3_demand_stop_cannot_be_bypassed_by_delta_or_old_run(){
    let(f,c)=synthetic().await;
    let o=offer(&c,"old_run",1);let s=status(&f,&c,&o).await;let input=prepare(&f,&c,s.clone()).await.unwrap().input().unwrap().clone();
    let command=command_for(&f,&c,&input).await;
    f.registry.stop_shared_demand_revision(cleanup(&f,&c,1).await,1).await.unwrap();
    assert!(prepare(&f,&c,s).await.is_err());
    assert!(f.registry.claim_shared_budget_successor(delivery(&f,command)).await.is_err());
    increase(&f,&c,"after_stop",25).await;
    assert!(prepare(&f,&c,status(&f,&c,&o).await).await.is_err());
    let later=offer(&c,"new_run",3);
    let later=prepare(&f,&c,status(&f,&c,&later).await).await.unwrap();assert_eq!(*later.disposition(),SharedDemandDisposition::Pending,"uncertain older start still owns predecessor");
    assert_eq!(count(&f,"owner_shared_budget_start_intents").await,1);
    f.registry.shutdown().await.unwrap();
}
#[tokio::test]
async fn budget_execution_v3_demand_new_run_after_stop_has_exact_control_pin(){
    let(f,c)=synthetic().await;
    f.registry.stop_shared_demand_revision(cleanup(&f,&c,1).await,1).await.unwrap();
    let o=offer(&c,"fresh_run",3);let input=prepare(&f,&c,status(&f,&c,&o).await).await.unwrap().input().unwrap().clone();
    let command=command_for(&f,&c,&input).await;
    let SharedBudgetAttemptClaim::Issued(custody)=f.registry.claim_shared_budget_successor(delivery(&f,command)).await.unwrap()else{panic!("new run claim")};
    assert_eq!(custody.demand_control_revision(),Some(3));
    f.registry.shutdown().await.unwrap();
}
#[tokio::test]
async fn budget_execution_v3_demand_failed_decision_rolls_back_offer_control_and_start(){
    let(f,c)=synthetic().await;let o=offer(&c,"atomic",1);let s=status(&f,&c,&o).await;
    f.registry.0.store.call(1024,|db|{db.db.execute_batch("CREATE TRIGGER fail_budget_demand BEFORE INSERT ON owner_shared_budget_demand_decisions BEGIN SELECT RAISE(ABORT,'test injected failure'); END;")?;Ok(())}).await.unwrap();
    assert!(prepare(&f,&c,s.clone()).await.is_err());
    for t in ["owner_shared_demands","owner_shared_demand_controls","owner_shared_budget_start_intents","owner_shared_budget_demand_decisions"] {assert_eq!(count(&f,t).await,0,"{t}");}
    f.registry.0.store.call(1024,|db|{db.db.execute_batch("DROP TRIGGER fail_budget_demand;")?;Ok(())}).await.unwrap();
    assert!(prepare(&f,&c,s).await.unwrap().input().is_some());
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn budget_execution_v3_terminal_unstarted_demand_unblocks_only_new_event() {
    let(mut f,c)=synthetic().await;
    call(&f,&c,"retained_unknown","unknown",40,7,false,false).await;
    let old=offer(&c,"old_failed_demand",1);let old_status=status(&f,&c,&old).await;
    let input=prepare(&f,&c,old_status.clone()).await.unwrap().input().unwrap().clone();
    let failed=command_for(&f,&c,&input).await;
    f.registry.fence_shared_budget_successor(cleanup(&f,&c,1).await,failed.clone()).await.unwrap();
    f.registry.record_unstarted_shared_budget_attempt(cleanup(&f,&c,1).await,failed.clone()).await.unwrap().unwrap();
    let mut new=offer(&c,"new_research",1);new.received_at_ms=old.received_at_ms+1;
    new.predecessor_command_id=failed.command_id.clone();new.predecessor_command_digest=digest(&failed);
    let new_status=status(&f,&c,&new).await;
    assert_eq!(*prepare(&f,&c,new_status.clone()).await.unwrap().disposition(),SharedDemandDisposition::Pending);
    assert!(f.registry.retain_shared_budget_demand_terminal(verified(&f,old_status.clone()),c.clone()).await.is_err());
    let mut terminal=old_status;terminal.state="head_changed".into();terminal.runtime_command_id=Some(failed.command_id.clone());
    f.registry.retain_shared_budget_demand_terminal(verified(&f,terminal.clone()),c.clone()).await.unwrap();
    f.registry.retain_shared_budget_demand_terminal(verified(&f,terminal),c.clone()).await.unwrap();
    assert_eq!(count(&f,"owner_shared_budget_demand_terminals").await,1);
    f.registry.shutdown().await.unwrap();f.registry=OwnerResourceRegistry::open_root(&f._temp.path().join("owner")).unwrap();
    let input=prepare(&f,&c,new_status).await.unwrap().input().unwrap().clone();
    assert_eq!(input.demand_digest,digest(&new));assert!(input.start.predecessor.process_digest.is_empty());
    assert_eq!((input.start.budget_observation.known_consumed_tokens,input.start.budget_observation.retained_tokens),(7,40));
    let mut fresh=next(&f,&c,"new_research_runtime").await;fresh.start_intent_id=input.start.start_intent_id.clone();fresh.predecessor=input.start.predecessor.clone();fresh.execution_budget=input.start.execution_budget.clone();fresh.budget_observation=input.start.budget_observation.clone();
    assert!(matches!(f.registry.claim_shared_budget_successor(delivery(&f,fresh)).await.unwrap(),SharedBudgetAttemptClaim::Issued(_)));
    assert_eq!(count(&f,"owner_shared_demands").await,2);assert_eq!(count(&f,"owner_shared_budget_start_intents").await,2);
    assert!(f.registry.claim_shared_budget_successor(delivery(&f,failed)).await.is_err());
    f.registry.shutdown().await.unwrap();
}
