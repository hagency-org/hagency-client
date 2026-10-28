use super::*;
fn budget() -> Value {
    json!({"requestId":"request","requestRevision":3,"assignmentGeneration":2,"assignmentId":"assignment","agentReservationId":"reservation","originalCeilingTokens":4096,"appliedDeltaTokens":1024,"effectiveCeilingTokens":5120,"budgetRevision":2,"budgetDigest":"b".repeat(64),"knownConsumedTokens":4813,"conservativeCommitmentTokens":6144,"nativeRetainedTokens":null,"nativeRemainingTokens":null,"usageComplete":null,"executionSupported":false,"canRequest":false,"blockingReason":"agent_budget_issuance_unavailable","privateReceipt":"secret"})
}
fn topup() -> Value {
    json!({"id":"topup","authorityRevision":4,"requestId":"request","assignmentId":"assignment","addTokens":1024,"purpose":"Continue the task","expectedBudgetRevision":1,"expectedBudgetDigest":"a".repeat(64),"expectedCeilingTokens":4096,"revision":2,"state":"approved","nativeState":"unknown","executorCommandId":"executor","createdAtMs":100,"expiresAtMs":200,"canDecide":false,"canCancel":true})
}
fn request() -> ManagementCommand {
    ManagementCommand::AgentTopUpRequest { approve_immediately: false, owner_funded: false,
        command_id: "original".into(),
        request_id: "request".into(),
        expected_request_revision: 3,
        expected_assignment_generation: 2,
        expected_budget_revision: 1,
        expected_budget_digest: "a".repeat(64),
        add_tokens: 1024,
        purpose: "Continue the task".into(),
    }
}
#[test]
fn human_budget_closed_routes_and_all_revision_fences() {
    let (_, path, input) = prepare(&request()).unwrap().request().unwrap();
    assert_eq!(
        path,
        "/hagency/api/v1/management/agent-requests/request/topups"
    );
    assert_eq!(
        input.unwrap(),
        json!({"commandId":"original","expectedRequestRevision":3,"expectedAssignmentGeneration":2,"expectedBudgetRevision":1,"expectedBudgetDigest":"a".repeat(64),"addTokens":1024,"purpose":"Continue the task"})
    );
    for n in [0, 17] {
        assert!(
            prepare(&ManagementCommand::AgentTopUps {
                id: "request".into(),
                after: None,
                limit: Some(n)
            })
            .is_err()
        );
    }
    assert!(
        prepare(&ManagementCommand::AgentTopUp {
            id: "../escape".into()
        })
        .is_err()
    );
    let mut v = serde_json::to_value(request()).unwrap();
    v["author"] = json!("forged");
    assert!(serde_json::from_value::<ManagementCommand>(v).is_err());
}
#[test]
fn owner_funding_is_explicit_on_wire_and_preserved_in_views() {
    let mut command=request();
    assert!(serde_json::to_value(&command).unwrap().get("ownerFunded").is_none());
    if let ManagementCommand::AgentTopUpRequest {owner_funded,..}=&mut command {*owner_funded=true;}
    let (_,_,input)=prepare(&command).unwrap().request().unwrap();
    assert_eq!(input.unwrap()["ownerFunded"],true);
    let mut b=budget();b["canOwnerFund"]=json!(true);
    let view=project_reply(&ManagementCommand::AgentBudget{id:"request".into()},json!({"budget":b,"observedAtMs":123})).unwrap();
    assert_eq!(view["budget"]["canOwnerFund"],true);
    let mut t=topup();t["ownerFunded"]=json!(true);
    let view=project_reply(&ManagementCommand::AgentTopUp{id:"topup".into()},json!({"topUp":t,"observedAtMs":123})).unwrap();
    assert_eq!(view["topUp"]["ownerFunded"],true);
}
#[test]
fn human_budget_projection_preserves_overrun_and_unknown_accounting() {
    let c = ManagementCommand::AgentBudget {
        id: "request".into(),
    };
    let mut b = budget();
    b["appliedDeltaTokens"] = json!(0);
    b["effectiveCeilingTokens"] = json!(4096);
    let v = project_reply(&c, json!({"budget":b,"observedAtMs":123})).unwrap();
    assert_eq!(v["budget"]["knownConsumedTokens"], 4813);
    assert!(v["budget"]["nativeRemainingTokens"].is_null());
    assert!(!v.to_string().contains("secret"));
    for (field, value) in [
        ("nativeRemainingTokens", json!(307)),
        ("nativeRetainedTokens", json!(4813)),
        ("usageComplete", json!(true)),
        ("executionSupported", json!(true)),
        ("effectiveCeilingTokens", json!(999)),
        ("knownConsumedTokens", json!(MAX_EXACT + 1)),
    ] {
        let mut b = budget();
        b[field] = value;
        assert!(
            project_reply(&c, json!({"budget":b,"observedAtMs":123})).is_err(),
            "{field}"
        );
    }
}
#[test]
fn human_budget_page_is_bounded_ordered_and_exact_request() {
    let c = ManagementCommand::AgentTopUps {
        id: "request".into(),
        after: None,
        limit: Some(1),
    };
    assert!(
        project_reply(
            &c,
            json!({"topUps":{"items":[topup()],"nextAfter":"topup"},"observedAtMs":123})
        )
        .is_ok()
    );
    for (items, next) in [
        (json!([topup(), topup()]), Value::Null),
        (json!([topup()]), json!("other")),
    ] {
        assert!(
            project_reply(
                &c,
                json!({"topUps":{"items":items,"nextAfter":next},"observedAtMs":123})
            )
            .is_err()
        );
    }
    let mut t = topup();
    t["requestId"] = json!("other");
    assert!(
        project_reply(
            &c,
            json!({"topUps":{"items":[t],"nextAfter":null},"observedAtMs":123})
        )
        .is_err()
    );
}
#[test]
fn human_budget_recovery_binds_original_ack_and_keeps_latest_state() {
    let c = ManagementCommand::Recover {
        command_id: "original".into(),
        original_operation: IntentOperation::AgentTopUpRequest,
    };
    let mut v = json!({"recorded":true,"result":{"receipt":{"commandId":"original","operation":"agent.topup.request","topUpId":"topup","appliedRevision":1},"topUp":topup()},"observedAtMs":123});
    let result = project_reply(&c, v.clone()).unwrap();
    assert_eq!(result["topUp"]["state"], "approved");
    assert_eq!(result["receipt"]["appliedRevision"], 1);
    v["result"]["receipt"]["commandId"] = json!("replacement");
    assert!(project_reply(&c, v).is_err());
    assert!(
        project_reply(
            &c,
            json!({"recorded":false,"result":null,"observedAtMs":123})
        )
        .is_ok()
    );
    assert!(
        project_reply(
            &c,
            json!({"recorded":false,"result":{"topUp":topup()},"observedAtMs":123})
        )
        .is_err()
    );
}
#[test]
fn human_budget_ambiguous_failure_never_discards_pending() {
    for (status, code) in [
        (401, "management_scope_changed"),
        (403, "management_permission_required"),
        (404, "agent_topup_not_found"),
        (409, "management_command_conflict"),
        (503, "management_agent_budget_invalid"),
        (503, "transport_failed"),
    ] {
        assert!(!known_failure(&failure(status, code)), "{code}");
    }
    assert!(known_failure(&failure(409, "management_revision_changed")));
    assert!(known_failure(&failure(
        503,
        "agent_budget_issuance_unavailable"
    )));
}
