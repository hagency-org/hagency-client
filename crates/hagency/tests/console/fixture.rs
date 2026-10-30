#![allow(dead_code)] // Shared by HTTP, process and real-browser selectors.
use hagency::{App, console::Console};
#[path = "credential_fixture.rs"]
mod credential_fixture;
pub use credential_fixture::CredentialFixture;
use hagency_core::tasks::*;
use hagency_metering::{Framework, observation::UsageObservation};
use hagency_store::*;
use salvo::prelude::*;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
// The O2 proof loads the store's common fixture through the matrix fixture
// (crates/hagency-matrix/tests/common); re-export it here so both surfaces
// share one module instance instead of loading the same file twice.
pub use super::real_agent::matrix_common::domain as common;
pub const TOKEN: &str = "fixture_operator_token_32_bytes_minimum";
pub const BASE: &str = "http://127.0.0.1:13300";
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub fn native_resource(preset: &str) -> hagency_core::project::Resource {
    serde_json::from_value(json!({"presetId":preset,"seatId":"private_resource_account","framework":"codex","model":"gpt-5.6-sol","reasoning":"medium","ceiling":{"tokens":5000,"period":"monthly"}})).unwrap()
}
pub fn seed(state: &Path) -> (DomainRepository, String) {
    let mut db = DomainRepository::open(state).unwrap();
    db.register(&common::registration()).unwrap();
    let pool = common::resource("private_usage_pool", "private_usage_seat", 1000);
    db.put_resource(&pool).unwrap();
    let proof = common::proof(&common::request("usage_request", "UsageWorker", &pool, 100));
    let engagement = db.admit(&proof, 1000).unwrap().id;
    db.approve("approve", &proof, 1000).unwrap();
    let effect = db.claim_effect().unwrap().unwrap();
    db.observe_effect(
        &effect.id,
        effect.fence,
        &EffectOutcome::Applied {
            receipt: "synthetic canonical fixture".into(),
        },
    )
    .unwrap();
    db.register_session(&SessionBinding {
        id: "private_session".into(),
        engagement_id: engagement.clone(),
        room_id: "!project:example.test".into(),
        thread_root: Some("$private_thread".into()),
    })
    .unwrap();
    db.create_canonical_task("private_task", "private_session", "Usage", 1001)
        .unwrap();
    db.register_workspace("private_workspace").unwrap();
    db.enqueue_dispatch(&DispatchInput {
        id: "private_dispatch".into(),
        session_id: "private_session".into(),
        task_id: Some("private_task".into()),
        resources: vec![ResourceLease {
            id: "private_workspace".into(),
            exclusive: true,
        }],
        payload: json!({}),
    })
    .unwrap();
    let cap = db
        .claim_dispatch("private_runner", 1002, 60000, 120000, 128)
        .unwrap()
        .unwrap();
    let scope = db.owned_dispatch_scope(&cap, 1003).unwrap();
    let started = db
        .start_owned_dispatch(&cap, scope.fingerprint(), 1004)
        .unwrap();
    let source = db.bind_usage_source(&cap, &started, 1005).unwrap();
    for (index, text) in [snapshot(7, 2, 3), String::new(), snapshot(4, 1, 1)]
        .iter()
        .enumerate()
    {
        let observation = UsageObservation::parse(Framework::Codex, text).unwrap();
        db.record_usage_observation(
            &source,
            &format!("observation_{index}"),
            &observation,
            now() - 10 + index as u64,
        )
        .unwrap();
    }
    // The console consumer slice (63ef17cf): one open ceiling-overrun alert for the console consumer, on
    // its OWN pool and seat so every other fixture consumer is unaffected
    // (new_engagement still admits 100 against private_usage_pool's 1000).
    // The retained flow: a commitment admissible when made (100 of a
    // generous ceiling), then the ceiling lowered under it (50), swept once
    // before the store starts. Diagnostic only — no usage source, no session,
    // no dispatch state. This engagement also carries the E4 project name:
    // 260 astral characters, which the verifier truncates to 255 Unicode
    // SCALAR values (authority.rs:286) = 510 UTF-16 units — a name exceeding
    // the old client's UTF-16 bound but never the server's scalar one.
    let alert_pool = common::resource(
        "private_alert_pool",
        "private_alert_seat",
        9_000_000_000_000_000,
    );
    db.put_resource(&alert_pool).unwrap();
    let alert_request = common::request("alert_request", "AlertWorker", &alert_pool, 100);
    let mut alert_observation = common::observation(&alert_request);
    alert_observation.project.name = Some("𝕏".repeat(260));
    let alert_proof = hagency_core::authority::verify_request(
        &common::registration(),
        alert_request,
        alert_observation,
    )
    .unwrap();
    db.admit(&alert_proof, 1000).unwrap();
    db.approve("approve_alert", &alert_proof, 1000).unwrap();
    // E3 of the engagements review: a third engagement so `?limit=1` yields
    // two content pages plus an EMPTY third page. Distinct agent name
    // (admit refuses a live name collision); admit alone is enough — the
    // read lists every engagement state, and `pending` is one of them.
    let page_proof = common::proof(&common::request(
        "pagination_request",
        "PageWorker",
        &pool,
        100,
    ));
    db.admit(&page_proof, 1000).unwrap();
    db.put_resource(&common::resource(
        "private_alert_pool",
        "private_alert_seat",
        50,
    ))
    .unwrap();
    db.sweep_ceiling_overruns(2000).unwrap();
    (db, engagement)
}
fn snapshot(input: u64, output: u64, cached: u64) -> String {
    json!({"payload":{"info":{"total_token_usage":{"input_tokens":input+cached,"output_tokens":output,"cached_input_tokens":cached,"reasoning_output_tokens":0,"total_tokens":input+cached+output}}}}).to_string()
}
pub struct Fixture {
    pub app: App,
    pub console: Console,
    pub domain: DomainStore,
    pub custody: Store,
    pub engagement: String,
    pub root: tempfile::TempDir,
    pub credentials: CredentialFixture,
}
impl Fixture {
    pub fn new(address: SocketAddr, _built: Option<&Path>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let credentials = CredentialFixture::default();
        let console =
            Console::for_sdk_with_state_and_credentials(Some(&state), credentials.vault()).unwrap();
        let custody = Store::start(Repository::open(&state).unwrap(), 16).unwrap();
        let (db, engagement) = seed(&state);
        let domain = DomainStore::start(db, 16).unwrap();
        let app = App::new(custody.clone(), TOKEN.as_bytes(), address)
            .unwrap()
            .with_domain(domain.clone())
            .with_console(console.clone());
        Self {
            app,
            console,
            domain,
            custody,
            engagement,
            root,
            credentials,
        }
    }
    pub fn service(&self) -> Service {
        Service::new(self.app.clone().router())
    }
    pub async fn close(self) {
        self.console.retire();
        self.domain.shutdown().await.unwrap();
        self.custody.shutdown().await.unwrap();
    }
    pub async fn new_engagement(&self) -> String {
        self.new_engagement_requesting("new_after_browser_build", 100)
            .await
    }
    /// A pending engagement asking for `tokens` from the 1000-token pool.
    pub async fn new_engagement_requesting(&self, request: &str, tokens: u64) -> String {
        let pool = common::resource("private_usage_pool", "private_usage_seat", 1000);
        self.domain
            .admit(
                common::proof(&common::request(request, "NewUsageWorker", &pool, tokens)),
                1000,
            )
            .await
            .unwrap()
            .id
    }
}
pub fn assert_private(value: &Value) {
    let text = value.to_string();
    for private in [
        TOKEN,
        "private_session",
        "private_task",
        "private_runner",
        "private_workspace",
        "private_usage",
        "roomId",
        "RoomId",
        "source_id",
        "snapshot_digest",
        "!private:example.test",
    ] {
        assert!(
            !text.contains(private),
            "unexpected private output {private}"
        );
    }
}

pub fn configuration_resource() -> hagency_core::project::Resource {
    native_resource("private_configuration_browser_source")
}
