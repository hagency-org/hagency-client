#![allow(dead_code)]
#[path = "../../../hagency-matrix/tests/common/mod.rs"]
pub mod common;
use hagency_core::{replies::*, tasks::*};
use hagency_store::{DomainRepository, EffectOutcome, private};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Bounds fixture waits that must outlast the operation budget under load —
/// the sibling fixtures (file_service, received_files) name the same value
/// `STARTUP_WATCHDOG`. The bootstrap fixture's own capability-poll bound and
/// the approval scenario's delivery drain both use it, so the two sides can
/// never disagree about how long a loaded host may take.
pub const STARTUP_WATCHDOG: Duration = Duration::from_secs(15);

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub state_dir: PathBuf,
    pub work: PathBuf,
    pub fake: common::Fake,
    pub address: SocketAddr,
    /// Requests the generic responder has answered (`serve_until`).
    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    served: u64,
    /// What the responder does to the worker's whoami while set (ADR-183):
    /// a remote refusal or another account's identity. The harness's fault,
    /// injected at the homeserver, never in the product.
    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    pub fault: Option<Fault>,
    /// When each whoami arrived, for the backoff scenario.
    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    pub whoami_at: Vec<std::time::Instant>,
}
/// A fault the fixture's homeserver answers with (ADR-183 scenarios).
#[derive(Clone, Copy, Debug)]
pub enum Fault {
    /// Every whoami answers this HTTP status.
    Remote(u16),
    /// whoami names another account.
    Identity,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
impl Fixture {
    pub async fn new(fenced: bool) -> Self {
        Self::with_account(fenced, false).await
    }
    pub async fn with_account(fenced: bool, managed: bool) -> Self {
        Self::with_settings(fenced, managed, None).await
    }
    /// An agent whose AGENT RECORD carries per-thread worktree settings
    /// (board #78; TS backend-v2.js:2994 record fields, consumed at
    /// backend-v2.js:2057-2075). The settings ride the production admission
    /// path (`agentDefinition` on the verified request) — not a serve-level
    /// or host-level switch.
    pub async fn with_worktree_agent(
        fenced: bool,
        worktrees_dir: PathBuf,
        bootstrap: Vec<String>,
    ) -> Self {
        Self::with_settings(fenced, false, Some((worktrees_dir, bootstrap))).await
    }
    async fn with_settings(
        fenced: bool,
        managed: bool,
        workspace: Option<(PathBuf, Vec<String>)>,
    ) -> Self {
        let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
        let state_dir = root.path().join("state");
        // Historical SDK-only fixture. Never call the production Init CLI.
        private::directory(&state_dir).unwrap();
        private::write_new(
            &state_dir.join("operator.token"),
            b"fixture_operator_token_32_bytes_minimum",
        )
        .unwrap();
        let work = root.path().join("工作目录");
        private::directory(&work).unwrap();
        let work = work.canonicalize().unwrap();
        let mut db = DomainRepository::open(&state_dir).unwrap();
        db.register(&common::domain::registration()).unwrap();
        let mut account_id = None;
        let resource = if managed {
            let reserved = db.reserve_account(hagency_store::ACCOUNT_PROFILE).unwrap();
            let choice = db.materialize_account(&reserved.id).unwrap();
            fs::write(
                state_dir.join(&choice.id).join("fixture-account-marker"),
                "bootstrap-selected",
            )
            .unwrap();
            fs::write(work.join("account-probe.required"), b"required").unwrap();
            let account = db.managed_account(&choice.id).unwrap();
            let access = hagency_store::AccountEnrollmentAccess::new(
                std::time::Instant::now() + Duration::from_secs(30),
                Default::default(),
            );
            let command = access
                .prepare(
                    &account,
                    choice.revision,
                    "gpt-5.6-sol".into(),
                    Some("medium".into()),
                    Some(
                        serde_json::from_value(json!({"tokens":1000,"period":"monthly"})).unwrap(),
                    ),
                    std::time::Instant::now() + Duration::from_secs(5),
                )
                .unwrap();
            let result = db.enroll_account_resource(command).unwrap();
            // MA-S2 (ADR-053 amendment): the readiness gate consumes the bound
            // account only when its login fact (MA-S1, migration 028) is
            // observed and unexpired. A managed bootstrap over an unobserved
            // account parks instead of running. Record the fact through the
            // store's own readiness path — exactly what a real deployment
            // writes when its login child exits — never a gate bypass.
            let attempt = db.begin_account_login(choice.id.as_str(), now()).unwrap();
            db.settle_account_login(
                attempt,
                hagency_store::LoginVerdict {
                    mode: hagency_store::AccountReadinessMode::Subscription,
                    provider_state: "logged-in-subscription".into(),
                    outcome: hagency_store::LoginOutcome::Observed,
                    expires_at_ms: Some(now() + 3_600_000),
                },
                now(),
            )
            .unwrap();
            account_id = Some(choice.id);
            db.resource_configuration(&result.resource_id).unwrap()
        } else {
            let resource = common::domain::resource("pool", "seat", 1000);
            db.put_resource(&resource).unwrap();
            resource
        };
        let mut request = common::domain::request("bootstrap", "Worker", &resource, 100);
        if let Some((worktrees_dir, bootstrap)) = &workspace {
            // Board #78: the agent record carries the per-agent workspace
            // settings through the production admission path. Mutate BEFORE
            // the observation is built, so the request digest and the
            // observed content stay the same serialization.
            let mut value = serde_json::to_value(&request).unwrap();
            value["agentDefinition"]["workspaceMode"] = json!("worktree");
            value["agentDefinition"]["worktreesDir"] =
                json!(worktrees_dir.to_string_lossy().into_owned());
            value["agentDefinition"]["worktreeBootstrap"] = json!(bootstrap);
            request = serde_json::from_value(value).unwrap();
        }
        let mut observation = common::domain::observation(&request);
        observation.observed_at_ms = now();
        let proof = hagency_core::authority::verify_request(
            &common::domain::registration(),
            request,
            observation,
        )
        .unwrap();
        let e = db.admit(&proof, now()).unwrap();
        db.approve("approve", &proof, now()).unwrap();
        let effect = db.claim_effect().unwrap().unwrap();
        db.observe_effect(
            &effect.id,
            effect.fence,
            &EffectOutcome::Applied {
                receipt: "fixture provision".into(),
            },
        )
        .unwrap();
        let transport = MatrixTransportObservation {
            engagement_id: e.id.clone(),
            registration_generation: 1,
            generation: 1,
            sender_mxid: "@worker:example.test".into(),
            device_id: "DEVICE_1".into(),
        };
        db.observe_matrix_transport(&transport, now()).unwrap();
        db.observe_matrix_room(
            &MatrixRoomObservation {
                engagement_id: e.id.clone(),
                registration_generation: 1,
                transport_generation: 1,
                room_id: "!project:example.test".into(),
                generation: 1,
                privacy: RoomPrivacy::Group {},
                joined: BTreeSet::from([
                    "@worker:example.test".into(),
                    "@owner:example.test".into(),
                ]),
                invite_only: true,
                encrypted: true,
            },
            now(),
        )
        .unwrap();
        db.resolve_verified_matrix_session(
            &SessionBinding {
                id: "session".into(),
                engagement_id: e.id.clone(),
                room_id: "!project:example.test".into(),
                thread_root: Some("$task_thread".into()),
            },
            now(),
        )
        .unwrap();
        db.create_canonical_task("task", "session", "Native bootstrap task", now())
            .unwrap();
        db.register_workspace("work").unwrap();
        db.enqueue_dispatch(&DispatchInput{id:"dispatch".into(),session_id:"session".into(),task_id:Some("task".into()),resources:vec![ResourceLease{id:"work".into(),exclusive:true}],payload:json!({"instruction":"Read and heartbeat the assigned task using native MCP."})}).unwrap();
        if fenced {
            db.invalidate_matrix_transport(
                &MatrixTransportInvalidation {
                    expected: transport.clone(),
                    reason: "fixture authentic negative evidence".into(),
                },
                now(),
            )
            .unwrap();
        }
        drop(db); // No issued cap, Started restoration, SQL availability or handoff.
        let fake = common::Fake::start(true).await;
        let executable = PathBuf::from(env!("CARGO_BIN_EXE_hagency-owned-mcp-probe"))
            .canonicalize()
            .unwrap();
        let executable_sha256: String = Sha256::digest(fs::read(&executable).unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let config = json!({"profile":"codex_app_server_development_v1","managed_account":account_id,"executable":executable,"executable_sha256":executable_sha256,"workspaces":{"work":work},"file_limit":4194304,"operation_ms":10000,"response_ms":1500,
            "matrix":{"origin":fake.endpoint,"server_name":"example.test","registration_fingerprint":"a".repeat(64),"engagement_id":e.id,"registration_generation":1,"transport_generation":1,"sender_mxid":"@worker:example.test","device_id":"DEVICE_1","rooms":[{"id":"!project:example.test","generation":1,"privacy":{"kind":"group"}}]}});
        private::write_new(
            &state_dir.join("development-driver.json"),
            &serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        private::write_new(
            &state_dir.join("matrix.access_token"),
            common::TOKEN.as_bytes(),
        )
        .unwrap();
        private::write_new(&state_dir.join("matrix.sdk_key"), &[42; 32]).unwrap();
        private::write_new(
            &state_dir.join("matrix.ca.pem"),
            include_bytes!("../../../hagency-matrix/tests/fixtures/ca.pem"),
        )
        .unwrap();
        let reserve = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = reserve.local_addr().unwrap();
        drop(reserve);
        Self {
            root,
            state_dir,
            work,
            fake,
            address,
            served: 0,
            fault: None,
            whoami_at: Vec::new(),
        }
    }
    pub fn attempts(&self) -> u64 {
        let sql = rusqlite::Connection::open(self.state_dir.join("domain.sqlite3")).unwrap();
        sql.query_row("SELECT COUNT(*) FROM runner_attempts", [], |r| r.get(0))
            .unwrap()
    }
}
