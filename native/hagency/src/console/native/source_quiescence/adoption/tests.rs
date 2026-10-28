use super::*;
use crate::console::native::reconciliation_sources::tests::Fixture;
use hagency_core::project::Seat;
use std::sync::atomic::Ordering;
fn provider() -> VerifiedProviderAccount {
    VerifiedProviderAccount::new(
        "codex",
        "https://provider.test",
        "synthetic-native-account",
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap()
}
fn package() -> ManagementPackage {
    ManagementPackage {
        digest: "a".repeat(64),
        instance: 1,
        account_epoch: 1,
        permissions: vec![
            Permission::ReadResources,
            Permission::PrepareSourceQuiescence,
            Permission::ContributeResources,
            Permission::ReadIntents,
            Permission::AcknowledgeIntents,
        ],
    }
}
async fn quiesce(
    f: &Fixture,
    registry: &OwnerResourceRegistry,
) -> (Arc<SourceQuiescence>, Arc<QuiescedSources>) {
    let live = f.live.clone();
    let mut request = SourceQuiescenceRequest::new(
        &f.owner,
        package(),
        registry.clone(),
        Instant::now() + Duration::from_secs(50),
        move || live.load(Ordering::Acquire),
    )
    .unwrap();
    request.lease_root =
        Some(hagency_platform::legacy_writer::test_support::initialize_process_root().unwrap());
    let work = Arc::new(f.owner.prepare_source_quiescence(request).await.unwrap());
    let sources = work.close_and_drain().await.unwrap();
    (work, sources)
}
#[test]
fn original_sources_survive_restart_after_fence_in_an_isolated_process() {
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "console::native::source_quiescence::adoption::tests::adoption_process_child",
            "--exact",
            "--nocapture",
        ])
        .env("HAGENCY_ADOPTION_PROCESS_CASE", "restart")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
#[tokio::test]
async fn adoption_process_child() {
    if std::env::var("HAGENCY_ADOPTION_PROCESS_CASE").as_deref() != Ok("restart") {
        return;
    }
    let f = Fixture::new().await;
    let (mut repository, source, resource) = f.domain();
    repository
        .put_seat(&Seat {
            id: resource.seat_id.clone(),
            declaration: None,
        })
        .unwrap();
    let mut second = resource.clone();
    second.preset_id = "second-original-preset".into();
    repository.put_resource(&second).unwrap();
    drop(repository);
    // Preserve a real completed original admission reply in the private source.
    let manager = f.owner.management(package()).await.unwrap();
    let (context, session) = manager.authorized().await.unwrap();
    let original = hagency_store::management::Intent::new(
        hagency_store::management::IntentOperation::Contribute,
        "original_admission".into(),
        &serde_json::json!({"commandId":"original_admission","resourceId":resource.id()}),
    )
    .unwrap();
    session
        .prepare_intent(&context, original.clone())
        .await
        .unwrap();
    session
        .record_intent_reply(
            &context,
            original,
            hagency_store::management::IntentReply::Success(
                serde_json::json!({"admission":{"contribution":{"id":"con_original"}}}),
            ),
        )
        .await
        .unwrap();
    drop(session);
    drop(manager);
    let (ledger, personal) = f.personal();
    drop(ledger);
    let registry =
        OwnerResourceRegistry::isolated_for_tests(&f.temp.path().join("canonical")).unwrap();
    let (work, sources) = quiesce(&f, &registry).await;
    let transfer = sources
        .prepare_resource_adoption("native_original".into(), provider(), 700)
        .await
        .unwrap();
    assert_eq!(transfer.status().await.unwrap().phase, "prepared");
    let main_before = std::fs::read(source.join("domain.sqlite3")).unwrap();
    let personal_before = std::fs::read(&personal).unwrap();
    // Crash boundary: durable source fences committed before the canonical
    // phase acknowledgement. No fake success or alternate source is supplied.
    let proof = {
        let mut files = sources.files.lock().unwrap();
        files
            .fence(
                &transfer.transfer,
                registry.id(),
                &transfer.binding,
                &sources.gate,
                &sources.lease,
            )
            .unwrap()
    };
    drop(proof);
    assert_eq!(transfer.status().await.unwrap().phase, "prepared");
    let backup = source.join("adoption_native_original");
    assert_eq!(
        std::fs::read(backup.join("domain.sqlite")).unwrap(),
        main_before
    );
    assert_eq!(
        std::fs::read(backup.join("personal.sqlite")).unwrap(),
        personal_before
    );
    drop(transfer);
    drop(sources);
    drop(work);
    assert!(hagency_store::DomainRepository::open(&source).is_err());
    assert!(hagency_agent_local::Ledger::open(&personal, "@owner:test").is_err());
    let f = f
        .restart_for_adoption(registry.clone(), "native_original".into())
        .await;
    assert_eq!(
        f.owner
            .execute(super::super::super::Command::Projects)
            .await
            .unwrap_err()
            .code,
        "source_adoption_recovery_only"
    );
    assert!(f.owner.management(package()).await.is_err());
    let (work, sources) = quiesce(&f, &registry).await;
    assert!(
        sources
            .resume_resource_adoption("replacement".into())
            .await
            .is_err()
    );
    let transfer = sources
        .resume_resource_adoption("native_original".into())
        .await
        .unwrap();
    let result = transfer.advance(provider()).await.unwrap();
    assert_eq!(result.phase, "active");
    let mut ids = vec![resource.id(), second.id()];
    ids.sort();
    assert_eq!(result.resource_ids, ids);
    assert_eq!(result.monthly_account_tokens, 700);
    let profile = ProfileAuthority::new(
        sources.profile.clone(),
        "@owner:test".into(),
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap();
    for id in [resource.id(), second.id()] {
        let actual = registry.resource(&profile, id.clone()).await.unwrap();
        assert_eq!(actual.id, id);
        assert_eq!(actual.budget.seat.quota.map(u64::from), Some(700));
        assert_eq!(actual.budget.seat.remaining.map(u64::from), Some(700));
        assert_eq!(u64::from(actual.budget.seat.committed), 0);
    }
    let other = ProfileAuthority::new(
        "f".repeat(64),
        "@other:test".into(),
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap();
    assert!(registry.resource(&other, resource.id()).await.is_err());
    let foreign = registry
        .prepare_enrollment(
            other.clone(),
            "foreign_definition".into(),
            provider(),
            "foreign_model".into(),
            None,
            700,
            Instant::now() + Duration::from_secs(20),
        )
        .unwrap()
        .approve()
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(backup.join("domain.sqlite")).unwrap(),
        main_before
    );
    assert_eq!(
        std::fs::read(backup.join("personal.sqlite")).unwrap(),
        personal_before
    );
    assert!(hagency_store::DomainRepository::open(&source).is_err());
    transfer.drain_journal().await.unwrap();
    drop(transfer);
    drop(sources);
    drop(work);
    let f = f
        .restart_adopted(registry.clone(), "native_original".into())
        .await;
    let manager = f.owner.management(package()).await.unwrap();
    let page = manager
        .resources(hagency_store::management::ResourceQuery::default())
        .await
        .unwrap();
    assert_eq!(
        page.resources
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    for id in [resource.id(), second.id()] {
        let detail = manager
            .resource_detail(hagency_store::management::ResourceDetailQuery { id: id.clone() })
            .await
            .unwrap();
        assert_eq!(detail.resource.id, id);
        assert_eq!(
            detail.resource.budget.seat.remaining.map(u64::from),
            Some(700)
        );
        assert!(!detail.editing.allowed);
    }
    let first = manager
        .resources(hagency_store::management::ResourceQuery {
            limit: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    let next = manager
        .resources(hagency_store::management::ResourceQuery {
            limit: 1,
            after: first.next_after.unwrap(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_ne!(first.resources[0].id, next.resources[0].id);
    assert!(
        manager
            .resource_detail(hagency_store::management::ResourceDetailQuery {
                id: foreign.resource_id.clone()
            })
            .await
            .is_err()
    );
    let saved = manager.saved_commands().await.unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].command_id, "original_admission");
    assert!(saved[0].reply.is_some());
    manager
        .acknowledge_command("original_admission".into())
        .await
        .unwrap();
    let (context, session) = manager.authorized().await.unwrap();
    let pending = hagency_store::management::Intent::new(
        hagency_store::management::IntentOperation::Contribute,
        "new_pending_after_adoption".into(),
        &serde_json::json!({"commandId":"new_pending_after_adoption","resourceId":resource.id()}),
    )
    .unwrap();
    session.prepare_intent(&context, pending).await.unwrap();
    assert_eq!(
        f.owner
            .execute(super::super::super::Command::Projects)
            .await
            .unwrap_err()
            .code,
        "source_adoption_recovery_only"
    );
    assert!(hagency_store::DomainRepository::open(&source).is_err());
    drop(session);
    drop(manager);
    // New pending history and an acknowledged original command are legitimate
    // archive evolution, not permission to recreate/import the old source.
    let f = f
        .restart_adopted(registry.clone(), "native_original".into())
        .await;
    let manager = f.owner.management(package()).await.unwrap();
    assert_eq!(
        manager
            .resources(Default::default())
            .await
            .unwrap()
            .resources
            .len(),
        2
    );
    let saved = manager.saved_commands().await.unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].command_id, "new_pending_after_adoption");
    assert!(saved[0].reply.is_none());
    registry
        .prepare_revoke(
            profile.clone(),
            "withdraw_second_access".into(),
            second.id(),
            Instant::now() + Duration::from_secs(20),
        )
        .unwrap()
        .approve()
        .await
        .unwrap();
    let visible = manager.resources(Default::default()).await.unwrap();
    assert_eq!(visible.resources.len(), 1);
    assert_eq!(visible.resources[0].id, resource.id());
    assert!(
        manager
            .resource_detail(hagency_store::management::ResourceDetailQuery { id: second.id() })
            .await
            .is_err()
    );
    // Keep the same archive shutdown acknowledgement after a caller is cancelled.
    let mut draining = Box::pin(f.owner.0.management.drain());
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(draining.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(draining);
    assert!(manager.is_retired());
    let (one, two) = tokio::join!(f.owner.0.management.drain(), f.owner.0.management.drain());
    one.unwrap();
    two.unwrap();
    drop(manager);
    f.owner.shutdown().await;
    // Profile shutdown drains only its private archive, never the shared registry.
    assert_eq!(
        registry
            .resource(&profile, resource.id())
            .await
            .unwrap()
            .budget
            .seat
            .remaining
            .map(u64::from),
        Some(700)
    );
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[path = "task_transfer_tests.rs"]
mod task_transfer_tests;

#[path = "task_wizard_tests.rs"]
mod task_wizard_tests;
