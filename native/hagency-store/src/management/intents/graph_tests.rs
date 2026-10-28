use super::*;
use serde_json::json;

#[test]
fn graph_intent_custody_preserves_large_fractional_and_opaque_values() {
    for (operation, size) in [
        (IntentOperation::GraphCreate, 64 * 1024),
        (IntentOperation::GraphUpdateNode, 128 * 1024),
        (IntentOperation::GraphBindRecipient, 32 * 1024),
    ] {
        let input = json!({"commandId":"original","opaque":"x".repeat(size),"condition":{"threshold":0.125},"result":[null,{"opaque":{"nested":1.75}}]});
        let original = Intent::new(operation, "original".into(), &input).unwrap();
        assert_eq!(original.input, input);
        let encoded = serde_json::to_vec(&original.input).unwrap();
        let restored: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            Intent::new(operation, "original".into(), &restored)
                .unwrap()
                .digest,
            original.digest
        );
        let mut changed = input.clone();
        changed["result"][1]["opaque"]["nested"] = json!(1.76);
        assert_ne!(
            Intent::new(operation, "original".into(), &changed)
                .unwrap()
                .digest,
            original.digest
        );
        let mut removed = input;
        removed.as_object_mut().unwrap().remove("result");
        assert_ne!(
            Intent::new(operation, "original".into(), &removed)
                .unwrap()
                .digest,
            original.digest
        );
        assert!(
            Intent::new(
                operation,
                "original".into(),
                &json!({"commandId":"original","opaque":"x".repeat(operation.input_limit())})
            )
            .is_err()
        );
    }
}

#[test]
fn every_old_intent_retains_original_integer_digest_and_input_limit() {
    use IntentOperation::*;
    let operations = [
        EnrollResource,
        EnrollAccountResource,
        ConfigureResource,
        EditLocalResource,
        CreateResourceConfiguration,
        RenameResource,
        TriageResourceAlert,
        RetireApprovalBinding,
        RevokeToolGrant,
        EditExecutionPolicy,
        ClearWorkspaceDirty,
        ReviewStopped,
        ResolveStopped,
        PrepareAccount,
        RetireAccount,
        LoginAccount,
        TaskCreate,
        TaskEdit,
        TaskDelete,
        TaskAccept,
        TaskTransition,
        TaskComment,
        PublishResource,
        Contribute,
        Reoffer,
        Decide,
        Revoke,
        Verify,
        Resume,
        ProjectRequest,
        ProjectDecide,
        ProjectRevoke,
        AgentRequest,
        AgentDecide,
        AgentCancel,
        AgentRevoke,
        AgentSettlementRequest,
        AgentTopUpRequest,
        AgentTopUpDecide,
        AgentTopUpCancel,
        AgentRunControl,
        AllocationDecide,
        AllocationRelease,
        PublishGrant,
        PersonalAgentPolicy,
        PersonalAgentObservation,
    ];
    for operation in operations {
        let input = json!({"commandId":"original","receipt":{"grantId":"grant"},"value":17});
        let intent = Intent::new(operation, "original".into(), &input).unwrap();
        assert_eq!(
            intent.digest,
            hagency_core::canonical::digest(&json!({"operation":operation,"input":input})).unwrap(),
            "{operation:?}"
        );
        assert_eq!(operation.input_limit(), 16 * 1024);
        let mut fractional = input.clone();
        fractional["value"] = json!(0.125);
        assert!(
            Intent::new(operation, "original".into(), &fractional).is_err(),
            "{operation:?}"
        );
        let mut large = input;
        large["padding"] = json!("x".repeat(16 * 1024));
        assert!(
            Intent::new(operation, "original".into(), &large).is_err(),
            "{operation:?}"
        );
    }
}

#[test]
fn graph_intent_queue_reserves_all_eight_large_inputs_and_replies() {
    let context = HostContext::new(
        "@owner:test".into(),
        1,
        "a".repeat(64),
        1,
        "device".into(),
        1,
    )
    .unwrap();
    let authority = ManagementAuthority::new(
        context.clone(),
        [
            Permission::ReadIntents,
            Permission::UpdateOperatorGraphNodes,
        ],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    let read = JournalCommand::new(authority.clone(), context.clone(), JournalAction::Read);
    assert!(read.weight() as usize >= 8 * (MAX_INTENT_INPUT + 64 * 1024));
    assert!((read.weight() as usize) < MAX_INTENT_OUTPUT);
    let original = Intent::new(
        IntentOperation::GraphUpdateNode,
        "large".into(),
        &json!({"commandId":"large","padding":"x".repeat(128*1024),"result":0.25}),
    )
    .unwrap();
    let write = JournalCommand::new(authority, context, JournalAction::Prepare(original));
    assert!(write.weight() as usize >= MAX_INTENT_INPUT + 64 * 1024);
}

#[test]
fn personal_edits_have_exact_grants_and_only_rules_get_larger_custody() {
    assert_eq!(IntentOperation::PersonalAgentModel.permission(),Permission::EditPersonalAgentModel);
    assert_eq!(IntentOperation::PersonalAgentWorkspace.permission(),Permission::PreparePersonalAgentWorkspace);
    assert_eq!(IntentOperation::PersonalAgentRules.permission(),Permission::EditPersonalAgentPolicy);
    let input=json!({"commandId":"rules","directories":["x".repeat(60000)]});
    let rules=Intent::new(IntentOperation::PersonalAgentRules,"rules".into(),&input).unwrap();
    assert_eq!(rules.input,input);
    for operation in [IntentOperation::PersonalAgentPolicy,IntentOperation::PersonalAgentModel,IntentOperation::PersonalAgentWorkspace] {
        assert!(Intent::new(operation,"rules".into(),&input).is_err());
    }
    let conn=rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE limits(operation TEXT)").unwrap();
    conn.execute("INSERT INTO limits VALUES (?)",[serde_json::to_string(&IntentOperation::PersonalAgentRules).unwrap()]).unwrap();
    let limit:usize=conn.query_row(&format!("SELECT {INPUT_LIMIT_SQL} FROM limits"),[],|row|row.get(0)).unwrap();
    assert_eq!(limit,IntentOperation::PersonalAgentRules.input_limit());
    let oversized=json!({"commandId":"rules","directories":["x".repeat(limit)]});
    assert!(Intent::new(IntentOperation::PersonalAgentRules,"rules".into(),&oversized).is_err());
}
