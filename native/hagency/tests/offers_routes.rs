use hagency::App;
use hagency_store::{DomainRepository, DomainStore, Repository, Store};
use salvo::{
    prelude::*,
    test::{ResponseExt, TestClient},
};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

const TOKEN: &str = "fixture_operator_token_32_bytes_minimum";
const BASE: &str = "http://127.0.0.1:13300";

/// The task #19 operator routes, wired the way every other operator test
/// wires the app: both writers behind App, bearer token, no query strings.
async fn app(state: &Path) -> Arc<Service> {
    let custody = Store::start(Repository::open(state).unwrap(), 16).unwrap();
    let domain = DomainStore::start(DomainRepository::open(state).unwrap(), 16).unwrap();
    let app = App::new(
        custody,
        TOKEN.as_bytes(),
        "127.0.0.1:13300".parse().unwrap(),
    )
    .unwrap()
    .with_domain(domain);
    Arc::new(Service::new(app.router()))
}

fn get(path: String) -> salvo::test::RequestBuilder {
    TestClient::get(format!("{BASE}{path}"))
        .add_header("host", "127.0.0.1:13300", true)
        .bearer_auth(TOKEN)
}
fn post(path: String) -> salvo::test::RequestBuilder {
    TestClient::post(format!("{BASE}{path}"))
        .add_header("host", "127.0.0.1:13300", true)
        .bearer_auth(TOKEN)
}
fn put(path: String) -> salvo::test::RequestBuilder {
    TestClient::put(format!("{BASE}{path}"))
        .add_header("host", "127.0.0.1:13300", true)
        .bearer_auth(TOKEN)
}
fn delete(path: String) -> salvo::test::RequestBuilder {
    TestClient::delete(format!("{BASE}{path}"))
        .add_header("host", "127.0.0.1:13300", true)
        .bearer_auth(TOKEN)
}

/// TS GET /api/offers (backend-v2.js:15330-15345): the empty store lists
/// EVERY role with the absent-offer default row, and the absent row has no
/// `updatedBy` key.
#[tokio::test]
async fn offers_route_lists_every_role_with_ts_default() {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let service = app(&dir.path().join("state")).await;
    let mut response = get("/api/native/v1/offers".into()).send(&*service).await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let value: Value = response.take_json().await.unwrap();
    let offers = value["offers"].as_array().expect("offers array");
    assert!(!offers.is_empty());
    let role_count = hagency_core::qualification::roles().count();
    assert_eq!(offers.len(), role_count);
    let row = offers
        .iter()
        .find(|o| o["role"] == "coding")
        .expect("coding row");
    assert_eq!(row["count"], json!(null));
    assert_eq!(row["budgetCapPerEngagement"], json!(null));
    assert_eq!(row["rateCap"], json!(null));
    assert_eq!(row["published"], json!(false));
    assert_eq!(row["updatedAt"], json!(null));
    assert!(row.get("updatedBy").is_none());
}

/// TS PUT /api/offers/:role then GET (backend-v2.js:15347-15361): the wire is
/// camelCase, the caps are echoed back, and an unknown role is a 400.
#[tokio::test]
async fn offers_route_put_and_read_back_camel_case() {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let service = app(&dir.path().join("state")).await;
    let mut response = put("/api/native/v1/offers/coding".into())
        .json(&json!({"count":3,"budgetCapPerEngagement":400000,"rateCap":20000,"published":true}))
        .send(&*service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let value: Value = response.take_json().await.unwrap();
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["offer"]["count"], json!(3));
    assert_eq!(value["offer"]["budgetCapPerEngagement"], json!(400000));
    assert_eq!(value["offer"]["rateCap"], json!(20000));
    assert_eq!(value["offer"]["published"], json!(true));

    let mut listed = get("/api/native/v1/offers".into()).send(&*service).await;
    let offers: Value = listed.take_json().await.unwrap();
    let row = offers["offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["role"] == "coding")
        .unwrap()
        .clone();
    assert_eq!(row["count"], json!(3));
    assert_eq!(row["published"], json!(true));
    assert!(row.get("updatedBy").is_some());

    // Unknown role: TS answers 400 with `unknown role: ...`.
    let denied = put("/api/native/v1/offers/not_a_role".into())
        .json(&json!({"published":true}))
        .send(&*service)
        .await;
    assert_eq!(denied.status_code, Some(StatusCode::BAD_REQUEST));
}

/// TS whitelist routes (backend-v2.js:15363-15388): POST echoes the entry,
/// GET lists it, DELETE returns `{ok, projectRoomId, stillActive}` and a
/// second DELETE is 404.
#[tokio::test]
async fn whitelist_route_round_trip() {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let service = app(&dir.path().join("state")).await;
    let mut added = post("/api/native/v1/whitelist".into())
        .json(&json!({"projectRoomId":"!room:example.test","displayName":"My Project"}))
        .send(&*service)
        .await;
    assert_eq!(added.status_code, Some(StatusCode::OK));
    let value: Value = added.take_json().await.unwrap();
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["entry"]["projectRoomId"], json!("!room:example.test"));
    assert_eq!(value["entry"]["displayName"], json!("My Project"));
    assert_eq!(value["entry"]["addedBy"], json!("operator"));

    // A non-room id is refused (TS: projectRoomId must be a Matrix room id).
    let denied = post("/api/native/v1/whitelist".into())
        .json(&json!({"projectRoomId":"not a room"}))
        .send(&*service)
        .await;
    assert_eq!(denied.status_code, Some(StatusCode::BAD_REQUEST));

    let mut listed = get("/api/native/v1/whitelist".into()).send(&*service).await;
    let value: Value = listed.take_json().await.unwrap();
    assert_eq!(
        value["whitelist"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["projectRoomId"] == "!room:example.test")
            .count(),
        1
    );

    let mut removed = delete("/api/native/v1/whitelist/!room:example.test".into())
        .send(&*service)
        .await;
    assert_eq!(removed.status_code, Some(StatusCode::OK));
    let value: Value = removed.take_json().await.unwrap();
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["projectRoomId"], json!("!room:example.test"));
    assert_eq!(value["stillActive"], json!([]));

    let missing = delete("/api/native/v1/whitelist/!room:example.test".into())
        .send(&*service)
        .await;
    assert_eq!(missing.status_code, Some(StatusCode::NOT_FOUND));
}

/// TS seat + resource deletes (backend-v2.js:15802-15810, 15960-15971): both
/// 404 when absent; a resource with a live definition is a 409.
#[tokio::test]
async fn delete_routes_guards() {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let service = app(&dir.path().join("state")).await;
    let seat = delete("/api/native/v1/seats/seat_none".into())
        .send(&*service)
        .await;
    assert_eq!(seat.status_code, Some(StatusCode::NOT_FOUND));

    let preset = delete("/api/native/v1/framework-presets/preset_none".into())
        .send(&*service)
        .await;
    assert_eq!(preset.status_code, Some(StatusCode::NOT_FOUND));

    // A published resource with a definition: delete is refused with the TS
    // 409 'Remove unused Agent definitions...' guard.
    let resource = json!({"presetId":"defs_preset","seatId":"defs_seat","framework":"codex","model":"gpt-5.6-sol","reasoning":"medium","ceiling":{"tokens":1000,"period":"monthly"}});
    let mut created = post("/api/native/v1/resources".into())
        .json(&resource)
        .send(&*service)
        .await;
    assert_eq!(created.status_code, Some(StatusCode::OK));
    let created: Value = created.take_json().await.unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    // The route answers the PUBLIC resource id, and the definition routes key
    // on it (read_resource takes either spelling — both map to the row).
    let mut added = post(format!("/api/native/v1/framework-presets/{id}/agents"))
        .json(&json!({"name":"helper","role":"coding"}))
        .send(&*service)
        .await;
    assert_eq!(added.status_code, Some(StatusCode::OK));
    let added: Value = added.take_json().await.unwrap();
    assert_eq!(added["ok"], json!(true));
    assert_eq!(added["definition"]["name"], json!("helper"));
    assert_eq!(added["definition"]["enabled"], json!(true));

    let denied = delete(format!("/api/native/v1/framework-presets/{id}"))
        .send(&*service)
        .await;
    assert_eq!(denied.status_code, Some(StatusCode::CONFLICT));

    // The definitions list carries the derived status shape.
    let mut listed = get(format!("/api/native/v1/framework-presets/{id}/agents"))
        .send(&*service)
        .await;
    let value: Value = listed.take_json().await.unwrap();
    let defs = value["agentDefinitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0]["status"], json!("defined"));
    assert_eq!(defs[0]["activeEngagements"], json!(0));

    // Remove the definition, then the resource deletes and echoes itself.
    let def_id = defs[0]["id"].as_str().unwrap().to_owned();
    let mut removed = delete(format!(
        "/api/native/v1/framework-presets/{id}/agents/{def_id}"
    ))
    .send(&*service)
    .await;
    assert_eq!(removed.status_code, Some(StatusCode::OK));
    let removed: Value = removed.take_json().await.unwrap();
    // TS edit() on delete returns the LAST REMAINING definition or null.
    assert_eq!(removed["definition"], json!(null));

    let mut deleted = delete(format!("/api/native/v1/framework-presets/{id}"))
        .send(&*service)
        .await;
    assert_eq!(deleted.status_code, Some(StatusCode::OK));
    let deleted: Value = deleted.take_json().await.unwrap();
    assert_eq!(deleted["ok"], json!(true));
    assert_eq!(deleted["preset"]["id"], json!(id));
}
