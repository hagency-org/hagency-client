//! Board #48 store reads: the offer book, the contributions list and the
//! engagement PREVIEW — the last proven to be a DRY RUN by whole-database
//! snapshot equality across the call.
mod common;
use common::*;
use hagency_store::DomainRepository;
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;

/// Every user table's full contents, in a stable order — the strongest
/// no-write proof available: any INSERT, UPDATE or DELETE the preview
/// committed would change this string.
fn digest(path: &Path) -> String {
    let conn = Connection::open(path).unwrap();
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut out = String::new();
    for table in tables {
        out.push_str(&table);
        out.push('\n');
        let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
        let columns = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for i in 0..columns {
                let value: rusqlite::types::Value = row.get(i).unwrap();
                out.push_str(&format!("{value:?}|"));
            }
            out.push('\n');
        }
    }
    out
}

/// One fleet with one published resource, and one APPROVED engagement on
/// `coding` — the real state every read below projects.
struct Book {
    root: tempfile::TempDir,
    db: DomainRepository,
}

fn open() -> Book {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    let pool = resource("private_offer_pool", "private_offer_seat", 1000);
    db.put_resource(&pool).unwrap();
    let proof = proof(&request("offer_request", "OfferWorker", &pool, 100));
    db.admit(&proof, 1000).unwrap();
    db.approve("approve", &proof, 1000).unwrap();
    // Apply the provision effect: only then is the engagement genuinely
    // `active`, which is the state these reads project.
    let effect = db.claim_effect().unwrap().unwrap();
    db.observe_effect(
        &effect.id,
        effect.fence,
        &hagency_store::EffectOutcome::Applied {
            receipt: "synthetic offer-book fixture".into(),
        },
    )
    .unwrap();
    Book { root, db }
}

fn path(book: &Book) -> std::path::PathBuf {
    book.root.path().join("state/domain.sqlite3")
}

/// The offer book lists the roles native actually publishes and, per role,
/// the REAL serving resource and resource list and the REAL live count.
/// The three cap fields are `null` — the retained store's own "unset"
/// encoding (`lib/engagement-store.js:451-454`), never `0`.
#[test]
fn offer_book_lists_real_roles_with_real_state() {
    let book = open();
    let value = serde_json::to_value(book.db.offer_book(None).unwrap()).unwrap();
    assert_eq!(value["whitelisted"], json!(null), "no room named");
    assert_eq!(value["projectRoomId"], json!(null));
    let roles = value["roles"].as_array().unwrap();
    let names: Vec<&str> = roles.iter().map(|r| r["role"].as_str().unwrap()).collect();
    // `private_offer_pool` is codex/gpt-5.6-sol at medium: it qualifies for the
    // medium-default roles and `documentation` (lightweight), never for the
    // strong-floor `architect`/`review`.
    assert_eq!(names, ["coding", "testing", "integration", "documentation"]);
    let coding = &roles[0];
    assert_eq!(
        coding["runningNow"],
        json!(1),
        "the one approved engagement"
    );
    assert_eq!(coding["crossFamilyOk"], json!(true));
    for cap in ["budgetCapPerEngagement", "rateCap", "count"] {
        assert_eq!(coding[cap], json!(null), "{cap} is unset, never 0");
    }
    assert_eq!(
        coding["serving"]["agent"],
        json!(null),
        "preset identity stays private"
    );
    assert_eq!(coding["serving"]["framework"], json!("codex"));
    assert_eq!(coding["serving"]["model"], json!("gpt-5.6-sol"));
    assert_eq!(coding["serving"]["tier"], json!("medium"));
    assert_eq!(coding["serving"]["provisioningRequired"], json!(true));
    let resources = coding["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["name"], json!("private_offer_pool"));
    assert_eq!(resources[0]["tier"], json!("medium"));
    // A role nobody can serve is omitted rather than published as a lie.
    assert!(!names.contains(&"architect"));
    // A named room is echoed; native still publishes no auto-join trust for a
    // requester (the retained route's own `requireRequester` rule).
    let named =
        serde_json::to_value(book.db.offer_book(Some("!room:example.test")).unwrap()).unwrap();
    assert_eq!(named["projectRoomId"], json!("!room:example.test"));
    assert_eq!(
        named["whitelisted"],
        json!(null),
        "never trusts, always approval"
    );
}

/// Contributions project the REAL agent<->project relationships. The two
/// membership-probe fields are `null` ("never checked"), NOT `false`.
#[test]
fn contributions_project_real_relationships() {
    let book = open();
    let rows = book.db.contributions().unwrap();
    assert_eq!(rows.len(), 1);
    let value = serde_json::to_value(&rows[0]).unwrap();
    assert_eq!(value["agent"], json!("OfferWorker"));
    assert_eq!(value["project"], json!("project_one"));
    assert_eq!(value["projectRoomId"], json!("!project:example.test"));
    assert_eq!(value["ownerMxid"], json!("@owner:example.test"));
    assert_eq!(value["active"], json!(true));
    assert_eq!(
        value["agentJoined"],
        json!(null),
        "never checked, not false"
    );
    assert_eq!(value["membershipCheckedAt"], json!(null));
}

/// The preview answers with native's real default route and the real headroom
/// — and changes NOTHING.
#[test]
fn preview_is_a_dry_run() {
    let book = open();
    let before = digest(&path(&book));
    let value = serde_json::to_value(book.db.preview("coding").unwrap()).unwrap();
    assert_eq!(
        value["route"],
        json!("notWhitelisted"),
        "an approval-only host trusts no room for auto-join"
    );
    assert_eq!(value["autoJoin"], json!(false));
    assert_eq!(value["agent"], json!("OfferWorker"));
    assert_eq!(
        value["agentRemainingTokens"],
        json!(900),
        "1000 ceiling less the 100-token live commitment"
    );
    // A role nothing is running answers without an agent, in the same shape.
    let idle = serde_json::to_value(book.db.preview("documentation").unwrap()).unwrap();
    assert_eq!(idle["route"], json!("notWhitelisted"));
    assert_eq!(idle["agent"], json!(null));
    assert_eq!(idle["agentRemainingTokens"], json!(null));
    // The dry run wrote nothing at all.
    assert_eq!(digest(&path(&book)), before, "the preview must not write");
    // An unknown role is refused, never answered.
    assert!(book.db.preview("not_a_role").is_err());
}
