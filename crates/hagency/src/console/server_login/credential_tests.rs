//! Explicit credential fixture. Production constructors never select this backend.
use super::*;
use hagency_store::credential_vault::{self as cv, SecretRef, SecretStore, Vault};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex as StdMutex},
};
use zeroize::Zeroizing;
#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) values: StdMutex<BTreeMap<String, Vec<u8>>>,
    pub(crate) failure: StdMutex<Option<cv::Error>>,
    pub(crate) puts: StdMutex<Vec<usize>>,
}
impl SecretStore for Fake {
    fn get(&self, reference: &SecretRef) -> Result<Zeroizing<Vec<u8>>, cv::Error> {
        if let Some(error) = *self.failure.lock().unwrap() {
            return Err(error);
        }
        self.values
            .lock()
            .unwrap()
            .get(&serde_json::to_string(reference).map_err(|_| cv::Error::Invalid)?)
            .cloned()
            .map(Zeroizing::new)
            .ok_or(cv::Error::Missing)
    }
    fn put(&self, reference: &SecretRef, bytes: &[u8]) -> Result<(), cv::Error> {
        if let Some(error) = *self.failure.lock().unwrap() {
            return Err(error);
        }
        self.puts.lock().unwrap().push(bytes.len());
        self.values.lock().unwrap().insert(
            serde_json::to_string(reference).map_err(|_| cv::Error::Invalid)?,
            bytes.to_vec(),
        );
        Ok(())
    }
    fn remove(&self, reference: &SecretRef) -> Result<(), cv::Error> {
        self.values
            .lock()
            .unwrap()
            .remove(&serde_json::to_string(reference).map_err(|_| cv::Error::Invalid)?)
            .map(|_| ())
            .ok_or(cv::Error::Missing)
    }
}
pub(crate) fn vault() -> Vault {
    Vault::new(Arc::new(Fake::default()))
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn rows() -> Vec<Revocation> {
    vec![Revocation {
        origin: "http://127.0.0.1:9/".into(),
        client_id: "original-client".into(),
        session_token: "S".repeat(64),
        oauth_token: format!("TOKEN_MARKER{}", "x".repeat(4084)),
        hint: "refresh_token".into(),
        session_done: false,
        oauth_done: false,
    }]
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[tokio::test]
async fn credential_migration_revoke_only_legacy_complete_long_tokens_and_progress() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    hagency_store::private::directory(&root).unwrap();
    let path = root.as_path().join("server-login-revocations.json");
    let original = rows();
    hagency_store::private::write_new(&path, &serde_json::to_vec(&original).unwrap()).unwrap();
    let fake = Arc::new(Fake::default());
    let vault = Vault::new(fake.clone());
    let login = ServerLogin::new_with_vault(Some(root.as_path()), vault.clone()).unwrap();
    assert!(
        fake.puts.lock().unwrap().is_empty(),
        "synchronous construction cannot call native"
    );
    let mut loaded = login.revocations.lock().await;
    login.load_revocations(&mut loaded).await.unwrap();
    assert!(*loaded == original);
    assert!(login.sessions.lock().await.is_empty());
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.windows(12).any(|v| v == b"TOKEN_MARKER"));
    assert!(fake.puts.lock().unwrap().iter().all(|n| *n < 2048));
    loaded[0].session_done = true;
    loaded[0].session_token.clear();
    login.save_revocations(&mut loaded).await.unwrap();
    drop(loaded);
    let restarted = ServerLogin::new_with_vault(Some(root.as_path()), vault).unwrap();
    let mut reopened = restarted.revocations.lock().await;
    restarted.load_revocations(&mut reopened).await.unwrap();
    assert_eq!(reopened[0].oauth_token.len(), 4096);
    assert!(reopened[0].session_done);
    assert!(reopened[0].session_token.is_empty());
    assert!(restarted.sessions.lock().await.is_empty());
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[tokio::test]
async fn credential_migration_revoke_locked_and_external_replacement_never_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    hagency_store::private::directory(&root).unwrap();
    let path = root.as_path().join("server-login-revocations.json");
    let original = serde_json::to_vec(&rows()).unwrap();
    hagency_store::private::write_new(&path, &original).unwrap();
    let fake = Arc::new(Fake::default());
    *fake.failure.lock().unwrap() = Some(cv::Error::Locked);
    let login =
        ServerLogin::new_with_vault(Some(root.as_path()), Vault::new(fake.clone())).unwrap();
    let mut loaded = login.revocations.lock().await;
    assert!(login.load_revocations(&mut loaded).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!login.revocations_loaded.load(Ordering::Acquire));
    *fake.failure.lock().unwrap() = None;
    login.load_revocations(&mut loaded).await.unwrap();
    let v2 = std::fs::read(&path).unwrap();
    hagency_store::private::replace(&path, &v2).unwrap();
    loaded[0].session_done = true;
    assert!(login.save_revocations(&mut loaded).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), v2);
    assert!(login.revocations_dirty.load(Ordering::Acquire));
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[tokio::test]
async fn credential_migration_revoke_missing_key_and_unknown_schema_cannot_restore_or_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    hagency_store::private::directory(&root).unwrap();
    let path = root.as_path().join("server-login-revocations.json");
    let fake = Arc::new(Fake::default());
    let vault = Vault::new(fake.clone());
    let login = ServerLogin::new_with_vault(Some(root.as_path()), vault.clone()).unwrap();
    let mut pending = login.revocations.lock().await;
    login.load_revocations(&mut pending).await.unwrap();
    *pending = rows();
    login.save_revocations(&mut pending).await.unwrap();
    drop(pending);
    let original = std::fs::read(&path).unwrap();
    fake.values.lock().unwrap().clear();
    let restarted = ServerLogin::new_with_vault(Some(root.as_path()), vault).unwrap();
    let mut pending = restarted.revocations.lock().await;
    assert!(restarted.load_revocations(&mut pending).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(restarted.sessions.lock().await.is_empty());
    hagency_store::private::replace(&path, b"{\"version\":99}").unwrap();
    assert!(restarted.load_revocations(&mut pending).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"version\":99}");
}
#[tokio::test]
async fn credential_migration_auth_retirement_ack_crosses_original_agent_publication_gate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    hagency_store::private::directory(&root).unwrap();
    let login = Arc::new(ServerLogin::new_with_vault(Some(&root), vault()).unwrap());
    let (device, _) = fixture_device("http://127.0.0.1:9/", "owner", "@owner:example.test");
    let original = device.clone();
    let now = Instant::now();
    let mut session = RemoteSession {
        matrix_source: None,
        token: "S".repeat(64),
        oauth_token: "R".repeat(64),
        user_id: "owner".into(),
        device: Some(device),
        refresh_token: None,
        oauth_expires: now + Duration::from_secs(60),
        authorized_until: now + Duration::from_secs(60),
        invalidated: false,
        binding: Binding {
            origin: "http://127.0.0.1:9/".into(),
            client_id: "client".into(),
            installation_id: "i".repeat(64),
            name: "fixture".into(),
            owner: Some("@owner:example.test".into()),
            subject: Some("owner".into()),
        },
        checked: now,
        expires: now + Duration::from_secs(900),
    };
    let publication = login.credential_publication.clone();
    let (ready_tx, ready) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _actual_publication = publication.lock().unwrap();
        ready_tx.send(()).unwrap();
        released.recv().unwrap();
    });
    ready.recv().unwrap();
    let runner = login.clone();
    let revoke = tokio::spawn(async move {
        runner.queue_revocation(&mut session).await;
        session
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while original.bearer().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !revoke.is_finished(),
        "request cannot acknowledge while original rename owns the gate"
    );
    release.send(()).unwrap();
    thread.join().unwrap();
    let retired = revoke.await.unwrap();
    assert!(retired.invalidated);
    assert!(original.bearer().is_err());
    assert_eq!(login.revocations.lock().await.len(), 1);
    assert!(!login.revocations_dirty.load(Ordering::Acquire));
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[tokio::test]
async fn credential_migration_dirty_revoke_queue_requires_durable_intent_before_real_http() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for filesystem in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/", listener.local_addr().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("state");
        hagency_store::private::directory(&root).unwrap();
        let path = root.join("server-login-revocations.json");
        let backup = root.join("retained-original");
        let fake = Arc::new(Fake::default());
        let vault = Vault::new(fake.clone());
        let login = ServerLogin::new_with_vault(Some(&root), vault.clone()).unwrap();
        *login.status.lock().await = json!({"state":"signed_out","remoteRevocationPending":true});
        let mut intended = rows();
        intended[0].origin = address;
        {
            let mut pending = login.revocations.lock().await;
            login.load_revocations(&mut pending).await.unwrap();
            if filesystem {
                *pending = intended.clone();
                login.save_revocations(&mut pending).await.unwrap();
                std::fs::rename(&path, &backup).unwrap();
            } else {
                *fake.failure.lock().unwrap() = Some(cv::Error::Locked);
            }
            intended[0].oauth_token = format!("ORIGINAL_INTENT{}", "z".repeat(4081));
            *pending = intended.clone();
            assert!(login.save_revocations(&mut pending).await.is_err());
        }
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let recorded = calls.clone();
        let expected = intended.clone();
        let persisted = path.clone();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut raw = Vec::new();
                loop {
                    let mut buffer = [0; 8192];
                    let n = stream.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    raw.extend_from_slice(&buffer[..n]);
                    if let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&raw[..split]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if raw.len() >= split + 4 + length {
                            break;
                        }
                    }
                }
                // Check real persisted authenticated bytes at the instant each
                // remote request arrives, not a mocked save return value.
                recorded.fetch_add(1, Ordering::AcqRel);
                let check = credentials::CredentialIo::new(vault.clone());
                let actual = check.load_revocations(persisted.clone()).await.unwrap();
                assert!(actual == expected);
                let bytes = std::fs::read(&persisted).unwrap();
                assert!(!bytes.windows(15).any(|s| s == b"ORIGINAL_INTENT"));
                let request = String::from_utf8(raw).unwrap();
                assert!(
                    request.starts_with("DELETE /hagency/api/v1/sessions/current ")
                        || request.starts_with("POST /auth/oauth2/revoke ")
                );
                if request.starts_with("POST ") {
                    assert!(request.contains(&format!("token={}", expected[0].oauth_token)));
                } else {
                    assert!(request.contains(&format!("Bearer {}", expected[0].session_token)));
                }
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{} ",
                    )
                    .await
                    .unwrap();
            }
        });
        login.flush_revocations().await;
        assert_eq!(
            calls.load(Ordering::Acquire),
            0,
            "uncommitted exact intent cannot make a remote call"
        );
        assert!(login.revocations_dirty.load(Ordering::Acquire));
        assert_eq!(login.status.lock().await["code"], "local_state_unavailable");
        assert!(!login.revocations.lock().await.is_empty());
        if filesystem {
            std::fs::rename(&backup, &path).unwrap();
        } else {
            *fake.failure.lock().unwrap() = None;
        }
        tokio::time::timeout(Duration::from_secs(5), login.flush_revocations())
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(calls.load(Ordering::Acquire), 2);
        assert!(login.revocations.lock().await.is_empty());
        assert!(!login.revocations_dirty.load(Ordering::Acquire));
        assert_eq!(login.status.lock().await["remoteRevocationPending"], false);
    }
}
