use hagency_core::project::Resource;
use hagency_store::{DomainRepository, DomainStore, Error, management::*};
use serde_json::json;
use std::time::{Duration, Instant};

fn context(account: &str, epoch: u64) -> HostContext {
    HostContext::new(
        account.into(),
        epoch,
        "a".repeat(64),
        1,
        "desktop_one".into(),
        1,
    )
    .unwrap()
}
fn session(store: &DomainStore, context: &HostContext, permissions: &[Permission]) -> Session {
    Session::new(
        store.clone(),
        context.clone(),
        permissions.iter().copied(),
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}
fn seed(db: &mut DomainRepository, id: &str, published: bool) -> Resource {
    let r: Resource = serde_json::from_value(json!({
        "presetId":id,"seatId":"local_account","framework":"codex",
        "model":"gpt-5.6-sol","reasoning":"medium","published":published,
        "ceiling":{"tokens":5000,"period":"monthly"}
    }))
    .unwrap();
    db.put_resource(&r).unwrap();
    r
}
fn create(resource: &Resource, key: &str, tokens: u64) -> ConfigureResource {
    serde_json::from_value(json!({
        "commandId":key,"resourceId":resource.id(),
        "expectedRevision":hagency_store::resource_publication_revision(resource).unwrap(),
        "create":true,"profile":{"kind":"preserve"},
        "ceiling":{"kind":"monthly","tokens":tokens}
    }))
    .unwrap()
}

#[tokio::test]
async fn native_management_creation_replays_concurrently_and_after_restart() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("ledger");
    let mut db = DomainRepository::open(&state).unwrap();
    let source = seed(&mut db, "source", false);
    let store = DomainStore::start(db, 8).unwrap();
    let ctx = context("@owner:example.test", 1);
    let manager = session(
        &store,
        &ctx,
        &[Permission::ConfigureResources, Permission::ReadResources],
    );
    let (a, b) = tokio::join!(
        manager.configure_resource(&ctx, create(&source, "create_once", 1000)),
        manager.configure_resource(&ctx, create(&source, "create_once", 1000)),
    );
    let result = a.unwrap();
    assert_eq!(result.resource_id, b.unwrap().resource_id);
    assert_eq!(
        manager
            .resources(&ctx, ResourceQuery::default())
            .await
            .unwrap()
            .resources
            .len(),
        2
    );
    assert!(matches!(
        manager
            .configure_resource(&ctx, create(&source, "create_once", 2000))
            .await,
        Err(Error::Conflict)
    ));
    let single = manager
        .resource(&ctx, result.resource_id.clone())
        .await
        .unwrap();
    assert_eq!(single.id, result.resource_id);
    assert_eq!(single.revision, result.revision);
    assert!(
        manager
            .resource(&context("@other:example.test", 1), single.id.clone())
            .await
            .is_err()
    );
    assert!(manager.resource(&ctx, "missing".into()).await.is_err());
    manager.retire();
    store.shutdown().await.unwrap();
    drop(manager);
    drop(store);

    // A new process/account epoch and package instance may recover the exact
    // original result. It must not allocate another random resource identifier.
    let store = DomainStore::start(DomainRepository::open(&state).unwrap(), 8).unwrap();
    let ctx = HostContext::new(
        "@owner:example.test".into(),
        2,
        "b".repeat(64),
        9,
        "desktop_one".into(),
        1,
    )
    .unwrap();
    let manager = session(
        &store,
        &ctx,
        &[Permission::ConfigureResources, Permission::ReadResources],
    );
    let replay = manager
        .configure_resource(&ctx, create(&source, "create_once", 1000))
        .await
        .unwrap();
    assert_eq!(result.resource_id, replay.resource_id);
    assert_eq!(
        manager
            .resources(&ctx, ResourceQuery::default())
            .await
            .unwrap()
            .resources
            .len(),
        2
    );
    manager.retire();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_management_account_package_device_permissions_and_retirement() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("ledger")).unwrap();
    let source = seed(&mut db, "source", false);
    let store = DomainStore::start(db, 8).unwrap();
    let ctx = context("@owner:example.test", 1);
    let reader = session(&store, &ctx, &[Permission::ReadResources]);
    assert!(matches!(
        reader
            .configure_resource(&ctx, create(&source, "denied", 1000))
            .await,
        Err(Error::LocalAuthority)
    ));
    let altered = [
        context("@other:example.test", 1),
        context("@owner:example.test", 2),
        HostContext::new(
            "@owner:example.test".into(),
            1,
            "b".repeat(64),
            1,
            "desktop_one".into(),
            1,
        )
        .unwrap(),
        HostContext::new(
            "@owner:example.test".into(),
            1,
            "a".repeat(64),
            2,
            "desktop_one".into(),
            1,
        )
        .unwrap(),
        HostContext::new(
            "@owner:example.test".into(),
            1,
            "a".repeat(64),
            1,
            "desktop_other".into(),
            1,
        )
        .unwrap(),
        HostContext::new(
            "@owner:example.test".into(),
            1,
            "a".repeat(64),
            1,
            "desktop_one".into(),
            2,
        )
        .unwrap(),
    ];
    for wrong in altered {
        assert!(matches!(
            reader.resources(&wrong, ResourceQuery::default()).await,
            Err(Error::LocalAuthority)
        ));
    }
    let clone = reader.clone();
    reader.retire();
    assert!(matches!(
        clone.resources(&ctx, ResourceQuery::default()).await,
        Err(Error::LocalAuthority)
    ));
    assert!(
        Session::new(
            store.clone(),
            ctx,
            [Permission::ReadResources],
            Instant::now() + Duration::from_secs(301)
        )
        .is_err()
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_management_filtered_pages_keep_all_matches_and_unknown_usage() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("ledger")).unwrap();
    for i in 0..23 {
        seed(&mut db, &format!("source_{i:02}"), i % 2 == 0);
    }
    let store = DomainStore::start(db, 8).unwrap();
    let ctx = context("@owner:example.test", 1);
    let manager = session(&store, &ctx, &[Permission::ReadResources]);
    let mut after = String::new();
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let page = manager
            .resources(
                &ctx,
                ResourceQuery {
                    after,
                    limit: 3,
                    published: Some(true),
                    capability: Some("coding".into()),
                    framework: Some("codex".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(page.protocol, PROTOCOL);
        assert!(page.observed_at_ms > 0);
        for row in page.resources {
            assert!(seen.insert(row.id));
            assert!(row.published);
            assert_eq!(u64::from(row.budget.remaining_tokens.unwrap()), 5000);
            assert_eq!(row.usage.spent, None);
            assert_eq!(row.usage.consumed, None);
        }
        if let Some(cursor) = page.next_after {
            after = cursor;
        } else {
            break;
        }
    }
    assert_eq!(seen.len(), 12);
    assert!(
        manager
            .resources(
                &ctx,
                ResourceQuery {
                    limit: 1000,
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_management_publication_replay_and_revision_conflict() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("ledger")).unwrap();
    let source = seed(&mut db, "source", false);
    let store = DomainStore::start(db, 8).unwrap();
    let ctx = context("@owner:example.test", 1);
    let manager = session(&store, &ctx, &[Permission::PublishResources]);
    let input = || PublishResource {
        command_id: "publish_once".into(),
        resource_id: source.id(),
        expected_revision: hagency_store::resource_publication_revision(&source).unwrap(),
        published: true,
    };
    let first = manager.publish_resource(&ctx, input()).await.unwrap();
    assert!(first.published);
    assert_eq!(
        first.revision,
        manager
            .publish_resource(&ctx, input())
            .await
            .unwrap()
            .revision
    );
    let mut stale = input();
    stale.command_id = "another_write".into();
    stale.published = false;
    assert!(matches!(
        manager.publish_resource(&ctx, stale).await,
        Err(Error::Conflict)
    ));
    manager.retire();
    assert!(matches!(
        manager.publish_resource(&ctx, input()).await,
        Err(Error::LocalAuthority)
    ));
    store.shutdown().await.unwrap();
}

#[test]
fn native_management_requests_cannot_supply_authority_or_arbitrary_paths() {
    assert!(
        serde_json::from_value::<ResourceQuery>(json!({"account":"@other:example.test"})).is_err()
    );
    assert!(
        serde_json::from_value::<ConfigureResource>(json!({
            "commandId":"c","resourceId":"r","expectedRevision":"a".repeat(64),"create":false,
            "profile":{"kind":"preserve"},"ceiling":{"kind":"preserve"},"workspace":"/tmp/arbitrary"
        }))
        .is_err()
    );
    assert!(
        HostContext::new(
            "not a Matrix user".into(),
            1,
            "a".repeat(64),
            1,
            "device".into(),
            1
        )
        .is_err()
    );
}

#[test]
fn managed_ledger_rejects_another_profile_and_populated_unscoped_import() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let managed = root.path().join("managed");
    let first = "a".repeat(64);
    let mut db = DomainRepository::open_managed(&managed, &first).unwrap();
    let resource = seed(&mut db, "retained", false);
    drop(db);
    assert!(matches!(
        DomainRepository::open_managed(&managed, &"b".repeat(64)),
        Err(Error::LocalAuthority)
    ));
    let db = DomainRepository::open_managed(&managed, &first).unwrap();
    assert_eq!(
        db.resource_configuration(&resource.id()).unwrap().model,
        resource.model
    );
    drop(db);
    let legacy = root.path().join("unscoped");
    let mut db = DomainRepository::open(&legacy).unwrap();
    seed(&mut db, "legacy", false);
    drop(db);
    assert!(matches!(
        DomainRepository::open_managed(&legacy, &first),
        Err(Error::LocalAuthority)
    ));
    assert_eq!(
        DomainRepository::open_for_migration(&legacy)
            .unwrap()
            .resource_configurations("", 16)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn failed_receipt_commit_rolls_back_the_resource_and_allows_exact_retry() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("ledger");
    let mut db = DomainRepository::open(&state).unwrap();
    let source = seed(&mut db, "source", false);
    let store = DomainStore::start(db, 8).unwrap();
    let ctx = context("@owner:example.test", 1);
    let manager = session(
        &store,
        &ctx,
        &[Permission::ConfigureResources, Permission::ReadResources],
    );
    let inspect = rusqlite::Connection::open(state.join("domain.sqlite3")).unwrap();
    inspect.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON management_commands BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;").unwrap();
    assert!(
        manager
            .configure_resource(&ctx, create(&source, "atomic_create", 1000))
            .await
            .is_err()
    );
    assert_eq!(
        manager
            .resources(&ctx, Default::default())
            .await
            .unwrap()
            .resources
            .len(),
        1
    );
    assert_eq!(
        inspect
            .query_row("SELECT COUNT(*) FROM management_commands", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    inspect.execute_batch("DROP TRIGGER fail_receipt;").unwrap();
    manager
        .configure_resource(&ctx, create(&source, "atomic_create", 1000))
        .await
        .unwrap();
    assert_eq!(
        manager
            .resources(&ctx, Default::default())
            .await
            .unwrap()
            .resources
            .len(),
        2
    );
    store.shutdown().await.unwrap();
}

#[test]
fn concurrent_private_directory_creation_remains_checked() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    for iteration in 0..16 {
        let target = root.path().join(format!("parallel_{iteration}"));
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        hagency_store::private::directory(&target)
                    })
                })
                .collect();
            for handle in handles {
                handle.join().unwrap().unwrap();
            }
        });
        hagency_store::private::directory(&target).unwrap();
    }
    let file = root.path().join("not_directory");
    std::fs::write(&file, b"unchanged").unwrap();
    assert!(hagency_store::private::directory(&file).is_err());
    #[cfg(unix)]
    {
        let link = root.path().join("linked");
        std::os::unix::fs::symlink(root.path(), &link).unwrap();
        assert!(hagency_store::private::directory(&link).is_err());
    }
}

fn enroll(key: &str) -> EnrollResource {
    EnrollResource {
        command_id: key.into(),
        runtime: "codex".into(),
        model_id: "gpt-5.6-sol".into(),
        reasoning: Some("medium".into()),
        monthly_tokens: 5000,
    }
}
fn selection(ctx: &HostContext, input: &EnrollResource, reference: &str) -> ProviderSelection {
    ProviderSelection::new(
        ctx.clone(),
        input,
        reference,
        "gpt-5.6-sol".into(),
        None,
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap()
}
#[tokio::test]
async fn executor_verification_requires_original_provider_revision_and_live_native_scope() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let store = DomainStore::start(
        DomainRepository::open(&root.path().join("ledger")).unwrap(),
        8,
    )
    .unwrap();
    let ctx = context("@owner:example.test", 1);
    let manager = session(
        &store,
        &ctx,
        &[
            Permission::ReadResources,
            Permission::EnrollResources,
            Permission::VerifyConnections,
        ],
    );
    let input = enroll("verify_source");
    let enrolled = manager
        .enroll_resource(
            &ctx,
            enroll("verify_source"),
            selection(&ctx, &input, "keychain:original"),
        )
        .await
        .unwrap();
    let witness = |ctx: &HostContext, reference: &str| {
        ProviderWitness::new(
            ctx.clone(),
            "codex".into(),
            reference,
            "gpt-5.6-sol".into(),
            Some("medium".into()),
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap()
    };
    let verified = manager
        .verify_resource(
            &ctx,
            enrolled.resource_id.clone(),
            enrolled.revision.clone(),
            witness(&ctx, "keychain:original"),
        )
        .await
        .unwrap();
    assert_eq!(verified.revision, enrolled.revision);
    let resumer = session(&store, &ctx, &[Permission::ResumeConnections]);
    assert!(
        resumer
            .verify_resource(
                &ctx,
                enrolled.resource_id.clone(),
                enrolled.revision.clone(),
                witness(&ctx, "keychain:original")
            )
            .await
            .is_err(),
        "reconnection does not grant verification authority"
    );
    assert!(
        resumer
            .resume_resource(
                &ctx,
                enrolled.resource_id.clone(),
                enrolled.revision.clone(),
                witness(&ctx, "keychain:original")
            )
            .await
            .is_ok()
    );
    assert!(
        !verified.published,
        "verification cannot publish or reserve capacity"
    );
    assert!(
        manager
            .verify_resource(
                &ctx,
                enrolled.resource_id.clone(),
                "f".repeat(64),
                witness(&ctx, "keychain:original")
            )
            .await
            .is_err()
    );
    assert!(
        manager
            .verify_resource(
                &ctx,
                enrolled.resource_id.clone(),
                enrolled.revision.clone(),
                witness(&ctx, "keychain:other")
            )
            .await
            .is_err()
    );
    assert!(
        manager
            .verify_resource(
                &ctx,
                enrolled.resource_id.clone(),
                enrolled.revision.clone(),
                witness(&context("@other:example.test", 1), "keychain:original")
            )
            .await
            .is_err()
    );
    let reader = session(&store, &ctx, &[Permission::ReadResources]);
    assert!(
        reader
            .verify_resource(
                &ctx,
                enrolled.resource_id.clone(),
                enrolled.revision.clone(),
                witness(&ctx, "keychain:original")
            )
            .await
            .is_err()
    );
    let expired = ProviderWitness::new(
        ctx.clone(),
        "codex".into(),
        "keychain:original",
        "gpt-5.6-sol".into(),
        Some("medium".into()),
        Instant::now() + Duration::from_millis(10),
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        manager
            .verify_resource(
                &ctx,
                enrolled.resource_id.clone(),
                enrolled.revision.clone(),
                expired
            )
            .await
            .is_err()
    );
    manager.retire();
    assert!(
        manager
            .verify_resource(
                &ctx,
                enrolled.resource_id.clone(),
                enrolled.revision.clone(),
                witness(&ctx, "keychain:original")
            )
            .await
            .is_err()
    );
    assert!(store.catalog(String::new(), 16).await.unwrap().is_empty());
    store.shutdown().await.unwrap();
}
#[tokio::test]
async fn first_resource_is_account_bound_recoverable_and_unpublished() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let path = root.path().join("ledger");
    let store = DomainStore::start(DomainRepository::open(&path).unwrap(), 8).unwrap();
    let ctx = context("@owner:example.test", 1);
    let manager = session(
        &store,
        &ctx,
        &[Permission::ReadResources, Permission::EnrollResources],
    );
    let (a, b) = tokio::join!(
        manager.enroll_resource(
            &ctx,
            enroll("first"),
            selection(&ctx, &enroll("first"), "keychain:test_provider")
        ),
        manager.enroll_resource(
            &ctx,
            enroll("first"),
            selection(&ctx, &enroll("first"), "keychain:test_provider")
        ),
    );
    let a = a.unwrap();
    assert_eq!(a.resource_id, b.unwrap().resource_id);
    assert!(!a.published);
    let resources = manager
        .resources(
            &ctx,
            ResourceQuery {
                capability: Some("documentation".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        resources.resources.len(),
        1,
        "unpublished definitions remain filterable by capability"
    );
    assert!(
        resources.resources[0]
            .capabilities
            .contains(&"documentation".into())
    );
    assert!(
        !serde_json::to_string(&resources)
            .unwrap()
            .contains("keychain:test_provider")
    );
    assert!(
        store.catalog(String::new(), 16).await.unwrap().is_empty(),
        "definition is not an advertised contribution"
    );
    manager.retire();
    store.shutdown().await.unwrap();
    drop(manager);
    drop(store);
    let store = DomainStore::start(DomainRepository::open(&path).unwrap(), 8).unwrap();
    let next = context("@owner:example.test", 2);
    let manager = session(
        &store,
        &next,
        &[Permission::ReadResources, Permission::EnrollResources],
    );
    let again = manager
        .enroll_resource(
            &next,
            enroll("first"),
            selection(&next, &enroll("first"), "keychain:test_provider"),
        )
        .await
        .unwrap();
    assert_eq!(a.resource_id, again.resource_id);
    assert!(matches!(
        manager
            .enroll_resource(
                &next,
                enroll("first"),
                selection(&next, &enroll("first"), "keychain:different_provider")
            )
            .await,
        Err(Error::Conflict)
    ));
    manager.retire();
    store.shutdown().await.unwrap();
}
#[tokio::test]
async fn enrollment_requires_current_provider_selection_and_separate_permission() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let store = DomainStore::start(
        DomainRepository::open(&root.path().join("ledger")).unwrap(),
        8,
    )
    .unwrap();
    let ctx = context("@owner:example.test", 1);
    let read = session(
        &store,
        &ctx,
        &[Permission::ReadResources, Permission::ConfigureResources],
    );
    assert!(matches!(
        read.enroll_resource(
            &ctx,
            enroll("first"),
            selection(&ctx, &enroll("first"), "keychain:test")
        )
        .await,
        Err(Error::LocalAuthority)
    ));
    let manager = session(
        &store,
        &ctx,
        &[Permission::ReadResources, Permission::EnrollResources],
    );
    assert!(matches!(
        manager
            .enroll_resource(
                &ctx,
                enroll("first"),
                selection(
                    &context("@other:example.test", 1),
                    &enroll("first"),
                    "keychain:test"
                )
            )
            .await,
        Err(Error::LocalAuthority)
    ));
    let mut changed = enroll("first");
    changed.monthly_tokens = 6000;
    assert!(matches!(
        manager
            .enroll_resource(
                &ctx,
                changed,
                selection(&ctx, &enroll("first"), "keychain:test")
            )
            .await,
        Err(Error::LocalAuthority)
    ));
    assert!(
        ProviderSelection::new(
            ctx.clone(),
            &enroll("first"),
            "keychain:test",
            "gpt-5.6-sol".into(),
            None,
            Instant::now() - Duration::from_secs(1)
        )
        .is_err()
    );
    assert!(
        manager
            .resources(&ctx, Default::default())
            .await
            .unwrap()
            .resources
            .is_empty()
    );
    read.retire();
    manager.retire();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_receipt_recovers_after_lost_reply_and_new_authorization_without_provider_proof() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let path = root.path().join("ledger");
    let store = DomainStore::start(DomainRepository::open(&path).unwrap(), 8).unwrap();
    let ctx = context("@owner:example.test", 1);
    let grants = [
        Permission::EnrollResources,
        Permission::ReadIntents,
        Permission::AcknowledgeIntents,
        Permission::ReadResources,
    ];
    let host = session(&store, &ctx, &grants);
    let input = enroll("lost_reply");
    let intent = || {
        Intent::new(
            IntentOperation::EnrollResource,
            input.command_id.clone(),
            &input,
        )
        .unwrap()
    };
    assert!(
        host.prepare_intent(&ctx, intent())
            .await
            .unwrap()
            .is_first_attempt()
    );
    let result = host
        .enroll_resource(
            &ctx,
            enroll("lost_reply"),
            selection(&ctx, &input, "keychain:original"),
        )
        .await
        .unwrap();
    // Stop after the real ledger effect but before recording the UI reply.
    host.retire();
    store.shutdown().await.unwrap();
    drop(host);
    drop(store);
    let store = DomainStore::start(DomainRepository::open(&path).unwrap(), 8).unwrap();
    let renewed = HostContext::new(
        "@owner:example.test".into(),
        2,
        "b".repeat(64),
        2,
        "desktop_one".into(),
        2,
    )
    .unwrap();
    let host = session(&store, &renewed, &grants);
    let recovered = host.prepare_intent(&renewed, intent()).await.unwrap();
    assert!(!recovered.is_first_attempt());
    assert_eq!(
        recovered.reply,
        Some(IntentReply::Success(serde_json::to_value(&result).unwrap()))
    );
    assert_eq!(host.pending_intents(&renewed).await.unwrap().len(), 1);
    host.acknowledge_intent(&renewed, input.command_id.clone())
        .await
        .unwrap();
    store.shutdown().await.unwrap();
}

#[cfg(feature = "resource-adoption")]
#[tokio::test]
async fn graph_history_permission_is_separate_and_legacy_is_not_empty_history() {
    let root=hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let store=DomainStore::start(DomainRepository::open(&root.path().join("ledger")).unwrap(),8).unwrap();
    let ctx=context("@owner:example.test",1);
    for permissions in [vec![],vec![Permission::ReadTasks],vec![Permission::ArchiveOperatorGraphHistory]] {
        let reader=session(&store,&ctx,&permissions);
        assert!(reader.operator_graph_archives(&ctx).await.is_err());
        assert!(reader.operator_graph_history_document(&ctx,"original".into()).await.is_err());
        reader.retire();
    }
    let reader=session(&store,&ctx,&[Permission::ReadOperatorGraphHistory]);
    assert!(reader.operator_graph_archives(&ctx).await.unwrap().is_none());
    assert!(reader.operator_graph_history_document(&ctx,"original".into()).await.is_err());
    assert!(reader.operator_graph_archives(&context("@other:example.test",1)).await.is_err());
    reader.retire();
    assert!(reader.operator_graph_archives(&ctx).await.is_err());
    store.shutdown().await.unwrap();
}

#[cfg(feature = "resource-adoption")]
#[tokio::test]
async fn graph_workflow_legacy_refuses_writes_and_never_discards_old_intent_custody() {
    let root=hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let store=DomainStore::start(DomainRepository::open(&root.path().join("ledger")).unwrap(),8).unwrap();
    let ctx=context("@owner:example.test",1);
    let manager=session(&store,&ctx,&[Permission::CreateTasks,Permission::ReadIntents,Permission::ReadOperatorGraphs,Permission::CreateOperatorGraphs,Permission::UpdateOperatorGraphNodes,Permission::CancelOperatorGraphs,Permission::ReadOperatorGraphRecipients,Permission::BindOperatorGraphRecipients]);
    manager.prepare_intent(&ctx,Intent::new(IntentOperation::TaskCreate,"original".into(),&json!({"commandId":"original","title":"kept"})).unwrap()).await.unwrap();
    for (permission,operation) in [(Permission::CreateOperatorGraphs,IntentOperation::GraphCreate),(Permission::UpdateOperatorGraphNodes,IntentOperation::GraphUpdateNode),(Permission::CancelOperatorGraphs,IntentOperation::GraphCancel),(Permission::BindOperatorGraphRecipients,IntentOperation::GraphBindRecipient)] {
        assert!(matches!(manager.operator_graph_access(&ctx,permission),Err(Error::Unqualified)));
        let intent=Intent::new(operation,"unsupported".into(),&json!({"commandId":"unsupported","result":0.25})).unwrap();
        assert!(matches!(manager.prepare_intent(&ctx,intent).await,Err(Error::Unqualified)));
    }
    assert!(matches!(manager.operator_graph_access(&ctx,Permission::ReadOperatorGraphs),Err(Error::Unqualified)));
    assert!(matches!(manager.operator_graph_access(&ctx,Permission::ReadOperatorGraphRecipients),Err(Error::Unqualified)));
    let rows=manager.pending_intents(&ctx).await.unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].command_id,"original");
    manager.retire();store.shutdown().await.unwrap();
}
