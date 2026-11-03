use hagency_metering::TokenCounts;
use hagency_metering::dashboard::collector::{
    DashboardCollector, ResourceRegistration, SnapshotOptions,
};
use hagency_metering::dashboard::ledger::{
    CountMode, CountScope, ExecutionBinding, GroupBy, IngestOutcome, LedgerError, LedgerRecord,
    Runtime, UsageEvent, UsageLedger, UsageQuery, UsageSource,
};
use hagency_metering::dashboard::period::{
    TimeRange, UsagePeriod, civil_from_days, days_from_civil, parse_rfc3339_ms,
};
use hagency_metering::dashboard::quota::{
    AccountIdentity, AccountRegistration, AdapterWindow, ClaudeRateLimitInfo,
    ClaudeRateLimitStatus, ClaudeStatusLineRateLimits, ClaudeStatusWindow, ClaudeUsageLimit,
    ClaudeUsageReportRateLimits, CodexRateLimitSnapshot, CodexRateLimitWindow, CodexRateLimitsRead,
    CodexRateLimitsUpdated, QuotaBook, QuotaCapability, QuotaError, RefreshPolicy,
};
use hagency_metering::dashboard::{
    AccountState, Activity, ActivityState, DASHBOARD_API_VERSION, DashboardSnapshot, HostingState,
    HostingStatus, OwnerIdentity, ResourceState, UNKNOWN_COUNTS, cache_hit_ratio,
};
use hagency_metering::observation::UsageObservation;
use hagency_metering::octos_usage::{Coverage as OctosCoverage, OctosUsage};
use hagency_metering::runtime_usage::{CodexUsage, CounterBreakdown, ProjectionDiagnostics};
use serde_json::json;

// 2026-11-02T00:00:00Z (a Monday).
const MONDAY: u64 = 1_793_577_600_000;
const HOUR: u64 = 3_600_000;
const MIN: u64 = 60_000;

fn counts(input: u64, output: u64, cache_write: u64, cache_read: u64) -> TokenCounts {
    TokenCounts {
        input: Some(input),
        output: Some(output),
        cache_write: Some(cache_write),
        cache_read: Some(cache_read),
    }
}

fn binding(execution: &str, resource: &str, account: Option<&str>) -> ExecutionBinding {
    ExecutionBinding {
        execution_id: execution.into(),
        resource_id: resource.into(),
        account_id: account.map(Into::into),
        allocation_id: Some("alloc-1".into()),
        agent_id: Some(format!("agent-{resource}")),
        requester_id: Some("@alice:example.org".into()),
        room_id: Some("!room:example.org".into()),
        runtime: Runtime::Codex,
        runtime_version: Some("0.200.0".into()),
        model: Some("gpt-5.5".into()),
        parent_execution_id: None,
        started_at_ms: MONDAY + HOUR,
    }
}

fn cumulative(
    execution: &str,
    epoch: &str,
    sequence: Option<u64>,
    at: u64,
    c: TokenCounts,
) -> UsageEvent {
    UsageEvent {
        execution_id: execution.into(),
        stream: "thread-1".into(),
        event_id: None,
        sequence,
        observed_at_ms: at,
        measured_at_ms: None,
        mode: CountMode::Cumulative {
            epoch: epoch.into(),
        },
        scope: CountScope::Own,
        counts: c,
        reasoning_output: None,
        complete: true,
        source: UsageSource::CodexRuntime,
    }
}

fn delta(execution: &str, id: Option<&str>, at: u64, c: TokenCounts) -> UsageEvent {
    UsageEvent {
        execution_id: execution.into(),
        stream: "otel".into(),
        event_id: id.map(Into::into),
        sequence: None,
        observed_at_ms: at,
        measured_at_ms: None,
        mode: CountMode::Delta,
        scope: CountScope::Own,
        counts: c,
        reasoning_output: None,
        complete: true,
        source: UsageSource::ClaudeTelemetry,
    }
}

fn day() -> UsageQuery {
    UsageQuery::range(UsagePeriod::Day.resolve(MONDAY + 12 * HOUR, 0).unwrap())
}

fn ledger_with(executions: &[ExecutionBinding]) -> UsageLedger {
    let mut ledger = UsageLedger::new(MONDAY - 30 * 24 * HOUR);
    for binding in executions {
        ledger.bind_execution(binding.clone()).unwrap();
    }
    ledger
}

// ---------------------------------------------------------------- DTO contract

#[test]
fn default_snapshot_is_unconfigured_and_unknown_not_ready() {
    let snapshot = DashboardSnapshot::default();
    assert_eq!(snapshot.version, DASHBOARD_API_VERSION);
    assert_eq!(snapshot.hosting.state, HostingState::Unconfigured);
    assert_eq!(snapshot.usage.counts, UNKNOWN_COUNTS);
    assert!(!snapshot.usage.complete);
    assert!(snapshot.usage.estimated_cost_usd.is_none());
    assert!(snapshot.accounts.is_empty() && snapshot.resources.is_empty());
    assert!(snapshot.is_compatible());
    let value = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(
        value,
        json!({
            "version": 1, "revision": 0, "observedAtMs": 0, "owner": null,
            "inventoryAvailable": false, "inventoryComplete": false,
            "hosting": {"state": "unconfigured", "detail": null, "activeRuns": null, "pendingApprovals": null},
            "accounts": [], "resources": [],
            "usage": {"counts": {"input": null, "output": null, "cacheWrite": null, "cacheRead": null},
                      "complete": false, "observedAtMs": null, "estimatedCostUsd": null},
            "activities": []
        })
    );
}

#[test]
fn dto_round_trips_with_camel_case_fields_and_snake_case_enums() {
    let mut collector = DashboardCollector::new(ledger_with(&[]), RefreshPolicy::default());
    collector.set_owner(
        Some(OwnerIdentity {
            user_id: "@owner:example.org".into(),
            display_name: "Owner".into(),
            server: "example.org".into(),
        }),
        || UsageLedger::new(MONDAY - 24 * HOUR),
    );
    collector.set_hosting(HostingStatus {
        state: HostingState::Draining,
        detail: Some("2 runs finishing".into()),
        active_runs: Some(2),
        pending_approvals: Some(1),
    });
    let id = collector
        .quotas_mut()
        .register(verified("openai", "acct-1"))
        .unwrap();
    collector
        .quotas_mut()
        .apply_codex_read(&id, MONDAY, &codex_read(25.0, 300, 10_080))
        .unwrap();
    collector
        .record_activity(Activity {
            id: "a1".into(),
            at_ms: MONDAY,
            title: "Approval needed".into(),
            detail: "Run tests".into(),
            state: ActivityState::Pending.as_str().into(),
        })
        .unwrap();
    let snapshot = collector
        .snapshot(MONDAY + MIN, SnapshotOptions::default())
        .unwrap();
    let text = serde_json::to_string(&snapshot).unwrap();
    for key in [
        "observedAtMs",
        "activeRuns",
        "pendingApprovals",
        "quotaWindows",
        "usedPercent",
        "resetsAtMs",
        "windowMinutes",
        "\"draining\"",
        "displayName",
        "userId",
    ] {
        assert!(text.contains(key), "missing {key} in {text}");
    }
    let back: DashboardSnapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(back, snapshot);
}

#[test]
fn state_vocabularies_parse_and_reject_unknown_values() {
    for state in AccountState::ALL {
        assert_eq!(AccountState::parse(state.as_str()), Some(*state));
        assert_eq!(serde_json::to_value(state).unwrap(), json!(state.as_str()));
    }
    for state in ResourceState::ALL {
        assert_eq!(ResourceState::parse(state.as_str()), Some(*state));
    }
    assert_eq!(
        ActivityState::parse("uncertain"),
        Some(ActivityState::Uncertain)
    );
    assert_eq!(AccountState::parse("ready"), None);
    for state in [
        "unconfigured",
        "starting",
        "running",
        "paused",
        "draining",
        "stopped",
        "offline",
        "failed",
    ] {
        let parsed: HostingState = serde_json::from_value(json!(state)).unwrap();
        assert_eq!(parsed.as_str(), state);
    }
}

#[test]
fn cache_hit_ratio_uses_aggregated_categories_and_is_unavailable_when_unknown() {
    assert_eq!(cache_hit_ratio(&counts(20, 99, 10, 70)), Some(0.7));
    assert_eq!(
        cache_hit_ratio(&counts(0, 5, 0, 0)),
        None,
        "zero denominator"
    );
    let mut partial = counts(20, 5, 10, 70);
    partial.cache_write = None;
    assert_eq!(cache_hit_ratio(&partial), None);
    assert_eq!(cache_hit_ratio(&UNKNOWN_COUNTS), None);
    // Ratio of sums, not mean of ratios.
    let b = binding("e1", "r1", None);
    let mut ledger = ledger_with(&[b]);
    ledger
        .ingest(&delta(
            "e1",
            Some("x"),
            MONDAY + 2 * HOUR,
            counts(90, 1, 0, 10),
        ))
        .unwrap();
    ledger
        .ingest(&delta(
            "e1",
            Some("y"),
            MONDAY + 2 * HOUR,
            counts(0, 1, 0, 100),
        ))
        .unwrap();
    let total = ledger.aggregate(&day());
    assert_eq!(total.cache_hit_ratio(), Some(110.0 / 200.0));
}

// ---------------------------------------------------------------- periods

#[test]
fn calendar_periods_follow_local_offset() {
    assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    let wednesday_noon = MONDAY + 2 * 24 * HOUR + 12 * HOUR;
    let day = UsagePeriod::Day.resolve(wednesday_noon, 0).unwrap();
    assert_eq!(
        day,
        TimeRange::new(MONDAY + 48 * HOUR, MONDAY + 72 * HOUR).unwrap()
    );
    let week = UsagePeriod::Week.resolve(wednesday_noon, 0).unwrap();
    assert_eq!(
        week,
        TimeRange::new(MONDAY, MONDAY + 7 * 24 * HOUR).unwrap()
    );
    // UTC+08:00: Monday 00:00Z is Monday 08:00 local, so local day began 8h earlier.
    let shanghai = UsagePeriod::Day.resolve(MONDAY, 480).unwrap();
    assert_eq!(shanghai.start_ms, MONDAY - 8 * HOUR);
    // UTC-05:00: Monday 00:00Z is Sunday 19:00 local; ISO week began the previous Monday.
    let new_york = UsagePeriod::Week.resolve(MONDAY, -300).unwrap();
    assert_eq!(new_york.start_ms, MONDAY - 7 * 24 * HOUR + 5 * HOUR);
    let month = UsagePeriod::Month.resolve(MONDAY, 0).unwrap();
    assert_eq!(
        month.start_ms,
        days_from_civil(2026, 11, 1) as u64 * 24 * HOUR
    );
    assert_eq!(
        month.end_ms,
        days_from_civil(2026, 12, 1) as u64 * 24 * HOUR
    );
    let december = UsagePeriod::Month
        .resolve(days_from_civil(2026, 12, 15) as u64 * 24 * HOUR, 0)
        .unwrap();
    assert_eq!(
        december.end_ms,
        days_from_civil(2027, 1, 1) as u64 * 24 * HOUR
    );
    assert!(
        UsagePeriod::Range {
            start_ms: 5,
            end_ms: 5
        }
        .resolve(0, 0)
        .is_err()
    );
    assert!(UsagePeriod::Day.resolve(MONDAY, 24 * 60).is_err());
    let period: UsagePeriod =
        serde_json::from_value(json!({"kind": "range", "startMs": 1, "endMs": 2})).unwrap();
    assert_eq!(
        period,
        UsagePeriod::Range {
            start_ms: 1,
            end_ms: 2
        }
    );
}

#[test]
fn rfc3339_parsing_requires_offsets() {
    assert_eq!(parse_rfc3339_ms("2026-11-02T00:00:00Z"), Some(MONDAY));
    assert_eq!(
        parse_rfc3339_ms("2026-11-02T08:00:00.250+08:00"),
        Some(MONDAY + 250)
    );
    assert_eq!(parse_rfc3339_ms("2026-11-02T00:00:00"), None);
    assert_eq!(parse_rfc3339_ms("2026-02-30T00:00:00Z"), None);
    assert_eq!(parse_rfc3339_ms("not a timestamp at all"), None);
}

// ---------------------------------------------------------------- ledger

#[test]
fn usage_requires_a_trusted_binding_and_bindings_cannot_be_rewritten() {
    let mut ledger = ledger_with(&[]);
    let event = cumulative("e1", "p1", Some(1), MONDAY + 2 * HOUR, counts(1, 1, 0, 0));
    assert_eq!(ledger.ingest(&event), Err(LedgerError::UnknownExecution));
    let original = binding("e1", "terra", Some("openai:abc"));
    assert!(ledger.bind_execution(original.clone()).unwrap());
    assert!(!ledger.bind_execution(original.clone()).unwrap());
    let mut moved = original;
    moved.resource_id = "astra".into();
    assert_eq!(
        ledger.bind_execution(moved),
        Err(LedgerError::BindingConflict)
    );
    let mut bad = binding("", "r", None);
    assert_eq!(
        ledger.bind_execution(bad.clone()),
        Err(LedgerError::InvalidIdentifier)
    );
    bad.execution_id = "self".into();
    bad.parent_execution_id = Some("self".into());
    assert_eq!(
        ledger.bind_execution(bad),
        Err(LedgerError::InvalidIdentifier)
    );
}

#[test]
fn cumulative_snapshots_dedup_out_of_order_and_restart_epochs() {
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    let t = MONDAY + 2 * HOUR;
    assert_eq!(
        ledger
            .ingest(&cumulative("e1", "p1", Some(1), t, counts(100, 10, 0, 50)))
            .unwrap(),
        IngestOutcome::Applied
    );
    assert_eq!(
        ledger
            .ingest(&cumulative(
                "e1",
                "p1",
                Some(3),
                t + MIN,
                counts(300, 30, 0, 150)
            ))
            .unwrap(),
        IngestOutcome::Applied
    );
    // Retry of the same sequence: duplicate.
    assert_eq!(
        ledger
            .ingest(&cumulative(
                "e1",
                "p1",
                Some(3),
                t + MIN,
                counts(300, 30, 0, 150)
            ))
            .unwrap(),
        IngestOutcome::Duplicate
    );
    // Late, older sequence already covered: stale, no double count.
    assert_eq!(
        ledger
            .ingest(&cumulative(
                "e1",
                "p1",
                Some(2),
                t + 2 * MIN,
                counts(200, 20, 0, 100)
            ))
            .unwrap(),
        IngestOutcome::Stale
    );
    // Same sequence, different counts: conflict, first kept.
    assert_eq!(
        ledger
            .ingest(&cumulative("e1", "p1", Some(3), t, counts(999, 30, 0, 150)))
            .unwrap(),
        IngestOutcome::Conflict
    );
    let total = ledger.aggregate(&day());
    assert_eq!(total.counts, counts(300, 30, 0, 150));
    assert_eq!(total.diagnostics.duplicates, 1);
    assert_eq!(total.diagnostics.stale, 1);
    assert_eq!(total.diagnostics.conflicts, 1);
    assert!(!total.complete, "conflict makes the measurement partial");

    // A restarted runtime process starts a new epoch from zero.
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    ledger
        .ingest(&cumulative("e1", "p1", Some(1), t, counts(100, 10, 0, 50)))
        .unwrap();
    ledger
        .ingest(&cumulative(
            "e1",
            "p2",
            Some(1),
            t + MIN,
            counts(40, 4, 0, 0),
        ))
        .unwrap();
    ledger
        .ingest(&cumulative(
            "e1",
            "p2",
            Some(2),
            t + 2 * MIN,
            counts(60, 6, 0, 10),
        ))
        .unwrap();
    let total = ledger.aggregate(&day());
    assert_eq!(total.counts, counts(160, 16, 0, 60));
    assert!(total.complete);
}

#[test]
fn regressed_cumulative_counters_keep_lower_bound_and_are_partial() {
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    let t = MONDAY + 2 * HOUR;
    ledger
        .ingest(&cumulative("e1", "p1", Some(1), t, counts(500, 50, 0, 0)))
        .unwrap();
    // Counter reset without a new epoch: never subtracts, flags regression.
    assert_eq!(
        ledger
            .ingest(&cumulative(
                "e1",
                "p1",
                Some(2),
                t + MIN,
                counts(20, 2, 0, 0)
            ))
            .unwrap(),
        IngestOutcome::Applied
    );
    ledger
        .ingest(&cumulative(
            "e1",
            "p1",
            Some(3),
            t + 2 * MIN,
            counts(600, 55, 0, 0),
        ))
        .unwrap();
    let total = ledger.aggregate(&day());
    assert_eq!(total.counts, counts(600, 55, 0, 0));
    assert_eq!(total.diagnostics.regressions, 1);
    assert!(!total.complete);
    // Without sequences, an identical running total is a duplicate.
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    ledger
        .ingest(&cumulative("e1", "p1", None, t, counts(5, 5, 0, 0)))
        .unwrap();
    assert_eq!(
        ledger
            .ingest(&cumulative("e1", "p1", None, t + MIN, counts(5, 5, 0, 0)))
            .unwrap(),
        IngestOutcome::Duplicate
    );
    assert_eq!(ledger.aggregate(&day()).counts, counts(5, 5, 0, 0));
}

#[test]
fn missing_categories_stay_unknown_and_are_never_zero() {
    let t = MONDAY + 2 * HOUR;
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    let mut c = counts(10, 5, 0, 3);
    c.cache_write = None;
    ledger
        .ingest(&cumulative("e1", "p1", Some(1), t, c))
        .unwrap();
    let total = ledger.aggregate(&day());
    assert_eq!(total.counts.cache_write, None);
    assert_eq!(total.counts.input, Some(10));
    assert_eq!(total.cache_hit_ratio(), None);
    assert!(!total.complete);

    // A later cumulative snapshot that omits a known category keeps the
    // known lower bound but marks the stream partial.
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    ledger
        .ingest(&cumulative("e1", "p1", Some(1), t, counts(10, 5, 0, 3)))
        .unwrap();
    let mut omitted = counts(20, 8, 0, 3);
    omitted.output = None;
    ledger
        .ingest(&cumulative("e1", "p1", Some(2), t + MIN, omitted))
        .unwrap();
    let total = ledger.aggregate(&day());
    assert_eq!(total.counts, counts(20, 5, 0, 3));
    assert_eq!(total.diagnostics.missing_after_known, 1);
    assert!(!total.complete);

    // Delta streams: an unknown category is unknown for every aggregate containing it.
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    ledger
        .ingest(&delta("e1", Some("a"), t, counts(1, 1, 1, 1)))
        .unwrap();
    let mut gap = counts(1, 1, 1, 1);
    gap.cache_read = None;
    ledger.ingest(&delta("e1", Some("b"), t, gap)).unwrap();
    assert_eq!(ledger.aggregate(&day()).counts.cache_read, None);
    assert_eq!(ledger.aggregate(&day()).counts.input, Some(2));

    // A bound execution with no measurement is not zero.
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    let total = ledger.aggregate(&day());
    assert_eq!(total.unmeasured_executions, 1);
    assert!(!total.complete);
    assert!(!total.measured());
    ledger
        .ingest(&delta("e1", Some("a"), t, counts(1, 1, 0, 0)))
        .unwrap();
    assert!(ledger.aggregate(&day()).complete);
}

#[test]
fn delta_events_dedup_by_identity_and_flag_undeduplicable() {
    let t = MONDAY + 2 * HOUR;
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    assert_eq!(
        ledger
            .ingest(&delta("e1", Some("req-1"), t, counts(10, 1, 0, 0)))
            .unwrap(),
        IngestOutcome::Applied
    );
    assert_eq!(
        ledger
            .ingest(&delta("e1", Some("req-1"), t, counts(10, 1, 0, 0)))
            .unwrap(),
        IngestOutcome::Duplicate
    );
    assert_eq!(
        ledger
            .ingest(&delta("e1", Some("req-1"), t, counts(11, 1, 0, 0)))
            .unwrap(),
        IngestOutcome::Conflict
    );
    // Out-of-order arrival of a distinct event is fine for deltas.
    assert_eq!(
        ledger
            .ingest(&delta("e1", Some("req-0"), t - MIN, counts(5, 1, 0, 0)))
            .unwrap(),
        IngestOutcome::Applied
    );
    let total = ledger.aggregate(&day());
    assert_eq!(total.counts, counts(15, 2, 0, 0));
    assert!(!total.complete);
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    ledger
        .ingest(&delta("e1", None, t, counts(10, 1, 0, 0)))
        .unwrap();
    ledger
        .ingest(&delta("e1", None, t, counts(10, 1, 0, 0)))
        .unwrap();
    let total = ledger.aggregate(&day());
    assert_eq!(
        total.counts.input,
        Some(20),
        "cannot prove a retry; counted but flagged"
    );
    assert_eq!(total.diagnostics.undeduplicable, 2);
    assert!(!total.complete);
    // Mixing cumulative and delta on one stream is refused.
    let mut mixed = cumulative("e1", "p1", Some(1), t, counts(1, 1, 0, 0));
    mixed.stream = "otel".into();
    assert_eq!(ledger.ingest(&mixed), Err(LedgerError::ModeMismatch));
    let mut huge = delta("e1", Some("z"), t, counts(1, 1, 0, 0));
    huge.counts.input = Some(u64::MAX);
    assert_eq!(ledger.ingest(&huge), Err(LedgerError::InvalidCounter));
}

#[test]
fn reasoning_is_a_breakdown_and_subagents_are_not_double_counted() {
    let t = MONDAY + 2 * HOUR;
    let parent = binding("parent", "terra", None);
    let mut child = binding("child", "terra", None);
    child.parent_execution_id = Some("parent".into());
    let mut grandchild = binding("grandchild", "terra", None);
    grandchild.parent_execution_id = Some("child".into());
    let mut ledger = ledger_with(&[parent, child, grandchild]);
    let mut parent_event = cumulative("parent", "p1", Some(1), t, counts(100, 40, 0, 0));
    parent_event.reasoning_output = Some(25);
    ledger.ingest(&parent_event).unwrap();
    ledger
        .ingest(&cumulative("child", "p1", Some(1), t, counts(30, 10, 0, 0)))
        .unwrap();
    let total = ledger.aggregate(&day());
    assert_eq!(
        total.counts,
        counts(130, 50, 0, 0),
        "own-scope parent: child adds"
    );
    assert_eq!(
        total.reasoning_output, None,
        "child did not report reasoning"
    );

    // Parent's totals include subagents: child and grandchild are contained.
    let mut session = cumulative("parent", "p1", Some(2), t + MIN, counts(160, 60, 0, 0));
    session.scope = CountScope::IncludesSubagents;
    session.reasoning_output = Some(30);
    ledger.ingest(&session).unwrap();
    ledger
        .ingest(&cumulative(
            "grandchild",
            "p1",
            Some(1),
            t,
            counts(7, 7, 0, 0),
        ))
        .unwrap();
    let total = ledger.aggregate(&day());
    assert_eq!(total.counts, counts(160, 60, 0, 0));
    assert_eq!(total.reasoning_output, Some(30));
    assert_eq!(total.contained_executions, 2);
    assert_eq!(total.executions, 1);
    let ceiling = total.counts.ceiling_volume().unwrap();
    assert_eq!(
        ceiling,
        Some(220),
        "existing budget arithmetic: input+output+cache_write"
    );
}

#[test]
fn runtime_observations_convert_with_provenance_and_digest_dedup() {
    let usage = CodexUsage {
        total: CounterBreakdown {
            total_tokens: Some(110),
            input_tokens: Some(100),
            cached_input_tokens: Some(40),
            cache_write_input_tokens: Some(0),
            output_tokens: Some(10),
            reasoning_output_tokens: Some(5),
        },
        last: CounterBreakdown::default(),
        context_window: Some(128_000),
        diagnostics: ProjectionDiagnostics::default(),
    };
    let observation = UsageObservation::codex_runtime(usage).unwrap();
    let event =
        UsageEvent::from_observation(&observation, "e1", "thread-1", "proc-1", MONDAY + 2 * HOUR)
            .unwrap();
    assert_eq!(event.source, UsageSource::CodexRuntime);
    assert_eq!(
        event.counts,
        counts(60, 10, 0, 40),
        "Codex input normalized to fresh input"
    );
    assert_eq!(event.reasoning_output, Some(5));
    assert!(
        !event.complete,
        "runtime evidence is conservatively incomplete"
    );
    assert!(event.event_id.as_deref().unwrap().starts_with("sha256:"));
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    assert_eq!(ledger.ingest(&event).unwrap(), IngestOutcome::Applied);
    let again =
        UsageEvent::from_observation(&observation, "e1", "thread-1", "proc-1", MONDAY + 3 * HOUR)
            .unwrap();
    assert_eq!(ledger.ingest(&again).unwrap(), IngestOutcome::Duplicate);
    let total = ledger.aggregate(&day());
    assert_eq!(total.counts, counts(60, 10, 0, 40));
    assert_eq!(total.sources, vec![UsageSource::CodexRuntime]);
    assert!(!total.complete);

    let octos = UsageObservation::octos_runtime(OctosUsage {
        counts: counts(10, 20, 40, 30),
        reasoning: Some(5),
        coverage: OctosCoverage::Session,
        turns: 2,
        diagnostics: ProjectionDiagnostics::default(),
    })
    .unwrap();
    let event = UsageEvent::from_observation(&octos, "e2", "session", "s1", MONDAY).unwrap();
    assert_eq!(event.scope, CountScope::IncludesSubagents);
    assert_eq!(event.source, UsageSource::OctosRuntime);

    let failed =
        UsageObservation::parse(hagency_metering::Framework::Claude, "{\"a\":1,\"a\":2}").unwrap();
    let event = UsageEvent::from_observation(&failed, "e3", "transcript", "f1", MONDAY).unwrap();
    assert_eq!(event.counts, UNKNOWN_COUNTS);
    assert!(!event.complete);
}

#[test]
fn usage_lands_in_the_local_day_it_was_measured() {
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    // 23:50 and 00:10 local at UTC+08:00 straddle local midnight (16:00Z).
    let midnight = MONDAY + 16 * HOUR;
    ledger
        .ingest(&delta(
            "e1",
            Some("a"),
            midnight - 10 * MIN,
            counts(1, 0, 0, 0),
        ))
        .unwrap();
    let mut late = delta("e1", Some("b"), midnight + 2 * HOUR, counts(2, 0, 0, 0));
    late.measured_at_ms = Some(midnight + 10 * MIN);
    ledger.ingest(&late).unwrap();
    let query =
        UsageQuery::range(TimeRange::new(midnight - 24 * HOUR, midnight + 24 * HOUR).unwrap());
    let series = ledger.daily_series(&query, 480).unwrap();
    assert_eq!(series.len(), 2);
    assert_eq!(series[0].usage.counts.input, Some(1));
    assert_eq!(
        series[1].usage.counts.input,
        Some(2),
        "provider measurement time wins"
    );
    assert_eq!(series[1].day_start_ms, midnight);
}

#[test]
fn shared_verified_account_groups_resources_but_unverified_never_merge() {
    let mut collector = DashboardCollector::new(
        UsageLedger::new(MONDAY - 24 * HOUR),
        RefreshPolicy::default(),
    );
    let shared = collector
        .quotas_mut()
        .register(verified("openai", "acct-shared"))
        .unwrap();
    let again = collector
        .quotas_mut()
        .register(verified("openai", "acct-shared"))
        .unwrap();
    assert_eq!(shared, again);
    assert!(
        !shared.contains("acct-shared"),
        "subject is hashed, not exposed"
    );
    for (id, name) in [("terra", "Terra"), ("astra", "Astra")] {
        collector
            .upsert_resource(resource(id, name, Some(&shared)))
            .unwrap();
    }
    // Two unverified accounts that might be the same person stay separate.
    let lone_a = collector
        .quotas_mut()
        .register(unverified("anthropic", "nova"))
        .unwrap();
    let lone_b = collector
        .quotas_mut()
        .register(unverified("anthropic", "luna"))
        .unwrap();
    assert_ne!(lone_a, lone_b);
    collector
        .upsert_resource(resource("nova", "Nova", Some(&lone_a)))
        .unwrap();
    assert!(
        collector
            .upsert_resource(resource("luna", "Luna", Some(&lone_a)))
            .is_err(),
        "cannot borrow another resource's unverified account"
    );
    assert!(
        collector
            .upsert_resource(resource("x", "X", Some("openai:nope")))
            .is_err()
    );

    let t = MONDAY + 2 * HOUR;
    collector
        .bind_execution(binding("t1", "terra", Some(&shared)))
        .unwrap();
    collector
        .bind_execution(binding("a1", "astra", Some(&shared)))
        .unwrap();
    collector
        .ingest_usage(&cumulative("t1", "p", Some(1), t, counts(10, 1, 0, 90)))
        .unwrap();
    collector
        .ingest_usage(&cumulative("a1", "p", Some(1), t, counts(30, 3, 0, 70)))
        .unwrap();
    let snapshot = collector
        .snapshot(MONDAY + 12 * HOUR, SnapshotOptions::default())
        .unwrap();
    let card: Vec<_> = snapshot
        .accounts
        .iter()
        .filter(|a| a.id == shared)
        .collect();
    assert_eq!(card.len(), 1, "one plan card for the shared account");
    let terra = snapshot.resources.iter().find(|r| r.id == "terra").unwrap();
    let astra = snapshot.resources.iter().find(|r| r.id == "astra").unwrap();
    assert_eq!(terra.counts, counts(10, 1, 0, 90));
    assert_eq!(astra.counts, counts(30, 3, 0, 70));
    assert_eq!(terra.account_id.as_deref(), Some(shared.as_str()));
    assert_eq!(terra.state, "measured");
    assert_eq!(
        snapshot
            .resources
            .iter()
            .find(|r| r.id == "nova")
            .unwrap()
            .state,
        "idle"
    );
    assert_eq!(
        snapshot
            .accounts
            .iter()
            .find(|a| a.id == lone_a)
            .unwrap()
            .state,
        "unverified"
    );
    let by_account = collector
        .ledger()
        .aggregate_by(&UsageQuery::range(TimeRange::ALL), GroupBy::Account);
    assert_eq!(
        by_account[&Some(shared.clone())].counts,
        counts(40, 4, 0, 160)
    );
    assert_eq!(snapshot.usage.counts, counts(40, 4, 0, 160));
    assert_eq!(snapshot.usage.cache_hit_ratio(), Some(0.8));
    assert!(snapshot.usage.complete);
    assert_eq!(
        snapshot.usage.estimated_cost_usd, None,
        "no fabricated cost"
    );
}

// ---------------------------------------------------------------- quota

fn verified(provider: &str, subject: &str) -> AccountRegistration {
    AccountRegistration {
        identity: AccountIdentity::Verified {
            provider: provider.into(),
            subject: subject.into(),
        },
        label: format!("{provider} plan"),
        plan: None,
        capability: QuotaCapability::Supported,
        runtime_version: None,
    }
}

fn unverified(provider: &str, resource: &str) -> AccountRegistration {
    AccountRegistration {
        identity: AccountIdentity::Unverified {
            provider: provider.into(),
            resource_id: resource.into(),
        },
        label: format!("{provider} account"),
        plan: None,
        capability: QuotaCapability::Supported,
        runtime_version: None,
    }
}

fn resource(id: &str, name: &str, account: Option<&str>) -> ResourceRegistration {
    ResourceRegistration {
        id: id.into(),
        name: name.into(),
        account_id: account.map(Into::into),
        provider: "codex".into(),
        model: "gpt-5.5".into(),
        agent_count: 1,
        budget_tokens: Some(1_000_000),
    }
}

fn window(used: f64, minutes: u64, resets_s: f64) -> CodexRateLimitWindow {
    CodexRateLimitWindow {
        used_percent: Some(used),
        window_duration_mins: Some(minutes),
        resets_at: Some(resets_s),
    }
}

fn codex_read(
    primary_used: f64,
    primary_minutes: u64,
    secondary_minutes: u64,
) -> CodexRateLimitsRead {
    let reset = (MONDAY / 1000 + 3600) as f64;
    CodexRateLimitsRead {
        rate_limits: Some(CodexRateLimitSnapshot {
            limit_id: Some("codex".into()),
            primary: Some(window(primary_used, primary_minutes, reset)),
            secondary: Some(window(10.0, secondary_minutes, reset + 86_400.0)),
            plan_type: Some("pro".into()),
            ..CodexRateLimitSnapshot::default()
        }),
        rate_limits_by_limit_id: None,
    }
}

#[test]
fn codex_rate_limits_read_and_updated_project_provider_windows() {
    let mut book = QuotaBook::new(RefreshPolicy::default());
    let id = book.register(verified("openai", "acct")).unwrap();
    assert_eq!(book.project(MONDAY)[0].state, "unknown");
    // Parse the documented wire shape.
    let read: CodexRateLimitsRead = serde_json::from_value(json!({
        "rateLimits": {"limitId": "codex", "limitName": null,
            "primary": {"usedPercent": 25, "windowDurationMins": 300, "resetsAt": 1793581200},
            "secondary": null, "rateLimitReachedType": null},
        "rateLimitsByLimitId": {
            "codex": {"limitId": "codex", "limitName": null,
                "primary": {"usedPercent": 25, "windowDurationMins": 300, "resetsAt": 1793581200},
                "secondary": {"usedPercent": 60, "windowDurationMins": 10080, "resetsAt": 1793836800},
                "rateLimitReachedType": null},
            "codex_other": {"limitId": "codex_other", "limitName": "codex_other",
                "primary": {"usedPercent": 42, "windowDurationMins": 60, "resetsAt": 1793581200},
                "secondary": null, "rateLimitReachedType": null}
        },
        "rateLimitResetCredits": {"availableCount": 2, "credits": null}
    }))
    .unwrap();
    book.apply_codex_read(&id, MONDAY, &read).unwrap();
    let account = &book.project(MONDAY + MIN)[0];
    assert_eq!(account.state, "fresh");
    assert_eq!(account.quota_windows.len(), 3);
    let primary = account
        .quota_windows
        .iter()
        .find(|w| w.id == "codex.codex.primary")
        .unwrap();
    assert_eq!(primary.used_percent, Some(25.0));
    assert_eq!(primary.window_minutes, Some(300));
    assert_eq!(primary.resets_at_ms, Some(1_793_581_200_000));
    assert_eq!(primary.label, "5-hour limit");
    let other = account
        .quota_windows
        .iter()
        .find(|w| w.id == "codex.codex_other.primary")
        .unwrap();
    assert_eq!(other.label, "codex_other · 1-hour");
    assert_eq!(
        account.quota_windows[0].window_minutes,
        Some(60),
        "sorted by window length"
    );

    // A notification merges one bucket; an older one cannot overwrite it.
    let update: CodexRateLimitsUpdated = serde_json::from_value(json!({"rateLimits": {
        "limitId": "codex", "primary": {"usedPercent": 31, "windowDurationMins": 300, "resetsAt": 1793581200}}}))
    .unwrap();
    book.apply_codex_updated(&id, MONDAY + 2 * MIN, &update)
        .unwrap();
    let stale_update: CodexRateLimitsUpdated = serde_json::from_value(json!({"rateLimits": {
        "limitId": "codex", "primary": {"usedPercent": 5, "windowDurationMins": 300, "resetsAt": 1793581200}}}))
    .unwrap();
    book.apply_codex_updated(&id, MONDAY + MIN, &stale_update)
        .unwrap();
    let account = &book.project(MONDAY + 3 * MIN)[0];
    let primary = account
        .quota_windows
        .iter()
        .find(|w| w.id == "codex.codex.primary")
        .unwrap();
    assert_eq!(primary.used_percent, Some(31.0));
    assert!(
        account
            .quota_windows
            .iter()
            .all(|w| w.id != "codex.codex.secondary"),
        "bucket replaced as a unit"
    );
    assert!(
        account
            .quota_windows
            .iter()
            .any(|w| w.id == "codex.codex_other.primary"),
        "other bucket kept"
    );

    // Reached limit -> limited; after the reset passes -> stale and unknown percent.
    let mut reached = codex_read(100.0, 300, 10_080);
    reached
        .rate_limits
        .as_mut()
        .unwrap()
        .rate_limit_reached_type = Some("primary".into());
    book.apply_codex_read(&id, MONDAY + 4 * MIN, &reached)
        .unwrap();
    let account = &book.project(MONDAY + 5 * MIN)[0];
    assert_eq!(account.state, "limited");
    assert_eq!(
        account.plan.as_deref(),
        Some("pro"),
        "plan as reported, nothing inferred from it"
    );
    let after_reset = &book.project(MONDAY + 2 * HOUR)[0];
    assert_eq!(after_reset.state, "stale");
    let primary = after_reset
        .quota_windows
        .iter()
        .find(|w| w.id == "codex.codex.primary")
        .unwrap();
    assert_eq!(primary.used_percent, None);
    // An older full read arriving late is ignored.
    book.apply_codex_read(&id, MONDAY, &codex_read(1.0, 300, 10_080))
        .unwrap();
    let primary = book.project(MONDAY + 5 * MIN)[0]
        .quota_windows
        .iter()
        .find(|w| w.id == "codex.codex.primary")
        .cloned()
        .unwrap();
    assert_eq!(primary.used_percent, Some(100.0));
}

#[test]
fn claude_status_line_event_and_usage_report_are_optional_sources() {
    let mut book = QuotaBook::new(RefreshPolicy::default());
    let id = book.register(verified("anthropic", "org-user")).unwrap();
    // Absent rate_limits (API key, before first response): unknown, not zero.
    book.apply_claude_status_line(&id, MONDAY, None).unwrap();
    assert_eq!(book.project(MONDAY)[0].state, "unknown");
    let limits: ClaudeStatusLineRateLimits = serde_json::from_value(json!({
        "five_hour": {"used_percentage": 23.5, "resets_at": 1793595600},
        "seven_day": {"used_percentage": 41.2, "resets_at": 1794009600}
    }))
    .unwrap();
    book.apply_claude_status_line(&id, MONDAY, Some(&limits))
        .unwrap();
    let account = &book.project(MONDAY + MIN)[0];
    assert_eq!(account.state, "fresh");
    let five = &account.quota_windows[0];
    assert_eq!(
        (five.id.as_str(), five.window_minutes, five.used_percent),
        ("claude.five_hour", Some(300), Some(23.5))
    );
    assert_eq!(five.resets_at_ms, Some(1_793_595_600_000));
    assert_eq!(account.quota_windows[1].window_minutes, Some(10_080));
    // Claude drops a window after it resets: the replacement set removes it.
    let only_week = ClaudeStatusLineRateLimits {
        seven_day: Some(ClaudeStatusWindow {
            used_percentage: Some(42.0),
            resets_at: Some(1_794_009_600.0),
        }),
        ..Default::default()
    };
    book.apply_claude_status_line(&id, MONDAY + 2 * MIN, Some(&only_week))
        .unwrap();
    assert_eq!(book.project(MONDAY + 3 * MIN)[0].quota_windows.len(), 1);

    // SDK rate_limit_event: status only; no window percentage is invented.
    let info: ClaudeRateLimitInfo = serde_json::from_value(
        json!({"status": "rejected", "resetsAt": 1793581200, "utilization": 0.97}),
    )
    .unwrap();
    book.apply_claude_rate_limit_event(&id, MONDAY + 4 * MIN, &info)
        .unwrap();
    let account = &book.project(MONDAY + 5 * MIN)[0];
    assert_eq!(account.state, "limited");
    let event_window = account
        .quota_windows
        .iter()
        .find(|w| w.id == "claude.rate_limit_event")
        .unwrap();
    assert_eq!(event_window.used_percent, None);
    let allowed = ClaudeRateLimitInfo {
        status: ClaudeRateLimitStatus::Allowed,
        resets_at: None,
        error_code: None,
    };
    book.apply_claude_rate_limit_event(&id, MONDAY + 6 * MIN, &allowed)
        .unwrap();
    assert_eq!(book.project(MONDAY + 7 * MIN)[0].state, "fresh");

    // Usage report rows: ISO reset times, server kinds, no invented window length.
    let report = ClaudeUsageReportRateLimits {
        limits: Some(vec![
            ClaudeUsageLimit {
                kind: "session".into(),
                percent: Some(12.0),
                resets_at: Some("2026-11-02T05:00:00Z".into()),
                ..Default::default()
            },
            ClaudeUsageLimit {
                kind: "weekly_scoped".into(),
                percent: None,
                resets_at: None,
                ..Default::default()
            },
        ]),
    };
    book.apply_claude_usage_report(&id, MONDAY + 8 * MIN, Some(&report))
        .unwrap();
    let account = &book.project(MONDAY + 9 * MIN)[0];
    let session = account
        .quota_windows
        .iter()
        .find(|w| w.id == "claude.usage.session")
        .unwrap();
    assert_eq!(session.resets_at_ms, Some(MONDAY + 5 * HOUR));
    assert_eq!(session.window_minutes, None);
    assert_eq!(account.state, "partial", "a row without a percentage");
    // `rate_limits: null` changes nothing.
    book.apply_claude_usage_report(&id, MONDAY + 10 * MIN, None)
        .unwrap();
    assert!(
        book.project(MONDAY + 10 * MIN)[0]
            .quota_windows
            .iter()
            .any(|w| w.id == "claude.usage.session")
    );
}

#[test]
fn freshness_backoff_and_unsupported_capability_are_explicit() {
    let policy = RefreshPolicy {
        interval_ms: 5 * MIN,
        stale_after_ms: 15 * MIN,
        max_backoff_ms: HOUR,
    };
    let mut book = QuotaBook::new(policy);
    let codex = book.register(verified("openai", "acct")).unwrap();
    let mut octos = verified("octos", "upstream-1");
    octos.capability = QuotaCapability::Unsupported;
    let octos = book.register(octos).unwrap();
    assert_eq!(
        book.project(MONDAY)
            .iter()
            .find(|a| a.id == octos)
            .unwrap()
            .state,
        "unsupported"
    );
    assert_eq!(
        book.due_for_refresh(MONDAY),
        vec![codex.clone()],
        "unsupported accounts are never polled"
    );
    book.apply_codex_read(&codex, MONDAY, &codex_read(10.0, 300, 10_080))
        .unwrap();
    assert!(book.due_for_refresh(MONDAY + MIN).is_empty());
    assert_eq!(book.due_for_refresh(MONDAY + 5 * MIN), vec![codex.clone()]);
    book.record_refresh_failure(&codex, MONDAY + 5 * MIN, "rate limited", Some(30 * MIN))
        .unwrap();
    assert!(
        book.due_for_refresh(MONDAY + 20 * MIN).is_empty(),
        "retry-after honored"
    );
    let freshness = book.freshness(&codex, MONDAY + 20 * MIN).unwrap();
    assert_eq!(freshness.state, AccountState::Stale);
    assert_eq!(freshness.age_ms, Some(20 * MIN));
    assert_eq!(freshness.last_error.as_deref(), Some("rate limited"));
    book.record_refresh_failure(&codex, MONDAY + 40 * MIN, "timeout", None)
        .unwrap();
    assert_eq!(
        book.freshness(&codex, MONDAY).unwrap().next_refresh_ms,
        MONDAY + 40 * MIN + 20 * MIN
    );
    // An upstream adapter can still supply windows with its own semantics.
    book.apply_adapter_windows(
        &octos,
        MONDAY,
        &[AdapterWindow {
            id: "monthly".into(),
            label: "Monthly credits".into(),
            used_percent: Some(12.5),
            resets_at_ms: None,
            window_minutes: None,
        }],
    )
    .unwrap();
    assert_eq!(
        book.project(MONDAY + MIN)
            .iter()
            .find(|a| a.id == octos)
            .unwrap()
            .state,
        "fresh"
    );
    assert_eq!(
        book.apply_codex_read("openai:missing", MONDAY, &CodexRateLimitsRead::default()),
        Err(QuotaError::UnknownAccount)
    );
}

// ---------------------------------------------------------------- persistence & collector

#[test]
fn export_import_and_journal_replay_preserve_totals_and_dedup() {
    let t = MONDAY + 2 * HOUR;
    let mut ledger = ledger_with(&[binding("e1", "terra", None)]);
    let journal = vec![
        LedgerRecord::Bind(binding("e2", "astra", None)),
        LedgerRecord::Usage(cumulative("e1", "p1", Some(1), t, counts(10, 1, 0, 5))),
        LedgerRecord::Usage(cumulative(
            "e1",
            "p1",
            Some(2),
            t + MIN,
            counts(20, 2, 0, 5),
        )),
        LedgerRecord::Usage(delta("e2", Some("r1"), t, counts(3, 3, 3, 3))),
    ];
    ledger.replay(&journal).unwrap();
    ledger.replay(&journal).unwrap(); // idempotent
    let expected = ledger.aggregate(&day());
    assert_eq!(expected.counts, counts(23, 5, 3, 8));

    let bytes = ledger.export_json().unwrap();
    let mut restored = UsageLedger::import_json(&bytes).unwrap();
    assert_eq!(restored.aggregate(&day()), expected);
    // Dedup state survives a restart: replaying again changes nothing.
    restored.replay(&journal).unwrap();
    assert_eq!(restored.aggregate(&day()).counts, expected.counts);
    assert_eq!(
        restored
            .ingest(&cumulative("e1", "p1", Some(1), t, counts(10, 1, 0, 5)))
            .unwrap(),
        IngestOutcome::Duplicate
    );

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["version"] = json!(99);
    assert_eq!(
        UsageLedger::import_json(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
        LedgerError::InvalidExport
    );
    assert!(UsageLedger::import_json(b"{not json").is_err());

    // Pruning moves coverage forward so older periods report the gap.
    let removed = restored.prune_before(t + HOUR);
    assert_eq!(removed, 2);
    let after = restored.aggregate(&day());
    assert!(after.coverage_gap);
    assert_eq!(after.counts, UNKNOWN_COUNTS);
}

#[test]
fn collector_revisions_owner_switch_and_coverage_gaps() {
    let now = MONDAY + 12 * HOUR;
    let mut collector =
        DashboardCollector::new(UsageLedger::new(now - HOUR), RefreshPolicy::default());
    let initial = collector.snapshot(now, SnapshotOptions::default()).unwrap();
    assert_eq!(initial.hosting.state, HostingState::Unconfigured);
    assert_eq!(
        initial.usage.counts, UNKNOWN_COUNTS,
        "ledger began after the day started"
    );
    assert!(!initial.usage.complete);

    let r0 = collector.revision();
    collector.set_hosting(HostingStatus {
        state: HostingState::Running,
        detail: None,
        active_runs: Some(0),
        pending_approvals: None,
    });
    assert!(collector.revision() > r0);
    let r1 = collector.revision();
    collector.set_hosting(collector.hosting().clone());
    assert_eq!(collector.revision(), r1, "no-op updates keep the revision");

    collector
        .upsert_resource(resource("terra", "Terra", None))
        .unwrap();
    let mut b = binding("e1", "terra", None);
    b.started_at_ms = now - 10 * MIN;
    collector.bind_execution(b).unwrap();
    let snapshot = collector.snapshot(now, SnapshotOptions::default()).unwrap();
    let terra = &snapshot.resources[0];
    assert_eq!(terra.state, "unmeasured");
    let r2 = collector.revision();
    assert_eq!(
        collector
            .ingest_usage(&delta("e1", Some("x"), now - 5 * MIN, counts(1, 1, 0, 0)))
            .unwrap(),
        IngestOutcome::Applied
    );
    assert!(collector.revision() > r2);
    let r3 = collector.revision();
    collector
        .ingest_usage(&delta("e1", Some("x"), now - 5 * MIN, counts(1, 1, 0, 0)))
        .unwrap();
    assert_eq!(
        collector.revision(),
        r3,
        "duplicates do not churn subscribers"
    );
    let snapshot = collector.snapshot(now, SnapshotOptions::default()).unwrap();
    assert_eq!(
        snapshot.resources[0].state, "partial",
        "coverage gap in today's range"
    );
    assert_eq!(snapshot.resources[0].counts, counts(1, 1, 0, 0));
    let range = SnapshotOptions {
        period: UsagePeriod::Range {
            start_ms: now - HOUR,
            end_ms: now,
        },
        utc_offset_minutes: 0,
    };
    let snapshot = collector.snapshot(now, range).unwrap();
    assert_eq!(snapshot.resources[0].state, "measured");
    assert!(snapshot.usage.complete);
    let report = collector.usage_report(now, range, None).unwrap();
    assert_eq!(report.by_resource.len(), 1);
    assert_eq!(report.daily.len(), 1);

    let owner = |id: &str| OwnerIdentity {
        user_id: id.into(),
        display_name: "Owner".into(),
        server: "example.org".into(),
    };
    assert!(collector.set_owner(Some(owner("@a:example.org")), || UsageLedger::new(now)));
    collector
        .upsert_resource(resource("terra", "Terra", None))
        .unwrap();
    let mut renamed = owner("@a:example.org");
    renamed.display_name = "Renamed".into();
    assert!(!collector.set_owner(Some(renamed), || unreachable!()));
    assert_eq!(
        collector
            .snapshot(now, SnapshotOptions::default())
            .unwrap()
            .resources
            .len(),
        1
    );
    assert!(collector.set_owner(Some(owner("@b:example.org")), || UsageLedger::new(now)));
    let switched = collector.snapshot(now, SnapshotOptions::default()).unwrap();
    assert!(
        switched.resources.is_empty()
            && switched.accounts.is_empty()
            && switched.activities.is_empty()
    );
    assert_eq!(switched.hosting.state, HostingState::Unconfigured);
    assert_eq!(switched.owner.unwrap().user_id, "@b:example.org");
}

#[test]
fn activities_are_bounded_and_newest_first() {
    let mut collector = DashboardCollector::new(UsageLedger::new(0), RefreshPolicy::default());
    for index in 0..300u64 {
        collector
            .record_activity(Activity {
                id: format!("a{index}"),
                at_ms: MONDAY + index,
                title: "Run".into(),
                detail: String::new(),
                state: ActivityState::Succeeded.as_str().into(),
            })
            .unwrap();
    }
    let snapshot = collector
        .snapshot(
            MONDAY + HOUR,
            SnapshotOptions {
                period: UsagePeriod::All,
                utc_offset_minutes: 0,
            },
        )
        .unwrap();
    assert_eq!(snapshot.activities.len(), 100);
    assert_eq!(snapshot.activities[0].id, "a299");
    assert!(
        collector
            .record_activity(Activity {
                id: String::new(),
                at_ms: 0,
                title: "x".into(),
                detail: String::new(),
                state: "info".into()
            })
            .is_err()
    );
}

#[test]
fn historical_merge_keeps_coverage_gap_and_bindings_respect_account_scope() {
    let t = MONDAY + 2 * HOUR;
    let mut history = UsageLedger::new(0);
    history
        .bind_execution(binding("old", "terra", None))
        .unwrap();
    history
        .ingest(&delta("old", Some("h1"), t, counts(5, 5, 0, 0)))
        .unwrap();
    let mut ledger = UsageLedger::new(t + HOUR);
    ledger.merge(history).unwrap();
    assert_eq!(ledger.coverage_start_ms(), t + HOUR, "imports retain gaps");
    let total = ledger.aggregate(&day());
    assert_eq!(total.counts, counts(5, 5, 0, 0));
    assert!(total.coverage_gap && !total.complete);

    let mut conflicting = UsageLedger::new(0);
    let mut moved = binding("old", "astra", None);
    moved.started_at_ms = t;
    conflicting.bind_execution(moved).unwrap();
    assert_eq!(ledger.merge(conflicting), Err(LedgerError::BindingConflict));

    let mut collector = DashboardCollector::new(UsageLedger::new(0), RefreshPolicy::default());
    let nova = collector
        .quotas_mut()
        .register(unverified("anthropic", "nova"))
        .unwrap();
    assert!(
        collector
            .bind_execution(binding("e1", "luna", Some(&nova)))
            .is_err(),
        "unverified account is scoped to its own resource"
    );
    assert!(
        collector
            .bind_execution(binding("e1", "nova", Some(&nova)))
            .unwrap()
    );
}

#[test]
fn quota_capacity_is_refused_without_partial_replacement() {
    let mut book = QuotaBook::new(RefreshPolicy::default());
    let id = book.register(verified("openai", "acct")).unwrap();
    book.apply_codex_read(&id, MONDAY, &codex_read(10.0, 300, 10_080))
        .unwrap();
    let many: Vec<AdapterWindow> = (0..31)
        .map(|index| AdapterWindow {
            id: format!("w{index}"),
            label: "Window".into(),
            used_percent: Some(1.0),
            resets_at_ms: None,
            window_minutes: None,
        })
        .collect();
    assert_eq!(
        book.apply_adapter_windows(&id, MONDAY + MIN, &many),
        Err(QuotaError::Capacity)
    );
    assert_eq!(book.project(MONDAY + MIN)[0].quota_windows.len(), 2);
}
