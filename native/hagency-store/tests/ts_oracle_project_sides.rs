//! TS-oracle tests for `tests/api-project-sides.test.js` and
//! `tests/project-side-store.test.js` (board #63).
//!
//! The retained project-side store (`lib/project-side-store.js`) is a
//! credential-bearing, verdict-recording, allocation-gated, staging-capable
//! store. The native side is deliberately narrower: `registrations` (ADR-016,
//! one row per fleet — the id IS the server name) plus a read-only
//! `project_sides()` projection, and the registration console route names the
//! thirteen retained columns native has NO source for (`console/project_sides.rs`
//! `UNAVAILABLE`). So the credential/verify/allocation/staging cases are parity
//! gaps (kept as the oracle, `#[ignore]`d), and the reachable identity +
//! projection + idempotency contract is asserted green.
mod common;
use common::*;
use hagency_core::authority::Registration;
use hagency_store::DomainRepository;
use rusqlite::Connection;

struct Sides {
    root: tempfile::TempDir,
    db: DomainRepository,
}

fn open() -> Sides {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    Sides { root, db }
}

/// A second fleet on a DIFFERENT homeserver — the "one side per homeserver"
/// identity is the server name, so two fleets cannot claim one server.
fn other_server_registration() -> Registration {
    let mut reg = registration();
    reg.fleet_id = format!("hf_{}", "b".repeat(32));
    reg.server_name = "other.test".into();
    reg.reception_room_id = "!reception:other.test".into();
    reg.representative_mxid = format!("@{}_representative:other.test", reg.fleet_id);
    reg.approval_bot_mxid = "@approval:other.test".into();
    reg
}

/// TS: `THE CASE THAT MATTERS: no endpoint returns a credential` +
/// `the id IS the server name, so two records cannot claim one homeserver`
/// (`tests/api-project-sides.test.js`, `tests/project-side-store.test.js`).
#[test]
fn ts_oracle_project_sides_projection_never_carries_a_credential() {
    let sides = open();
    // Seed credential-shaped values where native could ever grow one.
    let as_token = "as_token_7c1d3f9a2e8b4056";
    let hs_token = "hs_token_0b4e6d8c1f3a7295";
    Connection::open(sides.root.path().join("state/domain.sqlite3"))
        .unwrap()
        .execute(
            "UPDATE registrations SET config=json_set(config,'$.as_token',?1,'$.hs_token',?2)",
            rusqlite::params![as_token, hs_token],
        )
        .unwrap();
    let text = serde_json::to_string(&sides.db.project_sides().unwrap()).unwrap();
    for secret in [
        as_token, hs_token, "as_token", "hs_token", "asToken", "hsToken",
    ] {
        assert!(!text.contains(secret), "a credential leaked: {secret}");
    }
    // The id IS the server name, and the withheld owner fields are absent.
    assert!(text.contains("example.test"));
    assert!(
        !text.contains("owner_mxid"),
        "the owner is withheld (ADR-112)"
    );
    assert!(!text.contains("owner_room_id"));
}

/// TS: `the list read and the single read agree`, and the projection carries
/// exactly `{id, room_id}` per project — the retained `publicSide` shape.
#[test]
fn ts_oracle_project_sides_list_joins_registrations_to_projects() {
    let sides = open();
    let value = serde_json::to_value(sides.db.project_sides().unwrap()).unwrap();
    let rows = value.as_array().unwrap();
    assert_eq!(rows.len(), 1, "one row per fleet registration");
    let row = &rows[0];
    assert_eq!(row["id"], "example.test", "the id IS the server name");
    assert_eq!(row["representative"], registration().representative_mxid);
    assert_eq!(row["generation"], 1);
    assert_eq!(row["reception_room_id"], "!reception:example.test");
    assert_eq!(row["registered"], true);
    assert!(row["projects"].as_array().unwrap().is_empty());
    // An admitted engagement writes the project row the join reads, and the
    // projection carries exactly `{id, room_id}`.
    let mut sides = sides;
    let pool = resource("sides_pool", "sides_seat", 1000);
    sides.db.put_resource(&pool).unwrap();
    sides
        .db
        .admit(
            &proof(&request("sides_request", "SidesWorker", &pool, 100)),
            1000,
        )
        .unwrap();
    let value = serde_json::to_value(sides.db.project_sides().unwrap()).unwrap();
    let projects = value[0]["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["id"], "project_one");
    assert_eq!(projects[0]["room_id"], "!project:example.test");
    assert_eq!(
        projects[0].as_object().unwrap().len(),
        2,
        "exactly id and room_id"
    );
}

/// TS: `the id is the server name, and a second POST updates rather than
/// duplicates` (`tests/api-project-sides.test.js`) and
/// `re-adding the same name UPDATES rather than duplicating`.
///
/// The native analogue is the registration facade: an identical re-registration
/// is a no-op, a higher generation advances in place (no second row), and a
/// stale generation is refused. The native identity is the FLEET id, and the
/// server name is the row's `id` — the same one-side-per-homeserver posture.
#[test]
fn ts_oracle_project_sides_second_write_updates_rather_than_duplicates() {
    let mut sides = open();
    // Identical content: a no-op, still one row at one generation.
    sides.db.register(&registration()).unwrap();
    assert_eq!(sides.db.project_sides().unwrap().len(), 1);
    // Advance the generation: updates in place, never a second row.
    let mut next = registration();
    next.generation = 2;
    sides.db.register(&next).unwrap();
    let rows = sides.db.project_sides().unwrap();
    assert_eq!(rows.len(), 1, "one side per homeserver");
    assert_eq!(rows[0].generation, 2);
    // A STALE generation is refused, and the stored row is unchanged.
    assert!(sides.db.register(&registration()).is_err());
    assert_eq!(sides.db.project_sides().unwrap()[0].generation, 2);
    // A second fleet on another homeserver is a SECOND side, not an update.
    sides.db.register(&other_server_registration()).unwrap();
    let servers: Vec<String> = sides
        .db
        .project_sides()
        .unwrap()
        .into_iter()
        .map(|side| side.id)
        .collect();
    assert_eq!(servers, ["example.test", "other.test"]);
}

/// TS: `a URL as server_name is a 400 with the reason` — a URL is not a Matrix
/// server name. Native validates the fleet registration shape at the store, so
/// the refusal is the store's own.
#[test]
fn ts_oracle_project_sides_a_url_is_refused_as_a_server_name() {
    let mut sides = open();
    let mut bad = registration();
    bad.server_name = "http://palpo.test".into();
    assert!(
        sides.db.register(&bad).is_err(),
        "a URL is not a Matrix server name"
    );
}

/// The credential/verdict/allocation/staging family — TS
/// `an update that omits the credential does not erase it`, `verify records a
/// verdict...`, `generating the appservice registration`, `removal refuses to
/// be the first step of a cascade`, `a side's allocation, and refusing to mint
/// without one`, `the ACTING credential — a second, wider grant`,
/// `staging protects what works`.
///
/// PARITY GAP: native's `registrations` table has no credential column, no
/// access-verdict columns, no per-side allocation and no staging slot; the
/// registration console route names all thirteen as `unavailable`. Verify,
/// allocation and staging target a store native genuinely lacks.
#[test]
#[ignore = "parity gap: native registrations carry no credential/verdict/allocation/staging columns"]
fn ts_oracle_project_sides_credential_verdict_allocation_staging() {
    panic!("the native registration row has no credential family");
}

/// TS: the knock family (`tests/api-project-side-knock.test.js`, 7 cases) —
/// resolving an alias, knocking on a room, the unsupported-homeserver reason.
///
/// PARITY GAP: native has no knock route and no room-directory client; the
/// retained route talks to the homeserver, which native deliberately does not.
#[test]
#[ignore = "parity gap: no knock route or homeserver room-directory client natively"]
fn ts_oracle_project_side_knock_resolves_alias_and_knocks() {
    panic!("native has no knock route");
}

/// TS: the project add/archive family (`tests/api-project-side-projects.test.js`)
/// and the `外派员工` join.
///
/// PARITY GAP: native has no project add/archive route and no agent<->project
/// binding table, so the third level (a join over bindings) has no source.
#[test]
#[ignore = "parity gap: no project add/archive route and no agent<->project binding table"]
fn ts_oracle_project_side_projects_add_archive_and_staff_join() {
    panic!("native has no project add/archive route");
}

// Out of scope, listed as skipped in the report (not ported, no test):
// the tmux / legacy-groups / supervisor / Claude-runtime surfaces, which
// none of these twelve TS files exercise.
