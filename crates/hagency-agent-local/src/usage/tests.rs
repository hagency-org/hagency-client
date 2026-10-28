use super::*;
use crate::{Budget, Layer, RequestPolicy, ToolPolicy};
const OWNER: &str = "@alice:example.org";
fn scope(agent: &str, binding: &str, requester: &str) -> Scope {
    Scope {
        agent: agent.into(),
        binding: binding.into(),
        requester: requester.into(),
        room: format!("!{binding}:example.org"),
        thread: "main".into(),
    }
}
fn setup() -> (Ledger, Scope) {
    let mut l = Ledger::open(":memory:", OWNER).unwrap();
    let s = scope("a", "b", OWNER);
    l.register_binding(OWNER, &s).unwrap();
    (l, s)
}
fn policy(limit: u64) -> Policy {
    Policy {
        budget: Budget {
            limit: Limit::Tokens(limit),
            period: Period::Lifetime,
        },
        requests: RequestPolicy::Allow,
        high_risk: ToolPolicy::Deny,
    }
}
fn query() -> Query {
    Query {
        agent: "a".into(),
        binding: None,
        requester: None,
        from: 0,
        until: 40 * 86400,
        group: Group::Day,
        alerts_before: None,
    }
}
fn usage() -> Usage {
    Usage {
        input: 60,
        output: 20,
        cached_input: 30,
        reasoning_output: 7,
        accounting_version: "fixture-v1".into(),
    }
}
#[test]
fn daily_monthly_history_is_scoped_and_cache_is_not_double_counted() {
    let (mut l, s) = setup();
    l.reserve(&s, "c1", "d1", 100, 86399).unwrap();
    assert!(matches!(
        l.reserve(&s, "c1", "d1", 100, 86400).unwrap(),
        crate::Reservation::AlreadyPending
    ));
    l.settle(&s, "c1", &usage()).unwrap();
    l.reserve(&s, "pending", "d2", 25, 86400).unwrap();
    let other = scope("a", "other", "@bob:example.org");
    l.register_binding(OWNER, &other).unwrap();
    l.reserve(&other, "c3", "d3", 100, 86401).unwrap();
    l.settle(&other, "c3", &usage()).unwrap();
    let foreign = scope("foreign", "other-agent", OWNER);
    l.register_binding(OWNER, &foreign).unwrap();
    l.reserve(&foreign, "c4", "d4", 100, 86400).unwrap();
    let report = l.usage_report(OWNER, &query(), 100000).unwrap();
    assert_eq!(report.totals.input, 120);
    assert_eq!(report.totals.output, 40);
    assert_eq!(report.totals.cached_input, 60);
    assert_eq!(report.totals.reasoning_output, 14);
    assert_eq!(report.totals.calls, 3);
    assert_eq!(report.totals.pending_calls, 1);
    assert_eq!(report.totals.reserved, 25);
    assert_eq!(report.buckets[0].window, "1970-01-01");
    assert_eq!(report.buckets[1].window, "1970-01-02");
    let mut q = query();
    q.group = Group::Month;
    let monthly = l.usage_report(OWNER, &q, 100000).unwrap();
    assert_eq!(monthly.buckets.len(), 1);
    assert_eq!(monthly.buckets[0].window, "1970-01");
    assert_eq!(monthly.totals, report.totals);
    q.binding = Some("b".into());
    assert_eq!(l.usage_report(OWNER, &q, 100000).unwrap().totals.calls, 2);
    q.requester = Some("@bob:example.org".into());
    assert_eq!(l.usage_report(OWNER, &q, 100000).unwrap().totals.calls, 0);
    q.binding = None;
    assert_eq!(l.usage_report(OWNER, &q, 100000).unwrap().totals.calls, 1);
    q = query();
    q.from = 86400;
    q.until = 86401;
    let exact = l.usage_report(OWNER, &q, 100000).unwrap();
    assert_eq!(exact.totals.calls, 1);
    assert_eq!(exact.totals.input, 0);
    assert!(l.usage_report("@bob:example.org", &q, 100000).is_err());
}
#[test]
fn legacy_dates_stay_unknown_and_late_genuine_usage_replaces_provisional_zero() {
    let (mut l, s) = setup();
    l.reserve(&s, "legacy", "old", 100, 100).unwrap();
    l.settle(&s, "legacy", &usage()).unwrap();
    l.db.execute_batch("DROP TABLE call_observations").unwrap();
    l.db.execute_batch(SCHEMA).unwrap();
    l.reserve(&s, "unknown", "new", 100, 200).unwrap();
    l.mark_unknown(&s, "unknown").unwrap();
    let report = l.usage_report(OWNER, &query(), 300).unwrap();
    assert_eq!(report.undated.calls, 1);
    assert_eq!(report.undated.input, 60);
    assert_eq!(report.totals.unreported_calls, 1);
    assert_eq!(report.totals.reserved, 0);
    l.settle(&s, "unknown", &usage()).unwrap();
    l.settle(&s, "unknown", &usage()).unwrap();
    let report = l.usage_report(OWNER, &query(), 300).unwrap();
    assert_eq!(report.totals.unreported_calls, 0);
    assert_eq!(report.totals.input, 60);
    assert_eq!(report.totals.calls, 1);
}
#[test]
fn threshold_alerts_are_durable_deduplicated_and_acknowledgement_preserves_cost() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("usage.sqlite");
    let mut l = Ledger::open(&path, OWNER).unwrap();
    let s = scope("a", "b", OWNER);
    l.register_binding(OWNER, &s).unwrap();
    l.set_agent_policy(OWNER, "a", 0, &policy(100)).unwrap();
    l.reserve(&s, "c", "d", 80, 100).unwrap();
    let report = l.usage_report(OWNER, &query(), 101).unwrap();
    assert_eq!(report.alerts.len(), 1);
    assert_eq!(report.alerts[0].threshold, 80);
    let id = report.alerts[0].id.clone();
    l.acknowledge_usage_alert(OWNER, "a", &id, 102).unwrap();
    l.acknowledge_usage_alert(OWNER, "a", &id, 103).unwrap();
    assert!(
        l.acknowledge_usage_alert(OWNER, "another", &id, 104)
            .is_err()
    );
    let above = Usage {
        input: 100,
        output: 20,
        ..usage()
    };
    l.settle(&s, "c", &above).unwrap();
    drop(l);
    let mut l = Ledger::open(path, OWNER).unwrap();
    let report = l.usage_report(OWNER, &query(), 105).unwrap();
    assert_eq!(report.alerts.len(), 2);
    assert_eq!(
        report
            .alerts
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .acknowledged_at,
        Some(102)
    );
    assert_eq!(report.totals.input + report.totals.output, 120);
    assert_eq!(
        l.agent_account(OWNER, "a", Period::Lifetime, 105).unwrap(),
        (120, 0)
    );
    l.set_agent_policy(OWNER, "a", 1, &policy(120)).unwrap();
    let report = l.usage_report(OWNER, &query(), 106).unwrap();
    assert_eq!(report.alerts.len(), 4);
    assert_eq!(
        report
            .alerts
            .iter()
            .filter(|a| a.revision == 2 && a.acknowledged_at.is_none())
            .count(),
        2
    );
    assert_eq!(
        l.usage_report(OWNER, &query(), 107).unwrap().alerts.len(),
        4
    );
}
#[test]
fn report_limits_and_zero_budget_have_explicit_behavior() {
    let (mut l, s) = setup();
    l.set_agent_policy(OWNER, "a", 0, &policy(0)).unwrap();
    let report = l.usage_report(OWNER, &query(), 100).unwrap();
    assert_eq!(report.alerts.len(), 1);
    assert_eq!(report.alerts[0].threshold, 100);
    assert!(l.reserve(&s, "c", "d", 1, 100).is_err());
    for (from, until) in [(-1, 10), (10, 10), (0, 367 * 86400)] {
        let mut q = query();
        q.from = from;
        q.until = until;
        assert!(l.usage_report(OWNER, &q, 100).is_err());
    }
    let mut q = query();
    q.until = 367 * 86400;
    q.group = Group::Month;
    assert!(l.usage_report(OWNER, &q, 100).is_ok());
    l.set_policy(OWNER, &s, Layer::Requester, 0, &policy(0))
        .unwrap();
    q.requester = Some(OWNER.into());
    assert_eq!(l.usage_report(OWNER, &q, 100).unwrap().alerts.len(), 1);
}

#[test]
fn alert_history_pages_and_immutable_evidence_survive_acknowledgement() {
    let (mut l, s) = setup();
    l.reserve(&s, "c", "d", 100, 100).unwrap();
    l.settle(&s, "c", &usage()).unwrap();
    for revision in 0..60 {
        l.set_agent_policy(OWNER, "a", revision, &policy(80))
            .unwrap();
        l.usage_report(OWNER, &query(), 101 + revision).unwrap();
    }
    let first = l.usage_report(OWNER, &query(), 200).unwrap();
    assert_eq!(first.alerts.len(), 100);
    assert!(first.alerts_truncated);
    let mut q = query();
    q.alerts_before = first.alerts_next.clone();
    let older = l.usage_report(OWNER, &q, 201).unwrap();
    assert_eq!(older.alerts.len(), 20);
    assert!(!older.alerts_truncated);
    assert!(older.alerts_next.is_none());
    assert!(
        older
            .alerts
            .iter()
            .all(|a| first.alerts.iter().all(|b| a.id != b.id))
    );
    q.agent = "other".into();
    assert!(l.usage_report(OWNER, &q, 202).is_err());
    let id = &first.alerts[0].id;
    l.acknowledge_usage_alert(OWNER, "a", id, 203).unwrap();
    assert!(
        l.db.execute("DELETE FROM usage_alerts WHERE id=?", [id])
            .is_err()
    );
    assert!(
        l.db.execute(
            "UPDATE usage_alerts SET acknowledged_at=NULL WHERE id=?",
            [id]
        )
        .is_err()
    );
    assert!(
        l.db.execute("UPDATE call_observations SET started_at=999", [])
            .is_err()
    );
}

#[test]
fn late_settlement_alert_uses_original_budget_window_after_utc_rollover() {
    let (mut l, s) = setup();
    let mut p = policy(100);
    p.budget.period = Period::UtcDay;
    l.set_agent_policy(OWNER, "a", 0, &p).unwrap();
    l.reserve(&s, "before", "d1", 80, 86399).unwrap();
    l.settle(
        &s,
        "before",
        &Usage {
            input: 100,
            output: 20,
            ..usage()
        },
    )
    .unwrap();
    l.reserve(&s, "after", "d2", 20, 86400).unwrap();
    let report = l.usage_report(OWNER, &query(), 86401).unwrap();
    assert_eq!(report.alerts.len(), 2);
    assert!(report.alerts.iter().all(|a| a.window == "day:1970-1-1"));
    assert_eq!(
        l.agent_account(OWNER, "a", Period::UtcDay, 86401).unwrap(),
        (0, 20)
    );
    assert_eq!(
        report.buckets[0].totals.input + report.buckets[0].totals.output,
        120
    );
    assert_eq!(report.buckets[1].totals.reserved, 20);
}

#[test]
fn no_provider_deduplication_receipts_are_not_counted_as_model_calls() {
    let (mut l, s) = setup();
    let proof = Usage {
        input: 0,
        output: 0,
        cached_input: 0,
        reasoning_output: 0,
        accounting_version: "no-provider-call-v1".into(),
    };
    l.db.execute(
        "INSERT INTO calls VALUES ('no-provider','b',?,'proof',0,'[]','settled',?)",
        (crate::json(&s).unwrap(), crate::json(&proof).unwrap()),
    )
    .unwrap();
    let report = l.usage_report(OWNER, &query(), 200).unwrap();
    assert_eq!(report.totals.calls, 0);
    assert_eq!(report.undated.calls, 0);
    l.reserve(&s, "real-zero", "d", 1, 100).unwrap();
    l.settle(
        &s,
        "real-zero",
        &Usage {
            accounting_version: "reported-zero-v1".into(),
            ..proof
        },
    )
    .unwrap();
    assert_eq!(
        l.usage_report(OWNER, &query(), 200).unwrap().totals.calls,
        1
    );
}
