//! Real loopback HTTP + durable native journal; no Matrix rooms or agent runs.
use super::*;
use crate::native_owner::{
    AgentDefinition, AgentRequestInput, ManagementCommand, NativeManagement,
};
#[path = "agent_cleanup_http_tests.rs"]
mod cleanup_tests;
#[path = "agent_execution_status_http_tests.rs"]
mod execution_status_tests;
#[path = "agent_work_http_tests.rs"]
mod work_tests;
#[path = "allocations_http_tests.rs"]
mod allocation_tests;
#[path = "agent_budget_http_tests.rs"]
mod budget_tests;
#[derive(Default)]
pub(super) struct State {
    pub(super) budget: budget_tests::BudgetState,
    allocations: allocation_tests::AllocationState,
    current: Option<Value>,
    receipts: BTreeMap<String, Value>,
    writes: usize,
    lose_reply: bool,
    lose_before: bool,
    invalidate_reply: bool,
    reject: Option<&'static str>,
    malformed: bool,
    stage_replies: BTreeMap<String, Value>,
    stage_reads: Vec<String>,
    work_queries: Vec<Value>,
    invalidate_stage: Option<String>,
}
fn projection() -> Value {
    json!({"id":"human_request","projectId":"project_fixture","projectName":"Fixture project","proposedAgentId":"agent_proposed","managerMxid":"@owner:test","administratorMxid":"@owner:test","authorityRevision":1,"definition":{"name":"Writer","capability":"documentation","description":"Draft text","toolMode":"none"},"definitionDigest":"d".repeat(64),"requestedTokens":100,"periodKey":"2026-10","state":"pending_approval","revision":1,"createdAtMs":now(),"expiresAtMs":now()+60000,"canDecide":true,"canApprove":true,"canReject":true,"canCancel":true,"blockingReason":null,"executionAvailable":false})
}
pub(super) fn management(path: &str, body: &Value, state: &mut super::State) -> Option<Value> {
    if path.ends_with("/budget") || path.contains("/topups") || path.contains("/agent-topups/") || path.contains("/agent-topup-commands/") {return budget_tests::management(path,body,state);}
    if path.contains("/allocations/") || path.contains("/allocation-requests/") || path.ends_with("/inbox") || path.contains("/allocation-commands/") {
        return allocation_tests::management(path,body,state);
    }
    if path.ends_with("/agent-removals/list") {
        state.admission.work_queries.push(body.clone());
        if state.admission.invalidate_stage.as_deref()==Some("removals") {state.source.as_ref().unwrap().store(false,Ordering::Release);}
        return Some(state.admission.stage_replies.get("removals").cloned().unwrap_or_else(||json!({"requests":{"items":[],"nextAfter":null},"observedAtMs":now()})));
    }
    if path.ends_with("/agent-work/list") {
        state.admission.work_queries.push(body.clone());
        if state.admission.invalidate_stage.as_deref() == Some("index") { state.source.as_ref().unwrap().store(false,Ordering::Release); }
        return Some(state.admission.stage_replies.get("index").cloned().unwrap_or_else(||json!({"requests":{"items":[],"nextAfter":null},"observedAtMs":now()})));
    }
    if path.contains("/agent-commands/") {
        let id = path.split('/').rev().nth(1).unwrap();
        return Some(if state.admission.receipts.contains_key(id) {
            json!({"recorded":true,"agentRequest":state.admission.current,"observedAtMs":now()})
        } else {
            json!({"recorded":false,"agentRequest":null,"observedAtMs":now()})
        });
    }
    if let Some(stage) =
        path.strip_prefix("/hagency/api/v1/management/agent-requests/human_request/")
        && matches!(stage, "assignment" | "matrix" | "runtime" | "execution" | "work/query" | "removal")
    {
        state.admission.stage_reads.push(stage.into());
        if stage == "work/query" { state.admission.work_queries.push(body.clone()); }
        if state.admission.invalidate_stage.as_deref() == Some(stage) {
            state
                .source
                .as_ref()
                .unwrap()
                .store(false, Ordering::Release);
        }
        return Some(
            state
                .admission
                .stage_replies
                .get(stage)
                .cloned()
                .unwrap_or_else(|| {
                    if stage == "work/query" {return json!({"work":{"items":[],"nextAfter":null},"observedAtMs":now()})}
                    let field = if stage == "matrix" {
                        "membership"
                    } else {
                        stage
                    };
                    json!({field:null,"observedAtMs":now()})
                }),
        );
    }
    if path.ends_with("/agent-requests/human_request") {
        return Some(json!({"agentRequest":state.admission.current,"observedAtMs":now()}));
    }
    assert!(
        path.ends_with("/projects/project_fixture/agent-requests")
            || path.ends_with("/agent-requests/human_request/decide")
            || path.ends_with("/agent-requests/human_request/cancel")
            || path.ends_with("/agent-requests/human_request/revoke")
    );
    if std::mem::take(&mut state.admission.lose_before) {
        return None;
    }
    if let Some(code) = state.admission.reject {
        return Some(json!({"code":code}));
    }
    let id = body["commandId"].as_str().unwrap().to_owned();
    if !state.admission.receipts.contains_key(&id) {
        let mut current = state.admission.current.clone().unwrap_or_else(projection);
        if path.ends_with("/decide") {
            current["state"] = json!(if body["decision"] == "approve" {
                "approved_waiting_executor"
            } else {
                "rejected"
            });
            current["revision"] = json!(2);
            current["canApprove"] = json!(false);
            current["canReject"] = json!(false);
            current["canDecide"] = json!(false);
        }
        if path.ends_with("/cancel") || path.ends_with("/revoke") {
            current["state"] = json!(if path.ends_with("/revoke") {"revoked"} else {"cancelled"});
            current["revision"] = json!(3);
            current["canCancel"] = json!(false);
        }
        state.admission.current = Some(current.clone());
        state.admission.receipts.insert(id, current);
        state.admission.writes += 1;
    }
    if state.admission.invalidate_reply {
        state
            .source
            .as_ref()
            .unwrap()
            .store(false, Ordering::Release);
    }
    if std::mem::take(&mut state.admission.lose_reply) {
        return None;
    }
    if state.admission.malformed {
        return Some(json!({"agentRequest":{"id":"human_request"},"observedAtMs":now()}));
    }
    Some(json!({"agentRequest":state.admission.current,"observedAtMs":now()}))
}
async fn manager(f: &Fixture, instance: u64, grants: &[Permission]) -> Arc<NativeManagement> {
    let mut p = package(instance);
    p.permissions = grants.to_vec();
    f.owner.management(p).await.unwrap()
}
fn request(id: &str) -> ManagementCommand {
    ManagementCommand::AgentRequest {
        command_id: id.into(),
        project_id: "project_fixture".into(),
        request: AgentRequestInput {
            expected_setup_revision: 2,
            expected_setup_binding_revision: 1,
            expected_authority_revision: 1,
            expected_allocation_revision: 2,
            definition: AgentDefinition {
                name: "Writer".into(),
                capability: "documentation".into(),
                description: "Draft text".into(),
                tool_mode: "none".into(),
            },
            requested_tokens: 100,
        },
    }
}
fn grants() -> Vec<Permission> {
    vec![
        Permission::ReadIntents,
        Permission::AcknowledgeIntents,
        Permission::RequestAgents,
    ]
}
#[tokio::test]
async fn native_admission_lost_reply_restart_recovers_original_latest_without_second_post() {
    let f = Fixture::new().await;
    let first = manager(&f, 10, &grants()).await;
    f.state.lock().unwrap().admission.lose_reply = true;
    assert!(first.central(request("original_agent")).await.is_err());
    assert_eq!(first.saved_commands().await.unwrap().len(), 1);
    first.retire();
    drop(first);
    {
        let mut s = f.state.lock().unwrap();
        s.admission.current.as_mut().unwrap()["state"] = json!("cancelled");
    }
    let fresh = manager(&f, 11, &grants()).await;
    let result = fresh
        .retry_saved_command("original_agent".into())
        .await
        .unwrap();
    assert_eq!(result["operation"], "agent_request");
    assert_eq!(result["data"]["agentRequest"]["state"], "cancelled");
    assert_eq!(f.state.lock().unwrap().admission.writes, 1);
    assert_eq!(
        fresh.central(request("original_agent")).await.unwrap()["agentRequest"]["state"],
        "cancelled"
    );
    assert_eq!(f.state.lock().unwrap().admission.writes, 1);
    fresh
        .acknowledge_command("original_agent".into())
        .await
        .unwrap();
    drop(fresh);
    f.stop().await;
}
#[tokio::test]
async fn native_admission_scope_loss_after_commit_keeps_original_pending_for_fresh_authority() {
    let f = Fixture::new().await;
    let first = manager(&f, 10, &grants()).await;
    f.state.lock().unwrap().admission.invalidate_reply = true;
    assert!(first.central(request("committed_agent")).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.writes, 1);
    first.retire();
    f.current.store(true, Ordering::Release);
    let fresh = manager(&f, 11, &grants()).await;
    assert_eq!(
        fresh
            .retry_saved_command("committed_agent".into())
            .await
            .unwrap()["data"]["agentRequest"]["id"],
        "human_request"
    );
    assert!(first.central(request("committed_agent")).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.writes, 1);
    drop(first);
    drop(fresh);
    f.stop().await;
}
#[tokio::test]
async fn native_admission_permission_and_unknown_failure_never_become_authority_or_terminal() {
    let f = Fixture::new().await;
    let denied = manager(
        &f,
        10,
        &[
            Permission::ReadIntents,
            Permission::DecideProjects,
            Permission::DecideInbox,
        ],
    )
    .await;
    assert!(denied.central(request("denied")).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.writes, 0);
    let allowed = manager(&f, 11, &grants()).await;
    f.state.lock().unwrap().admission.lose_before = true;
    assert!(allowed.central(request("uncertain")).await.is_err());
    f.state.lock().unwrap().admission.reject = Some("agent_offer_unavailable");
    assert_eq!(
        allowed
            .retry_saved_command("uncertain".into())
            .await
            .unwrap_err()
            .code,
        "management_outcome_unknown"
    );
    let pending = allowed.saved_commands().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].reply.is_none());
    assert!(
        allowed
            .acknowledge_command("uncertain".into())
            .await
            .is_err()
    );
    drop(denied);
    drop(allowed);
    f.stop().await;
}
#[tokio::test]
async fn native_admission_first_rejection_is_terminal_but_malformed_committed_reply_is_not() {
    let f = Fixture::new().await;
    let allowed = manager(&f, 10, &grants()).await;
    f.state.lock().unwrap().admission.reject = Some("agent_offer_unavailable");
    assert_eq!(
        allowed.central(request("rejected")).await.unwrap_err().code,
        "agent_offer_unavailable"
    );
    assert!(allowed.saved_commands().await.unwrap()[0].reply.is_some());
    allowed
        .acknowledge_command("rejected".into())
        .await
        .unwrap();
    {
        let mut s = f.state.lock().unwrap();
        s.admission.reject = None;
        s.admission.malformed = true;
    }
    assert_eq!(
        allowed
            .central(request("malformed"))
            .await
            .unwrap_err()
            .code,
        "management_agent_reply_invalid"
    );
    assert!(allowed.saved_commands().await.unwrap()[0].reply.is_none());
    assert_eq!(f.state.lock().unwrap().admission.writes, 1);
    {
        f.state.lock().unwrap().admission.malformed = false;
    }
    allowed
        .retry_saved_command("malformed".into())
        .await
        .unwrap();
    assert_eq!(f.state.lock().unwrap().admission.writes, 1);
    drop(allowed);
    f.stop().await;
}

fn status_grants() -> Vec<Permission> {
    vec![
        Permission::ReadAgentRequests,
        Permission::ReadAgentStatus,
        Permission::ReadIntents,
    ]
}
#[tokio::test]
async fn native_agent_status_preserves_separate_stage_observations_and_drops_private_receipts() {
    let f = Fixture::new().await;
    {
        let mut s = f.state.lock().unwrap();
        s.admission.current = Some(projection());
        s.admission.stage_replies.insert("assignment".into(),json!({"assignment":{"id":"assignment_a","requestId":"human_request","agentId":"agent_proposed","state":"pending_native","generation":1,"executionAvailable":false,"reserveReceiptDigest":"private receipt"},"observedAtMs":1000}));
        s.admission.stage_replies.insert("matrix".into(),json!({"membership":{"assignmentId":"assignment_b","requestId":"human_request","agentId":"agent_proposed","puppetMxid":"@_hagency_agent:test","roomId":"!project:test","state":"matrix_verified","verifiedAtMs":1800,"cleanupObservedAtMs":null,"blockingReason":null,"workAvailable":false},"observedAtMs":2000}));
        s.admission.stage_replies.insert("runtime".into(),json!({"runtime":{"assignmentId":"assignment_b","state":"qualified","processState":"live","provisionExpiresAtMs":4000,"workAvailable":false,"capacityReleased":false,"latest":{"secret":"private receipt"}},"observedAtMs":3000}));
    }
    let m = manager(&f, 40, &status_grants()).await;
    let status = m.agent_status("human_request".into()).await.unwrap();
    assert_eq!(status.assignment.observed_at_ms, Some(1000));
    assert_eq!(status.membership.observed_at_ms, Some(2000));
    assert_eq!(status.runtime.observed_at_ms, Some(3000));
    assert_eq!(status.runtime.record.as_ref().unwrap().state, "qualified");
    assert!(!status.runtime.record.as_ref().unwrap().work_available);
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("private receipt")
    );
    assert!(m.saved_commands().await.unwrap().is_empty());
    assert_eq!(f.state.lock().unwrap().admission.writes, 0);
    drop(m);
    f.stop().await;
}
#[tokio::test]
async fn native_agent_status_distinguishes_disabled_no_record_and_errors() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().admission.current = Some(projection());
    let m = manager(&f, 40, &status_grants()).await;
    let empty = m.agent_status("human_request".into()).await.unwrap();
    assert!(empty.assignment.supported && empty.assignment.record.is_none());
    f.state.lock().unwrap().admission.stage_replies.insert(
        "runtime".into(),
        json!({"code":"agent_runtime_provisioning_unavailable","fixtureStatus":503}),
    );
    let disabled = m.agent_status("human_request".into()).await.unwrap();
    assert!(!disabled.runtime.supported && disabled.runtime.observed_at_ms.is_none());
    for (code, status) in [
        ("network_unavailable", 503),
        ("owner_authorization_required", 401),
        ("agent_runtime_provisioning_unavailable", 409),
    ] {
        f.state.lock().unwrap().admission.stage_replies.insert(
            "runtime".into(),
            json!({"code":code,"fixtureStatus":status}),
        );
        assert!(m.agent_status("human_request".into()).await.is_err());
    }
    drop(m);
    f.stop().await;
}
#[tokio::test]
async fn native_agent_status_rechecks_sdk_after_every_stage_and_requires_read_grant() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().admission.current = Some(projection());
    let denied = manager(&f, 40, &[Permission::ReadAgentRequests]).await;
    assert!(denied.agent_status("human_request".into()).await.is_err());
    assert!(f.state.lock().unwrap().admission.stage_reads.is_empty());
    drop(denied);
    for (i, stage) in ["assignment", "matrix", "runtime", "execution"].into_iter().enumerate() {
        f.current.store(true, Ordering::Release);
        {
            let mut s = f.state.lock().unwrap();
            s.admission.stage_reads.clear();
            s.admission.invalidate_stage = Some(stage.into());
        }
        let m = manager(&f, 41 + i as u64, &status_grants()).await;
        assert!(m.agent_status("human_request".into()).await.is_err());
        assert_eq!(
            f.state
                .lock()
                .unwrap()
                .admission
                .stage_reads
                .last()
                .unwrap(),
            stage
        );
        assert_eq!(f.state.lock().unwrap().admission.stage_reads.len(), i + 1);
        m.retire();
        drop(m);
    }
    assert_eq!(f.state.lock().unwrap().admission.writes, 0);
    f.stop().await;
}
#[tokio::test]
async fn native_agent_status_rejects_mismatched_identity_and_missing_stage_envelope() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().admission.current = Some(projection());
    let m = manager(&f, 40, &status_grants()).await;
    for bad in [
        json!({"observedAtMs":now()}),
        json!({"assignment":{"id":"assignment","requestId":"foreign","agentId":"agent_proposed","state":"pending_native","generation":1,"executionAvailable":false},"observedAtMs":now()}),
    ] {
        f.state
            .lock()
            .unwrap()
            .admission
            .stage_replies
            .insert("assignment".into(), bad);
        assert_eq!(
            m.agent_status("human_request".into())
                .await
                .unwrap_err()
                .code,
            "management_agent_status_invalid"
        );
    }
    assert!(m.agent_status("../foreign".into()).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.writes, 0);
    drop(m);
    f.stop().await;
}
