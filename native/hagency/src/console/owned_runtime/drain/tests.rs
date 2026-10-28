use super::*;

#[tokio::test]
async fn removed_runtime_handle_stays_in_custody_until_actual_ledger_drop() {
    let manager = OwnedRuntime::default();
    let directory = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let path = directory.path().join("ledger.db");
    let (ready, started) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let task = manager.tasks.track(tokio::spawn(async move {
        let _ledger = Ledger::open_scoped(&path, "@owner:test", &"a".repeat(64)).unwrap();
        ready.send(()).unwrap();
        blocked.await.unwrap();
    }));
    started.await.unwrap();
    let first = tokio::spawn(async move { task.await });
    tokio::task::yield_now().await;
    first.abort();
    let second = tokio::spawn({
        let manager = manager.clone();
        async move { manager.drain_for_migration().await }
    });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());
    release.send(()).unwrap();
    assert_eq!(second.await.unwrap(), Ok(()));
    assert!(manager.tasks.enter().is_err());
}

#[tokio::test]
async fn ordinary_stop_keeps_removed_task_for_concurrent_cancelled_callers() {
    let manager = OwnedRuntime::default();
    let (cancel, mut receiver) = watch::channel(false);
    let (_, status) = watch::channel(RuntimeStatus::new("agent", "fixture", None));
    let (seen, observed) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let task = manager.tasks.track(tokio::spawn(async move {
        canceled(&mut receiver).await;
        seen.send(()).unwrap();
        blocked.await.unwrap();
    }));
    let (device, _) = super::super::super::server_login::fixture_device(
        "https://server.example/",
        "subject",
        "@owner:test",
    );
    manager.entries.lock().await.insert(
        "fixture".into(),
        Entry {
            commands: mpsc::unbounded_channel().0,
            bindings: BTreeMap::new(),
            cancel,
            status,
            task,
            device: DevicePin::from(&device),
            approvals: BTreeMap::new(),
        },
    );
    let first = tokio::spawn({
        let manager = manager.clone();
        async move { manager.stop_all().await }
    });
    observed.await.unwrap();
    first.abort();
    let second = tokio::spawn({
        let manager = manager.clone();
        async move { manager.stop_all().await }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!second.is_finished());
    assert_eq!(manager.entries.lock().await.len(), 1);
    release.send(()).unwrap();
    second.await.unwrap();
    assert!(manager.entries.lock().await.is_empty());
    assert!(manager.tasks.enter().is_ok());
    assert_eq!(manager.drain_for_migration().await, Ok(()));
}
