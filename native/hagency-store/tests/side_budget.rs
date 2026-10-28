mod common;
use common::*;
use hagency_store::DomainRepository;

/// One registered side with one approved engagement of 100 tokens — the
/// smallest store that can be allocated against. `approve` leaves the
/// engagement `reserved` (the provision/reserved pairing, domain.rs:1396),
/// which `side_budget` counts: native mints commitments at reservation,
/// where the retained JavaScript counted `active` only.
fn allocated_store() -> (tempfile::TempDir, DomainRepository) {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    let pool = resource("side_pool", "side_seat", 1000);
    db.put_resource(&pool).unwrap();
    let proof = proof(&request("side_request", "SideWorker", &pool, 100));
    db.admit(&proof, 1000).unwrap();
    db.approve("approve", &proof, 1000).unwrap();
    (dir, db)
}

/// The route id is the SERVER NAME — `projectSideStore.setAllocation`
/// normalizes through `serverName()` (lib/project-side-store.js:455) — and
/// the side list serves that same name as `ProjectSide.id`, while the
/// tables key on the fleet id. A budget set and read through "example.test"
/// must land on the registration row the hf_… fleet id owns.
#[test]
fn side_budget_keys_on_the_server_name() {
    let (_dir, mut db) = allocated_store();
    // Before any allocation: unallocated, which is NOT unlimited
    // (lib/project-side-store.js:443-452) — remaining is null with it.
    let budget = db.side_budget("example.test").unwrap();
    assert_eq!(budget.allocated, None);
    assert_eq!(budget.remaining, None);
    assert_eq!(budget.committed, 100);
    // The server-name id that is not any registration: 404, the verdict the
    // store owns (setAllocation returning null does, backend-v2.js:9544).
    assert!(matches!(
        db.side_budget("nope.test"),
        Err(hagency_store::Error::NotFound)
    ));
    assert!(matches!(
        db.set_side_allocation("nope.test", Some(1)),
        Err(hagency_store::Error::NotFound)
    ));
    // The fleet id itself is NOT a side id on the route: the hf_… spelling
    // is not a server name and must not resolve.
    assert!(matches!(
        db.side_budget(&registration().fleet_id),
        Err(hagency_store::Error::NotFound)
    ));
    db.set_side_allocation("example.test", Some(500)).unwrap();
    let budget = db.side_budget("example.test").unwrap();
    assert_eq!(budget.allocated, Some(500));
    assert_eq!(budget.remaining, Some(400));
}

/// NULL is unallocated (not unlimited), zero is a real allocation of
/// nothing, and an over-committed allocation saturates at zero — the
/// retained `Math.max(0, allocated - committed)` (backend-v2.js:9261).
#[test]
fn side_allocation_null_zero_and_saturation() {
    let (_dir, mut db) = allocated_store();
    db.set_side_allocation("example.test", Some(0)).unwrap();
    let budget = db.side_budget("example.test").unwrap();
    assert_eq!(budget.allocated, Some(0), "zero is a real allocation");
    assert_eq!(budget.remaining, Some(0));
    db.set_side_allocation("example.test", Some(50)).unwrap();
    let budget = db.side_budget("example.test").unwrap();
    assert_eq!(
        budget.remaining,
        Some(0),
        "50 against 100 committed saturates at zero"
    );
    // The clear: absent allocated_tokens means NULL, and remaining goes
    // null with it — never a negative, never an invented zero.
    db.set_side_allocation("example.test", None).unwrap();
    let budget = db.side_budget("example.test").unwrap();
    assert_eq!(budget.allocated, None);
    assert_eq!(budget.remaining, None);
    assert_eq!(
        budget.committed, 100,
        "the clear erases only the allocation"
    );
}

/// The budget breakdown: what the 100 committed is made of, per commitment,
/// with the computed-not-stored agent existence and the empty pool split
/// (every native engagement carries an agent definition, so all rows land
/// in the legacy bucket — the fleet-without-pools shape of the retained
/// `committedForProjectSide({legacyOnly:true})`).
#[test]
fn side_budget_commitment_breakdown() {
    let (_dir, mut db) = allocated_store();
    let budget = db.side_budget("example.test").unwrap();
    assert_eq!(budget.commitments.len(), 1);
    let row = &budget.commitments[0];
    assert_eq!(row.agent, "SideWorker");
    assert_eq!(row.role, "coding");
    assert_eq!(row.project, "project_one");
    assert_eq!(row.project_name.as_deref(), Some("实际项目名称"));
    assert_eq!(row.allocated_tokens, 100);
    assert!(
        row.agent_exists,
        "computed against the engagement-keyed roster"
    );
    assert!(budget.pool_commitments.is_empty());
    assert_eq!(budget.pool_committed, 0);
    assert_eq!(budget.total_committed, 100);
    assert_eq!(budget.orphaned_committed, 0);
    // A retired engagement stops committing: revoke releases the promise.
    // `revoke` takes the engagement id; the proof carries it.
    let proof = proof(&request(
        "side_request",
        "SideWorker",
        &resource("side_pool", "side_seat", 1000),
        100,
    ));
    let id = proof.request().engagement_id().unwrap();
    db.revoke("retire", &id).unwrap();
    let budget = db.side_budget("example.test").unwrap();
    assert_eq!(budget.committed, 0);
    assert!(budget.commitments.is_empty());
}
