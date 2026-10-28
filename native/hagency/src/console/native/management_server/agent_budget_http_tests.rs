//! Actual HTTP response loss and private journal replay; no executor calls.
use super::*;
#[derive(Default)]
pub(in crate::console::native) struct BudgetState {
    pub enabled: bool,
    pub invalidate_discovery: bool,
    current: Option<Value>,
    receipts: BTreeMap<String, Value>,
    writes: usize,
    reads: usize,
    lose_after: bool,
    lose_before: bool,
    invalidate: bool,
    reject: Option<&'static str>,
}
fn budget() -> Value {
    json!({"requestId":"human_request","requestRevision":3,"assignmentGeneration":2,"assignmentId":"assignment","agentReservationId":"reservation","originalCeilingTokens":4096,"appliedDeltaTokens":0,"effectiveCeilingTokens":4096,"budgetRevision":1,"budgetDigest":"a".repeat(64),"knownConsumedTokens":4813,"conservativeCommitmentTokens":4813,"nativeRetainedTokens":null,"nativeRemainingTokens":null,"usageComplete":null,"executionSupported":false,"canRequest":true,"blockingReason":null})
}
fn topup() -> Value {
    json!({"id":"topup","authorityRevision":4,"requestId":"human_request","assignmentId":"assignment","addTokens":1024,"purpose":"Continue the task","expectedBudgetRevision":1,"expectedBudgetDigest":"a".repeat(64),"expectedCeilingTokens":4096,"revision":1,"state":"pending","nativeState":"not_delivered","executorCommandId":null,"createdAtMs":100,"expiresAtMs":200,"canDecide":true,"canCancel":true})
}
pub(super) fn management(
    path: &str,
    body: &Value,
    state: &mut super::super::State,
) -> Option<Value> {
    let s = &mut state.admission.budget;
    if path.contains("/agent-topup-commands/") {
        let id = path.split('/').rev().nth(1).unwrap();
        return Some(if let Some(ack) = s.receipts.get(id) {
            json!({"recorded":true,"result":{"receipt":ack,"topUp":s.current},"observedAtMs":now()})
        } else {
            json!({"recorded":false,"result":null,"observedAtMs":now()})
        });
    }
    if path.ends_with("/budget")
        || path.ends_with("/topups/query")
        || path.ends_with("/agent-topups/topup")
    {
        s.reads += 1;
        if s.invalidate {
            state
                .source
                .as_ref()
                .unwrap()
                .store(false, Ordering::Release);
        }
        return Some(if path.ends_with("/budget") {
            json!({"budget":budget(),"observedAtMs":now()})
        } else if path.ends_with("/query") {
            json!({"topUps":{"items":s.current.clone().into_iter().collect::<Vec<_>>(),"nextAfter":null},"observedAtMs":now()})
        } else {
            json!({"topUp":s.current,"observedAtMs":now()})
        });
    }
    if std::mem::take(&mut s.lose_before) {
        return None;
    }
    if let Some(code) = s.reject {
        return Some(json!({"code":code}));
    }
    let id = body["commandId"].as_str().unwrap().to_owned();
    if let std::collections::btree_map::Entry::Vacant(entry) = s.receipts.entry(id.clone()) {
        let op = if path.ends_with("/topups") {
            assert_eq!(body["expectedRequestRevision"], 3);
            assert_eq!(body["expectedAssignmentGeneration"], 2);
            assert_eq!(body["expectedBudgetRevision"], 1);
            assert_eq!(body["expectedBudgetDigest"], "a".repeat(64));
            let mut t=topup();
            if body["approveImmediately"]==true {
                assert_eq!(body["ownerFunded"],true);
                t["state"]=json!("approved"); t["revision"]=json!(2);
                t["ownerFunded"]=json!(true); t["canDecide"]=json!(false);t["canCancel"]=json!(false);
            }
            s.current = Some(t);
            "agent.topup.request"
        } else {
            let t = s.current.as_mut().unwrap();
            assert_eq!(body["expectedRevision"], t["revision"]);
            t["revision"] = json!(t["revision"].as_i64().unwrap() + 1);
            if path.ends_with("/decide") {
                assert_eq!(body["expectedAuthorityRevision"], 4);
                t["state"] = json!(if body["decision"] == "approve" {
                    "approved"
                } else {
                    "declined"
                });
                "agent.topup.decide"
            } else {
                t["state"] = json!("cancelled");
                "agent.topup.cancel"
            }
        };
        entry.insert(json!({"commandId":id,"operation":op,"topUpId":"topup","appliedRevision":if op=="agent.topup.request" {json!(1)} else {s.current.as_ref().unwrap()["revision"].clone()}}));
        s.writes += 1;
    }
    if s.invalidate {
        state
            .source
            .as_ref()
            .unwrap()
            .store(false, Ordering::Release);
    }
    if std::mem::take(&mut s.lose_after) {
        return None;
    }
    Some(json!({"receipt":s.receipts[&id],"topUp":s.current,"observedAtMs":now()}))
}
fn request(id: &str) -> ManagementCommand {
    ManagementCommand::AgentTopUpRequest { approve_immediately: false, owner_funded: false,
        command_id: id.into(),
        request_id: "human_request".into(),
        expected_request_revision: 3,
        expected_assignment_generation: 2,
        expected_budget_revision: 1,
        expected_budget_digest: "a".repeat(64),
        add_tokens: 1024,
        purpose: "Continue the task".into(),
    }
}
fn grants() -> Vec<Permission> {
    vec![
        Permission::ReadAgentBudget,
        Permission::RequestAgentTopUps,
        Permission::DecideAgentTopUps,
        Permission::CancelAgentTopUps,
        Permission::ReadIntents,
        Permission::AcknowledgeIntents,
    ]
}
#[tokio::test]
async fn human_budget_http_lost_reply_restart_keeps_original_and_latest_state() {
    let f = Fixture::new().await;
    let m = manager(&f, 81, &grants()).await;
    f.state.lock().unwrap().admission.budget.lose_after = true;
    assert!(m.central(request("original_topup")).await.is_err());
    assert_eq!(m.saved_commands().await.unwrap().len(), 1);
    m.retire();
    drop(m);
    f.state
        .lock()
        .unwrap()
        .admission
        .budget
        .current
        .as_mut()
        .unwrap()["state"] = json!("approved");
    let m = manager(&f, 82, &grants()).await;
    let r = m
        .retry_saved_command("original_topup".into())
        .await
        .unwrap();
    assert_eq!(r["data"]["receipt"]["commandId"], "original_topup");
    assert_eq!(r["data"]["topUp"]["state"], "approved");
    assert_eq!(f.state.lock().unwrap().admission.budget.writes, 1);
    assert_eq!(
        m.central(request("original_topup")).await.unwrap()["topUp"]["state"],
        "approved"
    );
    let mut changed = request("original_topup");
    if let ManagementCommand::AgentTopUpRequest { add_tokens, .. } = &mut changed {
        *add_tokens = 2048;
    }
    assert!(m.central(changed).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.budget.writes, 1);
    m.acknowledge_command("original_topup".into())
        .await
        .unwrap();
    assert!(m.saved_commands().await.unwrap().is_empty());
}
#[tokio::test]
async fn human_budget_http_original_decide_cancel_and_exact_permission() {
    let f = Fixture::new().await;
    let m = manager(&f, 83, &grants()).await;
    m.central(request("request_topup")).await.unwrap();
    let decide = || ManagementCommand::AgentTopUpDecide {
        command_id: "decide_topup".into(),
        top_up_id: "topup".into(),
        expected_revision: 1,
        expected_authority_revision: 4,
        approve: false,
    };
    let weak = manager(&f, 84, &[Permission::ReadAgentBudget]).await;
    assert!(weak.central(decide()).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.budget.writes, 1);
    f.state.lock().unwrap().admission.budget.lose_after = true;
    assert!(m.central(decide()).await.is_err());
    let r = m.retry_saved_command("decide_topup".into()).await.unwrap();
    assert_eq!(r["data"]["topUp"]["state"], "declined");
    m.central(ManagementCommand::AgentTopUpCancel {
        command_id: "cancel_topup".into(),
        top_up_id: "topup".into(),
        expected_revision: 2,
    })
    .await
    .unwrap();
    assert_eq!(f.state.lock().unwrap().admission.budget.writes, 3);
}
#[tokio::test]
async fn human_budget_http_unknown_retry_rejection_and_source_loss_retain_custody() {
    let f = Fixture::new().await;
    let m = manager(&f, 85, &grants()).await;
    f.state.lock().unwrap().admission.budget.lose_before = true;
    assert!(m.central(request("unknown_topup")).await.is_err());
    f.state.lock().unwrap().admission.budget.reject = Some("management_revision_changed");
    assert_eq!(
        m.retry_saved_command("unknown_topup".into())
            .await
            .unwrap_err()
            .code,
        "management_outcome_unknown"
    );
    assert!(m.saved_commands().await.unwrap()[0].reply.is_none());
    {
        let mut s = f.state.lock().unwrap();
        s.admission.budget.reject = None;
        s.admission.budget.invalidate = true;
    }
    assert!(m.retry_saved_command("unknown_topup".into()).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.budget.writes, 1);
    f.current.store(true, Ordering::Release);
    f.state.lock().unwrap().admission.budget.invalidate = false;
    let m = manager(&f, 86, &grants()).await;
    assert_eq!(
        m.retry_saved_command("unknown_topup".into()).await.unwrap()["data"]["receipt"]["commandId"],
        "unknown_topup"
    );
    assert_eq!(f.state.lock().unwrap().admission.budget.writes, 1);
}
#[tokio::test]
async fn human_budget_http_discovery_absence_is_readonly_and_rechecks_scope() {
    let f = Fixture::new().await;
    #[cfg(all(feature="agent-budget",feature="shared-execution"))]
    { f.state.lock().unwrap().budget.missing_capability=true; }
    let m = manager(&f, 87, &grants()).await;
    assert_eq!(
        m.agent_budget("human_request".into()).await.unwrap()["supported"],
        false
    );
    assert_eq!(f.state.lock().unwrap().admission.budget.reads, 0);
    f.state.lock().unwrap().admission.budget.enabled = true;
    let v = m.agent_budget("human_request".into()).await.unwrap();
    assert_eq!(v["budget"]["knownConsumedTokens"], 4813);
    assert!(v["budget"]["nativeRemainingTokens"].is_null());
    assert!(!v["budget"]["executionSupported"].as_bool().unwrap());
    f.state
        .lock()
        .unwrap()
        .admission
        .budget
        .invalidate_discovery = true;
    assert!(m.agent_budget("human_request".into()).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.budget.reads, 1);
}

#[tokio::test]
async fn owner_direct_budget_http_requires_both_grants_and_recovers_one_write() {
    let f=Fixture::new().await;
    let command=||{let mut command=request("owner_direct");
    if let ManagementCommand::AgentTopUpRequest {owner_funded,approve_immediately,..}=&mut command {
        *owner_funded=true; *approve_immediately=true;
    } command};
    for permissions in [vec![Permission::RequestAgentTopUps],vec![Permission::DecideAgentTopUps]] {
        let weak=manager(&f,90,&permissions).await;
        assert_eq!(weak.central(command()).await.unwrap_err().code,"management_permission_required");
    }
    assert_eq!(f.state.lock().unwrap().admission.budget.writes,0);
    let m=manager(&f,91,&grants()).await;
    f.state.lock().unwrap().admission.budget.lose_after=true;
    assert!(m.central(command()).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.budget.writes,1);
    m.retire(); drop(m);
    let m=manager(&f,92,&grants()).await;
    let reply=m.retry_saved_command("owner_direct".into()).await.unwrap();
    assert_eq!(reply["data"]["topUp"]["state"],"approved");
    assert_eq!(reply["data"]["topUp"]["canDecide"],false);
    assert_eq!(m.central(command()).await.unwrap()["topUp"]["state"],"approved");
    assert_eq!(f.state.lock().unwrap().admission.budget.writes,1);
}
