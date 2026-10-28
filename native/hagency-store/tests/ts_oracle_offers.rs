//! TS-oracle tests for `tests/api-offer-book.test.js` (board #63).
//!
//! The retained suite is the parity oracle; each test below asserts the SAME
//! observable outcome the JS asserts, against the native store read that
//! answers the same question (`DomainRepository::offer_book`, the #48 port).
//!
//! Where native genuinely differs, the test keeps the TS assertion, is marked
//! `#[ignore = "parity gap: ..."]`, and is listed in `.peer/report-63.md`.
mod common;
use common::*;
use hagency_store::DomainRepository;
use serde_json::json;

struct Book {
    // Keeps the state directory alive for the test.
    #[allow(dead_code)]
    root: tempfile::TempDir,
    db: DomainRepository,
}

/// A fleet with one published codex/medium resource. That resource qualifies
/// for the medium-default roles (`coding`, `testing`, `integration`) and
/// `documentation`, which is the native analogue of the TS seed's
/// `claude-agent`.
fn open() -> Book {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    db.put_resource(&resource("oracle_pool", "oracle_seat", 1000))
        .unwrap();
    Book { root, db }
}

fn roles(book: &Book) -> Vec<String> {
    let value = serde_json::to_value(book.db.offer_book(None).unwrap()).unwrap();
    value["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["role"].as_str().unwrap().to_owned())
        .collect()
}

/// TS: `an unpublished offer does not appear at all`
/// (`tests/api-offer-book.test.js`) — listing it would advertise capacity the
/// provider deliberately did not advertise.
#[test]
fn ts_oracle_offer_unpublished_role_does_not_appear() {
    let mut book = open();
    // `review` defaults to strong + cross-family; the medium resource cannot
    // serve it, and it is not published — so it must be absent either way.
    book.db.set_role_publication("review", false).unwrap();
    assert!(
        !roles(&book).contains(&"review".to_owned()),
        "an unpublished, unservable role is omitted"
    );
    // Publishing it makes it appear (the same role, the other side of the switch).
    book.db.set_role_publication("review", true).unwrap();
    assert!(roles(&book).contains(&"review".to_owned()));
}

/// TS: `published offers disclose qualifying unprovisioned resources without
/// private deployment fields` — the resource is disclosed, its deployment is not.
#[test]
fn ts_oracle_offer_resources_disclosed_without_private_deployment() {
    let book = open();
    let value = serde_json::to_value(book.db.offer_book(None).unwrap()).unwrap();
    let text = value.to_string();
    assert!(
        !text.contains("workspacePath"),
        "deployment does not travel"
    );
    assert!(!text.contains("private/ws"));
    let coding = value["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["role"] == json!("coding"))
        .expect("coding is published by its qualifying resource");
    let resources = coding["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["name"], json!("oracle_pool"));
    assert_eq!(resources[0]["framework"], json!("codex"));
    assert_eq!(resources[0]["model"], json!("gpt-5.6-sol"));
    assert_eq!(resources[0]["tier"], json!("medium"));
}

/// TS: `no ceiling is published, because it is state rather than a promise`.
#[test]
fn ts_oracle_offer_publishes_no_ceiling() {
    let book = open();
    let text = serde_json::to_value(book.db.offer_book(None).unwrap())
        .unwrap()
        .to_string();
    for word in ["remaining", "ceiling", "quota", "seat"] {
        assert!(
            !text.to_lowercase().contains(word),
            "{word} is state, not a promise: {text}"
        );
    }
}

/// TS: `a published role nothing can serve says so rather than being dropped`.
#[test]
fn ts_oracle_offer_unfillable_role_is_stated_not_dropped() {
    let mut book = open();
    // `architect` needs strong, which the medium resource cannot reach, and is
    // published explicitly — so it must be LISTED with a null serving.
    book.db.set_role_publication("architect", true).unwrap();
    let value = serde_json::to_value(book.db.offer_book(None).unwrap()).unwrap();
    let row = value["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["role"] == json!("architect"))
        .expect("a published-but-unfillable role is listed");
    assert_eq!(row["serving"], json!(null), "nothing can serve it");
}

/// TS: `with no room named the answer is null, not false` — `false` would
/// assert that the caller's room is not trusted, a claim about a room nobody
/// identified.
#[test]
fn ts_oracle_offer_no_room_named_is_null_not_false() {
    let book = open();
    let value = serde_json::to_value(book.db.offer_book(None).unwrap()).unwrap();
    assert_eq!(value["whitelisted"], json!(null));
    assert_eq!(value["projectRoomId"], json!(null));
}

/// TS: `a room that is not whitelisted is told that, and NOT who else is` and
/// `a whitelisted room is told it will auto-join`.
///
/// PARITY GAP: native has no whitelist store and never auto-joins (every
/// engagement is born `pending`), and the retained route is `requireRequester`,
/// so native's own `whitelisted` is `null` in every case
/// (`backend-v2.js:15323`). The TS assertion is kept as the oracle.
#[test]
#[ignore = "parity gap: native has no whitelist; offer-book whitelisted is always null"]
fn ts_oracle_offer_whitelisted_room_is_told_it_will_auto_join() {
    let book = open();
    let value =
        serde_json::to_value(book.db.offer_book(Some("!book:hq.example")).unwrap()).unwrap();
    assert_eq!(value["whitelisted"], json!(true));
}

/// TS: `the published caps are reported, because they are the promise`.
///
/// PARITY GAP: native has no offer-terms store, so the three caps are `null`
/// (the retained store's own "unset" encoding).
#[test]
#[ignore = "parity gap: native has no offer-terms store; caps are null"]
fn ts_oracle_offer_published_caps_are_reported() {
    let book = open();
    let value = serde_json::to_value(book.db.offer_book(None).unwrap()).unwrap();
    let coding = &value["roles"][0];
    assert_eq!(coding["budgetCapPerEngagement"], json!(400_000));
    assert_eq!(coding["rateCap"], json!(20_000));
    assert_eq!(coding["count"], json!(2));
}

/// TS: `a role with no offer is omitted, not returned as nulls`.
///
/// PARITY GAP: native lists every role its published RESOURCES can serve
/// (explicit publication OR ≥1 qualifying resource), so a resource-backed role
/// appears without a separate offer record — TS's `listOffers()` returns only
/// explicitly published offers.
#[test]
#[ignore = "parity gap: native also lists roles its resources qualify for, not only explicit offers"]
fn ts_oracle_offer_role_with_no_offer_is_omitted() {
    let book = open();
    assert_eq!(roles(&book).len(), 1, "only the explicitly offered role");
}

/// TS: `the serving framework, model, reasoning level and tier are disclosed`
/// and `the provider deployment is not disclosed with it`.
#[test]
fn ts_oracle_offer_serving_discloses_capability_not_deployment() {
    let mut book = open();
    book.db.set_role_publication("coding", true).unwrap();
    let value = serde_json::to_value(book.db.offer_book(None).unwrap()).unwrap();
    let coding = value["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["role"] == json!("coding"))
        .unwrap();
    assert_eq!(coding["serving"]["framework"], json!("codex"));
    assert_eq!(coding["serving"]["model"], json!("gpt-5.6-sol"));
    assert_eq!(coding["serving"]["tier"], json!("medium"));
    assert!(coding["serving"]["tier"].is_string(), "a tier is disclosed");
    // The live agent identity is a deployment fact native does not publish in
    // this branch (`backend-v2.js:15298-15301`); the capability is what travels.
    assert_eq!(coding["serving"]["agent"], json!(null));
}
