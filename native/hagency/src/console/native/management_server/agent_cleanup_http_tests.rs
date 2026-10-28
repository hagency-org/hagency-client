use super::*;
fn removal() -> ManagementCommand {
    ManagementCommand::AgentRemoval {
        id: "human_request".into(),
    }
}
fn revoke(id: &str) -> ManagementCommand {
    ManagementCommand::AgentRevoke {
        command_id: id.into(),
        request_id: "human_request".into(),
        expected_revision: 1,
    }
}
fn grants() -> Vec<Permission> {
    vec![
        Permission::ReadAgentCleanup,
        Permission::RevokeAgentRequests,
        Permission::ReadIntents,
        Permission::AcknowledgeIntents,
    ]
}
fn detail() -> Value {
    json!({"removal":{"requestId":"human_request","projectId":"project_fixture","projectName":"Project","proposedAgentId":"agent_proposed","agentName":"Writer","requestRevision":7,"admissionState":"revoked","canCancel":false,"canRevoke":false,"centralFenced":true,"nativeFence":{"deliveryState":"pending","nativeState":null,"ledgerRevision":null,"processState":null},"processObservation":{"kind":"shared_text","state":"live","ledgerRevision":12},"matrix":{"state":"cleanup_pending","cleanupObservedAtMs":null},"usage":{"periodKey":"2026-10","knownConsumedTokens":null,"retainedTokensFloor":100,"fenceUsageComplete":null,"requiresReconciliation":false},"receipt":"private"},"observedAtMs":1000,"token":"private"})
}
#[tokio::test]
async fn native_agent_cleanup_read_keeps_unknown_accounting_and_independent_stages() {
    let f = Fixture::new().await;
    let m = manager(&f, 80, &grants()).await;
    f.state
        .lock()
        .unwrap()
        .admission
        .stage_replies
        .insert("removal".into(), detail());
    let r = m.central(removal()).await.unwrap();
    assert_eq!(r["removal"]["usage"]["knownConsumedTokens"], Value::Null);
    assert_eq!(r["removal"]["usage"]["retainedTokensFloor"], 100);
    assert_eq!(r["removal"]["usage"]["fenceUsageComplete"], Value::Null);
    assert_eq!(r["removal"]["processObservation"]["state"], "live");
    assert!(!r.to_string().contains("private"));
    assert!(m.saved_commands().await.unwrap().is_empty());
    let mut bad = detail();
    bad["removal"]["requestId"] = json!("foreign");
    f.state
        .lock()
        .unwrap()
        .admission
        .stage_replies
        .insert("removal".into(), bad);
    assert_eq!(
        m.central(removal()).await.unwrap_err().code,
        "management_agent_cleanup_invalid"
    );
    let mut bad = detail();
    bad["removal"]["usage"]["knownConsumedTokens"] = json!(101);
    f.state
        .lock()
        .unwrap()
        .admission
        .stage_replies
        .insert("removal".into(), bad);
    assert!(m.central(removal()).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.writes, 0);
    drop(m);
    f.stop().await;
}
#[tokio::test]
async fn native_agent_cleanup_page_is_bounded_scoped_and_rechecks_sdk() {
    let f = Fixture::new().await;
    let m = manager(&f, 81, &grants()).await;
    let query = || ManagementCommand::AgentRemovals {
        project_id: Some("project_fixture".into()),
        after: Some("request_a".into()),
        limit: Some(1),
    };
    let page = json!({"requests":{"items":[{"requestId":"request_b","projectId":"project_fixture","projectName":"Project","agentName":"Writer","private":"secret"}],"nextAfter":"request_b"},"observedAtMs":1000});
    f.state
        .lock()
        .unwrap()
        .admission
        .stage_replies
        .insert("removals".into(), page.clone());
    let r = m.central(query()).await.unwrap();
    assert_eq!(r["requests"]["nextAfter"], "request_b");
    assert!(!r.to_string().contains("secret"));
    assert_eq!(
        f.state.lock().unwrap().admission.work_queries[0],
        json!({"projectId":"project_fixture","after":"request_a","limit":1})
    );
    let mut bad = page;
    bad["requests"]["items"][0]["projectId"] = json!("foreign");
    f.state
        .lock()
        .unwrap()
        .admission
        .stage_replies
        .insert("removals".into(), bad);
    assert!(m.central(query()).await.is_err());
    f.state.lock().unwrap().admission.invalidate_stage = Some("removals".into());
    assert!(m.central(query()).await.is_err());
    drop(m);
    f.stop().await;
}
#[tokio::test]
async fn native_agent_cleanup_revoke_recovers_original_after_commit_and_retirement() {
    let f = Fixture::new().await;
    let m = manager(&f, 82, &grants()).await;
    f.state.lock().unwrap().admission.invalidate_reply = true;
    assert!(m.central(revoke("original_revoke")).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.writes, 1);
    m.retire();
    f.current.store(true, Ordering::Release);
    let fresh = manager(&f, 83, &grants()).await;
    let result = fresh
        .retry_saved_command("original_revoke".into())
        .await
        .unwrap();
    assert_eq!(result["operation"], "agent_revoke");
    assert_eq!(result["data"]["agentRequest"]["state"], "revoked");
    assert_eq!(f.state.lock().unwrap().admission.writes, 1);
    assert!(m.central(revoke("original_revoke")).await.is_err());
    fresh
        .acknowledge_command("original_revoke".into())
        .await
        .unwrap();
    drop(m);
    drop(fresh);
    f.stop().await;
}
#[tokio::test]
async fn native_agent_cleanup_permissions_and_uncertainty_do_not_allow_new_authority() {
    let f = Fixture::new().await;
    let denied = manager(
        &f,
        84,
        &[
            Permission::CancelAgentRequests,
            Permission::DecideAgentRequests,
            Permission::ReadAgentStatus,
        ],
    )
    .await;
    assert!(denied.central(revoke("denied")).await.is_err());
    assert!(denied.central(removal()).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.writes, 0);
    drop(denied);
    let m = manager(&f, 85, &grants()).await;
    f.state.lock().unwrap().admission.reject = Some("assigned_project_administrator_required");
    assert_eq!(
        m.central(revoke("rejected")).await.unwrap_err().code,
        "assigned_project_administrator_required"
    );
    m.acknowledge_command("rejected".into()).await.unwrap();
    {
        let mut s = f.state.lock().unwrap();
        s.admission.reject = None;
        s.admission.lose_before = true;
    }
    assert!(m.central(revoke("uncertain")).await.is_err());
    f.state.lock().unwrap().admission.reject = Some("agent_request_closed");
    assert_eq!(
        m.retry_saved_command("uncertain".into())
            .await
            .unwrap_err()
            .code,
        "management_outcome_unknown"
    );
    assert!(m.saved_commands().await.unwrap()[0].reply.is_none());
    assert!(m.acknowledge_command("uncertain".into()).await.is_err());
    drop(m);
    f.stop().await;
}
