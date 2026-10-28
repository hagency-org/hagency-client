//! Board #53 compaction route (honest unsupported port of TS
//! `POST /api/runtime/compact`, backend-v2.js:13061-13077).
use hagency::App;
use hagency_store::*;
use salvo::{
    prelude::*,
    test::{RequestBuilder, ResponseExt, TestClient},
};
use serde_json::{Value, json};

#[path = "../../hagency-store/tests/common/mod.rs"]
mod common;
use common::*;

const TOKEN: &str = "fixture_operator_token_32_bytes_minimum";
const BASE: &str = "http://127.0.0.1:13300";
const URL: &str = "/api/native/v1/runtime/compact";

struct Fixture {
    service: Service,
    domain: DomainStore,
    custody: Store,
    _root: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
        let state = root.path().join("state");
        let custody = Store::start(Repository::open(&state).unwrap(), 16).unwrap();
        let mut db = DomainRepository::open(&state).unwrap();
        db.register(&registration()).unwrap();
        let pool = resource("compact_pool", "compact_seat", 1000);
        db.put_resource(&pool).unwrap();
        let ask = request("compact_request", "Worker", &pool, 100);
        let p = proof(&ask);
        db.admit(&p, 1000).unwrap();
        db.approve("approve", &p, 1000).unwrap();
        let domain = DomainStore::start(db, 16).unwrap();
        let app = App::new(
            custody.clone(),
            TOKEN.as_bytes(),
            "127.0.0.1:13300".parse().unwrap(),
        )
        .unwrap()
        .with_domain(domain.clone());
        Self {
            service: Service::new(app.router()),
            domain,
            custody,
            _root: root,
        }
    }
    fn post(&self, body: Value, token: Option<&str>) -> RequestBuilder {
        let mut req =
            TestClient::post(format!("{BASE}{URL}")).add_header("host", "127.0.0.1:13300", true);
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        req.json(&body)
    }
}

#[tokio::test]
async fn native_compact_route_authority_and_shapes() {
    let f = Fixture::new();
    // Authority: no token → 401.
    let res = f
        .post(json!({"agent": "Worker"}), None)
        .send(&f.service)
        .await;
    assert_eq!(res.status_code, Some(StatusCode::UNAUTHORIZED));
    // Missing agent → 400 "agent required" (the TS normalizeAgentName 400).
    let mut res = f.post(json!({}), Some(TOKEN)).send(&f.service).await;
    assert_eq!(res.status_code, Some(StatusCode::BAD_REQUEST));
    assert_eq!(
        res.take_json::<Value>().await.unwrap(),
        json!({"error": "agent required"})
    );
    // Unknown agent → {ok:true, ignored:'agent-not-found'}.
    let mut res = f
        .post(json!({"agent": "NoSuchAgent"}), Some(TOKEN))
        .send(&f.service)
        .await;
    assert_eq!(res.status_code, Some(StatusCode::OK));
    assert_eq!(
        res.take_json::<Value>().await.unwrap(),
        json!({"ok": true, "ignored": "agent-not-found", "agent": "NoSuchAgent"})
    );
    // Known agent → {ok:true, ignored:'unsupported'} (native has no initiator).
    let mut res = f
        .post(
            json!({"agent": "Worker", "mode": "auto", "summary": "x"}),
            Some(TOKEN),
        )
        .send(&f.service)
        .await;
    assert_eq!(res.status_code, Some(StatusCode::OK));
    assert_eq!(
        res.take_json::<Value>().await.unwrap(),
        json!({"ok": true, "ignored": "unsupported", "agent": "Worker"})
    );
    f.domain.shutdown().await.unwrap();
    f.custody.shutdown().await.unwrap();
}
