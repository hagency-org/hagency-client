//! Cross-repository qualification. The separately built current Server supplies
//! real HTTP/PG/AS/workers. Pasion/Matrix/provider doubles are explicit.
use super::*;
use crate::console::native::{Command, HistoryAction};
use hagency_agent_local::{
    Budget, Layer, Limit, ModelProfile, Period, Policy, RequestPolicy, ToolPolicy,
};
use std::process::{Child, Command as ProcessCommand, Stdio};
struct Fixture {
    child: Child,
    info: Value,
    root: PathBuf,
    port: u16,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Fixture {
    async fn start(root: PathBuf, port: u16, phase: &str) -> Self {
        let binary = PathBuf::from(
            std::env::var_os("HAGENCY_AGENT_SERVER_FIXTURE_BINARY")
                .expect("explicit current Server fixture binary"),
        );
        let database =
            std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("isolated PostgreSQL database");
        let parsed: reqwest::Url = database.parse().unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert!(parsed.path().starts_with("/agent_files_joint_"));
        let info_path = root.join(format!("server-{phase}.json"));
        let log_path = root.join(format!("server-{phase}.log"));
        let log = std::fs::File::create(&log_path).unwrap();
        let child = ProcessCommand::new(binary)
            .env("HAGENCY_AGENT_TEST_DATABASE_URL", database)
            .env("HAGENCY_FILE_FIXTURE_PORT", port.to_string())
            .env("HAGENCY_FILE_FIXTURE_INFO", &info_path)
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        let mut f = Self {
            child,
            info: Value::Null,
            root,
            port,
        };
        let ready = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Ok(bytes) = std::fs::read(&info_path) {
                    f.info = serde_json::from_slice(&bytes).unwrap();
                    break;
                }
                if let Some(exit) = f.child.try_wait().unwrap() {
                    panic!(
                        "original fixture exit {exit}: {}",
                        std::fs::read_to_string(&log_path).unwrap()
                    );
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        assert!(
            ready.is_ok(),
            "fixture startup: {}",
            std::fs::read_to_string(&log_path).unwrap()
        );
        assert_eq!(f.info["origin"], format!("http://127.0.0.1:{port}/"));
        f
    }
    async fn snapshot(&self) -> Value {
        reqwest::get(format!("http://127.0.0.1:{}/fixture/snapshot", self.port))
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}
struct Source {
    origin: String,
}
impl MatrixTokenSource for Source {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        super::Source.access_token()
    }
    fn supports_agent_file_receive(&self) -> bool {
        true
    }
    fn fetch_agent_attachment(
        &self,
        attachment: super::super::super::native::AgentAttachment,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u8>, NativeError>> + Send + '_>,
    > {
        Box::pin(async move {
            attachment.validate().map_err(|_| NativeError {
                status: 400,
                code: "fixture_bad_original".into(),
            })?;
            assert_eq!(attachment.source["url"], "mxc://test/joint-incoming");
            let response = reqwest::Client::new()
                .get(format!(
                    "{}_matrix/client/v1/media/download/test/joint-incoming",
                    self.origin
                ))
                .bearer_auth("fixture-sdk-only")
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
            let bytes = response.bytes().await.unwrap();
            assert_eq!(bytes.len(), 131_073);
            Ok(bytes.to_vec())
        })
    }
}
fn joint_provider(script: &std::path::Path, marker: &std::path::Path, path: &std::path::Path) {
    provider(script, marker, path, false);
    let source = std::fs::read_to_string(script).unwrap();
    let a = source.find("  time.sleep(6)").unwrap();
    let b = source[a..].find(" else: raise Exception(m)").unwrap() + a;
    let mut source = format!(
        "{}{}{}",
        &source[..a],
        "  emit('item/tool/call',{'threadId':'thread-a','turnId':'turn-a','callId':'list','tool':'list_received_files','arguments':{}},'joint-list')\n",
        &source[b..]
    );
    source=source.replace(" def emit(m,p): print(json.dumps({'method':m,'params':p}),flush=True)"," def emit(m,p,i=None): print(json.dumps(dict({'method':m,'params':p},**({'id':i} if i is not None else {}))),flush=True)");
    source=source.replace(" elif m in ['thread/start','thread/resume']:\n"," elif m in ['thread/start','thread/resume']:\n  workspace=p['cwd']\n  assert {'receive_file','list_received_files','send_file','get_file_delivery'} <= {t['name'] for t in p['dynamicTools']}\n  assert 'mxc://' not in str(p)\n");
    let branch = r#" if m is None:
  assert f['result']['success'],f
  result=json.loads(f['result']['contentItems'][0]['text'])
  assert 'mxc://' not in str(result)
  if f['id']=='joint-list':
   assert len(result['files'])==1
   assert result['files'][0]['event_id']=='$joint-incoming:test'
   emit('item/tool/call',{'threadId':'thread-a','turnId':'turn-a','callId':'receive','tool':'receive_file','arguments':{'event_id':'$joint-incoming:test'}},'joint-receive')
  elif f['id']=='joint-receive':
   data=open(result['path'],'rb').read()
   assert data==bytes(i%256 for i in range(131073))
   open(workspace+'/output.bin','wb').write(data+b'\nfixture output\n')
   query=0
   emit('item/tool/call',{'threadId':'thread-a','turnId':'turn-a','callId':'send','tool':'send_file','arguments':{'path':'output.bin'}},'joint-send')
  else:
   if result['state']=='sent':
    assert result['matrix_event_id'].startswith('$')
    emit('thread/tokenUsage/updated',{'threadId':'thread-a','turnId':'turn-a','tokenUsage':{'total':{'inputTokens':3,'outputTokens':2,'cachedInputTokens':0,'reasoningOutputTokens':0,'totalTokens':5},'last':{'inputTokens':3,'outputTokens':2,'cachedInputTokens':0,'reasoningOutputTokens':0,'totalTokens':5}}})
    emit('item/completed',{'threadId':'thread-a','turnId':'turn-a','item':{'id':'message-a','type':'agentMessage','text':'JOINT_FILES_COMPLETE'}})
    emit('turn/completed',{'threadId':'thread-a','turn':{'id':'turn-a','status':'completed'}})
   else:
    query+=1
    assert query<=10
    time.sleep(1)
    emit('item/tool/call',{'threadId':'thread-a','turnId':'turn-a','callId':'status-'+str(query),'tool':'get_file_delivery','arguments':{'delivery_id':result['delivery_id']}},'joint-status-'+str(query))
  continue
"#;
    source = source.replace(
        " if m=='initialize':",
        &format!("{branch} if m=='initialize':"),
    );
    std::fs::write(script, source).unwrap();
}
async fn host(
    console: &Console,
    config: StartConfig,
    path: &std::path::Path,
    done: impl std::future::Future<Output = ()>,
) -> Result<(), RuntimeError> {
    let device = console.authorized_device().await.unwrap();
    let agent = config.agent_id.clone();
    let (initial, binding_handle, _) = binding_work(config);
    let (commands, rx) = mpsc::unbounded_channel();
    let (stop, cancel) = watch::channel(false);
    let (status, mut status_rx) = watch::channel(RuntimeStatus::new(&agent, "joint_fixture", None));
    let worker = tokio::spawn(run_host(
        console.clone(),
        device,
        path.into(),
        initial,
        rx,
        cancel,
        stop.clone(),
        status,
    ));
    let result = tokio::time::timeout(Duration::from_secs(30), done).await;
    stop.send_replace(true);
    drop(commands);
    let outcome = tokio::time::timeout(Duration::from_secs(8), worker)
        .await
        .unwrap()
        .unwrap();
    if result.is_err() {
        eprintln!(
            "original joint binding: {:?}; outcome: {:?}; ledger: {}",
            binding_handle.status.borrow(),
            outcome,
            path.display()
        );
        eprintln!(
            "original joint runtime status: {:?}",
            status_rx.borrow_and_update()
        );
    }
    assert!(result.is_ok(), "joint original completion deadline");
    outcome
}
#[tokio::test]
#[ignore = "requires separately built current Server fixture and isolated PostgreSQL; run explicitly"]
async fn native_owner_current_server_file_round_trip_and_restart_preserve_original_bytes() {
    qualification().await;
}
#[tokio::test]
#[ignore = "paid real Codex qualification: explicit opt-in, current Server fixture and isolated PostgreSQL required"]
async fn native_owner_current_server_real_codex_file_round_trip() {
    assert_eq!(
        std::env::var("HAGENCY_JOINT_REAL_CODEX").as_deref(),
        Ok("1"),
        "explicit paid inference opt-in required"
    );
    let binary = PathBuf::from(
        std::env::var_os("HAGENCY_CLIENT_FILE_QUALIFICATION_BINARY")
            .expect("public NativeOwner qualification example binary"),
    );
    let root = tempfile::Builder::new()
        .prefix("hagency-joint-public-")
        .tempdir()
        .unwrap()
        .keep()
        .canonicalize()
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let f = Fixture::start(root.clone(), port, "initial").await;
    let output = tokio::time::timeout(
        Duration::from_secs(180),
        tokio::process::Command::new(binary)
            .kill_on_drop(true)
            .env("HAGENCY_JOINT_REAL_CODEX", "1")
            .env(
                "HAGENCY_FILE_FIXTURE_INFO",
                root.join("server-initial.json"),
            )
            .env("HAGENCY_FILE_QUALIFICATION_STATE", root.join("state"))
            .env_remove("HAGENCY_FILE_QUALIFICATION_VERIFY_EXISTING")
            .output(),
    )
    .await
    .expect("original public qualification deadline")
    .expect("public qualification process");
    assert!(
        output.status.success(),
        "original public qualification exit {}; stdout {}; stderr {}; evidence {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        root.display()
    );
    let evidence: Value =
        serde_json::from_slice(&std::fs::read(root.join("real-codex-files-result.json")).unwrap())
            .unwrap();
    assert_eq!(evidence["realCodex"], true);
    assert_eq!(evidence["fileBytes"], 131_089);
    assert_eq!(f.snapshot().await["uploads"], 1);
}
async fn qualification() {
    let root = tempfile::Builder::new()
        .prefix("hagency-joint-files-")
        .tempdir()
        .unwrap()
        .keep()
        .canonicalize()
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let f = Fixture::start(root.clone(), port, "initial").await;
    let origin = f.info["origin"].as_str().unwrap().to_owned();
    assert_eq!(f.info["owner"], OWNER);
    let native = NativeOwner::open_with_matrix_and_credentials(
        &root.join("state"),
        &origin,
        OWNER,
        Arc::new(Source {
            origin: origin.clone(),
        }),
        crate::console::server_login::credential_tests::vault(),
    )
    .await
    .unwrap();
    let console = native.test_console().clone();
    let agent = f.info["agent"]["id"].as_str().unwrap().to_owned();
    let binding = f.info["binding"]["id"].as_str().unwrap().to_owned();
    native
        .execute(Command::AssignAgentToCurrentDevice {
            agent_id: agent.clone(),
            expected_generation: f.info["agent"]["generation"].as_i64().unwrap(),
        })
        .await
        .unwrap();
    let device = console.authorized_device().await.unwrap();
    let (path, profile) = intent_location(&console, &device).unwrap();
    let path = path.with_file_name("agent-local.sqlite");
    let scope = Scope {
        agent: agent.clone(),
        binding: binding.clone(),
        room: f.info["room"].as_str().unwrap().into(),
        requester: f.info["requester"].as_str().unwrap().into(),
        thread: f.info["thread"].as_str().unwrap().into(),
    };
    let workspace = root.join("workspace");
    let home = path.parent().unwrap().join("provider-home");
    let codex_home = path.parent().unwrap().join("codex-home");
    for directory in [&workspace, &home, &codex_home] {
        std::fs::create_dir(directory).unwrap();
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let room_directory =
        hagency_agent_local::room_files::Workspace::open(path.parent().unwrap(), OWNER, &scope)
            .unwrap()
            .canonical_directory()
            .to_string_lossy()
            .into_owned();
    let script = root.join("fake-provider");
    let marker = root.join("turns");
    std::fs::write(&marker, "").unwrap();
    joint_provider(&script, &marker, &path);
    let mut l = Ledger::open_scoped(&path, OWNER, &profile).unwrap();
    l.register_binding(OWNER, &scope).unwrap();
    l.set_model_profile(
        OWNER,
        &scope,
        &ModelProfile {
            model: "test-model".into(),
            credential_ref: "keychain:fixture".into(),
            workspace_root: workspace.to_string_lossy().into(),
            reasoning_effort: String::new(),
            web_search: true,
            programming: Default::default(),
        },
    )
    .unwrap();
    for layer in [Layer::Agent, Layer::Room, Layer::Requester] {
        l.set_policy(
            OWNER,
            &scope,
            layer,
            0,
            &Policy {
                budget: Budget {
                    limit: Limit::Tokens(1000),
                    period: Period::Lifetime,
                },
                requests: RequestPolicy::Allow,
                high_risk: ToolPolicy::AllowWithRules {
                    tools: vec!["matrix.receive_file".into(), "matrix.send_file".into()],
                    directories: vec![room_directory.clone()],
                },
            },
        )
        .unwrap();
    }
    drop(l);
    let lost = std::env::var("HAGENCY_FILE_FIXTURE_LOST_QUEUE_ACK").as_deref() == Ok("1");
    reqwest::Client::new()
        .post(format!("{origin}fixture/inbound"))
        .json(&json!({"loseQueueAck":lost}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let config = || StartConfig {
        agent_id: agent.clone(),
        binding_id: binding.clone(),
        profile: Profile {
            executable: script.clone(),
            home: home.clone(),
            codex_home: codex_home.clone(),
            cwd: workspace.clone(),
            model: "test-model".into(),
            effort: "low".into(),
            shared_auth: false,
            web_search: true,
            programming: Default::default(),
        },
        mode: BudgetMode::Estimated { reservation: 10 },
        credential_ref: "keychain:fixture".into(),
        takeover: Takeover::Never,
        host_files: true,
        owner_direct: false,
        recovery_device_id: None,
        recovery_consent_id: None,
    };
    let complete = async {
        loop {
            let snapshot = f.snapshot().await;
            if snapshot["messages"].as_array().unwrap().iter().any(|m| {
                m["room_id"] == scope.room
                    && m["content"]["body"]
                        .as_str()
                        .is_some_and(|b| b.contains("JOINT_FILES_COMPLETE"))
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let outcome = host(&console, config(), &path, complete).await;
    if !matches!(outcome, Ok(()) | Err(RuntimeError::Stopped)) {
        eprintln!("original Server snapshot: {}", f.snapshot().await);
        eprintln!("joint evidence root: {}", root.display());
    }
    assert!(
        matches!(outcome, Ok(()) | Err(RuntimeError::Stopped)),
        "unexpected outcome after explicit owner stop: {outcome:?}"
    );
    let snapshot = f.snapshot().await;
    assert_eq!(snapshot["uploads"], 1);
    assert_eq!(snapshot["lostQueueAcks"], u64::from(lost));
    let messages = snapshot["messages"].as_array().unwrap();
    let files: Vec<_> = messages
        .iter()
        .filter(|m| m["content"]["msgtype"] == "m.file")
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["sender"], f.info["agent"]["puppetMxid"]);
    assert_eq!(files[0]["room_id"], scope.room);
    assert_eq!(
        files[0]["content"]["m.relates_to"]["event_id"],
        scope.thread
    );
    let mxc = files[0]["content"]["url"].as_str().unwrap();
    let id = mxc.strip_prefix("mxc://test/").unwrap();
    let bytes = reqwest::Client::new()
        .get(format!(
            "{origin}_matrix/client/v1/media/download/test/{id}"
        ))
        .bearer_auth("fixture-sdk-only")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let mut expected: Vec<u8> = (0..131_073).map(|n| n as u8).collect();
    expected.extend_from_slice(b"\nfixture output\n");
    assert_eq!(bytes.as_ref(), expected);
    let page = native
        .execute(Command::AgentHistory {
            action: HistoryAction::Files {
                agent: agent.clone(),
                cursor: None,
            },
        })
        .await
        .unwrap();
    let record = &page["files"]["records"][0]["record"];
    let local_id = record["id"].as_str().unwrap().to_owned();
    assert_eq!(record["remote"]["state"], "sent");
    let audit = record["audit"].as_str().unwrap();
    let l = Ledger::open_scoped(&path, OWNER, &profile).unwrap();
    assert_eq!(
        l.approval_phase(OWNER, audit).unwrap(),
        if lost {
            hagency_agent_local::audit::Phase::Unknown
        } else {
            hagency_agent_local::audit::Phase::Completed
        }
    );
    let usage = l
        .account(&scope, Layer::Agent, Period::Lifetime, now())
        .unwrap();
    assert_eq!(usage, (5, 0));
    let original_calls = l.snapshot(OWNER, &profile, now()).unwrap().summary().calls;
    drop(l);
    let restart_port = f.port;
    let restart_root = f.root.clone();
    drop(f);
    let f = Fixture::start(restart_root, restart_port, "restart").await;
    let refresh = native
        .execute(Command::AgentHistory {
            action: HistoryAction::RefreshFile {
                agent: agent.clone(),
                id: local_id,
            },
        })
        .await
        .unwrap();
    assert_eq!(
        refresh["file"]["record"]["remote"]["matrixEventId"],
        files[0]["event_id"]
    );
    let pause = async {
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    let stopped = host(&console, config(), &path, pause).await;
    assert!(
        matches!(stopped, Ok(()) | Err(RuntimeError::Stopped)),
        "unexpected outcome after explicit owner restart/stop: {stopped:?}"
    );
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "turn\n");
    let l = Ledger::open_scoped(&path, OWNER, &profile).unwrap();
    assert_eq!(
        l.snapshot(OWNER, &profile, now()).unwrap().summary().calls,
        original_calls
    );
    assert_eq!(
        l.account(&scope, Layer::Agent, Period::Lifetime, now())
            .unwrap(),
        usage
    );
    println!(
        "joint actual accounting: spent={}, held={}, calls={}, provider=explicit-fixture",
        usage.0, usage.1, original_calls
    );
    let after = f.snapshot().await;
    assert_eq!(after["uploads"], 0);
    assert!(
        !after["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["room_id"] == scope.room)
    );
    println!(
        "joint current Client/Server passed:131073 original bytes, original send/receipt, lost_ack={lost}, durable restart/no replay; evidence {}",
        root.display()
    );
}
