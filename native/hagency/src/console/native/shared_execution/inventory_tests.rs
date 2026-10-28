use super::*;

#[tokio::test]
async fn native_shared_successor_inventory_uses_current_device_and_never_creates_custody() {
    let (f, t, c) = fixture().await;
    assert!(
        t.list_successors(&f.registry, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    t.inventory_fixture_claim_original(&f.registry, c.clone())
        .await
        .unwrap();
    let before = rows(&f, "owner_shared_start_intents");
    let page = t.list_successors(&f.registry, None).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].assignment_id, c.assignment_id);
    assert_eq!(page.items[0].request_id, c.reserve_command.request_id);
    assert_eq!(page.items[0].process_state, None);
    assert!(!page.items[0].saved_start);
    assert_eq!(rows(&f, "owner_shared_start_intents"), before);
    assert_eq!(f.state.lock().unwrap().shared.starts, 0);
    explicit_new_generation(&f, 2).await;
    assert!(
        t.list_successors(&f.registry, None)
            .await
            .unwrap()
            .items
            .is_empty(),
        "old-generation records cannot be offered as current-device start targets"
    );
    assert_eq!(rows(&f, "owner_shared_runtime_lifetimes"), 1);
    f.stop().await;
}

#[tokio::test]
async fn native_shared_successor_inventory_requires_executor_permission_and_live_sdk() {
    let (f, t, _) = fixture().await;
    let mut read_only = package(23);
    read_only.permissions = vec![Permission::ReadResources];
    let denied = SharedExecutionTransport::new(f.owner.management(read_only).await.unwrap());
    assert!(denied.list_successors(&f.registry, None).await.is_err());
    f.current.store(false, Ordering::Release);
    assert!(t.list_successors(&f.registry, None).await.is_err());
    assert_eq!(rows(&f, "owner_shared_runtime_lifetimes"), 0);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 0);
    f.stop().await;
}
