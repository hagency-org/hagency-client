use super::*;
use crate::console::native::reconciliation_sources::tests::Fixture;
use super::super::tests::{package, prepared};
use std::path::Path;
mod fencing;
mod activation;

const DESCRIPTOR: &str = "adopted-management-initializer-v1.json";
fn count(root: &Path, table: &str) -> i64 {
    let db = rusqlite::Connection::open_with_flags(
        root.join("registry/domain.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
async fn reopened_source(f: &Fixture, registry: OwnerResourceRegistry) -> SourceQuiescence {
    let mut request = SourceQuiescenceRequest::new(
        &f.owner,
        package(),
        registry,
        Instant::now() + Duration::from_secs(30),
        || true,
    )
    .unwrap();
    request.lease_root = Some(f.temp.path().join("installation"));
    f.owner.prepare_source_quiescence(request).await.unwrap()
}

async fn restart_at_preparation_boundary(after_commit: bool) {
    let f = Fixture::new().await;
    let (work, registry, _live, path) = prepared(&f, true).await;
    let state = f.owner.0.host.state_directory().to_owned();
    let review = work.review_empty_device().await.unwrap();
    let source = work.close_and_drain().await.unwrap();
    let observed = source.validate_empty_device_review(&review).await.unwrap();
    let staged = source
        .stage_empty_initialization("original_setup".into(), observed, None)
        .await
        .unwrap();
    let original = std::fs::read(state.join(DESCRIPTOR)).unwrap();
    let encoded: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(encoded["version"], 3);
    assert!(
        encoded["emptyPrepare"]
            .get("monthlyAccountTokens")
            .is_none()
    );
    assert_eq!(
        count(f.temp.path(), "owner_resource_adoptions"),
        0,
        "native descriptor precedes canonical commit"
    );
    if after_commit {
        let pending = staged.commit().await.unwrap();
        assert_eq!(pending.summary().await.unwrap().phase, "prepared");
        drop(pending);
    } else {
        drop(staged);
    }
    drop(review);
    drop(source);
    drop(work);
    // Fully close and reopen NativeOwner, including its original root lock,
    // SDK authorization and source-only recovery mode. This is not store-only.
    let f = f
        .restart_for_adoption(registry.clone(), "original_setup".into())
        .await;
    assert_eq!(std::fs::read(state.join(DESCRIPTOR)).unwrap(), original);
    assert_eq!(
        count(f.temp.path(), "owner_resource_adoptions"),
        i64::from(after_commit)
    );
    assert!(
        f.owner.management(package()).await.is_err(),
        "preparation must not expose an active manager"
    );
    let work = reopened_source(&f, registry.clone()).await;
    let source = work.close_and_drain().await.unwrap();
    assert!(
        source
            .resume_empty_device_initialization("replacement_setup".into())
            .await
            .is_err()
    );
    let completed = source
        .resume_empty_device_initialization("original_setup".into())
        .await
        .unwrap();
    assert_eq!(
        completed.summary().await.unwrap().initialization_id,
        "original_setup"
    );
    assert_eq!(completed.summary().await.unwrap().phase, "prepared");
    assert_eq!(count(f.temp.path(), "owner_resource_adoptions"), 1);
    for table in [
        "managed_accounts",
        "resources",
        "seats",
        "owner_capacity_grants",
    ] {
        assert_eq!(count(f.temp.path(), table), 0, "unexpected {table}");
    }
    assert_eq!(std::fs::read(state.join(DESCRIPTOR)).unwrap(), original);
    assert!(
        !std::fs::read_dir(path.parent().unwrap()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("owner_"))
    );
    drop(completed);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}
#[tokio::test]
async fn empty_native_restart_between_descriptor_and_database_uses_original_preparation() {
    restart_at_preparation_boundary(false).await;
}
#[tokio::test]
async fn empty_native_restart_after_committed_result_loss_does_not_duplicate_setup() {
    restart_at_preparation_boundary(true).await;
}
#[tokio::test]
async fn original_native_handoff_slot_excludes_parallel_initialization() {
    let f = Fixture::new().await;
    let (work, registry, _live, _path) = prepared(&f, true).await;
    let review = work.review_empty_device().await.unwrap();
    let source = work.close_and_drain().await.unwrap();
    let original = source
        .prepare_empty_device_initialization("original_setup".into(), &review)
        .await
        .unwrap();
    assert_eq!(
        source
            .resume_empty_device_initialization("original_setup".into())
            .await
            .err()
            .unwrap()
            .code,
        "source_adoption_busy"
    );
    assert!(
        source
            .prepare_empty_device_initialization("replacement".into(), &review)
            .await
            .is_err()
    );
    drop(original);
    let recovered = source
        .resume_empty_device_initialization("original_setup".into())
        .await
        .unwrap();
    assert_eq!(count(f.temp.path(), "owner_resource_adoptions"), 1);
    drop(recovered);
    drop(review);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}
#[tokio::test]
async fn a_failed_native_descriptor_write_cannot_create_a_canonical_preparation() {
    let f = Fixture::new().await;
    let (work, registry, _live, _path) = prepared(&f, true).await;
    let review = work.review_empty_device().await.unwrap();
    let source = work.close_and_drain().await.unwrap();
    let path = f.owner.0.host.state_directory().join(DESCRIPTOR);
    hagency_store::private::write_new(&path, b"retained unreadable original").unwrap();
    assert!(
        source
            .prepare_empty_device_initialization("original_setup".into(), &review)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(path).unwrap(),
        b"retained unreadable original"
    );
    assert_eq!(count(f.temp.path(), "owner_resource_adoptions"), 0);
    drop(review);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}
#[tokio::test]
async fn changed_history_or_device_cannot_reconstruct_a_missing_canonical_commit() {
    let f = Fixture::new().await;
    let (work, registry, _live, path) = prepared(&f, true).await;
    let review = work.review_empty_device().await.unwrap();
    let source = work.close_and_drain().await.unwrap();
    let current = source.validate_empty_device_review(&review).await.unwrap();
    let staged = source
        .stage_empty_initialization("original_setup".into(), current, None)
        .await
        .unwrap();
    drop(staged);
    let descriptor = f.owner.0.host.state_directory().join(DESCRIPTOR);
    let original = std::fs::read(&descriptor).unwrap();
    let mut altered: serde_json::Value = serde_json::from_slice(&original).unwrap();
    altered["originalDevice"] = serde_json::json!("replacement-device");
    hagency_store::private::replace(&descriptor, &serde_json::to_vec(&altered).unwrap()).unwrap();
    assert!(
        source
            .resume_empty_device_initialization("original_setup".into())
            .await
            .is_err()
    );
    hagency_store::private::replace(&descriptor, &original).unwrap();
    let db = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    db.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )
    .unwrap();
    db.execute("INSERT INTO managed_accounts(id,ordinal,generation,state) VALUES('retained-account',1,1,'preparing')",[]).unwrap();
    drop(db);
    assert_eq!(
        source
            .resume_empty_device_initialization("original_setup".into())
            .await
            .err()
            .unwrap()
            .code,
        "empty_device_history_not_empty"
    );
    assert_eq!(count(f.temp.path(), "owner_resource_adoptions"), 0);
    assert_eq!(std::fs::read(descriptor).unwrap(), original);
    drop(review);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}
