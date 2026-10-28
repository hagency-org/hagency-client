use super::*;

#[tokio::test]
async fn cancelled_waiter_and_concurrent_waiter_keep_original_task() {
    let custody = TaskCustody::default();
    let (release, blocked) = tokio::sync::oneshot::channel();
    custody.track(tokio::spawn(async move {
        blocked.await.unwrap();
    }));
    let first = tokio::spawn({
        let c = custody.clone();
        async move { c.drain().await }
    });
    tokio::task::yield_now().await;
    first.abort();
    let second = tokio::spawn({
        let c = custody.clone();
        async move { c.drain().await }
    });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());
    release.send(()).unwrap();
    assert_eq!(second.await.unwrap(), Ok(()));
    assert_eq!(custody.drain().await, Ok(()));
    assert!(custody.enter().is_err());
}

#[tokio::test]
async fn panic_and_aborted_constructor_never_become_empty_success() {
    let custody = TaskCustody::default();
    custody.track(tokio::spawn(async {
        panic!("synthetic worker failure");
    }));
    assert_eq!(custody.drain().await, Err(DrainFailure::Unknown));
    assert_eq!(custody.drain().await, Err(DrainFailure::Unknown));
    let custody = TaskCustody::default();
    drop(custody.checked().unwrap());
    assert_eq!(custody.drain().await, Err(DrainFailure::Unknown));
}

#[tokio::test]
async fn constructor_race_keeps_late_cleanup_in_drain() {
    let custody = TaskCustody::default();
    let activity = custody.enter().unwrap();
    let pending = tokio::spawn({
        let c = custody.clone();
        async move { c.drain().await }
    });
    tokio::task::yield_now().await;
    let (release, blocked) = tokio::sync::oneshot::channel();
    custody.cleanup(async move {
        blocked.await.unwrap();
        Ok(())
    });
    drop(activity);
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    release.send(()).unwrap();
    assert_eq!(pending.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn finished_join_is_not_observed_cleanup() {
    let raw = tokio::spawn(async { Ok(()) });
    let retained = RetainedTask {
        abort: raw.abort_handle(),
        id: raw.id(),
        state: Arc::new(Mutex::new(JoinState {
            task: Some(raw),
            result: None,
        })),
    };
    while !retained.is_finished() {
        tokio::task::yield_now().await;
    }
    assert_eq!(retained.observed_result(), None);
    assert_eq!(retained.join().await, Ok(()));
    assert_eq!(retained.observed_result(), Some(Ok(())));
    let raw = tokio::spawn(std::future::pending::<Result<(), DrainFailure>>());
    let aborted = RetainedTask {
        abort: raw.abort_handle(),
        id: raw.id(),
        state: Arc::new(Mutex::new(JoinState {
            task: Some(raw),
            result: None,
        })),
    };
    aborted.abort();
    assert_eq!(aborted.join().await, Err(DrainFailure::Unknown));
    assert_eq!(aborted.observed_result(), Some(Err(DrainFailure::Unknown)));
}
