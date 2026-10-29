//! Isolated real TCP/typed-transport runtime test. Provider is an explicit fake
//! subprocess; no production provider credential or Matrix room is used.
use super::super::native::{MatrixAccessToken, MatrixTokenSource, NativeError, NativeOwner};
use super::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const OWNER: &str = "@alice:test";
struct Source;
impl MatrixTokenSource for Source {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Box::pin(async {
            Ok(MatrixAccessToken {
                access_token: "fixture-sdk-only".into(),
                client_id: "sdk-client".into(),
            })
        })
    }
}
fn hash(value: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}
fn event(id: &str, epoch: i64) -> Value {
    json!({"id":id,"bindingId":"binding-a","agentId":"agent-a","eventId":format!("${id}:test"),"roomId":"!room-a:test","requesterMxid":"@bob:test","threadRoot":"$root:test","body":"isolated fixture request","state":"offered","bindingGeneration":1,"dispatchEpoch":epoch,"dispatchDeviceId":"device-a","executionId":null,"outcome":null})
}
#[derive(Default)]
struct WireState {
    epoch: i64,
    attachment_response: Option<Value>,
    incoming_attachment: Option<Value>,
    attachment_input: Option<Value>,
    file_delivery_response: Option<Value>,
    file_delivery_input: Option<Value>,
    file_history: bool,
    encrypted_publication_response: Option<Value>,
    file_lookup_deny: Option<Arc<std::sync::atomic::AtomicBool>>,
    send_mode: u8,
    sent_remote: Option<Value>,
    sent_bytes: Vec<u8>,
    sent_queues: usize,
    sent_status: usize,
    restore_scope: bool,
    binding_states: BTreeMap<String, String>,
    no_messages: bool,
    device_generation: i64,
    restart_busy_remaining: usize,
    renewal_failures: usize,
    events: Vec<Value>,
    paths: Vec<String>,
    replies: usize,
    processing: usize,
    processing_input: Option<Value>,
    first_reply: Option<Value>,
}
impl WireState {
    fn history(&self) -> Value {
        let mut entries = vec![];
        let mut witness = Sha256::new();
        witness.update(b"hagency-started-executions-v1\n");
        for d in self.events.iter().filter(|d| d["executionId"].is_string()) {
            witness.update(serde_json::to_vec(&json!([d["id"], d["executionId"]])).unwrap());
            witness.update(b"\n");
            entries.push(json!({"dispatchId":d["id"],"executionId":d["executionId"],"bindingId":d["bindingId"],"agentId":d["agentId"],"eventId":d["eventId"],"roomId":d["roomId"],"requesterMxid":d["requesterMxid"],"threadRoot":d["threadRoot"],"bindingGeneration":1,"dispatchEpoch":d["dispatchEpoch"],"dispatchDeviceId":"device-a","immutableDigest":hash(&json!([d["id"],d["bindingId"],d["agentId"],d["eventId"],d["roomId"],d["requesterMxid"],d["threadRoot"],d["body"],1]))}));
        }
        json!({"history":{"agentId":"agent-a","snapshot":{"count":entries.len(),"digest":format!("{:x}",witness.finalize())},"executions":entries,"nextCursor":null}})
    }
    fn respond(&mut self, path: &str, input: &Value, origin: &str, unknown: bool) -> (u16, Value) {
        self.paths.push(path.into());
        let until = wall_ms() + 20_000;
        let generation = self.device_generation.max(1);
        let lease = |epoch| json!({"lease":{"agentId":"agent-a","ownerUserId":"uid","deviceId":"device-a","deviceGeneration":generation,"epoch":epoch,"expiresAtMs":until}});
        match path {
            "/hagency/api/v1/discovery" => (
                200,
                json!({"product":"hagency-server","version":"0.1.0","protocolVersion":3,"capabilities":(["pasion-oauth","owner-agent-appservice-v1","global-agent-identity-v2","execution-device-v1","owner-direct-v1"].into_iter().chain((self.incoming_attachment.is_some()||self.send_mode>0).then_some(hagency_agent_local::attachments::CAPABILITY)).chain((self.send_mode>0).then_some(hagency_agent_local::file_delivery::CAPABILITY)).chain(self.file_history.then_some(hagency_agent_local::file_delivery::HISTORY_CAPABILITY)).collect::<Vec<_>>()),"homeserver":origin,"issuer":format!("{origin}auth/")}),
            ),
            "/hagency/api/v1/agents/agent-a" if self.restore_scope => (
                200,
                json!({"agent":{"id":"agent-a","ownerUserId":"uid","puppetMxid":"@agent:test","displayName":"Fixture","state":"active","generation":1,"executionDeviceId":"device-a","ownerDirectRoomId":"!room-a:test"}}),
            ),
            "/hagency/api/v1/agents/agent-a/bindings" if self.restore_scope => (
                200,
                if self.binding_states.is_empty() {
                    json!({"bindings":[{"id":"binding-a","agentId":"agent-a","projectId":null,"scopeKind":"owner_direct","roomId":"!room-a:test","state":"active","generation":1}]})
                } else {
                    json!({"bindings":self.binding_states.iter().map(|(id,state)|json!({"id":id,"agentId":"agent-a","projectId":null,"scopeKind":if id=="binding-a"{"owner_direct"}else{"project"},"roomId":format!("!room-{}:test",id.trim_start_matches("binding-")),"state":state,"generation":1})).collect::<Vec<_>>()})
                },
            ),
            "/hagency/api/v1/sessions/pasion" => (
                200,
                json!({"token":"a".repeat(64),"userId":"uid","mxid":OWNER,"validUntilMs":wall_ms()+30_000}),
            ),
            "/hagency/api/v1/identity" => (
                200,
                json!({"userId":"uid","mxid":OWNER,"subject":"sub","clientId":"sdk-client","issuer":format!("{origin}auth/"),"validUntilMs":wall_ms()+30_000}),
            ),
            "/hagency/api/v1/devices" => {
                self.device_generation += 1;
                (
                    200,
                    json!({"token":"b".repeat(64),"deviceId":"device-a","generation":self.device_generation,"validUntilMs":wall_ms()+30_000}),
                )
            }
            "/hagency/api/v1/sessions/current/renew" => {
                (200, json!({"validUntilMs":wall_ms()+30_000}))
            }
            "/hagency/api/v1/sessions/current" => (200, json!({})),
            "/hagency/api/v1/execution/leases/release" => (200, json!({"released":true})),
            "/hagency/api/v1/execution/history" => (200, self.history()),
            "/hagency/api/v1/execution/leases/acquire" => {
                assert_eq!(input["takeover"], false);
                if self.restart_busy_remaining > 0 {
                    self.restart_busy_remaining -= 1;
                    return (409, json!({"code":"agent_leased_to_another_device"}));
                }
                assert_eq!(
                    input["historySnapshot"],
                    self.history()["history"]["snapshot"]
                );
                self.epoch += 1;
                if self.events.is_empty() && !self.no_messages {
                    self.events.push(event("dispatch-a", self.epoch));
                }
                (200, lease(self.epoch))
            }
            "/hagency/api/v1/execution/leases/renew" => {
                if self.renewal_failures > 0 {
                    self.renewal_failures -= 1;
                    (503, json!({"code":"server_unavailable"}))
                } else {
                    (200, lease(self.epoch))
                }
            }
            "/hagency/api/v1/execution/events/poll" => {
                if self.incoming_attachment.is_some() {
                    assert_eq!(input["acceptFiles"], true);
                }
                assert!(
                    input["bindingId"] == "binding-a"
                        || self
                            .binding_states
                            .contains_key(input["bindingId"].as_str().unwrap())
                );
                (
                    200,
                    json!({"events":self.events.iter().filter(|d|d["state"]=="offered").take(1).cloned().collect::<Vec<_>>()}),
                )
            }
            "/hagency/api/v1/execution/events/ack" => (200, json!({"acknowledged":true})),
            "/hagency/api/v1/execution/events/start" => {
                let d = self
                    .events
                    .iter_mut()
                    .find(|d| d["id"] == input["dispatchId"])
                    .unwrap();
                d["executionId"] = input["executionId"].clone();
                d["state"] = "running".into();
                (200, json!({"execution":{"dispatch":d,"newlyStarted":true}}))
            }
            "/hagency/api/v1/execution/events/processing" => {
                let dispatch = self
                    .events
                    .iter()
                    .find(|d| d["id"] == input["dispatchId"])
                    .unwrap();
                assert_eq!(dispatch["state"], "running");
                assert_eq!(dispatch["executionId"], input["executionId"]);
                assert_eq!(input["lease"]["epoch"], self.epoch);
                self.processing += 1;
                if self.processing == 1 {
                    self.processing_input = Some(input.clone());
                    return (503, json!({"code":"fixture_processing_response_lost"}));
                }
                if self.processing_input.as_ref().unwrap()["dispatchId"] == input["dispatchId"] {
                    assert_eq!(self.processing_input.as_ref().unwrap(), input);
                }
                (
                    200,
                    json!({"processing":{"id":"processing-a","dispatchId":input["dispatchId"],"executionId":input["executionId"],"state":"pending","matrixEventId":null}}),
                )
            }
            "/hagency/api/v1/execution/files/prepare"
            | "/hagency/api/v1/execution/files/chunk"
            | "/hagency/api/v1/execution/files/queue"
            | "/hagency/api/v1/execution/files/status" => {
                if self.send_mode > 0 {
                    return self.deliver_file(path, input);
                }
                self.file_delivery_input = Some(input.clone());
                (
                    200,
                    json!({"delivery":self.file_delivery_response.clone().expect("explicit original file response")}),
                )
            }
            "/hagency/api/v1/execution/encrypted/action" => {
                assert_eq!(input["action"], "status", "history must be read only");
                assert!(input.get("payload").is_none());
                self.file_delivery_input = Some(input.clone());
                if let Some(deny) = &self.file_lookup_deny {
                    deny.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                (
                    200,
                    json!({"publication":self.encrypted_publication_response}),
                )
            }
            "/hagency/api/v1/execution/files/lookup" => {
                if let Some(deny) = &self.file_lookup_deny {
                    deny.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                self.file_delivery_input = Some(input.clone());
                (
                    200,
                    json!({"observation":self.file_delivery_response.clone().expect("explicit original file observation")}),
                )
            }
            "/hagency/api/v1/execution/files/attachments" => {
                self.attachment_input = Some(input.clone());
                (
                    200,
                    if let Some(attachment) = &self.incoming_attachment {
                        json!({"dispatchId":input["dispatchId"],"executionId":input["executionId"],"attachments":[attachment]})
                    } else if self.send_mode > 0 {
                        json!({"dispatchId":input["dispatchId"],"executionId":input["executionId"],"attachments":[]})
                    } else {
                        self.attachment_response
                            .clone()
                            .expect("fixture attachment response must be explicit")
                    },
                )
            }
            "/hagency/api/v1/execution/events/authorize-tool" => (
                200,
                json!({"dispatch":self.events.iter().find(|d|d["id"]==input["dispatchId"]).unwrap()}),
            ),
            "/hagency/api/v1/execution/events/finish" => {
                let d = self
                    .events
                    .iter_mut()
                    .find(|d| d["id"] == input["dispatchId"])
                    .unwrap();
                d["state"] = "finished".into();
                d["outcome"] = input["outcome"].clone();
                if unknown && self.events.len() == 1 {
                    self.events.push(event("dispatch-b", self.epoch));
                }
                (200, json!({"finished":true}))
            }
            "/hagency/api/v1/execution/replies" => {
                self.replies += 1;
                if self.replies == 1 {
                    self.first_reply = Some(input.clone());
                    return (503, json!({"code":"fixture_reply_response_lost"}));
                }
                assert_eq!(
                    self.first_reply.as_ref(),
                    Some(input),
                    "retry preserves exact reply and execution identity"
                );
                let reply = &input["reply"];
                let d = self
                    .events
                    .iter_mut()
                    .find(|d| d["id"] == reply["dispatchId"])
                    .unwrap();
                d["state"] = "replied".into();
                let digest = format!(
                    "{:x}",
                    Sha256::digest(
                        format!(
                            "{{\"dispatchId\":{},\"executionId\":{},\"body\":{}}}",
                            reply["dispatchId"], reply["executionId"], reply["body"]
                        )
                        .as_bytes()
                    )
                );
                (
                    200,
                    json!({"reply":{"id":"reply-a","ownerEventId":d["id"],"agentId":"agent-a","bindingId":"binding-a","ownerUserId":"uid","roomId":"!room-a:test","puppetMxid":"@agent:test","bindingGeneration":1,"dispatchEpoch":d["dispatchEpoch"],"deliveryEpoch":self.epoch,"requesterMxid":"@bob:test","threadRoot":"$root:test","body":reply["body"],"payloadDigest":digest,"matrixTxnId":format!("hagency_{:x}",Sha256::digest(d["id"].as_str().unwrap().as_bytes())),"state":"sent","matrixEventId":"$fixture-sent:test"}}),
                )
            }
            _ => (404, json!({"code":"fixture_unknown_path"})),
        }
    }
}
async fn server(
    unknown: bool,
    processing_failure: u8,
) -> (
    String,
    Arc<StdMutex<WireState>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let base = origin.clone();
    let state = Arc::new(StdMutex::new(WireState::default()));
    let records = state.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let state = records.clone();
            let origin = base.clone();
            tokio::spawn(async move {
                let mut bytes = vec![];
                let mut chunk = [0; 4096];
                let split = loop {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&bytes[..split]).into_owned();
                let length = headers
                    .lines()
                    .find_map(|s| {
                        s.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|s| s.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while bytes.len() < split + length {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let path = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let body: Value =
                    serde_json::from_slice(&bytes[split..split + length]).unwrap_or(Value::Null);
                if path.starts_with("/hagency/api/v1/execution/") {
                    assert!(
                        headers
                            .to_ascii_lowercase()
                            .contains(&format!("authorization: bearer {}", "b".repeat(64)))
                    );
                }
                let (mut code, mut value) =
                    state.lock().unwrap().respond(path, &body, &origin, unknown);
                if path.ends_with("/events/processing") {
                    if processing_failure == 1 {
                        code = 404;
                        value = json!({"code":"fixture_processing_unavailable"});
                    } else if processing_failure == 2 {
                        tokio::time::sleep(Duration::from_secs(4)).await;
                    }
                }
                let body = value.to_string();
                let _=stream.write_all(format!("HTTP/1.1 {code} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await;
            });
        }
    });
    (origin, state, task)
}
fn provider(
    script: &std::path::Path,
    marker: &std::path::Path,
    ledger: &std::path::Path,
    unknown: bool,
) {
    let source=r##"#!/usr/bin/python3
import sys,json,time,sqlite3
marker=MARKER
ledger=LEDGER
unknown=UNKNOWN
for raw in sys.stdin:
 f=json.loads(raw);m=f.get('method');ident=f.get('id');p=f.get('params',{})
 def reply(v): print(json.dumps({'id':ident,'result':v}),flush=True)
 def emit(m,p): print(json.dumps({'method':m,'params':p}),flush=True)
 if m=='initialize': reply({})
 elif m=='initialized': pass
 elif m=='account/read': reply({'requiresOpenaiAuth':True,'account':{'type':'chatgpt'}})
 elif m=='config/read':
  c={'project_doc_max_bytes':0,'skills':{'include_instructions':False,'bundled':{'enabled':False}},'include_apps_instructions':False,'include_environment_context':False,'developer_instructions':'','web_search':'live','mcp_servers':{},'features':{}}
  for n in ['shell_tool','unified_exec','code_mode_host','code_mode','hooks','plugins','multi_agent','multi_agent_v2','skill_search','skill_mcp_dependency_install','shell_snapshot','view_image','browser_use','browser_use_external','browser_use_full_cdp_access','browser_annotation_api','computer_use','in_app_browser','goals','image_generation','apps','tool_search','tool_suggest','web_search','web_search_cached','web_search_request','standalone_web_search','memory_tool']:c['features'][n]=False
  c['features']['standalone_web_search']=True
  c['features']['code_mode_host']=True
  reply({'config':c})
 elif m in ['thread/start','thread/resume']:
  reply({'thread':{'id':'thread-a','cwd':p['cwd']},'cwd':p['cwd'],'model':p['model'],'approvalPolicy':'untrusted','approvalsReviewer':'user','sandbox':{'type':'readOnly','networkAccess':False}})
 elif m=='thread/name/set': reply({})
 elif m=='turn/start':
  db=sqlite3.connect('file:'+ledger+'?mode=ro',uri=True)
  assert db.execute("select count(*) from calls where state='pending' and reserved=10").fetchone()[0]==1
  assert db.execute("select count(*) from accounts where held=10").fetchone()[0]==3
  db.close()
  with open(marker,'a') as out:out.write('turn\n')
  reply({'turn':{'id':'turn-a','status':'inProgress'}})
  time.sleep(6)
  first_unknown = unknown and len(open(marker).readlines()) == 1
  if not first_unknown:emit('thread/tokenUsage/updated',{'threadId':'thread-a','turnId':'turn-a','tokenUsage':{'total':{'inputTokens':3,'outputTokens':2,'cachedInputTokens':0,'reasoningOutputTokens':0,'totalTokens':5},'last':{'inputTokens':3,'outputTokens':2,'cachedInputTokens':0,'reasoningOutputTokens':0,'totalTokens':5}}})
  emit('item/completed',{'threadId':'thread-a','turnId':'turn-a','item':{'id':'message-a','type':'agentMessage','text':'isolated fixture answer'}})
  emit('turn/completed',{'threadId':'thread-a','turn':{'id':'turn-a','status':'invalid' if first_unknown else 'completed'}})
 else: raise Exception(m)
"##.replace("MARKER",&serde_json::to_string(marker).unwrap()).replace("LEDGER",&serde_json::to_string(ledger).unwrap()).replace("UNKNOWN",if unknown{"True"}else{"False"});
    std::fs::write(script, source).unwrap();
    std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700)).unwrap();
}
async fn host_round(
    console: Console,
    config: StartConfig,
    path: PathBuf,
    state: Arc<StdMutex<WireState>>,
    unknown: bool,
    restart: bool,
) {
    let device = console.authorized_device().await.unwrap();
    let diagnostic = config
        .profile
        .executable
        .parent()
        .unwrap()
        .join("turns-receive-stderr");
    let (initial, _binding_handle, _) = binding_work(config);
    let local_path = path.clone();
    let device_profile = profile_identity(
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let baseline_polls = state
        .lock()
        .unwrap()
        .paths
        .iter()
        .filter(|p| p.ends_with("/events/poll"))
        .count();
    let (commands, rx) = mpsc::unbounded_channel();
    let (stop, cancel) = watch::channel(false);
    let (status, _) = watch::channel(RuntimeStatus::new("agent-a", "test", None));
    let worker = tokio::spawn(run_host(
        console,
        device,
        path,
        initial,
        rx,
        cancel,
        stop.clone(),
        status,
    ));
    let ready = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let done = {
                let s = state.lock().unwrap();
                if restart {
                    s.paths
                        .iter()
                        .filter(|p| p.ends_with("/leases/acquire"))
                        .count()
                        >= 2
                        && s.paths
                            .iter()
                            .filter(|p| p.ends_with("/events/poll"))
                            .count()
                            >= baseline_polls + 2
                } else if unknown {
                    s.events.len() == 2
                        && s.events[1]["state"] == "replied"
                        && Ledger::open_scoped(&local_path, OWNER, &device_profile)
                            .unwrap()
                            .inbox_record(OWNER, "dispatch-b")
                            .is_ok_and(|record| record.state == State::Replied)
                } else {
                    s.events.first().is_some_and(|d| d["state"] == "replied")
                        && Ledger::open_scoped(&local_path, OWNER, &device_profile)
                            .unwrap()
                            .inbox_record(OWNER, "dispatch-a")
                            .is_ok_and(|r| r.state == State::Replied)
                }
            };
            if done {
                break true;
            }
            if worker.is_finished() {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    stop.send_replace(true);
    drop(commands);
    let result = tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    if !matches!(ready, Ok(true)) {
        let ledger = Ledger::open_scoped(&local_path, OWNER, &device_profile).unwrap();
        eprintln!(
            "original fixture stderr: {}",
            std::fs::read_to_string(&diagnostic)
                .unwrap_or_default()
                .chars()
                .take(8192)
                .collect::<String>()
        );
        eprintln!(
            "original failed runtime event: {:?}; audit: {:?}",
            state.lock().unwrap().events,
            ledger.approval_history(OWNER, "agent-a", None)
        );
    }
    assert!(
        matches!(ready, Ok(true)),
        "runtime ended before completion: {result:?}; binding={:?}; paths={:?}",
        _binding_handle.status.borrow(),
        state.lock().unwrap().paths
    );
    assert!(
        result.is_ok() || matches!(result, Err(RuntimeError::Stopped)),
        "Runtime result: {result:?}; paths={:?}",
        state.lock().unwrap().paths
    );
}
async fn fixture(unknown: bool, budget: u64) {
    fixture_with_processing_failure(unknown, budget, 0).await;
}
async fn fixture_with_processing_failure(unknown: bool, budget: u64, processing_failure: u8) {
    fixture_with_receive(unknown, budget, processing_failure, false).await;
}
async fn fixture_with_receive(unknown: bool, budget: u64, processing_failure: u8, receive: bool) {
    fixture_with_files(unknown, budget, processing_failure, receive, 0).await;
}
async fn fixture_with_files(
    unknown: bool,
    budget: u64,
    processing_failure: u8,
    receive: bool,
    send_mode: u8,
) {
    let sending = send_mode > 0;
    use hagency_agent_local::{
        Budget, Layer, Limit, ModelProfile, Period, Policy, RequestPolicy, ToolPolicy,
    };
    let (origin, state, server) = server(unknown, processing_failure).await;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    state.lock().unwrap().send_mode = send_mode;
    if receive {
        state.lock().unwrap().incoming_attachment=Some(serde_json::to_value(hagency_agent_local::attachments::Attachment::from_content(&json!({"msgtype":"m.file","body":"inbound.txt","url":"mxc://media.test/incoming","info":{"size":5}})).unwrap()).unwrap());
    }
    let source: Arc<dyn MatrixTokenSource> = if receive || sending {
        Arc::new(FileSource)
    } else {
        Arc::new(Source)
    };
    let native = NativeOwner::open_with_matrix(&root.join("state"), &origin, OWNER, source)
        .await
        .unwrap();
    let console = native.test_console().clone();
    let device = console.authorized_device().await.unwrap();
    let path = ledger_path(
        &root.join("state"),
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let profile_id = profile_identity(
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let directory = path.parent().unwrap();
    let home = directory.join("provider-home");
    let codex_home = directory.join("codex-home");
    let workspace = root.join("workspace");
    for d in [&home, &codex_home, &workspace] {
        std::fs::create_dir(d).unwrap();
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let script = root.join("fake-provider");
    let marker = root.join("turns");
    std::fs::write(&marker, "").unwrap();
    provider(&script, &marker, &path, unknown);
    if receive {
        receive_provider(&script);
    }
    if sending {
        send_provider(&script, send_mode >= 2, send_mode == 3);
    }
    let scope = inbox::Dispatch {
        id: "dispatch-a".into(),
        binding_id: "binding-a".into(),
        agent_id: "agent-a".into(),
        event_id: "$dispatch-a:test".into(),
        room_id: "!room-a:test".into(),
        requester_mxid: "@bob:test".into(),
        thread_root: "$root:test".into(),
        body: "isolated fixture request".into(),
        state: "offered".into(),
        binding_generation: 1,
        dispatch_epoch: Some(1),
        dispatch_device_id: Some("device-a".into()),
        execution_id: None,
        outcome: None,
    }
    .scope();
    let mut ledger = Ledger::open_scoped(&path, OWNER, &profile_id).unwrap();
    ledger.register_binding(OWNER, &scope).unwrap();
    ledger
        .set_model_profile(
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
    ledger
        .set_policy(
            OWNER,
            &scope,
            Layer::Room,
            0,
            &Policy {
                budget: Budget {
                    limit: Limit::Tokens(budget),
                    period: Period::Lifetime,
                },
                requests: RequestPolicy::Allow,
                high_risk: ToolPolicy::Deny,
            },
        )
        .unwrap();
    if receive || sending {
        let incoming_workspace =
            hagency_agent_local::room_files::Workspace::open(directory, OWNER, &scope).unwrap();
        let directory = incoming_workspace
            .canonical_directory()
            .to_string_lossy()
            .into_owned();
        if sending {
            std::fs::write(
                incoming_workspace.canonical_directory().join("result.txt"),
                b"hello",
            )
            .unwrap();
        }
        drop(incoming_workspace);
        for layer in [Layer::Agent, Layer::Room, Layer::Requester] {
            let current = ledger.policy_snapshot(&scope).unwrap()[match layer {
                Layer::Agent => 0,
                Layer::Room => 1,
                Layer::Requester => 2,
            }]
            .clone();
            ledger
                .set_policy(
                    OWNER,
                    &scope,
                    layer,
                    current.revision,
                    &Policy {
                        budget: Budget {
                            limit: Limit::Tokens(budget),
                            period: Period::Lifetime,
                        },
                        requests: RequestPolicy::Allow,
                        high_risk: ToolPolicy::AllowWithRules {
                            tools: if sending {
                                vec!["matrix.send_file".into()]
                            } else {
                                vec!["matrix.receive_file".into()]
                            },
                            directories: vec![directory.clone()],
                        },
                    },
                )
                .unwrap();
        }
    }
    drop(ledger);
    let config = || StartConfig {
        agent_id: "agent-a".into(),
        binding_id: "binding-a".into(),
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
        host_files: receive || sending,
        owner_direct: false,
        recovery_device_id: None,
        recovery_consent_id: None,
    };
    host_round(
        console.clone(),
        config(),
        path.clone(),
        state.clone(),
        unknown,
        false,
    )
    .await;
    let mut ledger = Ledger::open_scoped(&path, OWNER, &profile_id).unwrap();
    if sending {
        let audit = ledger
            .approval_history(OWNER, "agent-a", None)
            .unwrap()
            .records
            .into_iter()
            .find(|a| a.tool == "matrix.send_file")
            .unwrap();
        assert_eq!(
            audit.phase,
            if send_mode >= 2 {
                hagency_agent_local::audit::Phase::Unknown
            } else {
                hagency_agent_local::audit::Phase::Completed
            }
        );
        let wire = state.lock().unwrap();
        assert_eq!(wire.sent_bytes, b"hello");
        assert_eq!(wire.sent_queues, 1);
        assert!(wire.sent_status >= 1);
        if send_mode == 3 {
            assert!(wire.sent_remote.as_ref().unwrap()["matrixEventId"].is_null());
        } else {
            assert_eq!(
                wire.sent_remote.as_ref().unwrap()["matrixEventId"],
                "$fixtureFileSent"
            );
        }
    }
    let expected_turns = if unknown { 2 } else { usize::from(budget > 0) };
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap().lines().count(),
        expected_turns
    );
    let account = ledger
        .account(&scope, Layer::Room, Period::Lifetime, now())
        .unwrap();
    if unknown {
        let review = ledger
            .inspect_recovery(OWNER, "agent-a", "dispatch-a", &[], now())
            .unwrap();
        assert_eq!(review.evidence.stop_scope.as_deref(), Some("leader"));
        assert_eq!(review.evidence.workspace.as_deref(), workspace.to_str());
        assert_eq!(review.evidence.input, "isolated fixture request");
        assert!(!review.evidence.requires_tree);
        assert_eq!(account, (5, 0));
        assert!(ledger.outstanding_calls(OWNER).unwrap().is_empty());
        assert_eq!(
            ledger.inbox_record(OWNER, "dispatch-b").unwrap().state,
            State::Replied
        );
        let reply = state.lock().unwrap().first_reply.as_ref().unwrap()["reply"]["body"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(reply.starts_with("isolated fixture answer"));
        assert!(reply.contains("本次用量 5 Token"));
        assert!(!reply.contains("历史请求的费用尚待确认"));
        assert_eq!(
            ledger.inbox_record(OWNER, "dispatch-a").unwrap().state,
            State::Unknown
        );
    } else {
        assert_eq!(account, (if budget > 0 { 5 } else { 0 }, 0));
        let reply = state.lock().unwrap().first_reply.as_ref().unwrap()["reply"]["body"]
            .as_str()
            .unwrap()
            .to_owned();
        if budget > 0 {
            assert!(reply.starts_with("isolated fixture answer\n\n✅"));
            assert!(reply.contains("本次用量 5 Token（输入 3 / 输出 2"));
        } else {
            assert!(reply.contains("可用额度不足"));
            assert!(reply.contains("本次未调用模型"));
            assert!(!reply.contains("本轮处理已结束"));
        }
        if sending {
            assert_eq!(
                reply.contains("尚未取得 1 个文件的投递回执"),
                send_mode == 3,
                "ordinary reply must not hide an unknown file"
            );
        }
        assert!(!reply.contains("Agent 配额"));
        assert_eq!(
            ledger.inbox_record(OWNER, "dispatch-a").unwrap().state,
            State::Replied
        );
        drop(ledger);
        host_round(console, config(), path, state.clone(), false, true).await;
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            expected_turns
        );
    }
    {
        let s = state.lock().unwrap();
        assert!(s.paths.iter().any(|p| p.ends_with("/events/ack")));
        assert!(s.paths.iter().any(|p| p.ends_with("/events/start")));
        assert_eq!(
            s.processing,
            if unknown {
                3
            } else if budget > 0 {
                2
            } else {
                0
            }
        );
        if budget > 0 {
            let processing = s
                .paths
                .iter()
                .position(|p| p.ends_with("/events/processing"))
                .unwrap();
            let finish = s
                .paths
                .iter()
                .position(|p| {
                    p.ends_with(if unknown {
                        "/events/finish"
                    } else {
                        "/replies"
                    })
                })
                .unwrap();
            assert!(
                processing < finish,
                "even a fast model turn must enqueue eyes before terminal delivery"
            );
        }
        if budget > 0 {
            assert!(s.paths.iter().any(|p| p.ends_with("/leases/renew")));
        }
        assert!(
            s.paths
                .iter()
                .filter(|p| p.ends_with("/events/authorize-tool"))
                .count()
                >= if budget > 0 { 2 } else { 1 }
        );
        assert!(s.paths.iter().any(|p| p.ends_with("/leases/release")));
        if !unknown {
            assert_eq!(s.replies, 2);
        }
    }
    native.shutdown().await;
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn typed_http_runtime_replies_retries_and_restart_never_repeat_provider_turn() {
    fixture(false, 100).await;
}
#[tokio::test]
async fn typed_http_runtime_unreported_failure_releases_hold_and_next_provider_turn_succeeds() {
    fixture(true, 100).await;
}
#[tokio::test]
async fn typed_http_runtime_budget_notice_is_durable_without_provider_turn_or_restart_replay() {
    fixture(false, 0).await;
}

#[tokio::test]
async fn typed_http_processing_404_or_timeout_preserves_completed_reply_and_charge() {
    fixture_with_processing_failure(false, 100, 1).await;
    fixture_with_processing_failure(false, 100, 2).await;
}

#[tokio::test]
async fn typed_http_runtime_intent_recovery_rechecks_scope_and_explicit_stop_survives_fresh_manager()
 {
    let (origin, state, server) = server(false, 0).await;
    let temp = tempfile::tempdir().unwrap();
    let native = NativeOwner::open_with_matrix(
        &temp.path().canonicalize().unwrap().join("state"),
        &origin,
        OWNER,
        Arc::new(Source),
    )
    .await
    .unwrap();
    let console = native.test_console();
    let device = console.authorized_device().await.unwrap();
    let (path, identity) = intent_location(console, &device).unwrap();
    let mut intent = RuntimeIntent {
        agent_id: "agent-a".into(),
        binding_id: "binding-a".into(),
        device_id: "different-device".into(),
        consent_id: new_consent_id().unwrap(),
        reservation: 50,
        effort: "low".into(),
        host_files: false,
    };
    change_intent(
        &path,
        &identity,
        "agent-a",
        "binding-a",
        Some(intent.clone()),
    )
    .unwrap();
    assert!(
        native.restore_runtime_intents().await.unwrap()["recoveries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    intent.device_id = device.device_id().into();
    change_intent(&path, &identity, "agent-a", "binding-a", Some(intent)).unwrap();
    // This fixture grants a device but no current Agent/binding scope. Restoring
    // consent must therefore fail before acquiring a lease or running a model.
    let recovered = native.restore_runtime_intents().await.unwrap();
    assert_eq!(recovered["recoveries"][0]["restored"], false);
    assert_eq!(read_intents(&path, &identity).unwrap().intents.len(), 1);
    assert!(
        state
            .lock()
            .unwrap()
            .paths
            .iter()
            .all(|p| !p.ends_with("/leases/acquire") && !p.ends_with("/events/start"))
    );
    let fresh = OwnedRuntime::default();
    assert_eq!(fresh.recovery_intents(console).await.unwrap().len(), 1);
    let saved = read_intents(&path, &identity).unwrap().intents.remove(0);
    let config = StartConfig {
        agent_id: saved.agent_id.clone(),
        binding_id: saved.binding_id.clone(),
        profile: Profile {
            executable: PathBuf::from("never-spawn"),
            home: PathBuf::new(),
            codex_home: PathBuf::new(),
            cwd: PathBuf::new(),
            model: "m".into(),
            effort: saved.effort.clone(),
            shared_auth: false,
            web_search: true,
            programming: Default::default(),
        },
        mode: BudgetMode::Estimated {
            reservation: saved.reservation,
        },
        credential_ref: "none".into(),
        takeover: Takeover::Never,
        host_files: saved.host_files,
        owner_direct: false,
        recovery_device_id: Some(saved.device_id.clone()),
        recovery_consent_id: Some(saved.consent_id.clone()),
    };
    assert!(verify_recovery_intent(console, &device, &config).is_ok());
    fresh.stop(console, "agent-a", "binding-a").await.unwrap();
    assert!(matches!(
        verify_recovery_intent(console, &device, &config),
        Err(RuntimeError::Stopped)
    ));
    assert!(
        OwnedRuntime::default()
            .recovery_intents(console)
            .await
            .unwrap()
            .is_empty()
    );
    native.shutdown().await;
    server.abort();
    let _ = server.await;
}

// Run the explicit binary override in an isolated child test process. Never
// mutate the parent test suite's environment or touch an installed Codex login.
#[test]
fn typed_http_runtime_intent_positive_restore_uses_isolated_fake_provider() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let script = root.join("fake-provider");
    let marker = root.join("model-turns");
    std::fs::write(&marker, "").unwrap();
    provider(&script, &marker, &root.join("never-opened.db"), false);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "console::owned_runtime::http_tests::typed_http_runtime_intent_positive_restore_child",
            "--ignored",
            "--nocapture",
        ])
        .env("HAGENCY_CODEX_BINARY", &script)
        .env("HAGENCY_RESTORE_TEST_MARKER", &marker)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "");
}
#[tokio::test]
#[ignore = "isolated child of typed_http_runtime_intent_positive_restore_uses_isolated_fake_provider"]
async fn typed_http_runtime_intent_positive_restore_child() {
    use hagency_agent_local::ModelProfile;
    assert!(std::env::var_os("HAGENCY_RESTORE_TEST_MARKER").is_some());
    let (origin, state, server) = server(false, 0).await;
    {
        let mut wire = state.lock().unwrap();
        wire.restore_scope = true;
        wire.no_messages = true;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("state");
    let native = NativeOwner::open_with_matrix(&root, &origin, OWNER, Arc::new(Source))
        .await
        .unwrap();
    let device = native.test_console().authorized_device().await.unwrap();
    let paths = super::super::owner_provider::paths(
        &root,
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let path = ledger_path(
        &root,
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let identity = profile_identity(
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let workspace = path.parent().unwrap().join("workspace");
    hagency_store::private::directory(&workspace).unwrap();
    let mut ledger = Ledger::open_scoped(&path, OWNER, &identity).unwrap();
    ledger
        .register_binding(
            OWNER,
            &Scope {
                agent: "agent-a".into(),
                binding: "binding-a".into(),
                room: "!room-a:test".into(),
                requester: OWNER.into(),
                thread: "profile".into(),
            },
        )
        .unwrap();
    ledger
        .set_agent_model_profile(
            OWNER,
            "agent-a",
            &ModelProfile {
                model: "fixture-model".into(),
                credential_ref: paths.credential_ref,
                workspace_root: workspace.to_string_lossy().into(),
                reasoning_effort: String::new(),
                web_search: true,
                programming: Default::default(),
            },
        )
        .unwrap();
    drop(ledger);
    let input = json!({"bindingId":"binding-a","reservation":50,"effort":"low","hostFiles":false,"takeover":false});
    native
        .execute(super::super::native::Command::Local {
            agent_id: "agent-a".into(),
            action: super::super::native::LocalAction::RuntimeStart,
            input: Some(input),
            binding_id: Some("binding-a".into()),
            requester: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.lock().unwrap().epoch < 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    native.shutdown().await;
    drop(native);
    // The next grant keeps the installation/device ID but increments generation.
    // Until natural expiry, the old generation's lease is a precise 409 conflict.
    state.lock().unwrap().restart_busy_remaining = 2;
    let restarted = NativeOwner::open_with_matrix(&root, &origin, OWNER, Arc::new(Source))
        .await
        .unwrap();
    let recovery = restarted.restore_runtime_intents().await.unwrap();
    assert_eq!(recovery["recoveries"][0]["restored"], true, "{recovery}");
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.lock().unwrap().epoch < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(state.lock().unwrap().events.is_empty());
    assert_eq!(state.lock().unwrap().restart_busy_remaining, 0);
    assert_eq!(state.lock().unwrap().device_generation, 2);
    assert!(
        state
            .lock()
            .unwrap()
            .paths
            .iter()
            .all(|p| !p.ends_with("/events/start"))
    );
    // Legacy per-Room configuration is copied to preferences before global Stop
    // clears consent. The owner private Room wins over a differing project value.
    let device = restarted.test_console().authorized_device().await.unwrap();
    let (intent_path, profile_id) = intent_location(restarted.test_console(), &device).unwrap();
    let mut intents = read_intents(&intent_path, &profile_id).unwrap().intents;
    let mut direct = intents.remove(0);
    direct.reservation = 15000;
    let mut project = direct.clone();
    project.binding_id = "binding-other".into();
    project.reservation = 7000;
    change_intent(
        &intent_path,
        &profile_id,
        "agent-a",
        "binding-a",
        Some(direct),
    )
    .unwrap();
    change_intent(
        &intent_path,
        &profile_id,
        "agent-a",
        "binding-other",
        Some(project),
    )
    .unwrap();
    restarted
        .test_console()
        .0
        .owned_runtime
        .stop_agent_service(restarted.test_console(), "agent-a")
        .await
        .unwrap();
    restarted.shutdown().await;
    drop(restarted);
    let stopped = NativeOwner::open_with_matrix(&root, &origin, OWNER, Arc::new(Source))
        .await
        .unwrap();
    assert!(
        stopped.restore_runtime_intents().await.unwrap()["recoveries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let saved = stopped
        .test_console()
        .0
        .owned_runtime
        .saved_service_options(stopped.test_console(), "agent-a")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.reservation, 15000);
    // Agent-wide intent starts all active Rooms under one lease, discovers a
    // newly joined Room, preserves pause, and Stop defeats restart recovery.
    use super::super::native::{Command, LocalAction};
    state.lock().unwrap().binding_states = BTreeMap::from([
        ("binding-a".into(), "active".into()),
        ("binding-b".into(), "active".into()),
        ("binding-c".into(), "suspended".into()),
    ]);
    let epoch_before = state.lock().unwrap().epoch;
    // An initial lease conflict must recover through bounded watcher retry,
    // without an owner Stop/Start or an additional model execution.
    state.lock().unwrap().restart_busy_remaining = 1;
    stopped
        .execute(Command::Local {
            agent_id: "agent-a".into(),
            action: LocalAction::StartAgentService,
            input: Some(json!({"reservation":15000,"effort":"low"})),
            binding_id: None,
            requester: None,
        })
        .await
        .unwrap();
    async fn wait_binding(native: &NativeOwner, binding: &str, online: bool) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let status = native
                    .execute(Command::Local {
                        agent_id: "agent-a".into(),
                        action: LocalAction::AgentServiceStatus,
                        input: None,
                        binding_id: None,
                        requester: None,
                    })
                    .await
                    .unwrap();
                let found = status["runtime"]["bindings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|child| child["bindingId"] == binding);
                if online && found.is_some_and(|child| child["phase"] == "online_chat_only")
                    || !online && found.is_none_or(|child| child["phase"] == "stopped")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
    }
    wait_binding(&stopped, "binding-a", true).await;
    wait_binding(&stopped, "binding-b", true).await;
    assert_eq!(
        state.lock().unwrap().epoch,
        epoch_before + 1,
        "Rooms must share one Agent lease"
    );
    let status = stopped
        .execute(Command::Local {
            agent_id: "agent-a".into(),
            action: LocalAction::AgentServiceStatus,
            input: None,
            binding_id: None,
            requester: None,
        })
        .await
        .unwrap();
    assert_eq!(status["service"]["enabled"], true);
    assert!(
        status["runtime"]["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|child| child["bindingId"] != "binding-c" && child["hostFileTools"] == false)
    );
    state
        .lock()
        .unwrap()
        .binding_states
        .insert("binding-d".into(), "active".into());
    wait_binding(&stopped, "binding-d", true).await;
    state
        .lock()
        .unwrap()
        .binding_states
        .insert("binding-b".into(), "suspended".into());
    wait_binding(&stopped, "binding-b", false).await;
    state
        .lock()
        .unwrap()
        .binding_states
        .insert("binding-b".into(), "active".into());
    wait_binding(&stopped, "binding-b", true).await;
    // Restart recovers one owner/device-pinned Agent intent, rather than
    // independently replaying every Room snapshot that discovery published.
    stopped.shutdown().await;
    drop(stopped);
    let stopped = NativeOwner::open_with_matrix(&root, &origin, OWNER, Arc::new(Source))
        .await
        .unwrap();
    let recovery = stopped.restore_runtime_intents().await.unwrap();
    assert_eq!(recovery["recoveries"].as_array().unwrap().len(), 1);
    assert_eq!(recovery["recoveries"][0]["bindingId"], "*");
    assert_eq!(recovery["recoveries"][0]["restored"], true, "{recovery}");
    wait_binding(&stopped, "binding-a", true).await;
    wait_binding(&stopped, "binding-b", true).await;
    wait_binding(&stopped, "binding-d", true).await;
    wait_binding(&stopped, "binding-c", false).await;
    stopped
        .execute(Command::Local {
            agent_id: "agent-a".into(),
            action: LocalAction::StopAgentService,
            input: None,
            binding_id: None,
            requester: None,
        })
        .await
        .unwrap();
    let status = stopped
        .execute(Command::Local {
            agent_id: "agent-a".into(),
            action: LocalAction::AgentServiceStatus,
            input: None,
            binding_id: None,
            requester: None,
        })
        .await
        .unwrap();
    assert_eq!(status["service"]["enabled"], false);
    assert!(status["runtime"].is_null());
    let epoch_after_stop = state.lock().unwrap().epoch;
    state
        .lock()
        .unwrap()
        .binding_states
        .insert("binding-e".into(), "active".into());
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert_eq!(state.lock().unwrap().epoch, epoch_after_stop);
    assert!(
        stopped.restore_runtime_intents().await.unwrap()["recoveries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(status["service"]["reservation"], 15000);
    assert_eq!(status["service"]["effort"], "low");
    // Preferences survive a fresh host, but never represent execution consent.
    stopped.shutdown().await;
    drop(stopped);
    let stopped = NativeOwner::open_with_matrix(&root, &origin, OWNER, Arc::new(Source))
        .await
        .unwrap();
    assert!(
        stopped.restore_runtime_intents().await.unwrap()["recoveries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let status = stopped
        .execute(Command::Local {
            agent_id: "agent-a".into(),
            action: LocalAction::StartAgentService,
            input: Some(json!({})),
            binding_id: None,
            requester: None,
        })
        .await
        .unwrap();
    assert_eq!(status["service"]["reservation"], 15000);
    assert_eq!(status["service"]["effort"], "low");
    wait_binding(&stopped, "binding-a", true).await;
    let preferences_path = intent_path.with_file_name("agent-service-options.json");
    let saved_preferences = std::fs::read(&preferences_path).unwrap();
    hagency_store::private::replace(&preferences_path, b"{corrupt-preferences").unwrap();
    let result = stopped
        .execute(Command::Local {
            agent_id: "agent-a".into(),
            action: LocalAction::StopAgentService,
            input: None,
            binding_id: None,
            requester: None,
        })
        .await
        .unwrap();
    assert_eq!(result["stopped"], true);
    assert_eq!(result["warning"], "service_preferences_not_preserved");
    let status = stopped
        .execute(Command::Local {
            agent_id: "agent-a".into(),
            action: LocalAction::AgentServiceStatus,
            input: None,
            binding_id: None,
            requester: None,
        })
        .await
        .unwrap();
    assert_eq!(status["service"]["enabled"], false);
    assert_eq!(
        status["service"]["warning"],
        "service_preferences_unavailable"
    );
    assert!(status["runtime"].is_null());
    assert!(
        stopped.restore_runtime_intents().await.unwrap()["recoveries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // A preference write failure must have the same cancellation semantics.
    hagency_store::private::replace(&preferences_path, &saved_preferences).unwrap();
    stopped
        .execute(Command::Local {
            agent_id: "agent-a".into(),
            action: LocalAction::StartAgentService,
            input: Some(json!({})),
            binding_id: None,
            requester: None,
        })
        .await
        .unwrap();
    wait_binding(&stopped, "binding-a", true).await;
    std::fs::remove_file(&preferences_path).unwrap();
    let options_lock = hagency_store::private::open(
        &intent_path.with_file_name("agent-service-options.lock"),
        false,
    )
    .unwrap();
    options_lock.try_lock().unwrap();
    let result = stopped
        .execute(Command::Local {
            agent_id: "agent-a".into(),
            action: LocalAction::StopAgentService,
            input: None,
            binding_id: None,
            requester: None,
        })
        .await
        .unwrap();
    assert_eq!(result["stopped"], true);
    assert_eq!(result["warning"], "service_preferences_not_preserved");
    assert!(
        stopped
            .test_console()
            .0
            .owned_runtime
            .status(stopped.test_console(), "agent-a")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stopped.restore_runtime_intents().await.unwrap()["recoveries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(options_lock);
    stopped.shutdown().await;
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn heartbeat_retries_transport_failure_but_never_extends_unconfirmed_lease() {
    for recover in [true, false] {
        let (origin, state, server) = server(false, 0).await;
        let root = tempfile::tempdir().unwrap();
        let native = NativeOwner::open_with_matrix(
            &root.path().join("state"),
            &origin,
            OWNER,
            Arc::new(Source),
        )
        .await
        .unwrap();
        let console = native.test_console().clone();
        let device = console.authorized_device().await.unwrap();
        {
            let mut wire = state.lock().unwrap();
            wire.epoch = 1;
            wire.renewal_failures = if recover { 1 } else { 100 };
        }
        let lease = Lease {
            agent_id: "agent-a".into(),
            owner_user_id: "uid".into(),
            device_id: "device-a".into(),
            device_generation: 1,
            epoch: 1,
            expires_at_ms: wall_ms() + 6_000,
        };
        let (stop, receiver) = watch::channel(false);
        let failed = Arc::new(std::sync::atomic::AtomicU8::new(0));
        let task = tokio::spawn(heartbeat(
            console,
            device,
            lease,
            stop.clone(),
            receiver,
            failed.clone(),
        ));
        if recover {
            tokio::time::timeout(Duration::from_secs(6), async {
                loop {
                    if state
                        .lock()
                        .unwrap()
                        .paths
                        .iter()
                        .filter(|p| p.ends_with("/leases/renew"))
                        .count()
                        >= 2
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("heartbeat must retry transient failure within existing lease");
            stop.send_replace(true);
            task.await.unwrap();
            assert_eq!(failed.load(std::sync::atomic::Ordering::Acquire), 0);
        } else {
            tokio::time::timeout(Duration::from_secs(7), task)
                .await
                .expect("unconfirmed lease must expire")
                .unwrap();
            assert!(*stop.borrow());
            assert_eq!(
                failed.load(std::sync::atomic::Ordering::Acquire),
                2,
                "transport deadline remains recoverable, not permanent credential revocation"
            );
        }
        native.shutdown().await;
        server.abort();
    }
}

mod file_history;
mod joint_files;
mod recovery;
mod remote_approvals;

#[tokio::test]
async fn typed_http_attachment_response_is_private_and_pinned_to_original_execution() {
    let (origin, state, server) = server(false, 0).await;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("state");
    let native = NativeOwner::open_with_matrix(&root, &origin, OWNER, Arc::new(Source))
        .await
        .unwrap();
    let console = native.test_console();
    let attachment = hagency_agent_local::attachments::Attachment::from_content(&json!({"msgtype":"m.file","body":"file.txt","url":"mxc://media.test/opaque","info":{"size":5}})).unwrap();
    let expected =
        json!({"dispatchId":"dispatch-a","executionId":"execution-a","attachments":[attachment]});
    state.lock().unwrap().attachment_response = Some(expected.clone());
    let operation = || Op::Attachments {
        lease: LeaseRef {
            agent_id: "agent-a".into(),
            epoch: 1,
        },
        dispatch_id: "dispatch-a".into(),
        execution_id: "execution-a".into(),
    };
    let response = console.execution_api(operation()).await.unwrap();
    let Response::Attachments(response) = response else {
        panic!("wrong response")
    };
    assert_eq!(response.attachments, vec![attachment]);
    assert!(!format!("{response:?}").contains("mxc://"));
    assert_eq!(
        state.lock().unwrap().attachment_input,
        Some(
            json!({"lease":{"agentId":"agent-a","epoch":1},"dispatchId":"dispatch-a","executionId":"execution-a"})
        )
    );
    let mut mismatched = expected.clone();
    mismatched["executionId"] = json!("foreign-execution");
    state.lock().unwrap().attachment_response = Some(mismatched);
    let Err(error) = console.execution_api(operation()).await else {
        panic!("foreign response accepted")
    };
    assert_eq!(error.code, "execution_scope_mismatch");
    let mut mutated = expected;
    mutated["attachments"][0]["source"]["url"] = json!("mxc://media.test/replaced");
    state.lock().unwrap().attachment_response = Some(mutated);
    let Err(error) = console.execution_api(operation()).await else {
        panic!("mutated response accepted")
    };
    assert_eq!(error.code, "invalid_execution_response");
    server.abort();
}

struct FileSource;
impl MatrixTokenSource for FileSource {
    fn supports_agent_file_receive(&self) -> bool {
        true
    }
    fn fetch_agent_attachment(
        &self,
        attachment: super::super::native::AgentAttachment,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u8>, NativeError>> + Send + '_>,
    > {
        Box::pin(async move {
            attachment.validate().unwrap();
            assert_eq!(attachment.source["url"], "mxc://media.test/incoming");
            Ok(b"hello".to_vec())
        })
    }
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Source.access_token()
    }
}
fn receive_provider(path: &std::path::Path) {
    let source = std::fs::read_to_string(path).unwrap();
    let start = source.find("  time.sleep(6)").unwrap();
    let end = source[start..].find(" else: raise Exception(m)").unwrap() + start;
    let source = format!(
        "{}{}{}",
        &source[..start],
        "  emit('item/tool/call',{'threadId':'thread-a','turnId':'turn-a','callId':'list-incoming','tool':'list_received_files','arguments':{}},'host-list')\n",
        &source[end..]
    );
    let branch = r#" if m is None:
  assert f['result']['success']
  result=json.loads(f['result']['contentItems'][0]['text'])
  assert 'mxc://' not in str(result)
  if f['id']=='host-list':
   assert result['files'][0]['event_id']=='$dispatch-a:test'
   emit('item/tool/call',{'threadId':'thread-a','turnId':'turn-a','callId':'receive-incoming','tool':'receive_file','arguments':{'event_id':'$dispatch-a:test'}},'host-receive')
  else:
   assert f['id']=='host-receive'
   assert result['bytes']==5
   assert open(result['path'],'rb').read()==b'hello'
   time.sleep(6)
   emit('thread/tokenUsage/updated',{'threadId':'thread-a','turnId':'turn-a','tokenUsage':{'total':{'inputTokens':3,'outputTokens':2,'cachedInputTokens':0,'reasoningOutputTokens':0,'totalTokens':5},'last':{'inputTokens':3,'outputTokens':2,'cachedInputTokens':0,'reasoningOutputTokens':0,'totalTokens':5}}})
   emit('item/completed',{'threadId':'thread-a','turnId':'turn-a','item':{'id':'message-a','type':'agentMessage','text':'isolated fixture answer'}})
   emit('turn/completed',{'threadId':'thread-a','turn':{'id':'turn-a','status':'completed'}})
  continue
"#;
    let source=source.replace(" def emit(m,p): print(json.dumps({'method':m,'params':p}),flush=True)"," def emit(m,p,i=None): print(json.dumps(dict({'method':m,'params':p},**({'id':i} if i is not None else {}))),flush=True)");
    let source=source.replace(" elif m in ['thread/start','thread/resume']:\n", " elif m in ['thread/start','thread/resume']:\n  assert {'receive_file','list_received_files'} <= {t['name'] for t in p['dynamicTools']}\n  assert 'mxc://' not in str(p)\n");
    let source = source.replace(
        " if m=='initialize':",
        &format!("{branch} if m=='initialize':"),
    );
    let source=source.replace("for raw in sys.stdin:","sys.stderr=open(marker+'-receive-stderr','a')\nfor raw in sys.stdin:\n with open(marker+'-receive-frames','a') as trace:trace.write(raw)");
    std::fs::write(path, source).unwrap();
}
#[tokio::test]
async fn typed_http_current_runtime_receives_original_file_into_private_workspace() {
    if std::env::var_os("HAGENCY_RECEIVE_TEST_DIAGNOSTICS").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_test_writer()
            .try_init();
    }
    fixture_with_receive(false, 100, 0, true).await;
}

#[tokio::test]
async fn typed_http_file_delivery_pins_manifest_and_preserves_unknown_without_sending_again() {
    use super::super::device_execution::FileExpectation;
    use hagency_agent_local::file_delivery::{Manifest, Target};
    let (origin, state, server) = server(false, 0).await;
    let temp = tempfile::tempdir().unwrap();
    let native = NativeOwner::open_with_matrix(
        &temp.path().canonicalize().unwrap().join("state"),
        &origin,
        OWNER,
        Arc::new(Source),
    )
    .await
    .unwrap();
    let console = native.test_console();
    let file = Manifest {
        filename: "report.bin".into(),
        caption: Some("Exact caption".into()),
        bytes: 5,
        sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
    };
    let expected = FileExpectation {
        target: Target {
            dispatch_id: "dispatch-a".into(),
            execution_id: "execution-a".into(),
            file_id: "file-a".into(),
        },
        request_id: "call-a".into(),
        file: file.clone(),
    };
    let lease = LeaseRef {
        agent_id: "agent-a".into(),
        epoch: 1,
    };
    let mut delivery = json!({"id":"file-a","dispatchId":"dispatch-a","executionId":"execution-a","requestId":"call-a","createdAtMs":1,"manifest":file,"manifestDigest":file.digest().unwrap(),"receivedBytes":0,"state":"staging","uploadAccepted":false,"matrixEventId":null});
    state.lock().unwrap().file_delivery_response = Some(delivery.clone());
    assert!(matches!(
        console
            .execution_api(Op::PrepareFile {
                lease: lease.clone(),
                dispatch_id: "dispatch-a".into(),
                execution_id: "execution-a".into(),
                request_id: "call-a".into(),
                file: file.clone()
            })
            .await
            .unwrap(),
        Response::FileDelivery(_)
    ));
    assert_eq!(
        state.lock().unwrap().file_delivery_input,
        Some(
            json!({"lease":lease,"dispatchId":"dispatch-a","executionId":"execution-a","requestId":"call-a","file":file})
        )
    );
    delivery["receivedBytes"] = 5.into();
    state.lock().unwrap().file_delivery_response = Some(delivery.clone());
    console
        .execution_api(Op::StageFileChunk {
            lease: lease.clone(),
            expected: expected.clone(),
            offset: 0,
            bytes: b"hello".to_vec(),
        })
        .await
        .unwrap();
    assert_eq!(
        state.lock().unwrap().file_delivery_input,
        Some(json!({"lease":lease,"target":expected.target,"offset":0,"data":"aGVsbG8"}))
    );
    delivery["state"] = "queued".into();
    state.lock().unwrap().file_delivery_response = Some(delivery.clone());
    console
        .execution_api(Op::QueueFile {
            lease: lease.clone(),
            expected: expected.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        state.lock().unwrap().file_delivery_input,
        Some(json!({"lease":lease,"target":expected.target}))
    );
    delivery["state"] = "unknown".into();
    delivery["uploadAccepted"] = true.into();
    state.lock().unwrap().file_delivery_response = Some(delivery.clone());
    let Response::FileDelivery(receipt) = console
        .execution_api(Op::FileDelivery {
            expected: expected.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("wrong response")
    };
    assert_eq!(receipt.state, "unknown");
    assert!(receipt.matrix_event_id.is_none());
    assert_eq!(
        state.lock().unwrap().file_delivery_input,
        Some(json!(expected.target))
    );
    for key in ["id", "requestId", "executionId", "manifestDigest"] {
        let mut changed = delivery.clone();
        changed[key] = "foreign".into();
        state.lock().unwrap().file_delivery_response = Some(changed);
        assert!(
            console
                .execution_api(Op::FileDelivery {
                    expected: expected.clone()
                })
                .await
                .is_err()
        );
    }
    let mut private = delivery.clone();
    private["mxcUrl"] = "mxc://private.test/blob".into();
    state.lock().unwrap().file_delivery_response = Some(private.clone());
    assert!(
        console
            .execution_api(Op::FileDelivery {
                expected: expected.clone()
            })
            .await
            .is_err()
    );
    let query = hagency_agent_local::file_delivery::FileLookup {
        dispatch_id: expected.target.dispatch_id.clone(),
        execution_id: expected.target.execution_id.clone(),
        request_id: expected.request_id.clone(),
    };
    for (value, valid) in [
        (
            json!({"request":query,"delivery":null,"preparationClosed":false}),
            true,
        ),
        (
            json!({"request":query,"delivery":null,"preparationClosed":true}),
            true,
        ),
        (
            json!({"request":query,"delivery":delivery,"preparationClosed":true}),
            true,
        ),
        (
            json!({"request":{"dispatchId":"foreign","executionId":query.execution_id,"requestId":query.request_id},"delivery":null,"preparationClosed":true}),
            false,
        ),
        (
            json!({"request":query,"delivery":private,"preparationClosed":true}),
            false,
        ),
        (
            json!({"request":query,"delivery":null,"preparationClosed":"true"}),
            false,
        ),
    ] {
        state.lock().unwrap().file_delivery_response = Some(value.clone());
        let observed = console
            .execution_api(Op::LookupFile {
                request: query.clone(),
                file: file.clone(),
            })
            .await;
        assert_eq!(observed.is_ok(), valid);
        if let Ok(Response::FileObservation(observation)) = observed {
            assert_eq!(serde_json::to_value(observation).unwrap(), value);
        }
        assert_eq!(
            state.lock().unwrap().file_delivery_input,
            Some(json!(query))
        );
    }
    // Modern event IDs are opaque; a colon is not required for the actual receipt.
    delivery["state"] = "sent".into();
    delivery["matrixEventId"] = "$actualOpaqueEventId".into();
    state.lock().unwrap().file_delivery_response = Some(delivery);
    let Response::FileDelivery(sent) = console
        .execution_api(Op::FileDelivery { expected })
        .await
        .unwrap()
    else {
        panic!("wrong response")
    };
    assert_eq!(
        sent.matrix_event_id.as_deref(),
        Some("$actualOpaqueEventId")
    );
    {
        let wire = state.lock().unwrap();
        for suffix in ["prepare", "chunk", "queue"] {
            assert_eq!(
                wire.paths
                    .iter()
                    .filter(|p| p.ends_with(&format!("/files/{suffix}")))
                    .count(),
                1,
                "status cannot resend"
            );
        }
    }
    native.shutdown().await;
    server.abort();
    let _ = server.await;
}

impl WireState {
    fn deliver_file(&mut self, path: &str, input: &Value) -> (u16, Value) {
        use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
        if path.ends_with("/prepare") {
            assert!(self.sent_remote.is_none());
            let file: hagency_agent_local::file_delivery::Manifest =
                serde_json::from_value(input["file"].clone()).unwrap();
            file.validate().unwrap();
            assert_eq!(file.filename, "report.txt");
            assert_eq!(file.caption.as_deref(), Some("Exact original"));
            self.sent_remote = Some(
                json!({"id":"fil_fixture","dispatchId":input["dispatchId"],"executionId":input["executionId"],"requestId":input["requestId"],"createdAtMs":1,"manifest":file,"manifestDigest":file.digest().unwrap(),"receivedBytes":0,"state":"staging","uploadAccepted":false,"matrixEventId":null}),
            );
        } else {
            let r = self.sent_remote.as_mut().unwrap();
            let target = if path.ends_with("/status") {
                input
            } else {
                &input["target"]
            };
            assert_eq!(
                target,
                &json!({"dispatchId":r["dispatchId"],"executionId":r["executionId"],"fileId":r["id"]})
            );
            if path.ends_with("/chunk") {
                assert_eq!(
                    input["offset"].as_u64().unwrap(),
                    self.sent_bytes.len() as u64
                );
                self.sent_bytes.extend(
                    STANDARD_NO_PAD
                        .decode(input["data"].as_str().unwrap())
                        .unwrap(),
                );
                r["receivedBytes"] = self.sent_bytes.len().into();
            } else if path.ends_with("/queue") {
                self.sent_queues += 1;
                assert_eq!(self.sent_queues, 1);
                assert_eq!(self.sent_bytes, b"hello");
                assert_eq!(
                    r["manifest"]["sha256"],
                    format!("{:x}", Sha256::digest(&self.sent_bytes))
                );
                r["state"] = if self.send_mode >= 2 {
                    "unknown"
                } else {
                    "queued"
                }
                .into();
                if self.send_mode >= 2 {
                    return (503, json!({"code":"fixture_queue_ack_lost"}));
                }
            } else {
                assert!(path.ends_with("/status"));
                assert!(input.get("lease").is_none());
                self.sent_status += 1;
                if self.send_mode == 3 {
                    r["state"] = "unknown".into();
                } else {
                    r["state"] = "sent".into();
                    r["uploadAccepted"] = true.into();
                    r["matrixEventId"] = "$fixtureFileSent".into();
                }
            }
        }
        (200, json!({"delivery":self.sent_remote.as_ref().unwrap()}))
    }
}
fn send_provider(path: &std::path::Path, lost: bool, unconfirmed: bool) {
    let source = std::fs::read_to_string(path).unwrap();
    let start = source.find("  time.sleep(6)").unwrap();
    let end = source[start..].find(" else: raise Exception(m)").unwrap() + start;
    let source = format!(
        "{}{}{}",
        &source[..start],
        "  emit('item/tool/call',{'threadId':'thread-a','turnId':'turn-a','callId':'send-original','tool':'send_file','arguments':{'path':'result.txt','filename':'report.txt','caption':'Exact original'}},'host-send')\n",
        &source[end..]
    );
    let branch=r#" if m is None:
  assert f['result']['success'],str(f)
  result=json.loads(f['result']['contentItems'][0]['text'])
  assert 'mxc://' not in str(result)
  if f['id']=='host-send':
   assert result['state']==EXPECTED_STATE,str(result)
   emit('item/tool/call',{'threadId':'thread-a','turnId':'turn-a','callId':'query-original','tool':'get_file_delivery','arguments':{'delivery_id':result['delivery_id']}},'host-status')
  else:
   assert f['id']=='host-status'
   assert (result['state']=='unknown' and result['matrix_event_id'] is None) if UNCONFIRMED else (result['state']=='sent' and result['matrix_event_id']=='$fixtureFileSent'),str(result)
   time.sleep(6)
   emit('thread/tokenUsage/updated',{'threadId':'thread-a','turnId':'turn-a','tokenUsage':{'total':{'inputTokens':3,'outputTokens':2,'cachedInputTokens':0,'reasoningOutputTokens':0,'totalTokens':5},'last':{'inputTokens':3,'outputTokens':2,'cachedInputTokens':0,'reasoningOutputTokens':0,'totalTokens':5}}})
   emit('item/completed',{'threadId':'thread-a','turnId':'turn-a','item':{'id':'message-a','type':'agentMessage','text':'isolated fixture answer'}})
   emit('turn/completed',{'threadId':'thread-a','turn':{'id':'turn-a','status':'completed'}})
  continue
"#.replace("EXPECTED_STATE",if lost{"'unknown'"}else{"'sent'"}).replace("UNCONFIRMED",if unconfirmed{"True"}else{"False"});
    let source=source.replace(" def emit(m,p): print(json.dumps({'method':m,'params':p}),flush=True)"," def emit(m,p,i=None): print(json.dumps(dict({'method':m,'params':p},**({'id':i} if i is not None else {}))),flush=True)");
    let source=source.replace(" elif m in ['thread/start','thread/resume']:\n"," elif m in ['thread/start','thread/resume']:\n  assert {'send_file','get_file_delivery'} <= {t['name'] for t in p['dynamicTools']}\n  assert 'mxc://' not in str(p)\n");
    let source = source.replace(
        " if m=='initialize':",
        &format!("{branch} if m=='initialize':"),
    );
    let source=source.replace("for raw in sys.stdin:","sys.stderr=open(marker+'-receive-stderr','a')\nfor raw in sys.stdin:\n with open(marker+'-receive-frames','a') as trace:trace.write(raw)");
    std::fs::write(path, source).unwrap();
}
#[tokio::test]
async fn typed_http_current_runtime_sends_frozen_file_and_queries_original_lost_ack() {
    fixture_with_files(false, 100, 0, false, 1).await;
    fixture_with_files(false, 100, 0, false, 2).await;
    fixture_with_files(false, 100, 0, false, 3).await;
}

#[test]
fn original_file_delivery_notice_cannot_be_hidden_by_model_text_or_large_utf8_reply() {
    use hagency_agent_local::sent_files::DeliverySummary;
    let unknown = DeliverySummary {
        not_prepared: 0,
        sent: 0,
        cancelled: 1,
        unconfirmed: 2,
    };
    let text = "已全部发送".repeat(3584);
    let reply = with_file_delivery_notice(text, &unknown);
    assert!(reply.len() <= 65_536);
    assert!(reply.contains("本轮结束时尚未取得 2 个文件的投递回执"));
    assert!(reply.contains("1 个文件投递已取消"));
    assert_eq!(
        with_file_delivery_notice(
            "original".into(),
            &DeliverySummary {
                sent: 1,
                cancelled: 0,
                unconfirmed: 0,
                not_prepared: 0
            }
        ),
        "original"
    );
}

#[tokio::test]
async fn native_pending_approvals_pages_all_local_scopes_without_provider_or_new_leases() {
    use super::super::native::Command;
    let (origin, wire, server) = server(false, 0).await;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("state");
    let native = NativeOwner::open_with_matrix(&root, &origin, OWNER, Arc::new(Source))
        .await
        .unwrap();
    let console = native.test_console();
    let device = console.authorized_device().await.unwrap();
    let empty = native
        .execute(Command::PendingApprovals { cursor: None })
        .await
        .unwrap();
    assert_eq!(empty["total"], 0);
    assert!(empty["approvals"].as_array().unwrap().is_empty());
    let mut decisions = Vec::new();
    let mut entries = console.0.owned_runtime.entries.lock().await;
    for agent in ["agent-a", "agent-b"] {
        let (_status_tx, status) = watch::channel(RuntimeStatus::new(agent, "online", None));
        let (cancel, mut stopped) = watch::channel(false);
        let mut approvals = BTreeMap::new();
        for index in 0..42 {
            let binding = format!("binding-{}", index % 2);
            let proposal = hagency_agent_local::ToolProposal {
                scope: hagency_agent_local::Scope {
                    agent: agent.into(),
                    binding,
                    room: format!("!room-{}:test", index % 2),
                    requester: OWNER.into(),
                    thread: "$root:test".into(),
                },
                dispatch: format!("dispatch-{index}"),
                tool: "model.request".into(),
                arguments: json!({"neverDisclose":"private exact arguments"}),
                canonical_directory: "/private/workspace".into(),
                risk: "model_request".into(),
                policy_revision: [1, 1, 1],
                expires: if index == 40 { now() - 1 } else { now() + 300 },
            };
            let (pending, decision) = codex::pending_approval(proposal.clone());
            if index == 41 {
                drop(decision);
            } else {
                decisions.push(decision);
            }
            let id = format!("proposal-{index:03}");
            approvals.insert(
                id.clone(),
                ApprovalSlot {
                    view: ApprovalView {
                        proposal_id: id,
                        args_digest: "a".repeat(64),
                        proposal,
                    },
                    pending,
                    device: DevicePin::from(&device),
                    gate: None,
                    turn_cancel: watch::channel(false).1,
                    remote: None,
                },
            );
        }
        let bindings = (0..2)
            .map(|index| {
                (
                    format!("binding-{index}"),
                    BindingHandle {
                        cancel: watch::channel(false).0,
                        status: status.clone(),
                    },
                )
            })
            .collect();
        entries.insert(
            identity(&device, agent),
            Entry {
                commands: mpsc::unbounded_channel().0,
                bindings,
                cancel,
                status,
                task: tokio::spawn(async move { canceled(&mut stopped).await }),
                device: DevicePin::from(&device),
                approvals,
            },
        );
    }
    drop(entries);
    let mut cursor = None;
    let mut all = BTreeSet::new();
    let mut first_cursor = None;
    loop {
        let page = native
            .execute(Command::PendingApprovals { cursor })
            .await
            .unwrap();
        assert_eq!(page["total"], 80);
        let rows = page["approvals"].as_array().unwrap();
        assert!(rows.len() <= 32);
        for row in rows {
            assert!(all.insert((
                row["agentId"].as_str().unwrap().to_owned(),
                row["proposalId"].as_str().unwrap().to_owned()
            )));
            assert!(row.get("proposal").is_none() && row.get("arguments").is_none());
            assert!(!row.to_string().contains("private exact arguments"));
        }
        cursor = page["nextCursor"].as_str().map(str::to_owned);
        if first_cursor.is_none() {
            first_cursor = cursor.clone();
        }
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(all.len(), 80);
    let path = ledger_path(
        &root,
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let profile = profile_identity(
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let ledger = Ledger::open_scoped(&path, OWNER, &profile).unwrap();
    assert!(ledger.outstanding_calls(OWNER).unwrap().is_empty());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM calls", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    {
        let wire = wire.lock().unwrap();
        assert_eq!(wire.processing, 0);
        assert_eq!(wire.replies, 0);
        assert!(
            wire.paths
                .iter()
                .all(|p| !p.ends_with("/leases/acquire") && !p.ends_with("/events/start"))
        );
    }
    native.retire();
    assert!(
        native
            .execute(Command::PendingApprovals {
                cursor: first_cursor
            })
            .await
            .is_err()
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), native.shutdown_checked())
        .await
        .expect("discovery fixture shutdown must join the original runtime entries")
        .expect("discovery fixture shutdown must be confirmed");
    drop(decisions);
    server.abort();
}
