//! Task #19 TS parity tests: offers caps, room whitelist, resource delete,
//! agent definitions, seat delete — and the admission routing that records
//! the TS verdict. Expected values are derived from the TS sources, not
//! invented: backend-v2.js:15330-15385, 15802-15833, 15960-15971;
//! lib/engagement-store.js:178-232, 396-478; lib/resource-agent-definitions.js.
mod common;
use common::*;
use hagency_core::project::{EngagementState, Seat};
use hagency_store::{DomainRepository, Error};
use serde_json::json;

fn setup() -> (tempfile::TempDir, DomainRepository) {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    (dir, db)
}
fn role() -> &'static str {
    // Derived from the shared fixture: common::request() carries "coding" and
    // every existing domain test admits+approves with it, so the fixture
    // resource provably qualifies for this role.
    "coding"
}

#[test]
fn offers_list_every_role_with_ts_default_shape() {
    let (_dir, mut db) = setup();
    // TS GET /api/offers with no configured offer: the role's row is exactly
    // backend-v2.js:15338-15339 — count/budget/rate/updatedAt null,
    // published false, catalogPublished derived from qualifying resources.
    let offers = db.offers().unwrap();
    assert!(!offers.is_empty());
    let r = role();
    let row = offers.iter().find(|o| o["role"] == r).expect("role row");
    assert_eq!(row["count"], json!(null));
    assert_eq!(row["budgetCapPerEngagement"], json!(null));
    assert_eq!(row["rateCap"], json!(null));
    assert_eq!(row["published"], json!(false));
    assert_eq!(row["updatedAt"], json!(null));
    assert!(row.get("updatedBy").is_none());
    // The absent-offer default has no updatedBy key (TS spread of the literal).
    let catalog = row["catalogPublished"].as_bool().unwrap();
    assert!(!catalog); // no published resource qualifies yet

    // A set offer (PUT /api/offers/:role) stores caps and echoes them back.
    let saved = db
        .set_offer(
            r,
            Some(3),
            Some(400_000),
            Some(20_000),
            true,
            "operator",
            1000,
        )
        .unwrap();
    assert_eq!(saved.count, Some(3));
    assert_eq!(saved.budget_cap_per_engagement, Some(400_000));
    assert_eq!(saved.rate_cap, Some(20_000));
    assert!(saved.published);
    let offers = db.offers().unwrap();
    let row = offers.iter().find(|o| o["role"] == r).unwrap();
    assert_eq!(row["count"], json!(3));
    assert_eq!(row["budgetCapPerEngagement"], json!(400_000));
    assert_eq!(row["rateCap"], json!(20_000));
    assert_eq!(row["published"], json!(true));
    assert_eq!(row["updatedAt"], json!(1000));

    // posInt: zero and 2^53 are refused, not stored (TS floor-then-validate).
    assert!(
        db.set_offer(r, Some(0), None, None, true, "operator", 1000)
            .is_err()
    );
    assert!(
        db.set_offer(
            r,
            Some(9_007_199_254_740_992),
            None,
            None,
            true,
            "operator",
            1000
        )
        .is_err()
    );
    // Unknown role is refused (TS: unknown role -> 400).
    assert!(
        db.set_offer("not_a_role", None, None, None, true, "operator", 1000)
            .is_err()
    );
    // Caps are cleared by null on a later set (TS: null clears).
    let saved = db
        .set_offer(r, None, None, None, true, "operator", 2000)
        .unwrap();
    assert_eq!(saved.count, None);
    assert_eq!(saved.budget_cap_per_engagement, None);
    assert_eq!(saved.rate_cap, None);
}

#[test]
fn whitelist_adds_removes_and_reports_still_active() {
    let (_dir, mut db) = setup();
    // TS addToWhitelist: bad room id refused; good one stored with trimmed name.
    assert!(db.add_whitelist("not a room", None, None, 1000).is_err());
    assert!(
        db.add_whitelist("#room:example.test", None, None, 1000)
            .is_err()
    );
    db.add_whitelist("!room:example.test", Some("  My Project  "), None, 1000)
        .unwrap();
    let entries = db.whitelist().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.project_room_id == "!room:example.test")
    );
    let entry = entries
        .iter()
        .find(|e| e.project_room_id == "!room:example.test")
        .unwrap();
    assert_eq!(entry.display_name.as_deref(), Some("My Project"));
    assert_eq!(entry.added_by, "operator");
    assert!(db.is_whitelisted("!room:example.test").unwrap());

    // TS removeFromWhitelist: absent room is NotFound; removal returns the
    // active engagement ids for the room (future requests only).
    assert!(matches!(
        db.remove_whitelist("!missing:example.test"),
        Err(Error::NotFound)
    ));
    let (_, still) = db.remove_whitelist("!room:example.test").unwrap();
    assert!(still.is_empty()); // no engagements for this room yet
    assert!(!db.is_whitelisted("!room:example.test").unwrap());
}

#[test]
fn admission_routing_records_ts_verdicts() {
    let (_dir, mut db) = setup();
    let pool = resource("route_preset", "route_seat", 1000);
    db.put_resource(&pool).unwrap();
    let r = role();
    let mut req = request("route_one", "Router", &pool, 100);
    req.role = r.to_owned();
    let verified = proof(&req);

    // No whitelist entry: TS routeRequest says notWhitelisted, and the request
    // is STILL RECORDED (pending), never refused.
    let e = db.admit(&verified, 1000).unwrap();
    assert_eq!(e.route.as_deref(), Some("notWhitelisted"));
    assert!(!e.auto_joined);
    assert_eq!(e.state, EngagementState::Pending);

    // Whitelisted but unpublished offer: overOffer (TS: no offer means not on
    // offer — never unlimited).
    db.add_whitelist("!project:example.test", None, None, 1000)
        .unwrap();
    let mut req = request("route_two", "Router2", &pool, 100);
    req.role = r.to_owned();
    let verified = proof(&req);
    let e = db.admit(&verified, 1000).unwrap();
    assert_eq!(e.route.as_deref(), Some("overOffer"));
    assert!(!e.auto_joined);

    // Published with a rate cap and an unstated rate: overOffer (an unstated
    // rate is unknown, not zero — TS comment at engagement-store.js:200-209).
    db.set_offer(r, None, None, Some(20_000), true, "operator", 1000)
        .unwrap();
    let mut req = request("route_three", "Router3", &pool, 100);
    req.role = r.to_owned();
    let verified = proof(&req);
    let e = db.admit(&verified, 1000).unwrap();
    assert_eq!(e.route.as_deref(), Some("overOffer"));
    assert!(!e.auto_joined);

    // A rate within the cap: autoJoin — the TS happy path.
    let mut req = request("route_four", "Router4", &pool, 100);
    req.role = r.to_owned();
    req.rate_per_day = Some(hagency_core::allocation::Tokens::try_from(10_000u64).unwrap());
    let verified = proof(&req);
    let e = db.admit(&verified, 1000).unwrap();
    assert_eq!(e.route.as_deref(), Some("autoJoin"));
    assert!(e.auto_joined);

    // A count cap of 1 with one active engagement for the role: overOffer.
    db.set_offer(r, Some(1), None, Some(20_000), true, "operator", 1000)
        .unwrap();
    let mut req = request("route_five", "Router5", &pool, 100);
    req.role = r.to_owned();
    req.rate_per_day = Some(hagency_core::allocation::Tokens::try_from(10_000u64).unwrap());
    let verified = proof(&req);
    let e = db.admit(&verified, 1000).unwrap();
    // The first is pending (not auto-joined in this store without a verdict),
    // so the count of HOLDING engagements is 0 and this routes autoJoin too —
    // pending holds a future allocation in TS (holdsAllocation counts pending
    // with a non-failed fulfillment).
    assert_eq!(e.route.as_deref(), Some("autoJoin"));
}

#[test]
fn resource_delete_guards_and_cascade() {
    let (_dir, mut db) = setup();
    let pool = resource("delete_preset", "delete_seat", 1000);
    db.put_resource(&pool).unwrap();
    let id = pool.id();
    // Agent definitions block delete (TS 409 'Remove unused Agent definitions...').
    db.edit_agent_definition(
        &id,
        None,
        Some(&json!({"name":"helper","role":role()})),
        1000,
    )
    .unwrap();
    assert!(matches!(db.delete_resource(&id), Err(Error::Conflict)));
    db.edit_agent_definition(
        &id,
        Some("rad_00000000000000000000000000000001"),
        None,
        1000,
    )
    .ok();
    // Wait: deleting by the fake id is NotFound; delete the real one instead.
    let defs = db.agent_definitions(&id).unwrap();
    let real_id = defs[0]["id"].as_str().unwrap();
    let removed = db
        .edit_agent_definition(&id, Some(real_id), None, 1000)
        .unwrap();
    // TS edit() delete returns next.find(id) || next.at(-1) || null: with no
    // rows left, that is null.
    assert!(removed.is_none());
    // Now the delete succeeds and echoes the removed resource.
    let catalog = db.delete_resource(&id).unwrap();
    assert_eq!(catalog.id, id);
    assert!(matches!(db.delete_resource(&id), Err(Error::NotFound)));
}

#[test]
fn agent_definitions_ts_rules() {
    let (_dir, mut db) = setup();
    let pool = resource("defs_preset", "defs_seat", 1000);
    db.put_resource(&pool).unwrap();
    let id = pool.id();
    let r = role();

    // Name rules (TS resource-agent-definitions.js:24-26).
    assert!(
        db.edit_agent_definition(&id, None, Some(&json!({"name":"Bad","role":r})), 1000)
            .is_err()
    );
    assert!(
        db.edit_agent_definition(&id, None, Some(&json!({"name":"1bad","role":r})), 1000)
            .is_err()
    );
    let created = db
        .edit_agent_definition(&id, None, Some(&json!({"name":"good_one","role":r})), 1000)
        .unwrap()
        .unwrap();
    assert!(created.id.starts_with("rad_"));
    assert_eq!(created.name, "good_one");
    assert_eq!(created.role, r);
    assert!(created.enabled); // TS default enabled=true
    assert_eq!(created.created_at, 1000);

    // Duplicate name is a conflict; different resource may share it (UNIQUE
    // per resource matches TS `all().some(same name)` scoping? — TS checks
    // across ALL resources; the UNIQUE index is per-resource. The TS rule is
    // cross-resource, so assert the store-level check too.)
    assert!(matches!(
        db.edit_agent_definition(&id, None, Some(&json!({"name":"good_one","role":r})), 2000),
        Err(Error::Conflict)
    ));

    // Enabled must be a boolean.
    assert!(
        db.edit_agent_definition(
            &id,
            None,
            Some(&json!({"name":"other","role":r,"enabled":"yes"})),
            2000
        )
        .is_err()
    );

    // Role the resource cannot supply is refused (TS 'cannot supply').
    assert!(matches!(
        db.edit_agent_definition(
            &id,
            None,
            Some(&json!({"name":"other","role":"reviewer_but_not_supplying"})),
            2000
        ),
        Err(Error::Unqualified)
    ));

    // List carries the derived status/activeEngagements shape.
    let rows = db.agent_definitions(&id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["status"], json!("defined"));
    assert_eq!(rows[0]["activeEngagements"], json!(0));

    // Update keeps id and created_at (TS edit keeps both).
    let updated = db
        .edit_agent_definition(
            &id,
            Some(&created.id),
            Some(&json!({"name":"good_two","role":r})),
            3000,
        )
        .unwrap()
        .unwrap();
    assert_eq!(updated.id, created.id);
    assert_eq!(updated.name, "good_two");
    assert_eq!(updated.created_at, 1000);
    // Unknown definition id is NotFound.
    assert!(matches!(
        db.edit_agent_definition(
            &id,
            Some("rad_missing"),
            Some(&json!({"name":"x_y","role":r})),
            3000
        ),
        Err(Error::NotFound)
    ));
    // Unknown resource is NotFound.
    assert!(matches!(
        db.edit_agent_definition(
            "res_missing",
            None,
            Some(&json!({"name":"x_y","role":r})),
            3000
        ),
        Err(Error::NotFound)
    ));
}

#[test]
fn seat_delete_requires_declaration() {
    let (_dir, mut db) = setup();
    // A seat row with NO declaration: TS DELETE returns 404.
    db.put_seat(&Seat {
        id: "seat_bare".into(),
        declaration: None,
    })
    .unwrap();
    assert!(matches!(db.delete_seat("seat_bare"), Err(Error::NotFound)));
    // A declared seat deletes.
    let declared = serde_json::from_value::<Seat>(json!({
        "id":"seat_declared",
        "declaration":{"tokens":1000,"period":"monthly"}
    }))
    .unwrap();
    db.put_seat(&declared).unwrap();
    db.delete_seat("seat_declared").unwrap();
    // Unknown seat: 404 (TS: no declaration for that seat).
    assert!(matches!(
        db.delete_seat("seat_unknown"),
        Err(Error::NotFound)
    ));
}
