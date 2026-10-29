use super::*;
use matrix_sdk::{
    media::{MediaFormat, MediaThumbnailSettings},
    ruma::api::MatrixVersion,
};
use std::sync::atomic::{AtomicBool, Ordering};
fn current() -> Current {
    Arc::new(|| true)
}
fn request(id: &str) -> MediaRequestParameters {
    MediaRequestParameters {
        source: matrix_sdk::ruma::events::room::MediaSource::Plain(
            format!("mxc://example.org/{id}").into(),
        ),
        format: MediaFormat::File,
    }
}
async fn client() -> Client {
    Client::builder()
        .homeserver_url("http://127.0.0.1:1")
        .server_versions([MatrixVersion::V1_11])
        .build()
        .await
        .unwrap()
}
async fn state_when(worker: &Worker, condition: impl Fn(&State) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if condition(&worker.state.lock().unwrap()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("actual owned worker state transition");
}
fn read_task(
    worker: &Arc<Worker>,
    client: &Client,
    request: MediaRequestParameters,
    scope: Current,
) -> tokio::task::JoinHandle<Result<Option<Vec<u8>>, String>> {
    let worker = worker.clone();
    let client = client.clone();
    tokio::spawn(async move { read_from(&worker, client, request, 1024, scope).await })
}

#[tokio::test]
async fn cancelled_original_read_keeps_one_worker_and_bounds_later_queue() {
    let worker = Worker::start();
    let (client, gate) = super::controlled_store::controlled_client().await;
    let first = read_task(&worker, &client, request("first"), current());
    gate.started().await;
    first.abort();
    let _ = first.await;
    // Original SDK get future remains owned even when its observer is gone.
    assert!(worker.state.lock().unwrap().active);
    let mut later = Vec::new();
    for n in 0..MAX_QUEUED {
        later.push(read_task(
            &worker,
            &client,
            request(&format!("queued-{n}")),
            current(),
        ));
        state_when(&worker, |state| state.queue.len() == n + 1).await;
    }
    assert!(
        read_from(
            &worker,
            client.clone(),
            request("overflow"),
            1024,
            current()
        )
        .await
        .is_err()
    );
    assert_eq!(gate.entered.load(Ordering::Acquire), 1);
    for task in later {
        task.abort();
        let _ = task.await;
    }
    gate.release();
    state_when(&worker, |state| !state.active && state.queue.is_empty()).await;
    assert_eq!(
        read_from(&worker, client, request("recovered"), 1024, current())
            .await
            .unwrap(),
        None
    );
    assert!(worker.stop(Instant::now() + Duration::from_secs(5)));
}

#[tokio::test]
async fn cancelled_write_keeps_original_bytes_until_actual_sdk_future_returns() {
    let worker = Worker::start();
    let (client, gate) = super::controlled_store::controlled_client().await;
    let owned = worker.clone();
    let original = client.clone();
    let payload = vec![7; MAX_WRITE_BYTES as usize];
    let first = tokio::spawn(async move {
        write_to(&owned, original, request("write"), &payload, current()).await
    });
    gate.started().await;
    assert_eq!(worker.writes.used(), MAX_WRITE_BYTES);
    first.abort();
    let _ = first.await;
    assert_eq!(worker.writes.used(), MAX_WRITE_BYTES);
    assert!(
        write_to(
            &worker,
            client.clone(),
            request("overflow"),
            &[1],
            current()
        )
        .await
        .is_err()
    );
    assert!(
        !worker.stop(Instant::now()),
        "shutdown cannot claim blocked SDK IO finished"
    );
    assert_eq!(worker.writes.used(), MAX_WRITE_BYTES);
    gate.release();
    state_when(&worker, |state| !state.active).await;
    assert!(worker.stop(Instant::now() + Duration::from_secs(5)));
    assert_eq!(worker.writes.used(), 0);
    // Cancellation alone never drops an already started SDK operation.
    assert_eq!(
        client
            .media_store()
            .lock()
            .await
            .unwrap()
            .get_media_content_bounded(&request("write"), MAX_WRITE_BYTES as usize)
            .await
            .unwrap()
            .unwrap()
            .len(),
        MAX_WRITE_BYTES as usize
    );
}

#[tokio::test]
async fn retired_scope_suppresses_active_results_and_skips_queued_writes() {
    let worker = Worker::start();
    let (client, gate) = super::controlled_store::controlled_client().await;
    let live = Arc::new(AtomicBool::new(true));
    let flag = live.clone();
    let scope: Current = Arc::new(move || flag.load(Ordering::Acquire));
    let first = read_task(&worker, &client, request("active"), scope.clone());
    gate.started().await;
    let owned = worker.clone();
    let original = client.clone();
    let queued =
        tokio::spawn(
            async move { write_to(&owned, original, request("retired"), b"old", scope).await },
        );
    state_when(&worker, |state| state.queue.len() == 1).await;
    live.store(false, Ordering::Release);
    gate.release();
    assert!(first.await.unwrap().is_err());
    assert!(queued.await.unwrap().is_err());
    state_when(&worker, |state| !state.active && state.queue.is_empty()).await;
    assert_eq!(worker.writes.used(), 0);
    assert_eq!(
        read_from(&worker, client.clone(), request("retired"), 1024, current())
            .await
            .unwrap(),
        None
    );
    assert!(worker.stop(Instant::now() + Duration::from_secs(5)));
    assert!(
        read_from(&worker, client, request("closed"), 1024, current())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn actual_sqlite_preserves_formats_and_rejects_oversize_before_returning_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let client = Client::builder()
        .homeserver_url("http://127.0.0.1:1")
        .server_versions([MatrixVersion::V1_11])
        .sqlite_store(directory.path(), None)
        .build()
        .await
        .unwrap();
    let worker = Worker::start();
    let file = request("formats");
    let mut thumbnail = file.clone();
    thumbnail.format =
        MediaFormat::Thumbnail(MediaThumbnailSettings::new(32u32.into(), 32u32.into()));
    write_to(&worker, client.clone(), file.clone(), b"file", current())
        .await
        .unwrap();
    write_to(
        &worker,
        client.clone(),
        thumbnail.clone(),
        b"thumb",
        current(),
    )
    .await
    .unwrap();
    assert_eq!(
        read_from(&worker, client.clone(), file.clone(), 4, current())
            .await
            .unwrap()
            .unwrap(),
        b"file"
    );
    assert_eq!(
        read_from(&worker, client.clone(), thumbnail, 5, current())
            .await
            .unwrap()
            .unwrap(),
        b"thumb"
    );
    assert!(
        read_from(&worker, client.clone(), file.clone(), 3, current())
            .await
            .is_err()
    );
    assert!(
        read_from(&worker, client.clone(), file, MAX_READ_BYTES + 1, current())
            .await
            .is_err()
    );
    assert!(worker.stop(Instant::now() + Duration::from_secs(5)));
    drop(client);
}

#[tokio::test]
async fn production_scope_rejects_unowned_client_without_keeping_it_alive() {
    let client = client().await;
    let weak = client.downgrade();
    let current = super::scope(&client);
    assert!(
        !current(),
        "an authless, non-CURRENT client has no account authority"
    );
    drop(client);
    assert!(weak.get().is_none(), "idle scope retains only WeakClient");
    assert!(!current());
}
