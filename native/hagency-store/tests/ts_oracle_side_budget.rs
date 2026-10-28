//! TS-oracle tests for `tests/api-engagement-side-budget.test.js` and
//! `tests/api-agent-project-side-binding.test.js` (board #63).
//!
//! Both files describe a per-SIDE world: a project side is a billing and
//! trust boundary, a per-side allocation gates admission, and an agent is
//! BOUND to exactly one side. Native has no such world — there is no
//! per-side allocation column, no agent↔side binding field, and no
//! auto-join (every engagement is born `pending` and only an operator verdict
//! moves it). So the two families land as parity gaps; the ONE behaviour they
//! share with native — an unknown engagement answers 404 on the verdict path,
//! not a budget refusal — is asserted green.
mod common;
use common::*;
use hagency_store::{DomainRepository, Error};

struct Fixture {
    // Keeps the state directory alive for the test.
    #[allow(dead_code)]
    root: tempfile::TempDir,
    db: DomainRepository,
    engagement: String,
}

fn open() -> Fixture {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    let pool = resource("side_pool", "side_seat", 1000);
    db.put_resource(&pool).unwrap();
    let proof = proof(&request("side_request", "SideWorker", &pool, 100));
    let engagement = db.admit(&proof, 1000).unwrap().id;
    Fixture {
        root,
        db,
        engagement,
    }
}

/// TS: `a verdict on an unknown engagement still answers 404, not a budget
/// refusal` (`tests/api-engagement-side-budget.test.js`, `:211-303`).
///
/// The one thing both worlds agree on: a verdict about an engagement that does
/// not exist is NOT FOUND, never a budget judgement about it.
#[test]
fn ts_oracle_side_budget_unknown_engagement_is_not_found_not_a_budget_refusal() {
    let fixture = open();
    // The known engagement resolves...
    assert_eq!(
        fixture.db.get(&fixture.engagement).unwrap().id,
        fixture.engagement
    );
    // ...and an unknown one is NotFound, so the route maps it to 404.
    assert!(matches!(
        fixture.db.get("engagement_does_not_exist"),
        Err(Error::NotFound)
    ));
}

/// TS: `AUTO-JOIN over the allocation is refused, and nothing is created`,
/// `within the allocation it auto-joins, and the side shows the commitment`,
/// `the SECOND auto-join sees what the first committed`,
/// `UNALLOCATED IS NOT UNLIMITED`, `zero is a real allocation`,
/// `THE LIMITATION, ASSERTED: a room on a server with no side record is not
/// gated`, `an approval that allocates MORE than was asked is checked against
/// the side`.
///
/// PARITY GAP: native has no per-side allocation and no auto-join. Every
/// engagement is born `pending` (`domain.rs::admit`) and admission is gated by
/// the RESOURCE's ceiling, not a side's allocation.
#[test]
#[ignore = "parity gap: no per-side allocation and no auto-join natively (admission gates on the resource ceiling)"]
fn ts_oracle_side_budget_auto_join_and_side_allocation() {
    panic!("native has no side allocation and never auto-joins");
}

/// TS: the alarm family (`tests/api-engagement-side-budget.test.js:303`) —
/// `a refusal raises a WARNING, not an alert the store quietly downgrades to
/// info`, `it names the side, the shortfall and the remedy`, `an UNALLOCATED
/// side alarms too`, `a retrying borrower produces ONE alert with a count`,
/// `raising the allocation enough RESOLVES it`, `TWO sides in trouble are TWO
/// alerts`, ...
///
/// PARITY GAP: native raises exactly one alert kind, `agent_ceiling_overrun`
/// (ADR-124); there is no side-budget refusal to alarm on and no
/// `side_over_budget` kind.
#[test]
#[ignore = "parity gap: native raises only agent_ceiling_overrun; no side-budget alert kind"]
fn ts_oracle_side_budget_alarm_kind_and_dedupe() {
    panic!("native has no side-budget alert kind");
}

/// TS: `an operator can bind, and the record shows it`, `the response says
/// what to do next`, `a side nobody configured is refused`, `null unbinds`,
/// `an unknown agent is a 404`, `an agent token cannot bind an agent to a
/// side`, `PATCH REFUSES projectSide rather than ignoring it`, `the snake_case
/// spelling is refused by PATCH too`, `PATCH still works for the fields it does
/// own`, `minting refuses before the binding and gets past that refusal after
/// it` (`tests/api-agent-project-side-binding.test.js`).
///
/// PARITY GAP: native carries no `projectSide` field on any agent record and
/// has no `PUT /api/agents/:name/project-side` route, so there is nothing to
/// bind, refuse or unbind. (The native agent routes own start/stop/preset/
/// refuse and the stopped-dispatch family, none of which touches a side.)
#[test]
#[ignore = "parity gap: native has no projectSide field and no agent project-side binding route"]
fn ts_oracle_agent_project_side_binding_family() {
    panic!("native has no agent<->side binding");
}
