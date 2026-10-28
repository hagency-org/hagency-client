//! TS oracle: `tests/api-engagement-side-budget.test.js` (the 21 side/ceiling
//! allocation cases).
//!
//! Ported into the STORE crate, whose real accounting path owns this surface:
//! `DomainRepository::resource_budget` / `resource_headroom` →
//! `allocation::resource_budget` (`hagency-core/src/allocation.rs`). The TS case
//! family charges a *project side*; native charges the **resource/pool+seat**
//! ceiling, and the commitment is derived from the live `reserved`/`active`
//! state — the same "the state transition IS the accounting" rule the TS file's
//! own header states (api-engagement-side-budget.test.js:18).
mod common;
use common::*;
use hagency_store::DomainRepository;
use hagency_store::Error;

/// One fleet, one published resource with a real ceiling — the state every
/// budget read below projects.
struct Book {
    #[allow(dead_code)]
    root: tempfile::TempDir,
    db: DomainRepository,
}

impl Book {
    fn open(ceiling: u64) -> Self {
        let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
        let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
        db.register(&registration()).unwrap();
        db.put_resource(&resource("budget_pool", "budget_seat", ceiling))
            .unwrap();
        Book { root, db }
    }
    fn pool() -> hagency_core::project::Resource {
        resource("budget_pool", "budget_seat", 0)
    }
    fn committed(&self) -> u64 {
        u64::from(
            self.db
                .resource_budget(&Self::pool().id())
                .unwrap()
                .pool
                .committed,
        )
    }
    fn remaining(&self) -> Option<u64> {
        self.db
            .resource_budget(&Self::pool().id())
            .unwrap()
            .remaining_tokens
            .map(u64::from)
    }
}

/// TS `api-engagement-side-budget.test.js:120` — *within the allocation it
/// auto-joins, and the side shows the commitment*. Native: an approved
/// engagement becomes `reserved` and its tokens count in `pool.committed`, with
/// `remaining` = ceiling − committed. Derived from `budget` (`domain.rs:589`):
/// only `reserved`/`active` rows are summed.
#[test]
fn ts_side_budget_commitment_follows_the_live_state() {
    let mut b = Book::open(1_000);
    let pool = resource("budget_pool", "budget_seat", 1_000);
    let request_proof = proof(&request("side_one", "SideWorker", &pool, 250));
    b.db.admit(&request_proof, 1000).unwrap();
    assert_eq!(b.committed(), 0, "a pending engagement commits nothing");
    b.db.approve("approve_side", &request_proof, 1000).unwrap();
    assert_eq!(b.committed(), 250, "the transition IS the accounting");
    assert_eq!(b.remaining(), Some(750));
}

/// TS `api-engagement-side-budget.test.js:138` — *the SECOND auto-join sees what
/// the first committed*. Two requests that each fit but together do not: the
/// second approval is refused and the first's commitment stands.
#[test]
fn ts_side_budget_second_commit_sees_the_first() {
    let mut b = Book::open(300);
    // The SAME pool+seat is the shared budget: the second request draws on it.
    let pool = resource("budget_pool", "budget_seat", 300);
    let first = proof(&request("second_a", "SecondA", &pool, 200));
    b.db.admit(&first, 1000).unwrap();
    b.db.approve("approve_second_a", &first, 1000).unwrap();
    assert_eq!(b.remaining(), Some(100));
    // A second 200 against 100 remaining is refused as an over-commit, and the
    // first's commitment stands. Derived from `approve` (`domain.rs:1447-1481`).
    let over = proof(&request("second_b", "SecondB", &pool, 200));
    b.db.admit(&over, 1000).unwrap();
    assert!(
        matches!(
            b.db.approve("approve_second_b", &over, 1000),
            Err(Error::OverCommit { .. })
        ),
        "the second commit is refused as an over-commit"
    );
    assert_eq!(b.committed(), 200, "the first commitment is untouched");
}

/// TS `api-engagement-side-budget.test.js:155` — *UNALLOCATED IS NOT UNLIMITED: a
/// configured side with no allocation refuses*. Derived from `tests/domain.rs:550-573`:
/// a declared ceiling whose seat declaration period MISMATCHES the pool's leaves
/// the remaining unknown, which native refuses as `NoCeiling` — never read as
/// unlimited.
#[test]
fn ts_side_budget_unallocated_is_not_unlimited() {
    let mut b = Book::open(1_000);
    // The pool declares a monthly ceiling; the seat declares a daily quota, so
    // the two periods cannot be compared and the remaining is unknown.
    let mismatched = resource("no_ceiling_pool", "mismatched_seat", 100);
    b.db.put_resource(&mismatched).unwrap();
    let seat: hagency_core::project::Seat = serde_json::from_value(serde_json::json!({
        "id":"mismatched_seat","declaration":{"quotaTokens":50,"period":"daily"}
    }))
    .unwrap();
    b.db.put_seat(&seat).unwrap();
    let request_proof = proof(&request(
        "no_ceiling_one",
        "NoCeilingWorker",
        &mismatched,
        1,
    ));
    b.db.admit(&request_proof, 1000).unwrap();
    assert!(
        matches!(
            b.db.approve("approve_no_ceiling", &request_proof, 1000),
            Err(Error::NoCeiling)
        ),
        "an unknown ceiling refuses rather than reading as unlimited"
    );
}

/// TS `api-engagement-side-budget.test.js:166` — *zero is a real allocation, and
/// refuses rather than being read as unset*. A zero ceiling is a declared ceiling
/// of zero: the approval is refused as over-commit, never accepted.
#[test]
fn ts_side_budget_zero_is_a_real_allocation() {
    let mut b = Book::open(1_000);
    let zero = resource("zero_pool", "zero_seat", 0);
    b.db.put_resource(&zero).unwrap();
    let request_proof = proof(&request("zero_one", "ZeroWorker", &zero, 1));
    b.db.admit(&request_proof, 1000).unwrap();
    assert!(
        b.db.approve("approve_zero", &request_proof, 1000).is_err(),
        "zero is a real allocation and refuses the request"
    );
}

/// TS `api-engagement-side-budget.test.js:248` — *the side's remaining is read at
/// the VERDICT, not remembered from the request*. Native recomputes the budget
/// inside `approve` from the live rows, so a commitment made between admit and
/// approve is seen.
#[test]
fn ts_side_budget_remaining_is_read_at_the_verdict() {
    let mut b = Book::open(1_000);
    let pool = resource("budget_pool", "budget_seat", 1_000);
    // Admit the request while there is room …
    let request_proof = proof(&request("verdict_one", "VerdictWorker", &pool, 400));
    b.db.admit(&request_proof, 1000).unwrap();
    // … then commit most of the headroom from another engagement.
    let other = proof(&request("verdict_other", "VerdictOther", &pool, 700));
    b.db.admit(&other, 1000).unwrap();
    b.db.approve("approve_other", &other, 1000).unwrap();
    // The verdict sees the NEW remaining (300), not the remembered 1000.
    assert!(
        b.db.approve("approve_verdict", &request_proof, 1000)
            .is_err(),
        "the verdict reads the live remaining"
    );
}

/// TS `api-engagement-side-budget.test.js:265` — *a REJECTION is never refused for
/// budget, on the very side that refuses approval*. Native's reject path takes no
/// budget branch at all (`end`, `domain.rs:1521`).
#[test]
fn ts_side_budget_rejection_is_never_refused_for_budget() {
    let mut b = Book::open(0);
    let zero = resource("reject_pool", "reject_seat", 0);
    b.db.put_resource(&zero).unwrap();
    let request_proof = proof(&request("reject_budget", "RejectWorker", &zero, 500));
    let e = b.db.admit(&request_proof, 1000).unwrap();
    // The approval would be refused, but the rejection is not.
    assert!(
        b.db.approve("approve_reject", &request_proof, 1000)
            .is_err()
    );
    assert!(
        b.db.reject("reject_cmd", &e.id).is_ok(),
        "a rejection takes no budget branch"
    );
}

/// TS `api-engagement-side-budget.test.js:290` — *a verdict on an unknown
/// engagement still answers 404, not a budget refusal*. Native returns
/// `NotFound` before any budget work (`read_engagement` first).
#[test]
fn ts_side_budget_unknown_engagement_is_not_found() {
    let mut b = Book::open(1_000);
    let pool = resource("budget_pool", "budget_seat", 1_000);
    let request_proof = proof(&request("unknown_verdict", "UnknownWorker", &pool, 10));
    // Never admitted: the verdict is NotFound, never a budget verdict.
    assert!(matches!(
        b.db.approve("approve_unknown", &request_proof, 1000),
        Err(Error::NotFound)
    ));
}

/// TS `api-engagement-side-budget.test.js:94,177,198,223` — the AUTO-JOIN cases
/// (over-allocation refused, a room on a server with no side record not gated,
/// one side's commitment does not reduce another's, an approval allocating MORE
/// than asked). Native never auto-joins and has no per-side allocation table
/// (`offer_book.rs:18-33`), so the auto-join half has no native counterpart; the
/// cross-resource isolation half IS native and asserted by
/// `tests/domain.rs` (per-preset/seat aggregation).
#[test]
#[ignore = "parity gap: native never auto-joins and has no per-side allocation table (offer_book.rs:18-33); cross-resource isolation is asserted by tests/domain.rs"]
fn ts_side_budget_auto_join_and_per_side_allocation() {}

/// TS `api-engagement-side-budget.test.js:325,350,365,373,392,406,430,440,466`
/// — the side-allocation ALERT cases (a refusal raises a WARNING not a downgraded
/// info, names the side/shortfall/remedy, one alert per retrying borrower,
/// raising resolves, unsetting does not, two sides are two alerts, an
/// unconfigured side refuses). Native's alert path is the ceiling-overrun sweep
/// (`ceiling_alerts.rs`), asserted green by `tests/ceiling_alerts.rs`; the
/// per-side shortfall vocabulary has no native counterpart.
#[test]
#[ignore = "covered: the ceiling-overrun alert path is asserted by tests/ceiling_alerts.rs; the per-side shortfall vocabulary has no native counterpart"]
fn ts_side_budget_allocation_alerts() {}
