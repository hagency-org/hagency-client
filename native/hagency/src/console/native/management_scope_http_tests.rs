use super::*;
use hagency_store::management as service;

#[tokio::test]
#[ignore = "real finite SDK scopes; waits across original deadline with delayed loopback HTTP"]
async fn native_management_fresh_scope_survives_response_after_old_thirteen_second_deadline() {
    let f = Fixture::new().await;
    let (context, old) = f.transport.manager.authorized().await.unwrap();
    let old_until = old.valid_until();
    tokio::time::sleep_until((old_until - Duration::from_secs(13)).into()).await;
    let remaining = old_until.saturating_duration_since(Instant::now());
    assert!(remaining > Duration::from_secs(11) && remaining <= Duration::from_secs(14));
    let scope = f.transport.scope(Permission::PublishGrants).await.unwrap();
    f.state.lock().unwrap().delay_contribution_until = Some(old_until + Duration::from_millis(250));
    let reply: Value = f
        .transport
        .request(&scope, Operation::Contribution("con_fixture".into()), false)
        .await
        .expect("fresh authenticated scope covers successful response past old scope expiry");
    assert_eq!(reply["admission"]["contribution"]["id"], "con_fixture");
    assert!(scope.deadline > old_until + Duration::from_secs(10));
    assert_eq!(old.valid_until(), old_until);
    assert!(!old.is_live());
    scope
        .session
        .authorize(&context, Permission::PublishGrants)
        .unwrap();
    f.transport.manager.retire();
    assert!(!scope.session.is_live());
    f.stop().await;
}

#[tokio::test]
#[ignore = "real concurrent worker, UI and grant-review scopes cross repeated finite SDK renewals"]
async fn native_management_three_concurrent_managers_keep_unused_overlap_out_of_bound() {
    let f = Fixture::new().await;
    let mut ui_package = package(91);
    ui_package.permissions.push(Permission::ReadResources);
    let ui = f.owner.management(ui_package).await.unwrap();
    let mut grant_package = package(92);
    grant_package.permissions.push(Permission::ConsentGrants);
    let grant = f.owner.management(grant_package).await.unwrap();
    let (_, original) = f.transport.manager.authorized().await.unwrap();
    let initial_until = original.valid_until();
    let until = Instant::now() + Duration::from_secs(36);
    let mut rounds = 0;
    while Instant::now() < until {
        let (worker, visible, review) = tokio::join!(
            f.transport.scope(Permission::ExecuteAllocations),
            ui.resources(service::ResourceQuery::default()),
            grant.authorized()
        );
        let worker = worker.unwrap();
        visible.unwrap();
        let (context, review) = review.unwrap();
        review
            .authorize(&context, Permission::ConsentGrants)
            .unwrap();
        assert!(
            f.owner.0.management.live_scope_count() <= 5,
            "only three current scopes plus held original/in-flight references"
        );
        drop(worker);
        drop(review);
        rounds += 1;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(rounds >= 10);
    assert_eq!(original.valid_until(), initial_until);
    assert!(!original.is_live());
    let (ui_context, ui_scope) = ui.authorized().await.unwrap();
    let (_, grant_scope) = grant.authorized().await.unwrap();
    ui.retire();
    assert!(!ui_scope.is_live());
    assert!(grant_scope.is_live());
    assert!(
        ui_scope
            .authorize(&ui_context, Permission::ReadResources)
            .is_err()
    );
    f.owner.retire();
    assert!(!grant_scope.is_live());
    f.stop().await;
}

#[tokio::test]
async fn native_management_context_change_and_package_drop_fence_original_proofs() {
    let f = Fixture::new().await;
    let manager = f.owner.management(package(94)).await.unwrap();
    let (old_context, old) = manager.authorized().await.unwrap();
    explicit_new_generation(&f, 2).await;
    let (current_context, current) = manager.authorized().await.unwrap();
    assert_ne!(old_context, current_context);
    assert!(!old.is_live());
    assert!(current.is_live());
    drop(manager);
    assert!(
        !current.is_live(),
        "dropping package retires its issued strong copies"
    );
    f.stop().await;
}
