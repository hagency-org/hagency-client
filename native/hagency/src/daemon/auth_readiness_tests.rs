use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct State {
    denied: usize,
    exchanges: AtomicUsize,
    refreshes: AtomicUsize,
    identities: AtomicUsize,
    device: Mutex<String>,
    presented: Mutex<Vec<String>>,
}
struct Fixture {
    origin: String,
    state: Arc<State>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new(denied: usize) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let base = origin.clone();
        let state = Arc::new(State {
            denied,
            exchanges: AtomicUsize::new(0),
            refreshes: AtomicUsize::new(0),
            identities: AtomicUsize::new(0),
            device: Mutex::new("ORIGINAL_DEVICE".into()),
            presented: Mutex::new(Vec::new()),
        });
        let calls = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let base = base.clone();
                let calls = calls.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut buffer = [0; 4096];
                    loop {
                        let Ok(n) = stream.read(&mut buffer).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..n]);
                        if let Some(header_end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                            let header = String::from_utf8_lossy(&bytes[..header_end]);
                            let length = header
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|v| v.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            if bytes.len() >= header_end + 4 + length {
                                break;
                            }
                        }
                        assert!(bytes.len() < 65536);
                    }
                    let request = String::from_utf8_lossy(&bytes);
                    let path = request.split_whitespace().nth(1).unwrap_or("");
                    let (status, value) = if path.contains("auth_metadata") {
                        (
                            200,
                            serde_json::json!({"issuer":format!("{base}auth/"),"authorization_endpoint":format!("{base}auth/authorize"),"token_endpoint":format!("{base}auth/token"),"revocation_endpoint":format!("{base}auth/revoke"),"response_types_supported":["code"],"response_modes_supported":["query","fragment"],"grant_types_supported":["authorization_code","refresh_token"],"code_challenge_methods_supported":["S256"]}),
                        )
                    } else if path == "/auth/token" {
                        if request.contains("grant_type=refresh_token") {
                            calls.refreshes.fetch_add(1, Ordering::SeqCst);
                        } else {
                            calls.exchanges.fetch_add(1, Ordering::SeqCst);
                        }
                        (
                            200,
                            serde_json::json!({"access_token":"fixture-issued-token","refresh_token":"fixture-refresh","token_type":"Bearer","expires_in":3600}),
                        )
                    } else if path.ends_with("/account/whoami") {
                        let presented = request
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                                    .map(|(_, value)| value.trim().to_owned())
                            })
                            .unwrap_or_default();
                        calls.presented.lock().unwrap().push(presented);
                        if calls.identities.fetch_add(1, Ordering::SeqCst) < calls.denied {
                            (
                                401,
                                serde_json::json!({"errcode":"M_UNKNOWN_TOKEN","error":"fixture provisioning pending"}),
                            )
                        } else {
                            (
                                200,
                                serde_json::json!({"user_id":"@owner:test","device_id":*calls.device.lock().unwrap()}),
                            )
                        }
                    } else if path == "/hagency/api/v1/discovery" {
                        (
                            200,
                            serde_json::json!({"product":"hagency-server","version":"1.0.0","protocolVersion":3,"homeserver":base,"issuer":format!("{base}auth/"),"capabilities":["pasion-oauth","owner-agent-appservice-v1","global-agent-identity-v2","execution-device-v1","owner-direct-v1"]}),
                        )
                    } else if path.ends_with("/versions") {
                        (200, serde_json::json!({"versions":["v1.15"]}))
                    } else {
                        (
                            404,
                            serde_json::json!({"errcode":"M_UNRECOGNIZED","error":"fixture route"}),
                        )
                    };
                    let body = serde_json::to_vec(&value).unwrap();
                    let header = format!(
                        "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        Self {
            origin,
            state,
            task,
        }
    }
    async fn authorization(&self) -> (Client, reqwest::Url) {
        let client = provisioning_client(&self.origin).await.unwrap();
        client
            .oauth()
            .restore_registered_client(ClientId::new("fixture-client".into()));
        let data = client
            .oauth()
            .login(
                reqwest::Url::parse("http://127.0.0.1/callback").unwrap(),
                Some("ORIGINAL_DEVICE".into()),
                None,
                None,
            )
            .build()
            .await
            .unwrap();
        assert_eq!(requested_device(&data.url).unwrap(), "ORIGINAL_DEVICE");
        let mut query = reqwest::Url::parse("http://127.0.0.1/callback").unwrap();
        query
            .query_pairs_mut()
            .append_pair("code", "single-use-fixture-code")
            .append_pair("state", data.state.secret());
        (client, query)
    }
    fn assert_single_exchange(&self) {
        assert_eq!(self.state.exchanges.load(Ordering::SeqCst), 1);
        assert_eq!(self.state.refreshes.load(Ordering::SeqCst), 0);
        assert!(
            self.state
                .presented
                .lock()
                .unwrap()
                .iter()
                .all(|token| token == "Bearer fixture-issued-token")
        );
    }
}

#[tokio::test]
async fn fresh_grant_401_then_ready_keeps_token_and_exchanges_code_once() {
    let fixture = Fixture::new(3).await;
    let (client, query) = fixture.authorization().await;
    let saved = complete_new_grant(
        &client,
        query.into(),
        &fixture.origin,
        Some("@owner:test"),
        "ORIGINAL_DEVICE",
        Duration::from_secs(1),
        Duration::from_millis(1),
    )
    .await
    .unwrap();
    assert_eq!(saved.user.tokens.access_token, "fixture-issued-token");
    assert_eq!(saved.user.meta.device_id.as_str(), "ORIGINAL_DEVICE");
    assert_eq!(fixture.state.identities.load(Ordering::SeqCst), 4);
    fixture.assert_single_exchange();
}
#[tokio::test]
async fn invalid_fresh_grant_is_bounded_and_never_refreshes_or_reexchanges() {
    let fixture = Fixture::new(usize::MAX).await;
    let (client, query) = fixture.authorization().await;
    let started = tokio::time::Instant::now();
    let result = complete_new_grant(
        &client,
        query.into(),
        &fixture.origin,
        None,
        "ORIGINAL_DEVICE",
        Duration::from_secs(1),
        Duration::from_millis(1),
    )
    .await;
    assert_eq!(result.err(), Some(AuthError::SignInRequired));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(fixture.state.identities.load(Ordering::SeqCst), 10);
    fixture.assert_single_exchange();
}
#[tokio::test]
async fn cancelling_fresh_grant_readiness_stops_probes_without_new_exchange() {
    let fixture = Fixture::new(usize::MAX).await;
    let (client, query) = fixture.authorization().await;
    let origin = fixture.origin.clone();
    let task = tokio::spawn(async move {
        complete_new_grant(
            &client,
            query.into(),
            &origin,
            None,
            "ORIGINAL_DEVICE",
            TIMEOUT,
            Duration::from_secs(1),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.identities.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(matches!(task.await,Err(error) if error.is_cancelled()));
    let calls = fixture.state.identities.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(fixture.state.identities.load(Ordering::SeqCst), calls);
    fixture.assert_single_exchange();
}
#[tokio::test]
async fn readiness_never_accepts_another_owner_or_device() {
    let fixture = Fixture::new(1).await;
    let (client, query) = fixture.authorization().await;
    *fixture.state.device.lock().unwrap() = "SUBSTITUTED_DEVICE".into();
    assert_eq!(
        complete_new_grant(
            &client,
            query.into(),
            &fixture.origin,
            Some("@owner:test"),
            "ORIGINAL_DEVICE",
            TIMEOUT,
            Duration::from_millis(1)
        )
        .await
        .err(),
        Some(AuthError::AccountMismatch)
    );
    fixture.assert_single_exchange();
}

#[tokio::test]
async fn browser_enrollment_persists_only_verified_original_grant_into_sdk_custody() {
    let fixture = Fixture::new(3).await;
    let home = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let state = fixture.state.clone();
    let custody = SdkCustody::enroll(
        home.path(),
        &fixture.origin,
        Some("@owner:test"),
        Some("fixture-client"),
        move |url| async move {
            let authorization = reqwest::Url::parse(&url).unwrap();
            *state.device.lock().unwrap() = requested_device(&authorization).unwrap();
            let fields: std::collections::HashMap<_, _> = authorization
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let mut callback = reqwest::Url::parse(&fields["redirect_uri"]).unwrap();
            callback
                .query_pairs_mut()
                .append_pair("code", "single-use-fixture-code")
                .append_pair("state", &fields["state"]);
            tokio::spawn(async move {
                reqwest::Client::new().get(callback).send().await.unwrap();
            });
            Ok(())
        },
    )
    .await
    .unwrap();
    let saved = read_saved(home.path()).unwrap().unwrap();
    assert_eq!(saved.user.tokens.access_token, "fixture-issued-token");
    assert_eq!(
        saved.user.meta.device_id.as_str(),
        fixture.state.device.lock().unwrap().as_str()
    );
    assert_eq!(
        custody.token().await.unwrap().access_token,
        "fixture-issued-token"
    );
    assert!(home.path().join(STORE).exists());
    assert!(custody.tokens().consume_explicit_reenrollment());
    fixture.assert_single_exchange();
    custody.retire();
}
