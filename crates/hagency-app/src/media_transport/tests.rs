use super::*;
// Test-only explicit scope; production fetch always uses the real session fence.
async fn fetch(client: &Client, request: &MediaRequestParameters) -> matrix_sdk::Result<Vec<u8>> {
    fetch_with_timeout(client, request, Duration::from_secs(30)).await
}
async fn fetch_with_timeout(
    client: &Client,
    request: &MediaRequestParameters,
    timeout: Duration,
) -> matrix_sdk::Result<Vec<u8>> {
    super::fetch_with_current(client, request, timeout, std::sync::Arc::new(|| true)).await
}
use matrix_sdk::ruma::api::MatrixVersion;
use std::{
    io::Write,
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) struct Server {
    pub(crate) url: String,
    pub(crate) requests: Arc<Mutex<Vec<String>>>,
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    pub(crate) fn new(ciphertext: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        let paths = requests.clone();
        let stop = done.clone();
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let Ok((mut socket, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                while !bytes.windows(4).any(|bytes| bytes == b"\r\n\r\n") && bytes.len() < 16384 {
                    match socket.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                    }
                }
                let request = String::from_utf8_lossy(&bytes).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                paths.lock().unwrap().push(request);
                if path.contains("/slow-body") {
                    let _ = write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\n"
                    );
                    std::thread::sleep(Duration::from_millis(500));
                    let _ = socket.write_all(b"preview");
                } else if path.contains("/oversized") {
                    let _ = write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        crate::memory_budget::IMAGE_FILE_BYTES + 1
                    );
                } else if path.contains("/chunked") {
                    let _ = write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                    );
                    let chunk = vec![1; crate::memory_budget::IMAGE_FILE_BYTES / 2 + 1];
                    for _ in 0..2 {
                        let _ = write!(socket, "{:x}\r\n", chunk.len());
                        let _ = socket.write_all(&chunk);
                        let _ = socket.write_all(b"\r\n");
                    }
                    let _ = socket.write_all(b"0\r\n\r\n");
                } else if path.contains("/missing") {
                    let _ = write!(
                        socket,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                } else {
                    let body = if path.contains("/encrypted") {
                        ciphertext.as_slice()
                    } else {
                        b"preview"
                    };
                    let _ = write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(body);
                }
            }
        });
        Self {
            url,
            requests,
            done,
            thread: Some(thread),
        }
    }
    pub(crate) async fn client(&self, version: MatrixVersion) -> Client {
        let client = Client::builder()
            .http_client(
                matrix_sdk::reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap(),
            )
            .homeserver_url(&self.url)
            .server_versions([version])
            .build()
            .await
            .unwrap();
        client.matrix_auth().restore_session(serde_json::from_value(serde_json::json!({"access_token":"fixture-token","user_id":"@test:example.org","device_id":"PREVIEW"})).unwrap(),Default::default()).await.unwrap();
        client
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn request(id: &str) -> MediaRequestParameters {
    MediaRequestParameters {
        source: MediaSource::Plain(format!("mxc://example.org/{id}").into()),
        format: MediaFormat::File,
    }
}

#[tokio::test]
async fn bounded_transport_preserves_authentication_cache_and_attachment_integrity() {
    let mut source = std::io::Cursor::new(b"encrypted preview".to_vec());
    let mut encryptor = matrix_sdk_base::crypto::AttachmentEncryptor::new(&mut source);
    let mut ciphertext = Vec::new();
    encryptor.read_to_end(&mut ciphertext).unwrap();
    let mut descriptor = serde_json::to_value(encryptor.finish()).unwrap();
    descriptor["url"] = "mxc://example.org/encrypted".into();
    let server = Server::new(ciphertext);
    let client = server.client(MatrixVersion::V1_11).await;
    let plain = request("plain");
    assert_eq!(fetch(&client, &plain).await.unwrap(), b"preview");
    assert_eq!(fetch(&client, &plain).await.unwrap(), b"preview");
    let requests = server.requests.lock().unwrap().clone();
    let downloads = requests
        .iter()
        .filter(|request| request.contains("/download/example.org/plain"))
        .collect::<Vec<_>>();
    assert_eq!(
        downloads.len(),
        1,
        "second fetch uses SDK cache: {requests:?}"
    );
    let captured = downloads[0].to_ascii_lowercase();
    assert!(captured.contains("/_matrix/client/v1/media/download/example.org/plain"));
    assert!(captured.contains("authorization: bearer fixture-token"));
    let encrypted = MediaRequestParameters {
        source: MediaSource::Encrypted(Box::new(
            serde_json::from_value(descriptor.clone()).unwrap(),
        )),
        format: MediaFormat::File,
    };
    assert_eq!(
        fetch(&client, &encrypted).await.unwrap(),
        b"encrypted preview"
    );
    descriptor["url"] = "mxc://example.org/encrypted-tampered".into();
    descriptor["hashes"]["sha256"] = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into();
    let tampered = MediaRequestParameters {
        source: MediaSource::Encrypted(Box::new(serde_json::from_value(descriptor).unwrap())),
        format: MediaFormat::File,
    };
    assert!(fetch(&client, &tampered).await.is_err());
    assert!(
        client
            .media_store()
            .lock()
            .await
            .unwrap()
            .get_media_content(&tampered)
            .await
            .unwrap()
            .is_none()
    );
    for id in ["oversized", "chunked"] {
        let error = fetch(&client, &request(id)).await.unwrap_err();
        assert!(matches!(error,Error::UnknownError(error) if error.is::<PreviewLimit>()));
    }
    let error = fetch(&client, &request("missing")).await.unwrap_err();
    assert!(
        matches!(crate::media_cache::error_to_media_cache_entry(error,&request("missing")),crate::media_cache::MediaCacheEntry::Failed(status) if status.as_u16()==404)
    );
    let legacy = server.client(MatrixVersion::V1_0).await;
    assert_eq!(
        fetch(&legacy, &request("legacy")).await.unwrap(),
        b"preview"
    );
    let requests = server.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|request| request.contains("/_matrix/media/r0/download/example.org/legacy")),
        "legacy requests: {requests:?}"
    );
}

#[tokio::test]
async fn preview_deadline_covers_queue_and_body_and_releases_permits() {
    let server = Server::new(Vec::new());
    let client = server.client(MatrixVersion::V1_11).await;
    let gate = FETCHES.acquire_many(4).await.unwrap();
    // More attempts than the request budget catches permits leaked on timeout.
    for _ in 0..160 {
        let error = fetch_with_timeout(&client, &request("queued"), Duration::from_millis(1))
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::UnknownError(ref error) if error.is::<tokio::time::error::Elapsed>())
        );
    }
    drop(gate);
    let error = fetch_with_timeout(&client, &request("slow-body"), Duration::from_millis(100))
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::UnknownError(ref error) if error.is::<tokio::time::error::Elapsed>())
    );
    assert_eq!(
        fetch(&client, &request("recovered")).await.unwrap(),
        b"preview"
    );
}

#[tokio::test]
async fn bounded_cache_keeps_remote_thumbnail_key_and_local_echo_file_key() {
    let server = Server::new(Vec::new());
    let client = server.client(MatrixVersion::V1_11).await;
    let file = request("cached-formats");
    let mut thumbnail = file.clone();
    thumbnail.format = MediaFormat::Thumbnail(matrix_sdk::media::MediaThumbnailSettings::new(
        32u32.into(),
        32u32.into(),
    ));
    let mut local = thumbnail.clone();
    local.source = MediaSource::Plain("mxc://send-queue.localhost/echo".into());
    let mut local_file = local.clone();
    local_file.format = MediaFormat::File;
    {
        let store = client.media_store().lock().await.unwrap();
        for (key, value) in [
            (&file, b"file".as_slice()),
            (&thumbnail, b"thumb".as_slice()),
            (&local_file, b"local".as_slice()),
        ] {
            store
                .add_media_content(
                    key,
                    value.to_vec(),
                    matrix_sdk_base::media::store::IgnoreMediaRetentionPolicy::No,
                )
                .await
                .unwrap();
        }
    }
    assert_eq!(fetch(&client, &thumbnail).await.unwrap(), b"thumb");
    assert_eq!(fetch(&client, &file).await.unwrap(), b"file");
    assert_eq!(fetch(&client, &local).await.unwrap(), b"local");
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.contains("/media/"))
    );
    // A pre-existing oversized local echo must fail at the bounded store API.
    client
        .media_store()
        .lock()
        .await
        .unwrap()
        .add_media_content(
            &local_file,
            vec![0; crate::memory_budget::IMAGE_FILE_BYTES + 1],
            matrix_sdk_base::media::store::IgnoreMediaRetentionPolicy::No,
        )
        .await
        .unwrap();
    assert!(fetch(&client, &local).await.is_err());
}

#[tokio::test]
async fn retired_network_preview_is_neither_cached_nor_published() {
    let server = Server::new(Vec::new());
    let client = server.client(MatrixVersion::V1_11).await;
    let original = client.clone();
    let key = request("slow-body");
    let target = key.clone();
    let live = Arc::new(AtomicBool::new(true));
    let flag = live.clone();
    let task = tokio::spawn(async move {
        super::fetch_with_current(
            &original,
            &target,
            Duration::from_secs(5),
            Arc::new(move || flag.load(Ordering::Acquire)),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if server
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.contains("/slow-body"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("real HTTP request began");
    live.store(false, Ordering::Release);
    assert!(task.await.unwrap().is_err());
    assert!(
        client
            .media_store()
            .lock()
            .await
            .unwrap()
            .get_media_content_bounded(&key, crate::memory_budget::IMAGE_FILE_BYTES)
            .await
            .unwrap()
            .is_none()
    );
}
