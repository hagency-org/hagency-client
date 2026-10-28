//! Actual TCP authorization and original completion loss recovery, no provider.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
struct Source;
impl MatrixTokenSource for Source {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Box::pin(async {
            Ok(MatrixAccessToken {
                access_token: "fixture-sdk".into(),
                client_id: "fixture-client".into(),
            })
        })
    }
}
#[derive(Default)]
struct State {
    work: Option<Value>,
    polls: usize,
    heartbeats: usize,
    completions: Vec<Value>,
    lost: bool,
    old_server: bool,
}
async fn fixture() -> (String, Arc<StdMutex<State>>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let base = origin.clone();
    let state = Arc::new(StdMutex::new(State::default()));
    let records = state.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let state = records.clone();
            let base = base.clone();
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0; 4096];
                let split = loop {
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(i) = raw.windows(4).position(|s| s == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&raw[..split]).into_owned();
                let path = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let len = headers
                    .lines()
                    .find_map(|l| {
                        l.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while raw.len() < split + len {
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                }
                let input: Value =
                    serde_json::from_slice(&raw[split..split + len]).unwrap_or(Value::Null);
                let value = {
                    let mut s = state.lock().unwrap();
                    let until = now() + 30000;
                    match path.as_str() {
                        "/hagency/api/v1/discovery" => {
                            let mut caps = vec![
                                "pasion-oauth",
                                "owner-agent-appservice-v1",
                                "global-agent-identity-v2",
                                "execution-device-v1",
                                "owner-direct-v1",
                            ];
                            if !s.old_server {
                                caps.push(CAPABILITY);
                            }
                            json!({"product":"hagency-server","version":"0.1.0","protocolVersion":3,"capabilities":caps,"homeserver":base,"issuer":format!("{base}auth/")})
                        }
                        "/hagency/api/v1/sessions/pasion" => {
                            json!({"token":"a".repeat(64),"userId":"uid","mxid":"@owner:test","validUntilMs":until})
                        }
                        "/hagency/api/v1/identity" => {
                            json!({"userId":"uid","mxid":"@owner:test","subject":"sub","clientId":"fixture-client","issuer":format!("{base}auth/"),"validUntilMs":until})
                        }
                        "/hagency/api/v1/devices" => {
                            json!({"deviceId":"dev_a","token":"b".repeat(64),"generation":1,"validUntilMs":until})
                        }
                        "/hagency/api/v1/devices/current" => {
                            json!({"userId":"uid","mxid":"@owner:test","subject":"sub","clientId":"fixture-client","issuer":format!("{base}auth/"),"deviceId":"dev_a","generation":1,"validUntilMs":until})
                        }
                        "/hagency/api/v1/sessions/current/renew" => json!({"validUntilMs":until}),
                        "/hagency/api/v1/sessions/current" => json!({}),
                        "/hagency/api/v1/agents" => {
                            json!({"agents":[{"id":"agent_a","ownerUserId":"uid","puppetMxid":"@agent:test","displayName":"A","state":"active","executionDeviceId":"dev_a","generation":1,"ownerDirectRoomId":null}]})
                        }
                        "/hagency/api/v1/agents/agent_a/configuration" => {
                            json!({"configuration":{"agentId":"agent_a","agentGeneration":1,"executionDeviceId":"dev_a","deviceGeneration":1,"online":true,"lastSeenAtMs":now(),"observedAtMs":null,"cached":false,"environment":null}})
                        }
                        "/hagency/api/v1/device-configuration/poll" => {
                            if input["acceptWork"] == true {
                                s.polls += 1;
                                let w =
                                    s.work
                                        .as_mut()
                                        .filter(|w| w["state"] == "pending")
                                        .map(|w| {
                                            w["state"] = "claimed".into();
                                            w["claimedAtMs"] = now().into();
                                            w.clone()
                                        });
                                json!({"request":w})
                            } else {
                                s.heartbeats += 1;
                                json!({"request":null})
                            }
                        }
                        "/hagency/api/v1/agents/agent_a/configuration/requests/cfg_a" => {
                            json!({"request":s.work})
                        }
                        "/hagency/api/v1/device-configuration/requests/cfg_a/complete" => {
                            s.completions.push(input.clone());
                            let w = s.work.as_mut().unwrap();
                            w["state"] = input["state"].clone();
                            w["result"] = input["result"].clone();
                            w["errorCode"] = input["errorCode"].clone();
                            w["completedAtMs"] = now().into();
                            let result = json!({"request":w});
                            if !s.lost {
                                s.lost = true;
                                return;
                            }
                            result
                        }
                        _ => json!({"code":"fixture_unknown_route"}),
                    }
                };
                let body = serde_json::to_vec(&value).unwrap();
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            });
        }
    });
    (origin, state, server)
}
async fn open(root: &Path, base: &str) -> NativeOwner {
    let native = NativeOwner::open_with_matrix(root, base, "@owner:test", Arc::new(Source))
        .await
        .unwrap();
    let tasks = std::mem::take(&mut *native.0.configuration_tasks.lock().unwrap());
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    native
}
#[tokio::test]
async fn idle_configuration_journal_does_not_create_open_or_migrate_personal_database() {
    let (base, state, server) = fixture().await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let a = open(&root.path().join("state"), &base).await;
    let identity = pin(&a).await.unwrap();
    let path = journal(&a, &identity).unwrap();
    let profile = path.parent().unwrap().parent().unwrap();
    let database = profile.join("agent-local.sqlite");
    assert!(!database.exists());
    tick(&a).await.unwrap();
    assert!(!database.exists());
    assert!(load(&path).unwrap().is_none());
    // Polling an idle executor has no authority to inspect or repair this file.
    hagency_store::private::write_new(&database, b"unrelated old database bytes").unwrap();
    let marker = std::fs::read(profile.join("profile-identity.json")).unwrap();
    tick(&a).await.unwrap();
    assert_eq!(
        std::fs::read(&database).unwrap(),
        b"unrelated old database bytes"
    );
    assert_eq!(
        std::fs::read(profile.join("profile-identity.json")).unwrap(),
        marker
    );
    assert!(!profile.join("agent-local.sqlite-wal").exists());
    assert!(!profile.join("agent-local.sqlite-shm").exists());
    state.lock().unwrap().old_server = true;
    assert_eq!(
        tick(&a).await.unwrap_err().code,
        "configuration_unsupported"
    );
    assert_eq!(
        std::fs::read(&database).unwrap(),
        b"unrelated old database bytes"
    );
    a.shutdown().await;
    server.abort();
}
#[tokio::test]
async fn worker_tcp_loss_reopen_retries_original_never_reapplies_and_old_server_refuses() {
    let (base, state, server) = fixture().await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let a = open(&root.path().join("state"), &base).await;
    state.lock().unwrap().work = Some(
        json!({"id":"cfg_a","agentId":"agent_a","agentGeneration":1,"executionDeviceId":"dev_a","deviceGeneration":1,"operation":"prepare_workspace","input":{},"state":"pending","createdAtMs":now(),"expiresAtMs":now()+60000,"claimedAtMs":null,"completedAtMs":null,"result":null,"errorCode":null}),
    );
    assert!(tick(&a).await.is_err());
    let path = journal(&a, &pin(&a).await.unwrap()).unwrap();
    let receipt = load(&path).unwrap().unwrap();
    let workspace = receipt.completion.as_ref().unwrap()["result"]["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(Path::new(&workspace).is_dir());
    assert_eq!(state.lock().unwrap().polls, 1);
    assert_eq!(state.lock().unwrap().completions.len(), 1);
    a.suspend().await.unwrap();
    drop(a);
    let a = open(&root.path().join("state"), &base).await;
    tick(&a).await.unwrap();
    assert!(load(&path).unwrap().is_none());
    {
        let s = state.lock().unwrap();
        assert_eq!(s.polls, 1);
        assert_eq!(s.completions.len(), 2);
        assert_eq!(s.completions[0], s.completions[1]);
    }
    let mut applying = receipt;
    applying.completion = None;
    save(&path, &applying).unwrap();
    tick(&a).await.unwrap();
    assert!(load(&path).unwrap().unwrap().completion.is_none());
    assert_eq!(state.lock().unwrap().polls, 1);
    assert_eq!(state.lock().unwrap().completions.len(), 2);
    state.lock().unwrap().old_server = true;
    assert_eq!(
        a.execute(Command::AgentConfigurationStatus {
            agent_id: "agent_a".into()
        })
        .await
        .unwrap_err()
        .code,
        "configuration_unsupported"
    );
    a.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn configuration_heartbeat_remains_online_while_local_dispatch_lock_is_busy() {
    let (base, state, server) = fixture().await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let a = NativeOwner::open_with_matrix(
        &root.path().join("state"),
        &base,
        "@owner:test",
        Arc::new(Source),
    )
    .await
    .unwrap();
    let guard = a.0.calls.lock().await;
    let before = state.lock().unwrap().heartbeats;
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert!(
        state.lock().unwrap().heartbeats >= before + 2,
        "heartbeat must run independently of serialized provider/runtime dispatch"
    );
    drop(guard);
    a.shutdown().await;
    server.abort();
}
