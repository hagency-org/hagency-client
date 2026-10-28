use super::super::reconciliation_sources::tests::Fixture;
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "resource-adoption")]
#[tokio::test]
async fn graph_history_native_review_keeps_exact_files_and_requires_current_sdk() {
    let f = Fixture::new().await;
    let root = f.owner.0.host.state_directory();
    let original = br#"{"saved":{"id":"saved","owner":"Historical owner","label":"Private title","nodes":{"a":{"assignee":"Historical agent","description":"Private task"}}}}"#;
    hagency_store::private::write_new(&root.join("task_graphs.json"), original).unwrap();
    let (work, registry, _live) = prepared(&f).await;
    let inventory = work.inspect_operator_graph_documents().await.unwrap();
    let review = inventory.review().await.unwrap();
    assert_eq!(review.history.graphs, 1);
    assert_eq!(review.history.unresolved_principal_names, 2);
    assert!(review.blockers.contains(&"graph_principal_identity_mapping_required"));
    assert!(!review.imported && !review.runtime_delivery_authorized && !review.external_roots_complete);
    let text = serde_json::to_string(&review).unwrap();
    assert!(!text.contains("Historical owner") && !text.contains("Private task"));
    assert_eq!(std::fs::read(root.join("task_graphs.json")).unwrap(),original);
    assert!(!root.join("messages.json").exists());
    assert!(!root.join(".msg_counter").exists());
    f.live.store(false,Ordering::Release);
    assert!(inventory.review().await.is_err());
    drop(inventory); drop(work);
    registry.shutdown().await.unwrap(); f.stop().await;
}

#[cfg(feature = "resource-adoption")]
#[tokio::test]
async fn graph_history_native_empty_review_is_not_adoption_and_rechecks_lease_and_files() {
    let f = Fixture::new().await;
    let root = f.owner.0.host.state_directory();
    let (work, registry, live) = prepared(&f).await;
    let inventory = work.inspect_operator_graph_documents().await.unwrap();
    let review = inventory.review().await.unwrap();
    assert_eq!(review.history.present_documents, 0);
    assert!(review.blockers.contains(&"graph_document_adoption_not_completed"));
    let source = work.close_and_drain().await.unwrap();
    let checked = source.validate_operator_graph_inventory(&inventory).await.unwrap();
    assert_eq!(checked.history, review.history);
    assert!(checked.blockers.contains(&"graph_writer_fence_not_completed"));
    assert!(inventory.review().await.is_err(), "retired SDK is not reused");
    hagency_store::private::write_new(&root.join("messages.json"),b"[]").unwrap();
    assert!(source.validate_operator_graph_inventory(&inventory).await.is_err());
    live.store(false,Ordering::Release);
    assert!(source.validate_operator_graph_inventory(&inventory).await.is_err());
    assert!(work.inspect_operator_graph_documents().await.is_err());
    drop(inventory); drop(source); drop(work);
    registry.shutdown().await.unwrap(); f.stop().await;
}

fn package() -> ManagementPackage {
    ManagementPackage {
        digest: "a".repeat(64),
        instance: 1,
        account_epoch: 1,
        permissions: vec![
            Permission::ReadResources,
            Permission::PrepareSourceQuiescence,
        ],
    }
}
async fn prepared(
    f: &Fixture,
) -> (
    Arc<SourceQuiescence>,
    OwnerResourceRegistry,
    Arc<AtomicBool>,
) {
    prepared_with(f, package()).await
}
async fn prepared_with(
    f: &Fixture,
    selected_package: ManagementPackage,
) -> (Arc<SourceQuiescence>, OwnerResourceRegistry, Arc<AtomicBool>) {
    let (repository, _, _) = f.domain();
    drop(repository);
    let _manager = f.owner.management(package()).await.unwrap();
    let registry =
        OwnerResourceRegistry::isolated_for_tests(&f.temp.path().join("registry")).unwrap();
    let live = Arc::new(AtomicBool::new(true));
    let check = live.clone();
    let mut request = SourceQuiescenceRequest::new(
        &f.owner,
        selected_package,
        registry.clone(),
        Instant::now() + Duration::from_secs(25),
        move || check.load(Ordering::Acquire),
    )
    .unwrap();
    request.lease_root = Some(f.temp.path().join("installation"));
    let work = f.owner.prepare_source_quiescence(request).await.unwrap();
    (Arc::new(work), registry, live)
}

#[cfg(feature = "resource-adoption")]
#[tokio::test]
async fn graph_history_archive_requires_separate_native_permission() {
    let f = Fixture::new().await;
    let (work, registry, _) = prepared(&f).await;
    let inventory = work.inspect_operator_graph_documents().await.unwrap();
    let source = work.close_and_drain().await.unwrap();
    let error = source.archive_operator_graph_inventory(&inventory, "no-permission".into()).await.unwrap_err();
    assert_eq!(error.status,403);
    assert_eq!(error.code,"graph_history_archive_permission_required");
    drop(source); drop(inventory); drop(work);
    registry.shutdown().await.unwrap(); f.stop().await;
}

#[cfg(feature = "resource-adoption")]
#[tokio::test]
async fn graph_history_archive_preserves_original_files_and_replay_then_rejects_source_change() {
    let f = Fixture::new().await;
    let root = f.owner.0.host.state_directory().to_owned();
    let original = br#"{"saved":{"id":"saved","owner":"Historical owner","label":"Private title","unknown":{"kept":true},"nodes":{"a":{"assignee":"Historical agent","description":"Private task"}}}}"#;
    hagency_store::private::write_new(&root.join("task_graphs.json"),original).unwrap();
    let mut p=package(); p.permissions.push(Permission::ArchiveOperatorGraphHistory);
    let (work,registry,live)=prepared_with(&f,p).await;
    let inventory=work.inspect_operator_graph_documents().await.unwrap();
    let source=work.close_and_drain().await.unwrap();
    let first=source.archive_operator_graph_inventory(&inventory,"exact-history".into()).await.unwrap();
    assert_eq!(first.state,"archived"); assert_eq!(first.history.graphs,1);
    assert!(!first.runtime_delivery_authorized && !first.source_writer_fenced);
    assert_eq!(source.archive_operator_graph_inventory(&inventory,"exact-history".into()).await.unwrap(),first);
    assert_eq!(std::fs::read(root.join("task_graphs.json")).unwrap(),original);
    assert!(!root.join("messages.json").exists() && !root.join(".msg_counter").exists());
    let profile=hagency_store::owner_resources::ProfileAuthority::new(source.profile.clone(),source.owner.0.expected_owner.clone(),Instant::now()+Duration::from_secs(20)).unwrap();
    assert_eq!(registry.operator_graph_archive_chunk(&profile,"exact-history".into(),0,0).await.unwrap(),original);
    hagency_store::private::write_new(&root.join("messages.json"),b"[]").unwrap();
    assert!(source.archive_operator_graph_inventory(&inventory,"exact-history".into()).await.is_err());
    live.store(false,Ordering::Release);
    assert!(source.archive_operator_graph_inventory(&inventory,"exact-history".into()).await.is_err());
    assert_eq!(registry.operator_graph_archive_receipt(&profile,"exact-history".into()).await.unwrap(),Some(first));
    drop(source);drop(inventory);drop(work);registry.shutdown().await.unwrap();f.stop().await;
}

#[cfg(feature = "resource-adoption")]
#[tokio::test]
async fn graph_history_native_archive_handles_full_64_mib_document_under_source_custody() {
    let f=Fixture::new().await;
    let root=f.owner.0.host.state_directory().to_owned();
    let mut bytes=vec![b' ';64*1024*1024]; bytes[0]=b'['; *bytes.last_mut().unwrap()=b']';
    hagency_store::private::write_new(&root.join("messages.json"),&bytes).unwrap();
    drop(bytes);
    let mut p=package();p.permissions.push(Permission::ArchiveOperatorGraphHistory);
    let (work,registry,_live)=prepared_with(&f,p).await;
    let inventory=work.inspect_operator_graph_documents().await.unwrap();
    let source=work.close_and_drain().await.unwrap();
    let receipt=source.archive_operator_graph_inventory(&inventory,"large-history".into()).await.unwrap();
    assert_eq!(receipt.state,"archived");assert_eq!(receipt.history.present_documents,1);
    assert_eq!(receipt.history.messages,0);assert!(!receipt.runtime_delivery_authorized);
    let profile=hagency_store::owner_resources::ProfileAuthority::new(source.profile.clone(),source.owner.0.expected_owner.clone(),Instant::now()+Duration::from_secs(20)).unwrap();
    let last=registry.operator_graph_archive_chunk(&profile,"large-history".into(),1,63).await.unwrap();
    assert_eq!(last.len(),1024*1024);assert_eq!(last.last(),Some(&b']'));
    assert_eq!(std::fs::metadata(root.join("messages.json")).unwrap().len(),64*1024*1024);
    assert!(!root.join("task_graphs.json").exists() && !root.join(".msg_counter").exists());
    drop(source);drop(inventory);drop(work);registry.shutdown().await.unwrap();f.stop().await;
}

#[tokio::test]
async fn cancelled_coordinator_waiter_keeps_same_drain_and_real_domain_lock() {
    let f = Fixture::new().await;
    let (work, registry, _live) = prepared(&f).await;
    let calls = f.owner.0.calls.lock().await;
    let first = tokio::spawn({
        let work = work.clone();
        async move { work.close_and_drain().await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    first.abort();
    let second = tokio::spawn({
        let work = work.clone();
        async move { work.close_and_drain().await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!second.is_finished());
    drop(calls);
    let proof = second.await.unwrap().unwrap();
    let status = proof.status().unwrap();
    assert!(!status.external_roots_complete);
    assert_eq!(status.adoption, "not_enabled");
    assert!(
        status
            .blockers
            .contains(&"installation_writer_coverage_not_complete")
    );
    assert!(status.blockers.contains(&"personal_history_not_present"));
    let repeated = work.close_and_drain().await.unwrap();
    assert!(Arc::ptr_eq(&proof, &repeated));
    {
        let files = proof.files.lock().unwrap();
        files.check().unwrap();
    }
    let directory = std::fs::read_dir(f.owner.0.host.state_directory().join("owned-agent-owners"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("management_")
        })
        .unwrap()
        .path();
    let contender = hagency_store::private::open(&directory.join("domain.lock"), false).unwrap();
    assert!(
        contender.try_lock().is_err(),
        "the original domain lock remains exclusively held"
    );
    assert!(f.owner.management(package()).await.is_err());
    drop(proof);
    drop(repeated);
    drop(work);
    contender.try_lock().unwrap();
    drop(contender);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn busy_personal_writer_cannot_become_success_on_second_wait() {
    let f = Fixture::new().await;
    let (ledger, path) = f.personal();
    let (work, registry, _live) = prepared(&f).await;
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let before = std::fs::read(&path).unwrap();
    let first = work.close_and_drain().await.err().unwrap();
    assert_eq!(first.code, "source_personal_busy");
    writer.execute_batch("ROLLBACK").unwrap();
    let second = work.close_and_drain().await.err().unwrap();
    assert_eq!(second.code, first.code);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    drop(work);
    drop(writer);
    drop(ledger);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn personal_writer_lock_is_retained_without_main_or_wal_changes() {
    let f = Fixture::new().await;
    let (ledger, path) = f.personal();
    let wal_keeper = rusqlite::Connection::open(&path).unwrap();
    wal_keeper.execute_batch("PRAGMA journal_mode=WAL").unwrap();
    let version: i64 = wal_keeper
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    wal_keeper
        .pragma_update(None, "user_version", version)
        .unwrap();
    let (work, registry, _live) = prepared(&f).await;
    let before = std::fs::read(&path).unwrap();
    let wal = path.with_file_name("agent-local.sqlite-wal");
    let before_wal = std::fs::read(&wal).unwrap();
    let proof = work.close_and_drain().await.unwrap();
    assert!(proof.status().unwrap().personal_database_present);
    let contender = rusqlite::Connection::open(&path).unwrap();
    contender
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
    contender.busy_timeout(Duration::from_millis(10)).unwrap();
    assert!(contender.execute_batch("BEGIN IMMEDIATE").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(&wal).unwrap(), before_wal);
    drop(proof);
    drop(work);
    contender
        .execute_batch("BEGIN IMMEDIATE; ROLLBACK")
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(&wal).unwrap(), before_wal);
    drop(contender);
    drop(wal_keeper);
    drop(ledger);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn current_host_gate_and_original_root_lock_remain_required() {
    let f = Fixture::new().await;
    let (work, registry, live) = prepared(&f).await;
    let proof = work.close_and_drain().await.unwrap();
    live.store(false, Ordering::Release);
    assert!(proof.status().is_err());
    drop(proof);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;

    let f = Fixture::new().await;
    let (work, registry, _live) = prepared(&f).await;
    let root = f.owner.0.host.state_directory();
    std::fs::rename(
        root.join(".owner-client.lock"),
        root.join(".old-owner-lock"),
    )
    .unwrap();
    hagency_store::private::open(&root.join(".owner-client.lock"), true).unwrap();
    assert!(f.owner.0.host.check_state_lock().is_err());
    let error = work.close_and_drain().await.err().unwrap();
    assert_eq!(error.code, "source_root_changed");
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn changed_sdk_source_before_confirmation_does_not_retire_owner() {
    let f = Fixture::new().await;
    let (work, registry, _live) = prepared(&f).await;
    f.live.store(false, Ordering::Release);
    let error = work.close_and_drain().await.err().unwrap();
    assert_eq!(error.code, "source_quiescence_authority_changed");
    assert!(!f.owner.0.configuration_stopped.load(Ordering::Acquire));
    assert!(matches!(*work.phase.lock().await, Phase::Ready));
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}
