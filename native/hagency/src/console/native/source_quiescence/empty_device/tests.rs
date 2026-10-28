use super::*;
use crate::console::native::reconciliation_sources::tests::Fixture;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn package() -> ManagementPackage {
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
pub(super) async fn prepared(
    f: &Fixture,
    empty: bool,
) -> (
    SourceQuiescence,
    OwnerResourceRegistry,
    Arc<AtomicBool>,
    std::path::PathBuf,
) {
    let (repository, path, _) = f.domain();
    drop(repository);
    if empty {
        let db = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
        db.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
        db.execute("DELETE FROM resources", []).unwrap();
    }
    let manager = f.owner.management(package()).await.unwrap();
    let registry =
        OwnerResourceRegistry::isolated_for_tests(&f.temp.path().join("registry")).unwrap();
    let live = Arc::new(AtomicBool::new(true));
    let check = live.clone();
    let mut request = SourceQuiescenceRequest::new(
        &f.owner,
        package(),
        registry.clone(),
        Instant::now() + Duration::from_secs(30),
        move || check.load(Ordering::Acquire),
    )
    .unwrap();
    request.lease_root = Some(f.temp.path().join("installation"));
    let work = f.owner.prepare_source_quiescence(request).await.unwrap();
    drop(manager);
    (work, registry, live, path)
}
fn hashes(path: &Path) -> Vec<(String, String)> {
    use sha2::Digest;
    let mut values = std::fs::read_dir(path)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            (entry.file_type().unwrap().is_file()
                && !entry.file_name().to_string_lossy().ends_with("-shm"))
            .then(|| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    format!(
                        "{:x}",
                        sha2::Sha256::digest(std::fs::read(entry.path()).unwrap())
                    ),
                )
            })
        })
        .collect::<Vec<_>>();
    values.sort();
    values
}

#[tokio::test]
async fn preview_and_cancellation_keep_original_owner_usable_and_files_unchanged() {
    let f = Fixture::new().await;
    let (work, registry, live, path) = prepared(&f, true).await;
    let manager = f.owner.management(package()).await.unwrap();
    let before = hashes(&path);
    let review = work.review_empty_device().await.unwrap();
    assert!(!review.personal_present());
    assert_eq!(review.completed_private_records(), 0);
    assert_eq!(before, hashes(&path));
    assert!(
        manager
            .resources(Default::default())
            .await
            .unwrap()
            .resources
            .is_empty()
    );
    // Cancelling this native review does not cancel the Matrix session or close
    // the original worker. Neither a provider nor a personal opener was used.
    live.store(false, Ordering::Release);
    assert!(work.review_empty_device().await.is_err());
    drop(review);
    drop(work);
    assert!(
        manager
            .resources(Default::default())
            .await
            .unwrap()
            .resources
            .is_empty()
    );
    assert_eq!(before, hashes(&path));
    assert!(
        !std::fs::read_dir(path.parent().unwrap()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("owner_"))
    );
    drop(manager);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn unchanged_preview_matches_only_after_actual_source_drain() {
    let f = Fixture::new().await;
    let (work, registry, _live, _path) = prepared(&f, true).await;
    let review = work.review_empty_device().await.unwrap();
    let source = work.close_and_drain().await.unwrap();
    let inventory = source.validate_empty_device_review(&review).await.unwrap();
    assert_eq!(
        inventory.review_digest().unwrap(),
        source
            .empty_device_inventory()
            .await
            .unwrap()
            .review_digest()
            .unwrap()
    );
    assert_eq!(
        work.review_empty_device().await.err().unwrap().code,
        "empty_device_review_requires_live_source"
    );
    drop(inventory);
    drop(review);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn a_still_empty_source_with_changed_settings_cannot_use_an_old_review() {
    let f = Fixture::new().await;
    let (ledger, personal) = f.personal();
    drop(ledger);
    let (work, registry, _live, _path) = prepared(&f, true).await;
    let review = work.review_empty_device().await.unwrap();
    assert!(review.personal_present());
    let db = rusqlite::Connection::open(&personal).unwrap();
    db.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )
    .unwrap();
    db.execute(
        "UPDATE inbox_limits SET max_records=max_records+1 WHERE singleton=1",
        [],
    )
    .unwrap();
    drop(db);
    let source = work.close_and_drain().await.unwrap();
    assert_eq!(
        source
            .validate_empty_device_review(&review)
            .await
            .err()
            .unwrap()
            .code,
        "empty_device_review_changed"
    );
    // Still an empty installation; refusal comes from the changed reviewed data.
    source.empty_device_inventory().await.unwrap();
    drop(review);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn another_native_review_cannot_be_substituted_for_the_original_scope() {
    let f = Fixture::new().await;
    let (work, registry, _live, _path) = prepared(&f, true).await;
    let mut request = SourceQuiescenceRequest::new(
        &f.owner,
        package(),
        registry.clone(),
        Instant::now() + Duration::from_secs(30),
        || true,
    )
    .unwrap();
    request.lease_root = Some(f.temp.path().join("installation"));
    let other = f.owner.prepare_source_quiescence(request).await.unwrap();
    let review = other.review_empty_device().await.unwrap();
    let source = work.close_and_drain().await.unwrap();
    assert_eq!(
        source
            .validate_empty_device_review(&review)
            .await
            .err()
            .unwrap()
            .code,
        "empty_device_review_scope_changed"
    );
    drop(review);
    drop(other);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn changed_sdk_session_cannot_preview_or_retire_the_original_owner() {
    let f = Fixture::new().await;
    let (work, registry, _live, path) = prepared(&f, true).await;
    let before = hashes(&path);
    f.live.store(false, Ordering::Release);
    assert_eq!(
        work.review_empty_device().await.err().unwrap().code,
        "source_quiescence_authority_changed"
    );
    assert_eq!(before, hashes(&path));
    f.live.store(true, Ordering::Release);
    let manager = f.owner.management(package()).await.unwrap();
    assert!(
        manager
            .resources(Default::default())
            .await
            .unwrap()
            .resources
            .is_empty()
    );
    drop(manager);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn absent_personal_source_is_not_created_and_original_lock_stays_held() {
    let f = Fixture::new().await;
    let (work, registry, _live, path) = prepared(&f, true).await;
    let source = work.close_and_drain().await.unwrap();
    let before = hashes(&path);
    let inventory = source.empty_device_inventory().await.unwrap();
    assert!(!inventory.personal_present());
    assert_eq!(inventory.completed_private_records(), 0);
    assert_eq!(
        inventory.review_digest().unwrap(),
        source
            .empty_device_inventory()
            .await
            .unwrap()
            .review_digest()
            .unwrap()
    );
    assert_eq!(before, hashes(&path));
    assert!(
        !std::fs::read_dir(path.parent().unwrap()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("owner_"))
    );
    let contender = hagency_store::private::open(&path.join("domain.lock"), false).unwrap();
    drop(source);
    drop(work);
    assert!(
        contender.try_lock().is_err(),
        "evidence retains original custody"
    );
    drop(inventory);
    contender.try_lock().unwrap();
    drop(contender);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn present_empty_personal_source_is_preserved_without_provider_or_fencing() {
    let f = Fixture::new().await;
    let (ledger, personal) = f.personal();
    drop(ledger);
    let (work, registry, _live, path) = prepared(&f, true).await;
    let source = work.close_and_drain().await.unwrap();
    let before = hashes(&path);
    let personal_before = hashes(personal.parent().unwrap());
    let inventory = source.empty_device_inventory().await.unwrap();
    assert!(inventory.personal_present());
    inventory.review_digest().unwrap();
    assert_eq!(before, hashes(&path));
    assert_eq!(personal_before, hashes(personal.parent().unwrap()));
    let db = rusqlite::Connection::open_with_flags(
        f.temp.path().join("registry/domain.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    for table in [
        "resources",
        "seats",
        "owner_resource_adoptions",
        "owner_capacity_grants",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
    drop(db);
    drop(inventory);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn a_new_personal_path_cannot_reuse_absence_observed_under_custody() {
    let f = Fixture::new().await;
    let (work, registry, _live, path) = prepared(&f, true).await;
    let source = work.close_and_drain().await.unwrap();
    let inventory = source.empty_device_inventory().await.unwrap();
    let profile = path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix("management_")
        .unwrap();
    hagency_store::private::directory(&path.parent().unwrap().join(format!("owner_{profile}")))
        .unwrap();
    assert!(inventory.review_digest().is_err());
    assert!(source.empty_device_inventory().await.is_err());
    drop(inventory);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}

#[tokio::test]
async fn existing_resources_and_cancelled_native_gates_cannot_initialize_as_empty() {
    let f = Fixture::new().await;
    let (work, registry, _live, _path) = prepared(&f, false).await;
    let source = work.close_and_drain().await.unwrap();
    assert_eq!(
        source.empty_device_inventory().await.err().unwrap().code,
        "empty_device_history_not_empty"
    );
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
    let f = Fixture::new().await;
    let (work, registry, live, _path) = prepared(&f, true).await;
    let source = work.close_and_drain().await.unwrap();
    let inventory = source.empty_device_inventory().await.unwrap();
    live.store(false, Ordering::Release);
    assert!(inventory.review_digest().is_err());
    assert!(source.empty_device_inventory().await.is_err());
    drop(inventory);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}
