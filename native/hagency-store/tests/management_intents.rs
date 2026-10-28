use hagency_store::{DomainRepository, DomainStore, Error, management::*};
use serde_json::json;
use std::time::{Duration, Instant};

fn context(account: &str, epoch: u64, generation: u64) -> HostContext {
    HostContext::new(
        account.into(),
        epoch,
        "a".repeat(64),
        epoch,
        "executor".into(),
        generation,
    )
    .unwrap()
}
fn session(store: &DomainStore, ctx: &HostContext, permissions: &[Permission]) -> Session {
    Session::new(
        store.clone(),
        ctx.clone(),
        permissions.iter().copied(),
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}
const GRANTS: &[Permission] = &[
    Permission::EnrollResources,
    Permission::ReadIntents,
    Permission::AcknowledgeIntents,
];
fn intent(id: &str, tokens: u64) -> Intent {
    Intent::new(
        IntentOperation::EnrollResource,
        id.into(),
        &json!({"commandId":id,"runtime":"codex","modelId":"gpt-5.6-sol","monthlyTokens":tokens}),
    )
    .unwrap()
}

#[tokio::test]
async fn custody_survives_restart_and_requires_known_reply_before_acknowledgement() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("ledger");
    let store = DomainStore::start(DomainRepository::open(&state).unwrap(), 8).unwrap();
    let ctx = context("@owner:test", 1, 1);
    let host = session(&store, &ctx, GRANTS);
    let first = host
        .prepare_intent(&ctx, intent("stable", 5000))
        .await
        .unwrap();
    assert!(first.reply.is_none());
    assert!(first.is_first_attempt());
    assert!(
        !host
            .prepare_intent(&ctx, intent("stable", 5000))
            .await
            .unwrap()
            .is_first_attempt()
    );
    assert!(
        matches!(
            host.acknowledge_intent(&ctx, "stable".into()).await,
            Err(Error::Conflict)
        ),
        "an unknown outcome cannot be forgotten"
    );
    assert!(matches!(
        host.prepare_intent(&ctx, intent("stable", 9000)).await,
        Err(Error::Conflict)
    ));
    host.retire();
    store.shutdown().await.unwrap();
    drop(host);
    drop(store);

    let store = DomainStore::start(DomainRepository::open(&state).unwrap(), 8).unwrap();
    let ctx = context("@owner:test", 2, 2);
    let host = session(&store, &ctx, GRANTS);
    let recovered = host.pending_intents(&ctx).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].command_id, "stable");
    assert_eq!(recovered[0].input["monthlyTokens"], 5000);
    assert!(recovered[0].reply.is_none());
    assert!(!recovered[0].is_first_attempt());
    let reply = IntentReply::Success(json!({"resourceId":"original_resource"}));
    host.record_intent_reply(&ctx, intent("stable", 5000), reply.clone())
        .await
        .unwrap();
    assert_eq!(
        host.prepare_intent(&ctx, intent("stable", 5000))
            .await
            .unwrap()
            .reply,
        Some(reply)
    );
    assert!(
        host.record_intent_reply(
            &ctx,
            intent("stable", 5000),
            IntentReply::Failure {
                status: 409,
                code: "different_outcome".into()
            }
        )
        .await
        .is_err()
    );
    host.acknowledge_intent(&ctx, "stable".into())
        .await
        .unwrap();
    host.acknowledge_intent(&ctx, "stable".into())
        .await
        .unwrap();
    assert!(host.pending_intents(&ctx).await.unwrap().is_empty());
    assert!(
        host.prepare_intent(&ctx, intent("stable", 5000))
            .await
            .is_err(),
        "acknowledged IDs remain tombstones"
    );
    let db = rusqlite::Connection::open(state.join("domain.sqlite3")).unwrap();
    let row: (Option<String>, Option<String>, String) = db
        .query_row(
            "SELECT input,reply,state FROM management_intents",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, (None, None, "acknowledged".into()));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn custody_recovery_requires_current_account_device_generation_and_operation_grant() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let store = DomainStore::start(
        DomainRepository::open(&root.path().join("ledger")).unwrap(),
        8,
    )
    .unwrap();
    let ctx = context("@owner:test", 1, 1);
    let host = session(&store, &ctx, GRANTS);
    host.prepare_intent(&ctx, intent("scoped", 1))
        .await
        .unwrap();
    let other_device = HostContext::new(
        "@owner:test".into(),
        1,
        "a".repeat(64),
        1,
        "another-executor".into(),
        1,
    )
    .unwrap();
    for other in [context("@other:test", 1, 1), other_device] {
        assert!(
            session(&store, &other, GRANTS)
                .pending_intents(&other)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(host.pending_intents(&other).await.is_err());
    }
    let renewed = context("@owner:test", 2, 2);
    assert!(
        host.pending_intents(&renewed).await.is_err(),
        "old sessions cannot adopt a newer generation"
    );
    assert_eq!(
        session(&store, &renewed, GRANTS)
            .pending_intents(&renewed)
            .await
            .unwrap()
            .len(),
        1,
        "freshly authorized same installation can recover its original request"
    );
    let reader = session(&store, &ctx, &[Permission::ReadIntents]);
    assert!(
        reader.pending_intents(&ctx).await.unwrap().is_empty(),
        "recovery does not imply the original operation permission"
    );
    assert!(
        reader
            .prepare_intent(&ctx, intent("denied", 1))
            .await
            .is_err()
    );
    host.retire();
    assert!(host.pending_intents(&ctx).await.is_err());
    assert!(
        host.record_intent_reply(&ctx, intent("scoped", 1), IntentReply::Success(json!({})))
            .await
            .is_err()
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn custody_is_bounded_and_failed_native_persistence_leaves_no_prepared_command() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("ledger");
    let store = DomainStore::start(DomainRepository::open(&state).unwrap(), 8).unwrap();
    let ctx = context("@owner:test", 1, 1);
    let host = session(&store, &ctx, GRANTS);
    let db = rusqlite::Connection::open(state.join("domain.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER deny_intent BEFORE INSERT ON management_intents BEGIN SELECT RAISE(ABORT,'test storage failure'); END;").unwrap();
    assert!(
        host.prepare_intent(&ctx, intent("blocked", 1))
            .await
            .is_err()
    );
    assert!(host.pending_intents(&ctx).await.unwrap().is_empty());
    db.execute_batch("DROP TRIGGER deny_intent").unwrap();
    for n in 0..8 {
        host.prepare_intent(&ctx, intent(&format!("pending_{n}"), 1))
            .await
            .unwrap();
    }
    assert!(matches!(
        host.prepare_intent(&ctx, intent("overflow", 1)).await,
        Err(Error::Capacity)
    ));
    assert_eq!(host.pending_intents(&ctx).await.unwrap().len(), 8);
    assert!(
        Intent::new(
            IntentOperation::EnrollResource,
            "large".into(),
            &json!({"commandId":"large","data":"x".repeat(16*1024)})
        )
        .is_err()
    );
    assert!(
        Intent::new(
            IntentOperation::EnrollResource,
            "different".into(),
            &json!({"commandId":"original"})
        )
        .is_err()
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn project_journal_restarts_without_promoting_resource_or_read_grants() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("ledger");
    let store = DomainStore::start(DomainRepository::open(&state).unwrap(), 8).unwrap();
    let ctx = context("@manager:test", 1, 1);
    let operations = [
        (
            IntentOperation::ProjectRequest,
            Permission::RequestProjects,
            "request",
        ),
        (
            IntentOperation::ProjectDecide,
            Permission::DecideProjects,
            "decision",
        ),
        (
            IntentOperation::ProjectRevoke,
            Permission::RevokeProjects,
            "revocation",
        ),
    ];
    let make = |op, id: &str| {
        Intent::new(
            op,
            id.into(),
            &json!({"commandId":id,"projectId":"project_1"}),
        )
        .unwrap()
    };
    let legacy = session(
        &store,
        &ctx,
        &[
            Permission::ReadIntents,
            Permission::ContributeResources,
            Permission::DecideInbox,
            Permission::RevokeConnections,
            Permission::ReadProjects,
        ],
    );
    for (op, permission, id) in operations {
        assert!(legacy.prepare_intent(&ctx, make(op, id)).await.is_err());
        let writer = session(&store, &ctx, &[permission, Permission::ReadIntents]);
        assert!(
            writer
                .prepare_intent(&ctx, make(op, id))
                .await
                .unwrap()
                .is_first_attempt()
        );
        writer.retire();
    }
    assert!(legacy.pending_intents(&ctx).await.unwrap().is_empty());
    legacy.retire();
    store.shutdown().await.unwrap();
    drop(legacy);
    drop(store);

    let store = DomainStore::start(DomainRepository::open(&state).unwrap(), 8).unwrap();
    let renewed = context("@manager:test", 2, 2);
    for (op, permission, id) in operations {
        let writer = session(
            &store,
            &renewed,
            &[
                permission,
                Permission::ReadIntents,
                Permission::AcknowledgeIntents,
            ],
        );
        let rows = writer.pending_intents(&renewed).await.unwrap();
        assert_eq!(
            rows.len(),
            1,
            "an exact operation grant exposes only its own journal"
        );
        assert_eq!(rows[0].command_id, id);
        assert!(
            !writer
                .prepare_intent(&renewed, make(op, id))
                .await
                .unwrap()
                .is_first_attempt()
        );
        assert!(
            writer
                .acknowledge_intent(&renewed, id.into())
                .await
                .is_err(),
            "unknown results cannot be discarded"
        );
        let reply = IntentReply::Success(
            json!({"project":{"id":"project_1","allocationStatus":"awaiting_allocation"}}),
        );
        writer
            .record_intent_reply(&renewed, make(op, id), reply.clone())
            .await
            .unwrap();
        assert_eq!(
            writer
                .prepare_intent(&renewed, make(op, id))
                .await
                .unwrap()
                .reply,
            Some(reply)
        );
        writer
            .acknowledge_intent(&renewed, id.into())
            .await
            .unwrap();
        assert!(writer.pending_intents(&renewed).await.unwrap().is_empty());
    }
    store.shutdown().await.unwrap();
}
