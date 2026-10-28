use super::*;

fn execution(state: &str, available: bool) -> Value {
    json!({"execution":{"assignmentId":"assignment_original","state":state,"policy":"codex_text_v1","activityExpiresAtMs":4000,"workAvailable":available,"capacityReleased":false,"recentWork":[{"private":"private receipt"}],"token":"private receipt"},"observedAtMs":3000})
}

#[tokio::test]
async fn native_agent_status_shared_execution_is_independent_from_disabled_metadata_runtime() {
    let f = Fixture::new().await;
    {
        let mut s = f.state.lock().unwrap();
        s.admission.current = Some(projection());
        s.admission.stage_replies.insert(
            "runtime".into(),
            json!({"code":"agent_runtime_provisioning_unavailable","fixtureStatus":503}),
        );
        s.admission
            .stage_replies
            .insert("execution".into(), execution("qualified", true));
    }
    let manager = manager(&f, 40, &status_grants()).await;
    let status = manager.agent_status("human_request".into()).await.unwrap();
    assert!(!status.runtime.supported);
    let observed = status.execution.as_ref().unwrap();
    assert!(observed.supported);
    assert_eq!(observed.observed_at_ms, Some(3000));
    assert!(observed.record.as_ref().unwrap().work_available);
    assert_eq!(observed.record.as_ref().unwrap().state, "qualified");
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("private receipt")
    );
    let mut old = serde_json::to_value(status).unwrap();
    old.as_object_mut().unwrap().remove("execution");
    let old: crate::native_owner::AgentStatus = serde_json::from_value(old).unwrap();
    assert!(old.execution.is_none());
    assert_eq!(f.state.lock().unwrap().admission.writes, 0);
    assert!(manager.saved_commands().await.unwrap().is_empty());
    drop(manager);
    f.stop().await;
}

#[tokio::test]
async fn native_agent_status_shared_policy_off_no_record_and_unknown_remain_distinct() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().admission.current = Some(projection());
    let manager = manager(&f, 40, &status_grants()).await;
    let status = manager.agent_status("human_request".into()).await.unwrap();
    let empty = status.execution.unwrap();
    assert!(empty.supported && empty.record.is_none() && empty.observed_at_ms.is_some());
    f.state.lock().unwrap().admission.stage_replies.insert(
        "execution".into(),
        json!({"code":"shared_execution_unavailable","fixtureStatus":503}),
    );
    let disabled = manager
        .agent_status("human_request".into())
        .await
        .unwrap()
        .execution
        .unwrap();
    assert!(!disabled.supported && disabled.record.is_none() && disabled.observed_at_ms.is_none());
    for state in [
        "pending",
        "issued",
        "qualified",
        "stopped",
        "outcome_unknown",
        "cleanup_pending",
    ] {
        f.state
            .lock()
            .unwrap()
            .admission
            .stage_replies
            .insert("execution".into(), execution(state, false));
        let record = manager
            .agent_status("human_request".into())
            .await
            .unwrap()
            .execution
            .unwrap()
            .record
            .unwrap();
        assert_eq!(record.state, state);
        assert!(!record.work_available && !record.capacity_released);
    }
    for (code, status) in [
        ("network_unavailable", 503),
        ("shared_execution_unavailable", 409),
        ("owner_authorization_required", 401),
        ("not_found", 404),
    ] {
        f.state.lock().unwrap().admission.stage_replies.insert(
            "execution".into(),
            json!({"code":code,"fixtureStatus":status}),
        );
        assert!(manager.agent_status("human_request".into()).await.is_err());
    }
    assert_eq!(f.state.lock().unwrap().admission.writes, 0);
    drop(manager);
    f.stop().await;
}

#[tokio::test]
async fn native_agent_status_shared_rejects_contradictory_or_unbounded_execution_projection() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().admission.current = Some(projection());
    let manager = manager(&f, 40, &status_grants()).await;
    for (field, bad) in [
        ("policy", json!("codex_no_tools_v1")),
        ("state", json!("running")),
        ("state", json!("stopped")),
        ("activityExpiresAtMs", json!(9007199254740992u64)),
        ("activityExpiresAtMs", Value::Null),
        ("capacityReleased", json!(true)),
        ("assignmentId", json!("../foreign")),
    ] {
        let mut reply = execution("qualified", true);
        reply["execution"][field] = bad;
        f.state
            .lock()
            .unwrap()
            .admission
            .stage_replies
            .insert("execution".into(), reply);
        assert_eq!(
            manager
                .agent_status("human_request".into())
                .await
                .unwrap_err()
                .code,
            "management_agent_status_invalid"
        );
    }
    assert_eq!(f.state.lock().unwrap().admission.writes, 0);
    drop(manager);
    f.stop().await;
}
