use super::*;
use crate::console::native::reconciliation_sources::tests::Fixture;
use super::super::tests::package;
use std::sync::atomic::Ordering;

async fn empty() -> (Fixture, std::path::PathBuf) { empty_with_installation(false).await }
async fn empty_with_installation(configured: bool) -> (Fixture, std::path::PathBuf) {
    let f = if configured { Fixture::with_configured_installation().await } else { Fixture::new().await };
    let (db, path, _) = f.domain();
    drop(db);
    let db = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    db.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )
    .unwrap();
    db.execute("DELETE FROM resources", []).unwrap();
    drop(db);
    (f, path)
}
#[tokio::test]
async fn empty_setup_preview_and_cancel_keep_source_and_no_provider_or_quota() {
    let (f, path) = empty().await;
    let before = std::fs::read(path.join("domain.sqlite3-wal")).unwrap();
    let context = f.owner.device_setup_context(package(), None).await.unwrap();
    assert_eq!(context.kind, DeviceSetupKind::EmptyDevice);
    assert_eq!(context.personal_database_present, Some(false));
    assert!(context.resources.is_empty() && context.original_monthly_tokens.is_none());
    let review = f
        .owner
        .review_device_setup(package(), None, None)
        .await
        .unwrap();
    assert!(!review.expired());
    assert!(!review.recovering());
    assert_eq!(review.monthly_tokens(), None);
    drop(review);
    let review = f
        .owner
        .review_device_setup(package(), None, None)
        .await
        .unwrap();
    assert!(
        f.owner
            .confirm_device_setup(review, package(), || false)
            .await
            .is_err()
    );
    let mut review = f
        .owner
        .review_device_setup(package(), None, None)
        .await
        .unwrap();
    let DeviceSetupReview::Empty(inner) = &mut review else {
        panic!("wrong setup kind")
    };
    inner.expires = Instant::now() - Duration::from_secs(1);
    assert!(
        f.owner
            .confirm_device_setup(review, package(), || true)
            .await
            .is_err()
    );
    assert!(
        f.owner
            .review_device_setup(package(), None, Some(700))
            .await
            .is_err()
    );
    assert!(!f.owner.0.configuration_stopped.load(Ordering::Acquire));
    assert!(
        !f.owner
            .0
            .host
            .state_directory()
            .join("adopted-management-initializer-v1.json")
            .exists()
    );
    assert!(std::fs::read(path.join("domain.sqlite3-wal")).unwrap() == before);
    f.stop().await;
}
#[tokio::test]
async fn zero_resource_inventory_with_retained_account_is_not_empty_setup() {
    let (f, _) = empty().await;
    let mut p = package();
    p.permissions.push(Permission::PrepareAccounts);
    let manager = f.owner.management(p).await.unwrap();
    let (context, session) = manager.authorized().await.unwrap();
    let account = session
        .prepare_account(
            &context,
            hagency_store::management::PrepareAccount {
                command_id: "keep_original".into(),
            },
        )
        .await
        .unwrap();
    assert!(!account.account_id.is_empty());
    assert!(f.owner.device_setup_context(package(), None).await.is_err());
    assert!(!f.owner.0.configuration_stopped.load(Ordering::Acquire));
    drop(session);
    drop(manager);
    f.stop().await;
}
#[tokio::test]
async fn existing_resources_keep_their_original_review_and_explicit_ceiling() {
    let f = Fixture::new().await;
    let (mut db, _, resource) = f.domain();
    db.put_seat(&hagency_core::project::Seat {
        id: resource.seat_id.clone(),
        declaration: None,
    })
    .unwrap();
    drop(db);
    let context = f.owner.device_setup_context(package(), None).await.unwrap();
    assert_eq!(context.kind, DeviceSetupKind::ExistingResources);
    assert_eq!(context.resources.len(), 1);
    assert!(
        f.owner
            .review_device_setup(package(), None, None)
            .await
            .is_err()
    );
    let review = f
        .owner
        .review_device_setup(package(), None, Some(700))
        .await
        .unwrap();
    assert_eq!(review.monthly_tokens(), Some(700));
    assert!(matches!(review, DeviceSetupReview::Resources(_)));
    drop(review);
    f.stop().await;
}
#[test]
fn empty_setup_confirmation_and_desktop_reopen_in_isolated_process() {
    for case in ["absent", "present"] {
        let result=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["console::native::resource_activation::empty_setup::tests::setup_process_child","--exact","--nocapture"])
            .env("HAGENCY_EMPTY_SETUP_PROCESS_CASE",case).output().unwrap();
        assert!(result.status.success(),"{} {}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
    }
}

#[tokio::test]
async fn setup_process_child() {
    let present=match std::env::var("HAGENCY_EMPTY_SETUP_PROCESS_CASE").as_deref() { Ok("present")=>true, Ok("absent")=>false, _=>return };
    let installation = tempfile::tempdir().unwrap();
    initialize_resource_activation_fixture(&installation.path().join("installation")).unwrap();
    {
        let (f, path) = empty_with_installation(true).await;
        if present {
            let (personal, _) = f.personal();
            drop(personal);
        }
        let review = f
            .owner
            .review_device_setup(package(), None, None)
            .await
            .unwrap();
        assert_eq!(review.context().personal_database_present, Some(present));
        let selected = f
            .owner
            .confirm_device_setup(review, package(), || true)
            .await
            .unwrap();
        assert!(
            path.join(format!("adoption_{}", selected.migration_id))
                .join("backup.json")
                .exists()
        );
        let f = f.restart_onboarding(selected.clone()).await;
        assert_eq!(
            selected.device_setup_phase(&f.owner).await.unwrap(),
            "active"
        );
        let mut p = package();
        p.permissions
            .extend([Permission::PrepareAccounts, Permission::ReadAccounts]);
        let manager = f.owner.management(p).await.unwrap();
        assert!(manager.account_enrollment_ready());
        assert!(
            manager
                .resources(Default::default())
                .await
                .unwrap()
                .resources
                .is_empty()
        );
        let account = manager
            .prepare_account(hagency_store::management::PrepareAccount {
                command_id: "new_account".into(),
            })
            .await
            .unwrap();
        drop(manager);
        let f = f.restart_onboarding(selected.clone()).await;
        assert_eq!(
            selected.device_setup_phase(&f.owner).await.unwrap(),
            "active"
        );
        let mut p = package();
        p.permissions
            .extend([Permission::PrepareAccounts, Permission::ReadAccounts]);
        let manager = f.owner.management(p).await.unwrap();
        assert_eq!(
            manager
                .prepare_account(hagency_store::management::PrepareAccount {
                    command_id: "new_account".into()
                })
                .await
                .unwrap()
                .account_id,
            account.account_id
        );
        drop(manager);
        let registry = selected.registry.clone();
        drop(selected);
        f.stop().await;
        registry.shutdown().await.unwrap();
    }
}
