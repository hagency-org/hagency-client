use super::*;
pub(in crate::console::native) fn room_state() -> Value {
    json!([
        {"type":"m.room.create","state_key":"","event_id":"$create","sender":"@owner:test","content":{"creator":"@owner:test"}},
        {"type":"m.room.member","state_key":"@owner:test","event_id":"$owner","content":{"membership":"join"}},
        {"type":"m.room.member","state_key":"@personal:test","event_id":"$agent","content":{"membership":"join"}},
        {"type":"m.room.member","state_key":"@human:test","event_id":"$human","content":{"membership":"join","displayname":"Human"}},
        {"type":"m.room.power_levels","state_key":"","event_id":"$power","content":{"users":{"@owner:test":50},"events_default":0}}
    ])
}
#[test]
fn current_matrix_identity_excludes_unrelated_events_and_detects_membership_power_changes() {
    let mut state = room_state();
    let (identity, blockers) =
        recipients::matrix_proof(&state, "@owner:test", "@human:test").unwrap();
    assert!(blockers.is_empty());
    state
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"m.room.topic","state_key":"","content":{"topic":"Changed"}}));
    assert_eq!(
        recipients::matrix_proof(&state, "@owner:test", "@human:test")
            .unwrap()
            .0,
        identity
    );
    state[3]["event_id"] = json!("$human2");
    assert_ne!(
        recipients::matrix_proof(&state, "@owner:test", "@human:test")
            .unwrap()
            .0,
        identity
    );
    state[3]["content"]["membership"] = json!("leave");
    assert!(
        recipients::matrix_proof(&state, "@owner:test", "@human:test")
            .unwrap()
            .1
            .contains(&"recipient_not_joined".into())
    );
    state[4]["content"]["events_default"] = json!(100);
    assert!(
        recipients::matrix_proof(&state, "@owner:test", "@human:test")
            .unwrap()
            .1
            .contains(&"sender_cannot_send".into())
    );
    let duplicate = state[3].clone();
    state.as_array_mut().unwrap().push(duplicate);
    assert!(recipients::matrix_proof(&state, "@owner:test", "@human:test").is_err());
}
#[test]
fn encrypted_room_identity_is_bindable_but_uses_encrypted_send_power() {
    let mut state = room_state();
    state.as_array_mut().unwrap().push(json!({"type":"m.room.encryption","state_key":"","event_id":"$encryption","content":{"algorithm":"m.megolm.v1.aes-sha2"}}));
    let (proof, blockers) = recipients::matrix_proof(&state, "@owner:test", "@human:test").unwrap();
    assert!(blockers.is_empty());
    assert_eq!(proof["eventType"], "m.room.encrypted");
    assert_eq!(proof["encryption"]["eventId"], "$encryption");
    state[4]["content"]["events"] = json!({"m.room.message":0,"m.room.encrypted":100});
    assert!(
        recipients::matrix_proof(&state, "@owner:test", "@human:test")
            .unwrap()
            .1
            .contains(&"sender_cannot_send".into())
    );
}
#[test]
fn selections_are_closed_bounded_and_picker_pages_preserve_exact_identity() {
    let valid = json!({"id":"graph_1","nodeId":"n","kind":"personal_agent","agentId":"a"});
    assert!(
        serde_json::from_value::<OperatorGraphRecipientOptionsQuery>(valid.clone())
            .unwrap()
            .validate()
            .is_ok()
    );
    let mut wrong = valid;
    wrong["roomId"] = json!("!room:test");
    assert!(
        serde_json::from_value::<OperatorGraphRecipientOptionsQuery>(wrong)
            .unwrap()
            .validate()
            .is_err()
    );
    assert!(serde_json::from_value::<OperatorGraphRecipientObserve>(json!({"id":"g","nodeId":"n","target":{"kind":"human","projectId":"p","roomId":"!r:test","userId":"@u:test","accessToken":"forbidden"}})).is_err());
    let rows=(0..20).map(|n|json!({"id":format!("node_{n:02}"),"target":{"kind":"human","projectId":"p","roomId":"!r:test","userId":format!("@user{n}:test")}})).collect::<Vec<_>>();
    let (first, next) = option_page(rows.clone(), None, None).unwrap();
    assert_eq!(first.len(), 16);
    assert_eq!(next.as_deref(), Some("node_15"));
    let (second, next) = option_page(rows, Some("node_15"), None).unwrap();
    assert_eq!(second.len(), 4);
    assert!(next.is_none());
}
#[test]
fn picker_cursor_validation_matches_the_selected_stage_before_authorization() {
    let stages = [
        (json!({"kind":"shared_agent"}), "project_1", "!room:test"),
        (
            json!({"kind":"shared_agent","projectId":"project_1"}),
            "request_1",
            "@user:test",
        ),
        (json!({"kind":"personal_agent"}), "agent_1", "agent/1"),
        (
            json!({"kind":"personal_agent","agentId":"agent_1"}),
            "binding_1",
            "binding 1",
        ),
        (json!({"kind":"human"}), "project_1", "!room:test"),
        (
            json!({"kind":"human","projectId":"project_1"}),
            "!room:test",
            "@user:test",
        ),
        (
            json!({"kind":"human","projectId":"project_1","roomId":"!room:test"}),
            "@user:test",
            "!room:test",
        ),
    ];
    for (mut input, valid, invalid) in stages {
        input["id"] = json!("graph_1");
        input["nodeId"] = json!("node_1");
        input["after"] = json!(valid);
        assert!(
            serde_json::from_value::<OperatorGraphRecipientOptionsQuery>(input.clone())
                .unwrap()
                .validate()
                .is_ok(),
            "{input}"
        );
        input["after"] = json!(invalid);
        assert!(
            serde_json::from_value::<OperatorGraphRecipientOptionsQuery>(input.clone())
                .unwrap()
                .validate()
                .is_err(),
            "{input}"
        );
    }
    let oversized = json!({"id":"g","nodeId":"n","kind":"shared_agent","after":"a".repeat(129)});
    assert!(
        serde_json::from_value::<OperatorGraphRecipientOptionsQuery>(oversized)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn personal_binding_uses_owner_and_exact_generation_not_names() {
    let mut agent = json!({"id":"a","ownerUserId":"uid","puppetMxid":"@personal:test","state":"active","generation":1,"executionDeviceId":null,"ownerDirectRoomId":"!r:test"});
    let binding = json!({"id":"b","agentId":"a","scopeKind":"owner_direct","roomId":"!r:test","state":"active","generation":1});
    let mut blockers = Vec::new();
    let old = personal_identity("uid", "a", "b", &agent, &binding, &mut blockers).unwrap();
    assert!(blockers.is_empty());
    agent["generation"] = json!(2);
    assert_ne!(
        old.0,
        personal_identity("uid", "a", "b", &agent, &binding, &mut blockers)
            .unwrap()
            .0
    );
    assert!(personal_identity("other", "a", "b", &agent, &binding, &mut blockers).is_err());
    let mut revoked = binding;
    revoked["state"] = json!("revoked");
    personal_identity("uid", "a", "b", &agent, &revoked, &mut blockers).unwrap();
    assert!(blockers.contains(&"personal_binding_inactive".into()));
}
fn shared() -> (super::super::ProjectReadiness, super::super::AgentStatus) {
    let readiness=serde_json::from_value(json!({"projectId":"p","administrator":{"mxid":"@admin:test","revision":1},"setup":null,"eligibleAllocation":null,"capacityReady":false,"matrixReady":false,"canAssignAdministrator":false,"canBegin":false,"canConfirm":false,"canCancel":false,"canRebase":false,"blockingReason":null,"observedAtMs":1})).unwrap();
    let request = json!({"id":"r","projectId":"p","proposedAgentId":"a","managerMxid":"@manager:test","administratorMxid":"@admin:test","authorityRevision":1,"definition":{"name":"Name is not authority","capability":"coding","description":"Work","toolMode":"none"},"definitionDigest":"a".repeat(64),"requestedTokens":100,"periodKey":"2026-10","state":"approved","revision":1,"createdAtMs":1,"expiresAtMs":1000,"canDecide":false,"canApprove":false,"canReject":false,"canCancel":false,"blockingReason":null,"executionAvailable":false});
    let stage = json!({"supported":false,"observedAtMs":null,"record":null});
    let status=serde_json::from_value(json!({"protocol":"hagency-management-v1","requestObservedAtMs":1,"agentRequest":request,"assignment":stage,"membership":stage,"runtime":stage,"execution":null})).unwrap();
    (readiness, status)
}
#[test]
fn shared_administrator_visibility_never_grants_manager_ingress() {
    let (mut readiness, mut status) = shared();
    let mut admin = Vec::new();
    shared_identity("@admin:test", "p", "r", &readiness, &status, &mut admin).unwrap();
    assert!(admin.contains(&"shared_project_manager_required".into()));
    let mut manager = Vec::new();
    let first =
        shared_identity("@manager:test", "p", "r", &readiness, &status, &mut manager).unwrap();
    assert!(!manager.contains(&"shared_project_manager_required".into()));
    assert!(manager.contains(&"shared_membership_unavailable".into()));
    readiness.observed_at_ms = 999;
    status.request_observed_at_ms = 999;
    let second = shared_identity(
        "@manager:test",
        "p",
        "r",
        &readiness,
        &status,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(first.0, second.0);
    status.agent_request.revision += 1;
    assert_ne!(
        first.0,
        shared_identity(
            "@manager:test",
            "p",
            "r",
            &readiness,
            &status,
            &mut Vec::new()
        )
        .unwrap()
        .0
    );
}
