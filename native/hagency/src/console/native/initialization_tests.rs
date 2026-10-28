use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Source(Arc<AtomicUsize>);
impl MatrixTokenSource for Source {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Box::pin(async {
            self.0.fetch_add(1, Ordering::AcqRel);
            Err(failure(503, "matrix_authorization_unavailable"))
        })
    }
}

#[tokio::test]
async fn guarded_owner_rejection_precedes_sdk_authorization_and_all_worker_startup() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let retained = Arc::new(StdMutex::new(None));
    let capture = retained.clone();
    let result = NativeOwner::open_with_matrix_guarded(
        &root.path().join("owner"),
        &origin,
        "@owner:test",
        Arc::new(Source(calls.clone())),
        move |owner| {
            assert!(owner.0.configuration_tasks.lock().unwrap().is_empty());
            *capture.lock().unwrap() = Some(owner.clone());
            Err(failure(401, "matrix_account_changed"))
        },
    )
    .await;
    assert_eq!(result.err().unwrap().code, "matrix_account_changed");
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
    let owner = retained.lock().unwrap().take().unwrap();
    assert!(owner.identity().await.is_err());
    assert!(owner.0.configuration_tasks.lock().unwrap().is_empty());
    assert!(owner.0.configuration_stopped.load(Ordering::Acquire));
}

#[tokio::test]
async fn guarded_owner_retirement_inside_successful_guard_never_authorizes() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let result = NativeOwner::open_with_matrix_guarded(
        &root.path().join("owner"),
        &origin,
        "@owner:test",
        Arc::new(Source(calls.clone())),
        |owner| {
            owner.retire();
            Ok(())
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}
