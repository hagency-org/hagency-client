//! TS test oracle — `tests/bridge-matrix-http-primitives.test.js`,
//! describe block `uploadMedia` (`:274-292`).
//!
//! TS `uploadMedia` POSTs the bytes to `/_matrix/media/v3/upload` and returns
//! the response's `content_uri`; a non-2xx throws an error that carries the
//! status (`Media upload failed: 413`). The native counterpart is the encrypted
//! uploader (`hagency-matrix/src/media_upload.rs`), whose `Http::upload`
//! maps a non-2xx to `Error::Remote(status)` (`http.rs:293`) and whose
//! `UploadAttempt::media_id()` yields the `content_uri` as a `MediaId`.
//!
//! Deviation from TS, recorded not hidden: native uploads CIPHERTEXT with
//! `application/octet-stream` (E2EE media), where TS uploads the raw buffer
//! with the caller's `Content-Type`. This is a deliberate native property
//! (the whole point of `Encrypted`), so the oracle asserts the shared,
//! TS-visible outcome — the path, the method, and the returned `mxc://` URI —
//! and records the content-type difference as a parity gap.
mod common;
use common::{Fake, TOKEN};
use hagency_core::replies::{MatrixTransportObservation, RoomPrivacy};
use hagency_matrix::{
    CancellationToken, HostConfig, HostIdentity, HostRoom, MediaUploadError as Failure,
    MediaUploadLimits, MediaUploader, UploadResponse, UploadState,
};
use hagency_media::{Codec, Encrypted};
use serde_json::json;
use std::time::Duration;

fn config(endpoint: &str) -> HostConfig {
    HostConfig::new(
        HostIdentity {
            server_name: "example.test".into(),
            registration_fingerprint: "a".repeat(64),
            transport: MatrixTransportObservation {
                engagement_id: "ts-oracle-upload".into(),
                registration_generation: 1,
                generation: 1,
                sender_mxid: "@worker:example.test".into(),
                device_id: "DEVICE_1".into(),
            },
        },
        endpoint,
        TOKEN,
        std::path::PathBuf::from("unused-ts-oracle-upload-sdk"),
        [42; 32],
        vec![HostRoom {
            room_id: "!direct:example.test".into(),
            generation: 1,
            privacy: RoomPrivacy::Direct {
                human_mxid: "@owner:example.test".into(),
            },
        }],
        common::limits(),
    )
    .unwrap()
    .with_root_pem(include_bytes!("fixtures/ca.pem"))
    .unwrap()
}

fn uploader(fake: &Fake) -> MediaUploader {
    MediaUploader::new(
        &config(&fake.endpoint),
        MediaUploadLimits::new(1024, 1, 1).unwrap(),
    )
    .unwrap()
}

struct Media {
    encrypted: Encrypted,
    _root: tempfile::TempDir,
}

fn media(plaintext: &[u8]) -> Media {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("private-file"), plaintext).unwrap();
    let dir =
        cap_std::fs::Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap();
    let workspace =
        hagency_files::Workspace::from_directory(dir, hagency_files::Limits::default()).unwrap();
    let snapshot = workspace
        .snapshot(&hagency_files::RelativeFile::new("private-file").unwrap())
        .unwrap();
    let encrypted = Codec::new(hagency_media::Limits::default())
        .encrypt(snapshot)
        .unwrap();
    Media {
        _root: root,
        encrypted,
    }
}

/// TS: `POSTs the bytes and returns the content_uri` — the request is a POST to
/// `/_matrix/media/v3/upload`, and a 200 body `{content_uri}` is returned as the
/// `mxc://` URI.
#[tokio::test]
async fn ts_upload_posts_and_returns_the_content_uri() {
    let media = media(b"oracle upload payload");
    let mut fake = Fake::start(true).await;
    let client = uploader(&fake);
    let mut attempt = client.prepare(&media.encrypted).unwrap();
    let cancel = CancellationToken::new();
    let (result, ()) = tokio::join!(attempt.send(&cancel), async {
        let request = fake.next().await;
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/_matrix/media/v3/upload");
        assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
        request.json(200, json!({"content_uri":"mxc://fake.test/xyz"}));
    });
    result.unwrap();
    assert_eq!(attempt.state(), UploadState::Accepted);
    assert_eq!(attempt.media_id().unwrap().to_mxc(), "mxc://fake.test/xyz");
    fake.close().await;
}

/// TS: the upload body is the raw bytes, never a JSON wrapper — "a refactor that
/// wrapped it would upload a JSON document as an image and only fail later".
/// Native sends the ciphertext bytes themselves.
#[tokio::test]
async fn ts_upload_body_is_the_raw_bytes_not_json_wrapped() {
    let media = media(b"raw bytes, not JSON");
    let cipher = media.encrypted.ciphertext().to_vec();
    let mut fake = Fake::start(true).await;
    let client = uploader(&fake);
    let mut attempt = client.prepare(&media.encrypted).unwrap();
    let cancel = CancellationToken::new();
    let (result, ()) = tokio::join!(attempt.send(&cancel), async {
        let request = fake.next().await;
        // Ciphertext can start with any byte, including '{'. Exact equality
        // rejects a JSON wrapper without making assumptions about random bytes.
        assert_eq!(request.body, cipher, "the body is the exact bytes");
        request.json(200, json!({"content_uri":"mxc://fake.test/xyz"}));
    });
    result.unwrap();
    fake.close().await;
}

/// TS: `a failed upload throws with the status` — a 413 becomes an error
/// carrying the status, so a caller can distinguish "too large" from an outage.
#[tokio::test]
async fn ts_failed_upload_carries_the_status() {
    let media = media(b"too large");
    let mut fake = Fake::start(true).await;
    let client = uploader(&fake);
    let mut attempt = client.prepare(&media.encrypted).unwrap();
    let cancel = CancellationToken::new();
    let (result, ()) = tokio::join!(attempt.send(&cancel), async {
        let request = fake.next().await;
        request.json(413, json!({"errcode":"M_TOO_LARGE"}));
    });
    let error = result.expect_err("a 413 is a refusal");
    assert!(
        matches!(
            error,
            Failure::Transport(hagency_matrix::Error::Remote(413))
        ),
        "the status is carried: {error:?}"
    );
    fake.close().await;
}

/// TS: `uploadMedia` sends the caller's `Content-Type`. Native sends
/// `application/octet-stream` because it uploads ciphertext (E2EE media).
/// Recorded as a parity gap — not changed here (no product edits in this task).
#[test]
#[ignore = "parity gap: TS uploadMedia sends the caller's Content-Type (bridge-matrix-http-primitives.test.js:281); native media upload is encrypted and always sends application/octet-stream"]
fn ts_upload_sends_the_callers_content_type() {
    // See ts_upload_media_type_is_octet_stream_for_ciphertext for the native
    // property asserted instead.
}

/// The native property that stands in for the TS content-type assertion: an
/// encrypted upload is framed as opaque bytes.
#[tokio::test]
async fn ts_upload_media_type_is_octet_stream_for_ciphertext() {
    let media = media(b"ciphertext framing");
    let mut fake = Fake::start(true).await;
    let client = uploader(&fake);
    let mut attempt = client.prepare(&media.encrypted).unwrap();
    let cancel = CancellationToken::new();
    let (result, ()) = tokio::join!(attempt.send(&cancel), async {
        let request = fake.next().await;
        assert_eq!(request.headers["content-type"], "application/octet-stream");
        request.json(200, json!({"content_uri":"mxc://fake.test/xyz"}));
    });
    result.unwrap();
    fake.close().await;
}

/// Silence the unused-import warning for `UploadResponse`, which the TS oracle
/// documents as the native return type of a validated 200.
#[test]
fn ts_upload_response_type_is_the_bounded_body() {
    fn assert_type<T>() {}
    assert_type::<UploadResponse>();
    let _ = Duration::from_millis(1);
}
