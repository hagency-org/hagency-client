use super::*;
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const OWNER: &str = "@manager:example.test";
fn original() -> ProjectSetup {
    let mut s = ProjectSetup {
        id: "mps_fixture".into(),
        project_id: "mpj_fixture".into(),
        allocation_id: "mal_original".into(),
        allocation_receipt_digest: "a".repeat(64),
        project_revision: 2,
        authority_revision: 1,
        definition_digest: String::new(),
        mode: "create".into(),
        state: "awaiting_matrix_setup".into(),
        revision: 1,
        name: "Recovery fixture".into(),
        manager_mxid: OWNER.into(),
        administrator_mxid: "@first_admin:example.test".into(),
        space_creation_command_id: "mps_fixture_space".into(),
        room_creation_command_id: "mps_fixture_room".into(),
        space_id: None,
        room_id: None,
        domain_project_id: None,
        verified_at_ms: None,
        current_binding: Some(SetupBinding {
            revision: 1,
            allocation_id: "mal_original".into(),
            allocation_receipt_digest: "a".repeat(64),
            project_revision: 2,
            authority_revision: 1,
            administrator_mxid: "@first_admin:example.test".into(),
        }),
    };
    s.definition_digest = digest(&s.definition()).unwrap();
    s
}
fn identity() -> Identity {
    Identity {
        origin: "https://example.test/".into(),
        owner: OWNER.into(),
        issuer: "https://example.test/auth/".into(),
        subject: "subject".into(),
        client: "client".into(),
        installation: "installation".into(),
        device: "stable-device".into(),
    }
}
fn begin() -> Command {
    Command::new(
        "mpj_fixture".into(),
        Mutation::Begin {
            allocation_id: Some("mal_original".into()),
            expected_project_revision: 2,
            expected_allocation_revision: 2,
            expected_authority_revision: 1,
        },
    )
    .unwrap()
}
fn initial() -> ProjectReadiness {
    ProjectReadiness {
        project_id: "mpj_fixture".into(),
        administrator: Some(ProjectAdministrator {
            mxid: "@first_admin:example.test".into(),
            revision: 1,
        }),
        setup: None,
        eligible_allocation: Some(EligibleSetupAllocation {
            id: "mal_original".into(),
            revision: 2,
            project_revision: 2,
            receipt_digest: "a".repeat(64),
        }),
        capacity_ready: true,
        matrix_ready: false,
        can_assign_administrator: false,
        can_begin: true,
        can_confirm: false,
        can_cancel: false,
        can_rebase: false,
        blocking_reason: Some("matrix_setup_required".into()),
        observed_at_ms: 100,
    }
}
#[derive(Default)]
struct State {
    readiness: Option<ProjectReadiness>,
    commands: BTreeMap<String, (String, Value)>,
    setup_attempts: Vec<Value>,
    rooms: BTreeMap<String, Value>,
    creates: Vec<Value>,
    invites: Vec<(String, String)>,
    lose_begin: bool,
    lose_create: bool,
    cancel_create: bool,
    invalidate_create: bool,
    join_invites: bool,
    force_revision_failure: bool,
    service_invited: bool,
    retired_assignments: bool,
}
impl State {
    fn readiness(&mut self) -> ProjectReadiness {
        self.readiness.get_or_insert_with(initial).clone()
    }
    fn respond(
        &mut self,
        method: &str,
        path: &str,
        body: Value,
        live: &AtomicBool,
    ) -> (u16, Value) {
        if path == "/hagency/api/v1/discovery" {
            return (
                200,
                json!({"serverName":"example.test","serviceMxid":"@_hagency_service:example.test"}),
            );
        }
        if path == "/_matrix/client/v3/joined_rooms" {
            return (
                200,
                json!({"joined_rooms":self.rooms.keys().collect::<Vec<_>>()}),
            );
        }
        if path == "/_matrix/client/v3/createRoom" {
            self.creates.push(body.clone());
            let id = format!("!room{}:example.test", self.creates.len());
            let mut state = body["initial_state"].as_array().unwrap().clone();
            for e in &mut state {
                e["sender"] = json!(OWNER);
            }
            state.extend([json!({"type":"m.room.create","state_key":"","sender":OWNER,"content":{"creator":OWNER,"type":body["creation_content"]["type"]}}),json!({"type":"m.room.power_levels","state_key":"","content":{"users":{OWNER:100},"state_default":50,"invite":0}}),json!({"type":"m.room.member","state_key":OWNER,"content":{"membership":"join"}}),json!({"type":"m.room.member","state_key":"@_hagency_service:example.test","content":{"membership":"join"}})]);
            if self.service_invited {
                for e in &mut state {
                    if e["type"] == "m.room.member" && e["state_key"] == "@_hagency_service:example.test" {
                        e["content"]["membership"] = json!("invite");
                    }
                }
            }
            self.rooms.insert(id.clone(), json!(state));
            if self.cancel_create {
                self.cancel_create = false;
                let mut r = self.readiness();
                r.setup.as_mut().unwrap().state = "cancelled".into();
                r.setup.as_mut().unwrap().revision += 1;
                r.can_confirm = false;
                r.can_cancel = false;
                r.blocking_reason = Some("project_setup_cancelled".into());
                self.readiness = Some(r);
            }
            if self.invalidate_create {
                self.invalidate_create = false;
                live.store(false, Ordering::Release);
            }
            if self.lose_create {
                self.lose_create = false;
                return (200, json!({"accepted_but_reply_lost":true}));
            }
            return (200, json!({"room_id":id}));
        }
        if let Some(rest) = path.strip_prefix("/_matrix/client/v3/rooms/") {
            let parts = rest.split('/').collect::<Vec<_>>();
            let room = percent_encoding::percent_decode_str(parts[0])
                .decode_utf8()
                .unwrap()
                .into_owned();
            if method == "GET" && parts.get(1) == Some(&"state") {
                return self
                    .rooms
                    .get(&room)
                    .cloned()
                    .map(|v| (200, v))
                    .unwrap_or((404, json!({"code":"not_found"})));
            }
            if method == "POST" && parts.get(1) == Some(&"invite") {
                let user = body["user_id"].as_str().unwrap().to_owned();
                self.invites.push((room.clone(), user.clone()));
                let list = self.rooms.get_mut(&room).unwrap().as_array_mut().unwrap();
                list.retain(|e| !(e["type"] == "m.room.member" && e["state_key"] == user));
                list.push(json!({"type":"m.room.member","state_key":user,"content":{"membership":if self.join_invites{"join"}else{"invite"}}}));
                return (200, json!({}));
            }
            if method == "PUT" && parts.len() == 4 {
                let key = percent_encoding::percent_decode_str(parts[3])
                    .decode_utf8()
                    .unwrap();
                let list = self.rooms.get_mut(&room).unwrap().as_array_mut().unwrap();
                list.retain(|e| !(e["type"] == parts[2] && e["state_key"] == *key));
                list.push(json!({"type":parts[2],"state_key":key,"content":body}));
                return (200, json!({"event_id":"$event"}));
            }
        }
        if path == "/hagency/api/v1/management/projects/mpj_fixture/setup" && method == "GET" {
            return (
                200,
                json!({"readiness":self.readiness(),"observedAtMs":100}),
            );
        }
        if path.contains("/project-setup-commands/") && path.ends_with("/recover") {
            let id = path.split('/').rev().nth(1).unwrap();
            if self.commands.contains_key(id) {
                return (
                    200,
                    json!({"recorded":true,"readiness":self.readiness(),"observedAtMs":100}),
                );
            }
            if self.retired_assignments && body["operation"] == "project.admin.assign" {
                return (409, json!({"code":"project_administrator_assignment_retired"}));
            }
            return (200, json!({"recorded":false,"observedAtMs":100}));
        }
        if path.starts_with("/hagency/api/v1/management/") {
            let id = body["commandId"].as_str().unwrap().to_owned();
            if let Some((old_path, old_body)) = self.commands.get(&id) {
                if old_path != path || old_body != &body {
                    return (409, json!({"code":"management_command_conflict"}));
                }
                return (
                    200,
                    json!({"readiness":self.readiness(),"observedAtMs":100}),
                );
            }
            if path.ends_with("/setup") {
                self.setup_attempts.push(body.clone());
            }
            if self.force_revision_failure {
                return (409, json!({"code":"management_revision_changed"}));
            }
            let mut r = self.readiness();
            if path.ends_with("/setup") {
                if body.get("allocationId").is_none() {
                    return (400, json!({"code":"project_setup_allocation_required"}));
                }
                if r.eligible_allocation.as_ref().is_none_or(|a| {
                    body["allocationId"] != a.id || body["expectedAllocationRevision"] != a.revision
                }) {
                    return (409, json!({"code":"live_reservation_required"}));
                }
                if r.setup.is_some() {
                    return (409, json!({"code":"project_setup_already_exists"}));
                }
                r.setup = Some(original());
                r.can_begin = false;
                r.can_confirm = true;
                r.can_cancel = true;
                r.can_rebase = true;
                r.blocking_reason = Some("matrix_verification_required".into());
            } else if path.ends_with("/cancel") {
                r.setup.as_mut().unwrap().state = "cancelled".into();
                r.setup.as_mut().unwrap().revision += 1;
                r.can_confirm = false;
                r.can_cancel = false;
                r.matrix_ready = false;
                r.blocking_reason = Some("project_setup_cancelled".into());
            } else if path.ends_with("/rebase") {
                let s = r.setup.as_mut().unwrap();
                s.revision += 1;
                s.state = "awaiting_matrix_setup".into();
                let b = s.current_binding.as_mut().unwrap();
                b.revision += 1;
                b.allocation_id = body["allocationId"].as_str().unwrap().into();
                b.authority_revision = body["expectedAuthorityRevision"].as_u64().unwrap();
                b.administrator_mxid = r.administrator.as_ref().unwrap().mxid.clone();
                r.can_confirm = true;
                r.can_cancel = true;
                r.matrix_ready = false;
                r.blocking_reason = Some("matrix_verification_required".into());
            } else if path.ends_with("/confirm") {
                let s = r.setup.as_mut().unwrap();
                if s.revision != body["expectedRevision"] {
                    return (409, json!({"code":"management_revision_changed"}));
                }
                s.state = "ready".into();
                s.revision += 1;
                s.space_id = Some(body["spaceId"].as_str().unwrap().into());
                s.room_id = Some(body["roomId"].as_str().unwrap().into());
                s.domain_project_id = Some("prj_actual".into());
                s.verified_at_ms = Some(100);
                r.matrix_ready = true;
                r.blocking_reason = None;
            } else {
                return (404, json!({"code":"unexpected_route"}));
            }
            self.commands.insert(id, (path.into(), body));
            self.readiness = Some(r.clone());
            if self.lose_begin && path.ends_with("/setup") {
                self.lose_begin = false;
                return (200, json!({"accepted_reply_lost":true}));
            }
            return (200, json!({"readiness":r,"observedAtMs":100}));
        }
        panic!("unexpected fake HTTP operation {method} {path}");
    }
}
#[derive(Clone)]
struct HttpIo {
    origin: String,
    live: Arc<AtomicBool>,
}
impl HttpIo {
    async fn request(
        &self,
        method: reqwest::Method,
        path: String,
        body: Option<Value>,
    ) -> Result<Value, NativeError> {
        self.check().await?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap();
        let mut q = client.request(method, format!("{}{path}", self.origin));
        if let Some(b) = body {
            q = q.json(&b);
        }
        let r = q
            .send()
            .await
            .map_err(|_| failure(503, "fixture_unavailable"))?;
        let status = r.status().as_u16();
        let v = r.json::<Value>().await.map_err(|_| invalid())?;
        self.check().await?;
        if status >= 400 {
            return Err(failure(status, v["code"].as_str().unwrap()));
        }
        Ok(v)
    }
}
impl Io for HttpIo {
    fn owner(&self) -> &str {
        OWNER
    }
    async fn check(&self) -> Result<(), NativeError> {
        if self.live.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(failure(401, "management_scope_changed"))
        }
    }
    async fn read(&self, p: &str) -> Result<ProjectReadiness, NativeError> {
        let (m, p, b) = ServerOperation(Operation::Read(p.into()))
            .request()
            .unwrap();
        let r: ReadReply = decode(self.request(m, p, b).await?)?;
        Ok(r.readiness)
    }
    async fn command(&self, c: &Command) -> Result<ProjectReadiness, NativeError> {
        let (m, p, b) = ServerOperation(Operation::Command(Box::new(c.clone())))
            .request()
            .unwrap();
        let r: ReadReply = decode(self.request(m, p, b).await?)?;
        Ok(r.readiness)
    }
    async fn recover(&self, c: &Command) -> Result<Option<ProjectReadiness>, NativeError> {
        let (m, p, b) = ServerOperation(Operation::Recover(Box::new(c.clone())))
            .request()
            .unwrap();
        let r: RecoveryReply = decode(self.request(m, p, b).await?)?;
        assert_eq!(r.recorded, r.readiness.is_some());
        Ok(r.readiness)
    }
    async fn matrix(&self, op: matrix::Operation) -> Result<Value, NativeError> {
        let (m, p, b) = op.request().unwrap();
        self.request(m, p, b).await
    }
}
struct Fixture {
    root: tempfile::TempDir,
    path: std::path::PathBuf,
    io: HttpIo,
    state: Arc<Mutex<State>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let live = Arc::new(AtomicBool::new(true));
        let server = tokio::spawn({
            let state = state.clone();
            let live = live.clone();
            async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let state = state.clone();
                    let live = live.clone();
                    tokio::spawn(async move {
                        let mut bytes = Vec::new();
                        let end = loop {
                            let mut chunk = [0; 4096];
                            let n = socket.read(&mut chunk).await.unwrap();
                            if n == 0 {
                                return;
                            }
                            bytes.extend_from_slice(&chunk[..n]);
                            if let Some(n) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                                break n + 4;
                            }
                        };
                        let header = String::from_utf8(bytes[..end].to_vec()).unwrap();
                        let line = header.lines().next().unwrap();
                        let parts = line.split_whitespace().collect::<Vec<_>>();
                        let length = header
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        while bytes.len() < end + length {
                            let mut chunk = [0; 4096];
                            let n = socket.read(&mut chunk).await.unwrap();
                            if n == 0 {
                                return;
                            }
                            bytes.extend_from_slice(&chunk[..n]);
                        }
                        let body = if length == 0 {
                            Value::Null
                        } else {
                            serde_json::from_slice(&bytes[end..end + length]).unwrap()
                        };
                        let (status, value) = state
                            .lock()
                            .unwrap()
                            .respond(parts[0], parts[1], body, &live);
                        let body = serde_json::to_vec(&value).unwrap();
                        let response = format!(
                            "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        socket.write_all(response.as_bytes()).await.unwrap();
                        socket.write_all(&body).await.unwrap();
                    });
                }
            }
        });
        let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
        let private = root.path().join("private");
        hagency_store::private::directory(&private).unwrap();
        let path = private.join("managed-project-setups-v1.json");
        Self {
            root,
            path,
            io: HttpIo { origin, live },
            state,
            server,
        }
    }
    fn driver(&self) -> Driver<HttpIo> {
        Driver {
            io: self.io.clone(),
            history: History::load(&self.path, identity()).unwrap(),
        }
    }
    async fn begun(&self) -> Driver<HttpIo> {
        let mut d = self.driver();
        d.apply(begin()).await.unwrap();
        d
    }
    fn creates(&self) -> usize {
        self.state.lock().unwrap().creates.len()
    }
}
#[test]
fn canonical_schema7_fixture_and_closed_inputs() {
    let s = original();
    assert_eq!(
        s.definition_digest,
        "f60f9aa7a8ea95eb94144d6f14a195f8e1c68dac3ae00fada92f4c5204624fde"
    );
    s.validate().unwrap();
    let p = matrix::Create {
        setup: s.clone(),
        space: false,
        parent: Some("!original:example.test".into()),
        service: "@_hagency_service:example.test".into(),
    };
    assert_eq!(
        p.marker(),
        json!({"version":1,"setupId":"mps_fixture","projectId":"mpj_fixture","definitionDigest":s.definition_digest,"commandId":"mps_fixture_room","owner":OWNER,"kind":"room","spaceId":"!original:example.test"})
    );
    let mut changed = s.clone();
    changed.current_binding.as_mut().unwrap().administrator_mxid =
        "@replacement:example.test".into();
    assert!(s.same_original(&changed));
    changed.name = "changed".into();
    assert!(!s.same_original(&changed));
    assert!(changed.validate().is_err());
    assert!(serde_json::from_value::<BeginSetup>(json!({"projectId":"mpj_fixture","allocationId":"mal_original","expectedProjectRevision":2,"expectedAllocationRevision":2,"expectedAuthorityRevision":1,"allowCreateRoom":true})).is_err());
}
#[tokio::test]
async fn creates_once_waits_real_admin_join_then_confirms_without_personal_adoption() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().join_invites = true;
    let mut d = f.begun().await;
    let p = d
        .continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    assert_eq!(p.blocker.as_deref(), Some("project_membership_pending"));
    assert!(!p.readiness.matrix_ready);
    assert_eq!(f.creates(), 2);
    let p = f
        .driver()
        .continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    assert!(p.readiness.matrix_ready);
    assert_eq!(f.creates(), 2);
    assert_eq!(f.state.lock().unwrap().invites.len(), 2);
    let raw = std::fs::read_to_string(&f.path).unwrap();
    assert!(!raw.contains("access_token"));
    assert!(!raw.contains("bearer"));
    assert!(!f.root.path().join("matrix-creations.json").exists());
}

#[tokio::test]
async fn invited_service_is_handled_by_original_typed_confirm_not_generic_binding() {
    let f = Fixture::new().await;
    {
        let mut state = f.state.lock().unwrap();
        state.join_invites = true;
        state.service_invited = true;
    }
    let mut driver = f.begun().await;
    let p = driver.continue_setup("mpj_fixture", SetupCandidates::default()).await.unwrap();
    assert_eq!(p.blocker.as_deref(), Some("project_membership_pending"));
    assert_eq!(f.creates(), 2);
    // This native boundary test proves that the original private-journaled
    // confirmation is reached. Server PG/HTTP tests perform actual service joins.
    let p = f.driver().continue_setup("mpj_fixture", SetupCandidates::default()).await.unwrap();
    assert!(p.readiness.matrix_ready);
    assert_eq!(f.creates(), 2);
    let state = f.state.lock().unwrap();
    assert_eq!(state.commands.values().filter(|(path,_)| path.ends_with("/confirm")).count(),1);
    assert!(state.commands.values().all(|(path,_)| !path.contains("/bindings") && !path.contains("/register")));
}
#[tokio::test]
async fn lost_begin_response_recovers_only_from_saved_pre_begin_intent() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().lose_begin = true;
    let mut d = f.driver();
    assert!(d.apply(begin()).await.is_err());
    assert_eq!(f.creates(), 0);
    let p = f.driver().resume_begin("mpj_fixture").await.unwrap();
    assert!(p.local_history);
    assert_eq!(p.space.unwrap().state, "not_issued");
    assert_eq!(f.state.lock().unwrap().commands.len(), 1);
    std::fs::remove_file(&f.path).unwrap();
    assert_eq!(
        f.driver()
            .resume_begin("mpj_fixture")
            .await
            .unwrap_err()
            .code,
        "local_setup_history_required"
    );
    assert!(f.driver().apply(begin()).await.is_err());
    assert_eq!(f.creates(), 0);
}
#[tokio::test]
async fn lost_create_reply_restart_recovers_marker_never_reposts() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    f.state.lock().unwrap().lose_create = true;
    let p = d
        .continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    assert_eq!(p.blocker.as_deref(), Some("space_creation_outcome_unknown"));
    assert_eq!(f.creates(), 1);
    let p = f
        .driver()
        .continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    assert_eq!(p.blocker.as_deref(), Some("project_membership_pending"));
    assert_eq!(f.creates(), 2);
    f.driver()
        .continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    assert_eq!(f.creates(), 2);
}
#[tokio::test]
async fn cancellation_during_create_retains_room_and_rebase_preserves_original_stage() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    f.state.lock().unwrap().cancel_create = true;
    assert_eq!(
        d.continue_setup("mpj_fixture", SetupCandidates::default())
            .await
            .unwrap_err()
            .code,
        "project_setup_cancelled"
    );
    assert_eq!(f.creates(), 1);
    let mut d = f.driver();
    assert_eq!(
        d.history.data.records[0]
            .space
            .as_ref()
            .unwrap()
            .room_id
            .as_deref(),
        Some("!room1:example.test")
    );
    d.apply(
        Command::new(
            "mpj_fixture".into(),
            Mutation::Rebase {
                setup_id: "mps_fixture".into(),
                expected_revision: 2,
                allocation_id: "mal_replacement".into(),
                expected_project_revision: 2,
                expected_allocation_revision: 3,
                expected_authority_revision: 1,
            },
        )
        .unwrap(),
    )
    .await
    .unwrap();
    d.continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    assert_eq!(f.creates(), 2);
    let state = f.state.lock().unwrap();
    assert_eq!(
        state.creates[1]["initial_state"][1]["content"]["spaceId"],
        "!room1:example.test"
    );
    assert_eq!(
        state.creates[1]["initial_state"][1]["content"]["definitionDigest"],
        original().definition_digest
    );
}
#[tokio::test]
async fn administrator_replacement_invites_new_admin_without_new_rooms_or_markers() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    d.continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    let old = f.state.lock().unwrap().creates.clone();
    {
        let mut s = f.state.lock().unwrap();
        let mut r = s.readiness();
        r.administrator = Some(ProjectAdministrator {
            mxid: "@replacement:example.test".into(),
            revision: 2,
        });
        r.setup.as_mut().unwrap().revision = 2;
        r.can_confirm = false;
        s.readiness = Some(r);
    }
    d.apply(
        Command::new(
            "mpj_fixture".into(),
            Mutation::Rebase {
                setup_id: "mps_fixture".into(),
                expected_revision: 2,
                allocation_id: "mal_replacement".into(),
                expected_project_revision: 2,
                expected_allocation_revision: 3,
                expected_authority_revision: 2,
            },
        )
        .unwrap(),
    )
    .await
    .unwrap();
    d.continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    let s = f.state.lock().unwrap();
    assert_eq!(s.creates, old);
    assert_eq!(
        s.invites
            .iter()
            .filter(|(_, u)| u == "@replacement:example.test")
            .count(),
        2
    );
    assert_eq!(
        d.history.data.records[0]
            .original
            .as_ref()
            .unwrap()
            .administrator_mxid,
        "@first_admin:example.test"
    );
}
#[tokio::test]
async fn account_change_after_create_keeps_unknown_and_denies_followup() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    f.state.lock().unwrap().invalidate_create = true;
    assert_eq!(
        d.continue_setup("mpj_fixture", SetupCandidates::default())
            .await
            .unwrap_err()
            .code,
        "management_scope_changed"
    );
    assert_eq!(f.creates(), 1);
    assert!(matches!(
        f.driver().history.data.records[0]
            .space
            .as_ref()
            .unwrap()
            .state,
        StageState::Submitted
    ));
    f.io.live.store(true, Ordering::Release);
    f.driver()
        .continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    assert_eq!(f.creates(), 2);
}
#[tokio::test]
async fn missing_stage_or_other_installation_never_gains_create_authority() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    let mut other = identity();
    other.installation = "other-installation".into();
    assert!(History::load(&f.path, other).is_err());
    d.history.data.records[0].space = None;
    d.history.save().unwrap();
    assert!(History::load(&f.path, identity()).is_err());
    assert_eq!(f.creates(), 0);
}
#[tokio::test]
async fn first_definite_rejection_terminal_but_later_rejection_keeps_unknown() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().force_revision_failure = true;
    let mut d = f.driver();
    assert!(d.apply(begin()).await.is_err());
    assert!(matches!(
        f.driver().history.data.records[0].commands[0].outcome,
        Outcome::Failed { .. }
    ));
    let f = Fixture::new().await;
    let mut d = f.driver();
    let n = d.history.add(begin()).unwrap();
    d.history.data.records[n].commands[0].attempted = true;
    d.history.save().unwrap();
    f.state.lock().unwrap().force_revision_failure = true;
    assert!(d.resume_begin("mpj_fixture").await.is_err());
    assert_eq!(
        f.driver().history.data.records[0].commands[0].outcome,
        Outcome::Pending
    );
}

#[tokio::test]
async fn retired_assignment_recovers_recorded_history_or_closes_confirmed_absence() {
    for recorded in [false, true] {
        let f = Fixture::new().await;
        let mut c = Command::new("mpj_fixture".into(), Mutation::Assign {
            expected_authority_revision: 0,
            administrator_mxid: "@former:example.test".into(),
        }).unwrap();
        c.attempted = true;
        let id = c.id.clone();
        let mut d = f.driver();
        d.history.add(c).unwrap();
        {
            let mut state = f.state.lock().unwrap();
            state.retired_assignments = true;
            if recorded { state.commands.insert(id.clone(), (String::new(), json!({}))); }
        }
        let result = d.retry(&id).await;
        if recorded {
            result.unwrap();
            assert_eq!(f.driver().history.data.records[0].commands[0].outcome, Outcome::Succeeded);
        } else {
            assert_eq!(result.unwrap_err().code, "project_administrator_assignment_retired");
            assert!(matches!(f.driver().history.data.records[0].commands[0].outcome, Outcome::Failed { .. }));
        }
        assert_eq!(f.creates(), 0);
        assert_eq!(f.state.lock().unwrap().commands.len(), usize::from(recorded));
    }
}
#[tokio::test]
async fn conflicting_link_and_duplicate_marker_require_explicit_repair() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    f.state.lock().unwrap().lose_create = true;
    d.continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    {
        let mut s = f.state.lock().unwrap();
        let room = s.rooms.get_mut("!room1:example.test").unwrap();
        let mark = room
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "im.hagency.management.project_setup")
            .unwrap()
            .clone();
        room.as_array_mut().unwrap().push(mark);
    }
    assert!(
        d.continue_setup(
            "mpj_fixture",
            SetupCandidates {
                space_id: Some("!room1:example.test".into()),
                room_id: None
            }
        )
        .await
        .is_err()
    );
    assert_eq!(f.creates(), 1);
}
#[tokio::test]
async fn two_serialized_drivers_load_latest_stage_and_never_duplicate_post() {
    let f = Fixture::new().await;
    f.begun().await;
    async fn run(io: HttpIo, path: std::path::PathBuf) -> Result<SetupProgress, NativeError> {
        let _guard = WORKFLOWS.lock().await;
        let mut d = Driver {
            io,
            history: History::load(&path, identity())?,
        };
        d.continue_setup("mpj_fixture", SetupCandidates::default())
            .await
    }
    let (a, b) = tokio::join!(
        run(f.io.clone(), f.path.clone()),
        run(f.io.clone(), f.path.clone())
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(f.creates(), 2);
}

#[tokio::test]
async fn existing_conflicting_link_is_not_overwritten() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    d.continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    {
        let mut s = f.state.lock().unwrap();
        let state = s
            .rooms
            .get_mut("!room1:example.test")
            .unwrap()
            .as_array_mut()
            .unwrap();
        let link = state
            .iter_mut()
            .find(|e| e["type"] == "m.space.child")
            .unwrap();
        link["content"] = json!({"via":["other.example"]});
    }
    assert_eq!(
        d.continue_setup("mpj_fixture", SetupCandidates::default())
            .await
            .unwrap_err()
            .code,
        "project_matrix_link_conflict"
    );
    assert_eq!(f.creates(), 2);
    let s = f.state.lock().unwrap();
    let link = s.rooms["!room1:example.test"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "m.space.child")
        .unwrap();
    assert_eq!(link["content"]["via"], json!(["other.example"]));
}

#[tokio::test]
async fn invitation_outcome_is_not_repeated_after_decline_or_unknown() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    d.continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    {
        let mut s = f.state.lock().unwrap();
        for room in s.rooms.values_mut() {
            for event in room.as_array_mut().unwrap() {
                if event["type"] == "m.room.member"
                    && event["state_key"] == "@first_admin:example.test"
                {
                    event["content"]["membership"] = json!("leave");
                }
            }
        }
    }
    let result = f
        .driver()
        .continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    assert_eq!(
        result.blocker.as_deref(),
        Some("administrator_invitation_requires_review")
    );
    assert_eq!(f.state.lock().unwrap().invites.len(), 2);
    assert_eq!(f.creates(), 2);
}

#[tokio::test]
async fn central_stale_confirmation_never_resets_creation_history() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().join_invites = true;
    let mut d = f.begun().await;
    d.continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    f.state.lock().unwrap().force_revision_failure = true;
    assert_eq!(
        d.continue_setup("mpj_fixture", SetupCandidates::default())
            .await
            .unwrap_err()
            .code,
        "management_revision_changed"
    );
    assert_eq!(f.creates(), 2);
    let d = f.driver();
    let record = &d.history.data.records[0];
    assert!(matches!(
        record.commands.last().unwrap().outcome,
        Outcome::Failed { .. }
    ));
    assert!(matches!(
        record.space.as_ref().unwrap().state,
        StageState::Known
    ));
    assert!(matches!(
        record.room.as_ref().unwrap().state,
        StageState::Known
    ));
    assert!(!f.state.lock().unwrap().readiness().matrix_ready);
}

#[tokio::test]
async fn deleting_all_stage_fields_after_success_does_not_reinitialize_them() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    let record = &mut d.history.data.records[0];
    record.original = None;
    record.space = None;
    record.room = None;
    d.history.save().unwrap();
    assert!(History::load(&f.path, identity()).is_err());
    assert_eq!(f.creates(), 0);
}

#[tokio::test]
async fn recovered_begin_with_confirmed_rooms_cannot_create_fresh_stages() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().lose_begin = true;
    assert!(f.driver().apply(begin()).await.is_err());
    {
        let mut state = f.state.lock().unwrap();
        let mut r = state.readiness();
        let setup = r.setup.as_mut().unwrap();
        setup.space_id = Some("!already:example.test".into());
        setup.room_id = Some("!exists:example.test".into());
        setup.domain_project_id = Some("prj_existing".into());
        state.readiness = Some(r);
    }
    assert_eq!(
        f.driver()
            .resume_begin("mpj_fixture")
            .await
            .unwrap_err()
            .code,
        "local_setup_history_required"
    );
    let d = f.driver();
    assert!(d.history.data.records[0].original.is_none());
    assert_eq!(
        d.history.data.records[0].commands[0].outcome,
        Outcome::Pending
    );
    assert_eq!(f.creates(), 0);
}

#[tokio::test]
async fn confirmed_server_ids_fence_restored_never_issued_stages() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    {
        let mut state = f.state.lock().unwrap();
        let mut r = state.readiness();
        let setup = r.setup.as_mut().unwrap();
        setup.space_id = Some("!already:example.test".into());
        setup.room_id = Some("!exists:example.test".into());
        setup.domain_project_id = Some("prj_existing".into());
        r.matrix_ready = true;
        state.readiness = Some(r);
    }
    assert_eq!(
        d.continue_setup("mpj_fixture", SetupCandidates::default())
            .await
            .unwrap_err()
            .code,
        "local_setup_history_required"
    );
    assert_eq!(f.creates(), 0);
}

#[tokio::test]
async fn confirmed_server_ids_must_match_the_original_local_rooms() {
    let f = Fixture::new().await;
    let mut d = f.begun().await;
    d.continue_setup("mpj_fixture", SetupCandidates::default())
        .await
        .unwrap();
    {
        let mut state = f.state.lock().unwrap();
        let mut r = state.readiness();
        let setup = r.setup.as_mut().unwrap();
        setup.space_id = Some("!different:example.test".into());
        setup.room_id = Some("!room2:example.test".into());
        state.readiness = Some(r);
    }
    assert_eq!(
        d.continue_setup("mpj_fixture", SetupCandidates::default())
            .await
            .unwrap_err()
            .code,
        "project_matrix_link_immutable"
    );
    assert_eq!(f.creates(), 2);
}

#[test]
fn optional_eligible_allocation_is_validated_independently_of_original_binding() {
    let mut r = initial();
    r.setup = Some(original());
    let mut wire = serde_json::to_value(&r).unwrap();
    wire.as_object_mut().unwrap().remove("eligibleAllocation");
    let old: ProjectReadiness = decode(wire.clone()).unwrap();
    assert!(old.eligible_allocation.is_none());
    old.validate("mpj_fixture").unwrap();
    wire["eligibleAllocation"] = json!({"id":"mal_replacement","revision":9,
        "projectRevision":7,"receiptDigest":"b".repeat(64)});
    let replacement: ProjectReadiness = decode(wire.clone()).unwrap();
    replacement.validate("mpj_fixture").unwrap();
    assert_eq!(replacement.setup.unwrap().project_revision, 2);
    for (field, invalid) in [
        ("id", json!("../wrong")),
        ("revision", json!(0)),
        ("projectRevision", json!(0)),
        ("receiptDigest", json!("invalid")),
    ] {
        let mut bad = wire.clone();
        bad["eligibleAllocation"][field] = invalid;
        assert!(
            decode::<ProjectReadiness>(bad)
                .unwrap()
                .validate("mpj_fixture")
                .is_err()
        );
    }
    wire["eligibleAllocation"]["revision"] = json!(JSON_SAFE_MAX_FOR_TEST + 1);
    assert!(decode::<ProjectReadiness>(wire).is_err());
}
const JSON_SAFE_MAX_FOR_TEST: u64 = 9_007_199_254_740_991;

fn historical_begin(attempted: bool) -> Command {
    serde_json::from_value(json!({"id":"original_begin","project":"mpj_fixture",
        "mutation":{"operation":"begin","expected_project_revision":2,
        "expected_allocation_revision":2,"expected_authority_revision":1},
        "attempted":attempted,"outcome":{"state":"pending"}}))
    .unwrap()
}
#[test]
fn old_begin_journal_and_wire_keep_omitted_field_while_public_input_requires_selection() {
    let old = historical_begin(true);
    let bytes = serde_json::to_vec(&old).unwrap();
    let roundtrip: Command = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(bytes, serde_json::to_vec(&roundtrip).unwrap());
    assert!(
        !serde_json::to_value(&old).unwrap()["mutation"]
            .as_object()
            .unwrap()
            .contains_key("allocation_id")
    );
    let (_, _, body) = old.route();
    assert_eq!(
        body,
        json!({"commandId":"original_begin","expectedProjectRevision":2,
        "expectedAllocationRevision":2,"expectedAuthorityRevision":1,"mode":"create"})
    );
    assert!(
        serde_json::from_value::<BeginSetup>(json!({"projectId":"mpj_fixture",
        "expectedProjectRevision":2,"expectedAllocationRevision":2,"expectedAuthorityRevision":1}))
        .is_err()
    );
    let (_, _, new_body) = begin().route();
    assert_eq!(new_body["allocationId"], "mal_original");
}
#[tokio::test]
async fn legacy_begin_without_recorded_result_never_posts_or_selects_current_capacity() {
    for attempted in [false, true] {
        let f = Fixture::new().await;
        let mut d = f.driver();
        d.history.add(historical_begin(attempted)).unwrap();
        assert_eq!(
            f.driver()
                .resume_begin("mpj_fixture")
                .await
                .unwrap_err()
                .code,
            "project_setup_allocation_required"
        );
        let d = f.driver();
        let command = &d.history.data.records[0].commands[0];
        if attempted {
            assert_eq!(command.outcome, Outcome::Pending);
        } else {
            assert!(matches!(command.outcome, Outcome::Failed { .. }));
        }
        let state = f.state.lock().unwrap();
        assert!(state.setup_attempts.is_empty());
        assert!(state.commands.is_empty());
        assert!(state.creates.is_empty());
        assert!(d.history.data.records[0].original.is_none());
    }
}
#[tokio::test]
async fn legacy_begin_recorded_result_recovers_original_custody_without_new_post() {
    let f = Fixture::new().await;
    let old = historical_begin(true);
    f.driver().history.add(old.clone()).unwrap();
    {
        let (_, path, body) = old.route();
        let mut state = f.state.lock().unwrap();
        state.commands.insert(old.id.clone(), (path, body));
        let mut r = initial();
        r.setup = Some(original());
        r.can_begin = false;
        r.can_confirm = true;
        state.readiness = Some(r);
    }
    let progress = f.driver().resume_begin("mpj_fixture").await.unwrap();
    assert!(progress.local_history);
    assert_eq!(progress.space.unwrap().state, "not_issued");
    let recovered = &f.driver().history.data.records[0].commands[0];
    assert_eq!(recovered.route(), old.route());
    assert_eq!(recovered.outcome, Outcome::Succeeded);
    assert!(f.state.lock().unwrap().setup_attempts.is_empty());
    assert_eq!(f.creates(), 0);
}
#[tokio::test]
async fn equal_revision_replacement_cannot_silently_substitute_begin_allocation() {
    let f = Fixture::new().await;
    let mut r = initial();
    r.eligible_allocation.as_mut().unwrap().id = "mal_replacement".into();
    f.state.lock().unwrap().readiness = Some(r);
    assert_eq!(
        f.driver().apply(begin()).await.unwrap_err().code,
        "live_reservation_required"
    );
    let state = f.state.lock().unwrap();
    assert_eq!(state.setup_attempts.len(), 1);
    assert_eq!(state.setup_attempts[0]["allocationId"], "mal_original");
    assert_eq!(state.setup_attempts[0]["expectedAllocationRevision"], 2);
    assert!(state.commands.is_empty());
    assert!(state.creates.is_empty());
    let d = f.driver();
    assert!(d.history.data.records[0].original.is_none());
    assert!(matches!(
        d.history.data.records[0].commands[0].outcome,
        Outcome::Failed { .. }
    ));
}
#[tokio::test]
async fn substituted_begin_reply_cannot_initialize_creation_stages() {
    let f = Fixture::new().await;
    let mut d = f.driver();
    d.history.add(begin()).unwrap();
    let mut r = initial();
    let mut s = original();
    s.allocation_id = "mal_substituted".into();
    s.definition_digest = digest(&s.definition()).unwrap();
    r.setup = Some(s);
    assert_eq!(
        d.accept(0, 0, &r).unwrap_err().code,
        "invalid_project_setup_response"
    );
    assert!(d.history.data.records[0].original.is_none());
    assert!(d.history.data.records[0].space.is_none());
    assert!(d.history.data.records[0].room.is_none());
    assert_eq!(f.creates(), 0);
}
