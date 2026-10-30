//! Rust-only SDK authorization. OAuth snapshots never enter local persistence.
use super::super::native::MatrixTokenSource;
use super::*;
use std::sync::Arc;

fn device_name(host: &str, os: &str) -> String {
    let host = host
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(96)
        .collect::<String>();
    if host.is_empty() {
        format!("Hagency ({os})")
    } else {
        format!("{host} ({os})")
    }
}

fn local_device_name() -> String {
    let host = gethostname::gethostname();
    device_name(&host.to_string_lossy(), std::env::consts::OS)
}
impl ServerLogin {
    #[cfg(unix)]
    pub(in crate::console) async fn agent_file_source(
        &self,
        expected: &super::super::device_execution::DeviceIdentity,
    ) -> Option<Arc<dyn MatrixTokenSource>> {
        self.matrix_source(expected)
            .await
            .filter(|source| source.supports_agent_file_receive())
    }
    pub(in crate::console) async fn matrix_source(
        &self,
        expected: &super::super::device_execution::DeviceIdentity,
    ) -> Option<Arc<dyn MatrixTokenSource>> {
        if self.stopped.load(Ordering::Acquire) {
            return None;
        }
        self.sessions
            .lock()
            .await
            .values()
            .find(|s| {
                !s.invalidated
                    && s.authorized_until > Instant::now()
                    && s.device.as_ref().is_some_and(|d| {
                        super::super::device_execution::DeviceIdentity::from(d) == *expected
                    })
            })
            .and_then(|s| s.matrix_source.clone())
    }

    pub(in crate::console) async fn authorize_matrix(
        &self,
        console: &super::super::Console,
        expected_origin: &str,
        expected_owner: &str,
        source: Arc<dyn MatrixTokenSource>,
    ) -> Result<String, &'static str> {
        let _transition = self.transition.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return Err("matrix_account_changed");
        }
        let started = Instant::now();
        tracing::debug!(
            stage = "owner_adoption_discovery",
            "Adopting SDK authorization"
        );
        crate::server_admission::discover(expected_origin)
            .await
            .map_err(|e| e.code())?;
        let snapshot = source
            .access_token()
            .await
            .map_err(super::super::native::sdk_source_failure)?;
        if snapshot.access_token.is_empty()
            || snapshot.access_token.len() > 4096
            || snapshot.client_id.is_empty()
            || snapshot.client_id.len() > 255
        {
            return Err("invalid_sdk_token");
        }
        let server = origin(expected_origin)?;
        let client = http()?;
        tracing::debug!(
            stage = "owner_adoption_grant",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Requesting owner session"
        );
        let grant = response(
            client
                .post(server.join("/hagency/api/v1/sessions/pasion").unwrap())
                .json(&json!({"accessToken":snapshot.access_token}))
                .send()
                .await
                .map_err(|error| {
                    tracing::warn!(
                        stage = "owner_adoption_grant",
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        timeout = error.is_timeout(),
                        connect = error.is_connect(),
                        body = error.is_body(),
                        "Owner session request transport failed"
                    );
                    "server_unavailable"
                })?,
        )
        .await?;
        let token = grant["token"]
            .as_str()
            .filter(|s| s.len() == 64)
            .ok_or("invalid_server_response")?
            .to_owned();
        let result = async {
            tracing::debug!(stage="owner_adoption_identity",elapsed_ms=started.elapsed().as_millis() as u64,"Checking owner identity");
            let identity = get(&client, server.join("/hagency/api/v1/identity").unwrap(), Some(&token)).await?;
            let subject = identity["subject"].as_str().filter(|s| !s.is_empty() && s.len() <= 255).ok_or("invalid_server_response")?;
            let user = identity["userId"].as_str().filter(|s| !s.is_empty() && s.len() <= 128).ok_or("invalid_server_response")?;
            if identity["mxid"] != expected_owner || identity["issuer"] != server.join("/auth/").unwrap().as_str() || identity["clientId"] != snapshot.client_id || grant["mxid"] != identity["mxid"] || grant["userId"] != identity["userId"] { return Err("matrix_account_mismatch"); }
            let previous = self.binding.lock().await.clone();
            if let Some(previous) = &previous && previous.origin == expected_origin && previous.owner.as_deref() == Some(expected_owner) && previous.subject.as_deref().is_some_and(|s| s != subject) { return Err("matrix_account_mismatch"); }
            let binding = Binding { origin: expected_origin.into(), client_id: snapshot.client_id.clone(), installation_id: previous.as_ref().filter(|p| p.origin == expected_origin && p.client_id == snapshot.client_id && p.owner.as_deref() == Some(expected_owner) && p.subject.as_deref() == Some(subject)).map(|p| p.installation_id.clone()).unwrap_or(random()?), name: local_device_name(), owner: Some(expected_owner.into()), subject: Some(subject.into()) };
            let until = deadline(&grant)?.min(deadline(&identity)?);
            tracing::debug!(stage="owner_adoption_device",elapsed_ms=started.elapsed().as_millis() as u64,"Registering execution device");
            let (_, device) = register_device(&client, &server, &token, &binding, user).await?;
            let after = source.access_token().await.map_err(super::super::native::sdk_source_failure)?;
            if after.client_id != binding.client_id || self.stopped.load(Ordering::Acquire) { return Err("matrix_account_changed"); }
            self.save_matrix_binding(binding.clone(), &source).await?;
            let cookie = console.0.authority.server_session(Duration::from_secs(900)).map_err(|_| "local_state_unavailable")?;
            let mut sessions = self.sessions.lock().await;
            if self.stopped.load(Ordering::Acquire) { return Err("matrix_account_changed"); }
            for previous in sessions.values_mut() { self.queue_revocation(previous).await; }
            sessions.clear();
            sessions.insert(session_key(&cookie), RemoteSession { matrix_source: Some(source), token: token.clone(), oauth_token: String::new(), user_id: user.into(), device: Some(device), refresh_token: None, oauth_expires: until, authorized_until: until, invalidated: false, binding, checked: Instant::now(), expires: Instant::now() + Duration::from_secs(900) });
            *self.status.lock().await = json!({"state":"device_authorized","deviceAuthorized":true,"transportOnline":false});
            Self::start_worker(console);
            tracing::info!(stage="owner_adoption",elapsed_ms=started.elapsed().as_millis() as u64,"SDK owner authorization ready");
            Ok(cookie)
        }.await;
        if result.is_err() {
            // The SDK owns OAuth even when adoption fails. Only our issued grant
            // is revoked; neither the snapshot nor an SDK refresh token persists.
            let mut pending = self.revocations.lock().await;
            pending.push(Revocation {
                origin: expected_origin.into(),
                client_id: snapshot.client_id,
                session_token: token,
                oauth_token: String::new(),
                hint: "access_token".into(),
                session_done: false,
                oauth_done: true,
            });
            let _ = self.save_revocations(&mut pending).await;
            drop(pending);
            self.flush_revocations().await;
        }
        result
    }
    pub(in crate::console) async fn check_matrix_source(
        &self,
        cookie: &str,
    ) -> Result<(), &'static str> {
        if self.stopped.load(Ordering::Acquire) {
            return Err("matrix_account_changed");
        }
        let session = self
            .sessions
            .lock()
            .await
            .get(&session_key(cookie))
            .cloned()
            .filter(|s| !s.invalidated)
            .ok_or("matrix_account_changed")?;
        let source = session
            .matrix_source
            .as_ref()
            .ok_or("matrix_account_changed")?;
        let snapshot = source
            .access_token()
            .await
            .map_err(super::super::native::sdk_source_failure)?;
        let sessions = self.sessions.lock().await;
        if sessions
            .values()
            .find(|s| s.token == session.token)
            .is_none_or(|s| s.invalidated)
            || snapshot.client_id != session.binding.client_id
            || self.stopped.load(Ordering::Acquire)
        {
            return Err("matrix_account_changed");
        }
        Ok(())
    }
    pub(in crate::console) async fn renew_matrix(
        &self,
        console: &super::super::Console,
        cookie: &str,
    ) -> Result<Option<String>, Error> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(Error::Unauthorized);
        }
        let _renewal = self.renewal.lock().await;
        let key = session_key(cookie);
        self.renew_session_inner(&key).await?;
        let mut sessions = self.sessions.lock().await;
        let session = sessions.get_mut(&key).ok_or(Error::Unauthorized)?;
        if session.invalidated
            || session.matrix_source.is_none()
            || self.stopped.load(Ordering::Acquire)
        {
            return Err(Error::Unauthorized);
        }
        if session.expires <= Instant::now() + Duration::from_secs(60) {
            let next = console
                .0
                .authority
                .server_session(Duration::from_secs(900))?;
            let mut session = sessions.remove(&key).ok_or(Error::Unauthorized)?;
            session.expires = Instant::now() + Duration::from_secs(900);
            sessions.insert(session_key(&next), session);
            Ok(Some(next))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::native::{Command, MatrixAccessToken, NativeError, NativeOwner};
    use super::*;
    #[test]
    fn device_names_identify_the_computer_and_fit_server_limits() {
        assert_eq!(device_name("  CHRIS-PC  ", "windows"), "CHRIS-PC (windows)");
        assert_eq!(device_name("\n\t", "linux"), "Hagency (linux)");
        assert_eq!(device_name("Office\nMac", "macos"), "OfficeMac (macos)");
        assert!(device_name(&"电".repeat(300), "windows").chars().count() <= 128);
        assert!(!local_device_name().chars().any(char::is_control));
    }
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct Source {
        token: std::sync::Mutex<String>,
        current: Arc<AtomicBool>,
        error_code: &'static str,
        calls: std::sync::atomic::AtomicUsize,
        refreshes: std::sync::atomic::AtomicUsize,
    }
    impl MatrixTokenSource for Source {
        fn access_token(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::Relaxed);
                if !self.current.load(Ordering::Acquire) {
                    return Err(NativeError {
                        status: 401,
                        code: self.error_code.into(),
                    });
                }
                Ok(MatrixAccessToken {
                    access_token: self.token.lock().unwrap().clone(),
                    client_id: "sdk-client".into(),
                })
            })
        }
        fn refresh_access_token(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>>
                    + Send
                    + '_,
            >,
        > {
            self.refreshes.fetch_add(1, Ordering::Relaxed);
            {
                let mut token = self.token.lock().unwrap();
                if token.as_str() == "sdk-short" {
                    *token = "sdk-refreshed".into();
                }
            }
            self.access_token()
        }
    }
    async fn fixture(
        mxid: &str,
        late_epoch: Option<Arc<AtomicBool>>,
    ) -> (
        String,
        Arc<std::sync::Mutex<Vec<(String, Value)>>>,
        tokio::task::JoinHandle<()>,
    ) {
        fixture_controlled(
            mxid,
            late_epoch,
            Arc::new(std::sync::atomic::AtomicU16::new(0)),
        )
        .await
    }
    // 1 = slow management response, 2 = transient renewal 503, 3 = terminal renewal 401.
    async fn fixture_controlled(
        mxid: &str,
        late_epoch: Option<Arc<AtomicBool>>,
        behavior: Arc<std::sync::atomic::AtomicU16>,
    ) -> (
        String,
        Arc<std::sync::Mutex<Vec<(String, Value)>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let records = requests.clone();
        let issuer = format!("{origin}auth/");
        let mxid = mxid.to_owned();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let records = records.clone();
                let issuer = issuer.clone();
                let mxid = mxid.clone();
                let late_epoch = late_epoch.clone();
                let behavior = behavior.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut chunk = [0; 4096];
                    let split = loop {
                        let n = stream.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..n]);
                        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let headers = String::from_utf8_lossy(&raw[..split]).to_string();
                    let length = headers
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|n| n.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    while raw.len() < split + length {
                        let n = stream.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..n]);
                    }
                    let path = headers
                        .lines()
                        .next()
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .to_owned();
                    let mut body: Value =
                        serde_json::from_slice(&raw[split..split + length]).unwrap_or(Value::Null);
                    if path.starts_with("/_matrix/") {
                        let bearer = headers
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(key, _)| key.eq_ignore_ascii_case("authorization"))
                                    .map(|(_, value)| value.trim().to_owned())
                            })
                            .unwrap_or_default();
                        body = json!({"testBearer":bearer});
                    }
                    let short_proof = body["accessToken"] == "sdk-short";
                    records.lock().unwrap().push((path.clone(), body));
                    if path == "/hagency/api/v1/identity"
                        && let Some(flag) = &late_epoch
                    {
                        flag.store(false, Ordering::Release);
                    }
                    let until = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64
                        + 30_000;
                    let value = match path.as_str() {
                        "/hagency/api/v1/discovery" => {
                            json!({"product":"hagency-server","version":"0.1.0","protocolVersion":3,"capabilities":["pasion-oauth","owner-agent-appservice-v1","global-agent-identity-v2","execution-device-v1","owner-direct-v1"],"homeserver":issuer.strip_suffix("auth/").unwrap(),"issuer":issuer})
                        }
                        "/hagency/api/v1/sessions/pasion" => {
                            json!({"token":"a".repeat(64),"userId":"uid","mxid":mxid,"validUntilMs":until})
                        }
                        "/hagency/api/v1/identity" => {
                            json!({"userId":"uid","mxid":mxid,"issuer":issuer,"subject":"sub","clientId":"sdk-client","validUntilMs":until})
                        }
                        "/hagency/api/v1/devices" => {
                            json!({"deviceId":"device","token":"b".repeat(64),"generation":1,"validUntilMs":until})
                        }
                        "/hagency/api/v1/sessions/current/renew" => {
                            json!({"validUntilMs":if short_proof{until-25_000}else{until}})
                        }
                        "/hagency/api/v1/sessions/current" => json!({}),
                        "/hagency/api/v1/projects" => json!({"projects":[]}),
                        "/hagency/api/v1/owned-agents/agent-a" => {
                            json!({"agent":{"id":"agent-a","ownerUserId":"uid","puppetMxid":"@agent:test","displayName":"Agent","state":"active","generation":1,"executionDeviceId":"device"}})
                        }
                        "/hagency/api/v1/owned-agents/agent-a/bindings" => json!({"bindings":[]}),
                        "/_matrix/client/v3/joined_rooms" => json!({"joined_rooms":[]}),
                        _ => json!({"code":"unexpected_request"}),
                    };
                    let mode = behavior.load(Ordering::Acquire);
                    if path.ends_with("/projects") && mode == 1 {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    let status = if path.ends_with("/renew") && mode == 2 {
                        503
                    } else if path.ends_with("/renew") && mode == 3 {
                        401
                    } else {
                        200
                    };
                    let value = if status == 200 {
                        value
                    } else {
                        json!({"code":if status==401 {"sign_in_required"} else {"server_unavailable"}})
                    };
                    let body = value.to_string();
                    let response = format!(
                        "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (origin, requests, task)
    }
    #[tokio::test]
    async fn missing_local_model_is_reported_before_starting_a_service() {
        use super::super::super::native::LocalAction;
        let (origin, requests, task) = fixture("@owner:test", None).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("sdk-first".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let native = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source,
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        let command = |action, input| Command::Local {
            agent_id: "agent-a".into(),
            action,
            input,
            binding_id: None,
            requester: None,
        };
        let status = native
            .execute(command(LocalAction::AgentServiceStatus, None))
            .await
            .unwrap();
        assert_eq!(status["service"]["modelConfigured"], false);
        assert_eq!(status["service"]["enabled"], false);
        assert!(
            !root
                .path()
                .join("state")
                .read_dir()
                .unwrap()
                .any(|entry| entry.unwrap().file_name() == "runtime-intents.json")
        );
        let error = native
            .execute(command(LocalAction::StartAgentService, Some(json!({}))))
            .await
            .unwrap_err();
        assert_eq!(error.code, "agent_model_required");
        assert!(requests.lock().unwrap().iter().all(
            |(path, _)| !path.ends_with("/leases/acquire") && !path.ends_with("/events/start")
        ));
        let device = native.test_console().authorized_device().await.unwrap();
        let path = super::super::super::owned_agents::ledger_path(
            &root.path().join("state"),
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .unwrap();
        let profile_id = super::super::super::owned_agents::profile_identity(
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .unwrap();
        let mut ledger =
            hagency_agent_local::Ledger::open_scoped(&path, device.owner_mxid(), &profile_id)
                .unwrap();
        ledger
            .set_agent_model_profile(
                device.owner_mxid(),
                "agent-a",
                &hagency_agent_local::ModelProfile {
                    model: "test-model".into(),
                    credential_ref: "keychain:fixture".into(),
                    workspace_root: root.path().to_string_lossy().into(),
                    reasoning_effort: "low".into(),
                    web_search: true,
                    programming: Default::default(),
                },
            )
            .unwrap();
        drop(ledger);
        let status = native
            .execute(command(LocalAction::AgentServiceStatus, None))
            .await
            .unwrap();
        assert_eq!(
            status["service"]["modelConfigured"], true,
            "saving a model must remove the setup requirement without starting it"
        );
        assert_eq!(status["service"]["enabled"], false);
        assert!(
            !path.with_file_name("runtime-intents.json").exists(),
            "status reads must never create a service intent"
        );
        native.shutdown().await;
        task.abort();
    }
    #[tokio::test]
    async fn shared_runtime_proof_is_bounded_and_expiry_always_forces_fresh_authorization() {
        let (origin, requests, task) = fixture("@owner:test", None).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("sdk-first".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let native = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source.clone(),
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        let login = &native.test_console().0.server_login;
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .find(|(path, _)| path == "/hagency/api/v1/devices")
                .unwrap()
                .1["name"],
            local_device_name(),
            "device registration must publish the computer name rather than the app name"
        );
        source.calls.store(0, Ordering::Relaxed);
        for _ in 0..25 {
            login.authorized_device().await.unwrap();
        }
        assert_eq!(
            source.calls.load(Ordering::Relaxed),
            0,
            "finite fresh proof prevents poll amplification"
        );
        for session in login.sessions.lock().await.values_mut() {
            session.checked = Instant::now() - Duration::from_secs(6);
        }
        login.authorized_device().await.unwrap();
        assert_eq!(source.calls.load(Ordering::Relaxed), 2);
        for session in login.sessions.lock().await.values_mut() {
            session.checked = Instant::now();
            session.authorized_until = Instant::now() + Duration::from_secs(1);
            session.oauth_expires = session.authorized_until;
        }
        login.authorized_device().await.unwrap();
        assert_eq!(
            source.refreshes.load(Ordering::Relaxed),
            0,
            "fresh thirty-second server proof must not rotate long-lived OAuth grant"
        );
        assert_eq!(
            source.calls.load(Ordering::Relaxed),
            4,
            "near-expiry proof cannot use cache"
        );
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(p, _)| p.ends_with("/renew"))
                .count(),
            2
        );
        source.current.store(false, Ordering::Release);
        for session in login.sessions.lock().await.values_mut() {
            session.checked = Instant::now() - Duration::from_secs(6);
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(1), login.authorized_device())
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(1), login.authorized_device())
                .await
                .unwrap()
                .is_err(),
            "invalidated proof stays closed and lock drains"
        );
        native.shutdown().await;
        task.abort();
    }
    #[tokio::test]
    async fn temporary_renewal_preserves_session_without_extending_expired_proof() {
        let mode = Arc::new(std::sync::atomic::AtomicU16::new(0));
        let (origin, requests, task) = fixture_controlled("@owner:test", None, mode.clone()).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("sdk-first".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let native = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source,
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        let login = &native.test_console().0.server_login;
        let old = login.authorized_device().await.unwrap();
        mode.store(2, Ordering::Release);
        for s in login.sessions.lock().await.values_mut() {
            s.checked = Instant::now() - Duration::from_secs(6);
        }
        assert!(matches!(
            login.authorized_device().await,
            Err(Error::Unavailable)
        ));
        assert!(old.bearer().is_ok());
        assert!(login.revocations.lock().await.is_empty());
        for s in login.sessions.lock().await.values_mut() {
            s.authorized_until = Instant::now() - Duration::from_secs(1);
            s.oauth_expires = s.authorized_until;
        }
        assert!(matches!(
            login.authorized_device().await,
            Err(Error::Unavailable)
        ));
        assert!(
            login
                .sessions
                .lock()
                .await
                .values()
                .all(|s| !s.invalidated && s.authorized_until < Instant::now())
        );
        mode.store(0, Ordering::Release);
        assert!(login.authorized_device().await.unwrap().bearer().is_ok());
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(p, _)| p.ends_with("/sessions/pasion"))
                .count(),
            1
        );
        mode.store(3, Ordering::Release);
        for s in login.sessions.lock().await.values_mut() {
            s.checked = Instant::now() - Duration::from_secs(6);
        }
        assert!(matches!(
            login.authorized_device().await,
            Err(Error::Unauthorized)
        ));
        assert!(old.bearer().is_err());
        native.shutdown().await;
        task.abort();
    }
    #[tokio::test]
    async fn slow_management_does_not_block_renewal_and_late_logout_discards_reply() {
        let mode = Arc::new(std::sync::atomic::AtomicU16::new(1));
        let (origin, requests, task) = fixture_controlled("@owner:test", None, mode).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("sdk-first".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let native = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source,
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        let console = native.test_console().clone();
        let login = &console.0.server_login;
        let device = login.authorized_device().await.unwrap();
        let pin = super::super::super::device_execution::DeviceIdentity::from(&device);
        let query_console = console.clone();
        let read = tokio::spawn(async move {
            query_console
                .owner_api_pinned(&pin, OwnerOperation::Projects)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(p, _)| p.ends_with("/projects"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for s in login.sessions.lock().await.values_mut() {
            s.checked = Instant::now() - Duration::from_secs(6);
        }
        tokio::time::timeout(Duration::from_millis(400), login.authorized_device())
            .await
            .expect("a slow management request cannot hold the authorization lock")
            .unwrap();
        login.revoke_native_all().await;
        assert!(
            read.await.unwrap().is_err(),
            "late reply cannot revive logged-out authority"
        );
        native.shutdown().await;
        task.abort();
    }
    #[tokio::test]
    async fn cookie_rotation_preserves_inflight_management_reply() {
        let mode = Arc::new(std::sync::atomic::AtomicU16::new(1));
        let (origin, requests, task) = fixture_controlled("@owner:test", None, mode).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("sdk-first".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let native = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source,
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        let console = native.test_console().clone();
        let login = &console.0.server_login;
        let device = login.authorized_device().await.unwrap();
        let pin = super::super::super::device_execution::DeviceIdentity::from(&device);
        let query_console = console.clone();
        let read = tokio::spawn(async move {
            query_console
                .owner_api_pinned(&pin, OwnerOperation::Projects)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(p, _)| p.ends_with("/projects"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for s in login.sessions.lock().await.values_mut() {
            s.checked = Instant::now() - Duration::from_secs(6);
        }
        tokio::time::timeout(Duration::from_millis(400), login.authorized_device())
            .await
            .expect("a slow management request cannot hold the authorization lock")
            .unwrap();
        let old_key = {
            let mut sessions = login.sessions.lock().await;
            let (key, session) = sessions.iter_mut().next().unwrap();
            session.expires = Instant::now() + Duration::from_secs(30);
            key.clone()
        };
        native.execute(Command::LoginStatus).await.unwrap();
        assert!(!login.sessions.lock().await.contains_key(&old_key));
        read.await
            .unwrap()
            .expect("same grant survives cookie rotation while HTTP is in flight");
        native.shutdown().await;
        task.abort();
    }
    #[tokio::test]
    async fn cookie_rotation_cannot_make_concurrent_runtime_authorization_stale() {
        let (origin, _, task) = fixture("@owner:test", None).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("sdk-first".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let native = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source,
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        let console = native.test_console().clone();
        let login = &console.0.server_login;
        let old_key = {
            let mut sessions = login.sessions.lock().await;
            let (key, s) = sessions.iter_mut().next().unwrap();
            s.expires = Instant::now() + Duration::from_secs(30);
            key.clone()
        };
        let gate = login.renewal.lock().await;
        let rotating = native.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let rotation = tokio::spawn(async move {
            started.send(()).unwrap();
            rotating.execute(Command::LoginStatus).await
        });
        ready.await.unwrap();
        let runtime_console = console.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let runtime = tokio::spawn(async move {
            started.send(()).unwrap();
            runtime_console.authorized_device().await
        });
        ready.await.unwrap();
        drop(gate);
        rotation.await.unwrap().unwrap();
        assert!(runtime.await.unwrap().unwrap().bearer().is_ok());
        assert!(
            !login.sessions.lock().await.contains_key(&old_key),
            "test must exercise a real cookie rotation"
        );
        native.shutdown().await;
        task.abort();
    }
    #[tokio::test]
    async fn actual_short_server_proof_refreshes_sdk_once_before_returning_a_live_device() {
        let (origin, requests, task) = fixture("@owner:test", None).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("sdk-first".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let native = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source.clone(),
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        *source.token.lock().unwrap() = "sdk-short".into();
        for session in native
            .test_console()
            .0
            .server_login
            .sessions
            .lock()
            .await
            .values_mut()
        {
            session.checked = Instant::now() - Duration::from_secs(6);
        }
        let device = native.test_console().authorized_device().await.unwrap();
        assert!(device.bearer().is_ok());
        assert_eq!(source.refreshes.load(Ordering::Relaxed), 1);
        assert!(
            requests
                .lock()
                .unwrap()
                .iter()
                .any(|(p, b)| p.ends_with("/renew") && b["accessToken"] == "sdk-refreshed")
        );
        assert!(device.valid_until > Instant::now() + Duration::from_secs(20));
        native.shutdown().await;
        task.abort();
    }
    #[tokio::test]
    async fn sdk_authorization_uses_rotated_snapshot_and_never_refreshes_or_revokes_oauth() {
        let (origin, requests, task) = fixture("@owner:test", None).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("sdk-first".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let native = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source.clone(),
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        assert_eq!(
            native.execute(Command::BeginLogin).await.unwrap_err().code,
            "sdk_owns_matrix_login"
        );
        *source.token.lock().unwrap() = "sdk-rotated".into();
        for session in native
            .test_console()
            .0
            .server_login
            .sessions
            .lock()
            .await
            .values_mut()
        {
            session.checked = Instant::now() - Duration::from_secs(6);
        }
        assert_eq!(native.identity().await.unwrap().subject, "sub");
        native.execute(Command::LoginStatus).await.unwrap();
        native
            .execute(Command::SpaceCandidates { cursor: None })
            .await
            .unwrap();
        {
            let mut sessions = native.test_console().0.server_login.sessions.lock().await;
            for session in sessions.values_mut() {
                session.expires = Instant::now() - Duration::from_secs(1);
                assert!(session.oauth_token.is_empty());
                assert!(session.refresh_token.is_none());
            }
        }
        native.execute(Command::LoginStatus).await.unwrap();
        native.execute(Command::Logout).await.unwrap();
        assert!(source.access_token().await.is_ok());
        let records = requests.lock().unwrap();
        assert!(
            records
                .iter()
                .any(|(path, body)| path == "/_matrix/client/v3/joined_rooms"
                    && body["testBearer"] == "Bearer sdk-rotated")
        );
        assert!(
            records.iter().any(
                |(path, body)| path.ends_with("/renew") && body["accessToken"] == "sdk-rotated"
            )
        );
        assert!(
            records
                .iter()
                .any(|(path, _)| path == "/hagency/api/v1/sessions/current")
        );
        assert!(!records.iter().any(|(path, _)| path.starts_with("/auth/")));
        let metadata =
            std::fs::read_to_string(root.path().join("state/server-login-revocations.json"))
                .unwrap_or_default();
        assert!(!metadata.contains("sdk-first") && !metadata.contains("sdk-rotated"));
        task.abort();
    }
    #[tokio::test]
    async fn wrong_sdk_account_is_rejected_and_only_our_grant_is_revoked() {
        let (origin, requests, task) = fixture("@other:test", None).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("shared-secret".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let error = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source,
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.code, "matrix_account_mismatch");
        let records = requests.lock().unwrap();
        assert!(!records.iter().any(|(path, _)| path.starts_with("/auth/")));
        assert!(!records.iter().any(|(path, _)| path.ends_with("/devices")));
        assert!(
            records
                .iter()
                .any(|(path, _)| path == "/hagency/api/v1/sessions/current")
        );
        task.abort();
    }
    #[tokio::test]
    async fn late_sdk_epoch_change_cannot_install_owner_or_persist_shared_token() {
        let current = Arc::new(AtomicBool::new(true));
        let (origin, requests, task) = fixture("@owner:test", Some(current.clone())).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("shared-epoch-secret".into()),
            current,
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let error = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source,
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.code, "matrix_account_changed");
        assert!(!root.path().join("state/server-login.json").exists());
        let records = requests.lock().unwrap();
        assert!(
            records
                .iter()
                .any(|(path, _)| path == "/hagency/api/v1/sessions/current")
        );
        assert!(!records.iter().any(|(path, _)| path.starts_with("/auth/")));
        let saved =
            std::fs::read_to_string(root.path().join("state/server-login-revocations.json"))
                .unwrap_or_default();
        assert!(!saved.contains("shared-epoch-secret"));
        task.abort();
    }
    #[tokio::test]
    async fn authorized_sdk_owner_can_retry_after_temporary_source_outage() {
        let (origin, requests, task) = fixture("@owner:test", None).await;
        let root = tempfile::tempdir().unwrap();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("sdk-temporary".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_authorization_unavailable",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let native = NativeOwner::open_with_matrix_and_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            source.clone(),
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        let old = native
            .test_console()
            .0
            .server_login
            .authorized_device()
            .await
            .unwrap();
        source.current.store(false, Ordering::Release);
        let error = native.execute(Command::LoginStatus).await.unwrap_err();
        assert_eq!(error.code, "matrix_authorization_unavailable");
        assert_eq!(error.status, 503);
        assert!(
            old.bearer().is_ok(),
            "temporary failure cannot revoke an unexpired proof"
        );
        source.current.store(true, Ordering::Release);
        native.execute(Command::LoginStatus).await.unwrap();
        assert!(native.identity().await.is_ok());
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(p, _)| p.ends_with("/sessions/pasion"))
                .count(),
            1,
            "temporary outage must recover the existing session"
        );
        native.shutdown().await;
        assert!(
            !requests
                .lock()
                .unwrap()
                .iter()
                .any(|(path, _)| path.starts_with("/auth/"))
        );
        task.abort();
    }
    #[tokio::test]
    async fn retire_while_waiting_for_profile_lock_does_not_publish_sdk_binding() {
        let (origin, requests, task) = fixture("@owner:test", None).await;
        let root = tempfile::tempdir().unwrap();
        let native = NativeOwner::open_with_credentials(
            &root.path().join("state"),
            &origin,
            "@owner:test",
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        let console = native.test_console().clone();
        let locked = console.0.server_login.profiles.lock().await;
        let runner = console.clone();
        let origin2 = origin.clone();
        let source = Arc::new(Source {
            token: std::sync::Mutex::new("sdk-late".into()),
            current: Arc::new(AtomicBool::new(true)),
            error_code: "matrix_account_changed",
            calls: std::sync::atomic::AtomicUsize::new(0),
            refreshes: std::sync::atomic::AtomicUsize::new(0),
        });
        let authorize = tokio::spawn(async move {
            runner
                .0
                .server_login
                .authorize_matrix(&runner, &origin2, "@owner:test", source)
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(path, _)| path.ends_with("/devices"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        native.retire();
        drop(locked);
        assert_eq!(
            authorize.await.unwrap().unwrap_err(),
            "matrix_account_changed"
        );
        assert!(!root.path().join("state/server-login.json").exists());
        assert!(
            !root
                .path()
                .join("state/server-login-profiles.json")
                .exists()
        );
        native.shutdown().await;
        task.abort();
    }
    struct FailingSource(&'static str);
    impl MatrixTokenSource for FailingSource {
        fn access_token(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                Err(NativeError {
                    status: 503,
                    code: self.0.into(),
                })
            })
        }
    }
    #[tokio::test]
    async fn sdk_source_offline_and_unsupported_errors_preserve_retry_semantics_without_grants() {
        let (origin, requests, task) = fixture("@owner:test", None).await;
        for code in [
            "matrix_authorization_unavailable",
            "matrix_oauth_sign_in_required",
            "unsupported_hagency_issuer",
        ] {
            let root = tempfile::tempdir().unwrap();
            let error = NativeOwner::open_with_matrix_and_credentials(
                &root.path().join("state"),
                &origin,
                "@owner:test",
                Arc::new(FailingSource(code)),
                crate::console::server_login::credential_tests::vault(),
            )
            .await
            .err()
            .unwrap();
            assert_eq!(error.code, code);
            if code == "matrix_authorization_unavailable" {
                assert_eq!(error.status, 503);
            }
        }
        assert!(
            requests
                .lock()
                .unwrap()
                .iter()
                .all(|(path, _)| path == "/hagency/api/v1/discovery")
        );
        task.abort();
    }
    #[test]
    fn sdk_access_snapshot_debug_never_discloses_token() {
        let token = MatrixAccessToken {
            access_token: "private-token".into(),
            client_id: "sdk".into(),
        };
        assert!(!format!("{token:?}").contains("private-token"));
    }
}
