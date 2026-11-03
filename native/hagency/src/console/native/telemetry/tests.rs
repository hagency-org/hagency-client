use super::*;
use hagency_metering::dashboard::ledger::{CountMode, CountScope, UsageSource};
use hagency_metering::dashboard::period::{TimeRange, UsagePeriod};
use serde_json::json;

const BUCKET: u64 = 900_000;
fn binding(id: &str, account: &str) -> ExecutionBinding {
    ExecutionBinding { execution_id: id.into(), resource_id: "same-resource".into(),
        account_id: Some(account.into()), allocation_id: None, agent_id: None,
        requester_id: None, room_id: None, runtime: Runtime::Codex, runtime_version: None,
        model: None, parent_execution_id: None, started_at_ms: 0 }
}
fn event(id: &str, n: u64, at: u64, sequence: u64) -> UsageEvent {
    UsageEvent { execution_id: id.into(), stream: "thread".into(), event_id: None,
        sequence: Some(sequence), observed_at_ms: at, measured_at_ms: None,
        mode: CountMode::Cumulative { epoch: "epoch".into() }, scope: CountScope::Own,
        counts: TokenCounts { input: Some(n), output: Some(0), cache_read: Some(0), cache_write: Some(0) },
        reasoning_output: None, complete: true, source: UsageSource::CodexRuntime }
}
fn query(start: u64, end: u64) -> UsageQuery {
    UsageQuery::range(TimeRange::new(start, end).unwrap())
}

#[test]
fn checkpoint_restart_retains_cumulative_baseline_and_duplicate_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::Store::open(&dir.path().join("telemetry"), 0).unwrap();
    store.put_binding(&binding("e1", "a"), 0).unwrap();
    store.put_event(&event("e1", 100, 1, 1), "epoch").unwrap();
    store.put_event(&event("e1", 150, BUCKET + 1, 2), "epoch").unwrap();
    let expected = store.load().unwrap().ledger.aggregate(&query(BUCKET, 2 * BUCKET));
    assert_eq!(expected.counts.input, Some(50));
    store.checkpoint().unwrap();
    drop(store);
    let store = store::Store::open(&dir.path().join("telemetry"), 3 * BUCKET).unwrap();
    assert_eq!(store.load().unwrap().ledger.aggregate(&query(BUCKET, 2 * BUCKET)), expected);
    store.put_event(&event("e1", 150, BUCKET + 1, 2), "epoch").unwrap();
    let duplicate = store.load().unwrap().ledger.aggregate(&query(BUCKET, 2 * BUCKET));
    assert_eq!(duplicate.counts, expected.counts);
    assert_eq!(duplicate.complete, expected.complete);
    assert_eq!(duplicate.diagnostics.duplicates, 1);
    store.put_event(&event("e1", 180, 2 * BUCKET + 1, 3), "epoch").unwrap();
    assert_eq!(store.load().unwrap().ledger.aggregate(&query(2 * BUCKET, 3 * BUCKET)).counts.input, Some(30));
}

#[test]
fn separate_quota_buckets_survive_restart_and_older_update_cannot_replace_newer() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::Store::open(&dir.path().join("telemetry"), 0).unwrap();
    for (bucket, percent, at) in [("codex", 40, 10), ("review", 80, 20), ("codex", 10, 5)] {
        store.put_quota("account", CODEX_UPDATED, &json!({"rateLimits":{"limitId":bucket,"primary":{"usedPercent":percent,"windowDurationMins":300}}}), at).unwrap();
    }
    drop(store);
    let store = store::Store::open(&dir.path().join("telemetry"), 30).unwrap();
    let quotas = store.load().unwrap().quotas;
    assert_eq!(quotas.len(), 2);
    assert_eq!(quotas[0].1, CODEX_UPDATED);
    assert_eq!(quotas[0].2["rateLimits"]["primary"]["usedPercent"], 40);
    assert_eq!(quotas[1].2["rateLimits"]["limitId"], "review");
}

#[test]
fn short_unaligned_range_is_partial_instead_of_panicking_or_claiming_exact_zero() {
    let mut ledger = UsageLedger::new(0);
    ledger.bind_execution(binding("e1", "a")).unwrap();
    ledger.ingest(&event("e1", 100, 10, 1)).unwrap();
    let usage = ledger.aggregate(&query(1, 20));
    assert!(!usage.complete);
}

#[test]
fn account_history_is_not_reassigned_when_one_resource_switches_accounts() {
    let telemetry = OwnerTelemetry::new(0);
    {
        let mut inner = telemetry.lock().unwrap();
        for (execution, account, n) in [("old", "account-a", 100), ("new", "account-b", 200)] {
            inner.collector.ledger_mut().bind_execution(binding(execution, account)).unwrap();
            inner.collector.ledger_mut().ingest(&event(execution, n, 1, 1)).unwrap();
        }
    }
    let parts = telemetry.parts(BUCKET, SnapshotOptions { period: UsagePeriod::Range { start_ms: 0, end_ms: BUCKET }, utc_offset_minutes: 0 }).unwrap();
    assert_eq!(parts.resources.len(), 1);
    assert_eq!(parts.resources[0].account_id, None);
    assert_eq!(parts.resources[0].counts.input, Some(300));
    let inner = telemetry.lock().unwrap();
    let mut a = query(0, BUCKET);
    a.account_id = Some("account-a".into());
    assert_eq!(inner.collector.ledger().aggregate(&a).counts.input, Some(100));
    a.account_id = Some("account-b".into());
    assert_eq!(inner.collector.ledger().aggregate(&a).counts.input, Some(200));
}

#[test]
fn personal_runtime_categories_are_deduplicated_persisted_and_separate_from_rentals() {
    let dir = tempfile::tempdir().unwrap();
    let telemetry = OwnerTelemetry::new(0);
    telemetry.attach_store(&dir.path().join("telemetry")).unwrap();
    let at = now_ms();
    let start = at - at % BUCKET;
    for (index, runtime) in [Runtime::Codex, Runtime::Claude, Runtime::Octos].into_iter().enumerate() {
        let mut binding = binding(&format!("personal-{index}"), "unused");
        binding.account_id = None;
        binding.resource_id = "personal-agent:agent".into();
        binding.agent_id = Some("agent".into());
        binding.runtime = runtime;
        let usage = hagency_agent_local::Usage { input: 17, output: 3, cached_input: 5,
            cache_write_input: Some(2), reasoning_output: 0, accounting_version: "verified-test".into() };
        telemetry.personal_usage(&binding, &usage).unwrap();
        telemetry.personal_usage(&binding, &usage).unwrap();
    }
    drop(telemetry);
    let telemetry = OwnerTelemetry::new(at);
    telemetry.attach_store(&dir.path().join("telemetry")).unwrap();
    let aggregate = telemetry.read(|c| c.ledger().aggregate(&query(start, start + BUCKET))).unwrap();
    assert_eq!(aggregate.counts, TokenCounts { input: Some(30), output: Some(9), cache_read: Some(15), cache_write: Some(6) });
    assert_eq!(aggregate.executions, 3);
    let rows = telemetry.personal_resources(at, SnapshotOptions {
        period: UsagePeriod::Range { start_ms: start, end_ms: start + BUCKET }, ..Default::default()
    }).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "personal-agent:agent");
    assert_eq!(rows[0].provider, "mixed");
    assert_eq!(rows[0].account_id, None);
    assert_eq!(rows[0].budget_tokens, None);
}

#[test]
fn transient_storage_failure_retains_measurements_for_ordered_retry() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("telemetry");
    let telemetry = OwnerTelemetry::new(0);
    telemetry.attach_store(&folder).unwrap();
    let mut execution = binding("e1", "a");
    execution.account_id = None;
    telemetry.bind_execution(execution).unwrap();
    let db = rusqlite::Connection::open(folder.join(STORE_FILE)).unwrap();
    db.execute_batch("ALTER TABLE usage_events RENAME TO unavailable_events").unwrap();
    telemetry.lock().unwrap().event(event("e1", 100, 1, 1), "epoch").unwrap();
    assert_eq!(telemetry.lock().unwrap().unsaved.len(), 1);
    assert!(telemetry.status().failures > 0);
    db.execute_batch("ALTER TABLE unavailable_events RENAME TO usage_events").unwrap();
    telemetry.lock().unwrap().event(event("e1", 150, 2, 2), "epoch").unwrap();
    assert!(telemetry.lock().unwrap().unsaved.is_empty());
    drop(db);
    drop(telemetry);
    let reopened = store::Store::open(&folder, 3).unwrap();
    assert_eq!(reopened.load().unwrap().ledger.aggregate(&query(0, BUCKET)).counts.input, Some(150));
}

#[test]
fn rejected_personal_observation_degrades_every_usage_projection() {
    let telemetry=OwnerTelemetry::new(0);
    let mut good=binding("accepted","unused");good.account_id=None;
    let usage=hagency_agent_local::Usage {input:10,output:2,cached_input:0,cache_write_input:Some(0),reasoning_output:0,accounting_version:"verified-test".into()};
    telemetry.personal_usage(&good,&usage).unwrap();
    let start = now_ms() / BUCKET * BUCKET;
    let options=SnapshotOptions{period:UsagePeriod::Range { start_ms: start, end_ms: start + BUCKET },..Default::default()};
    assert!(telemetry.usage_report(start + BUCKET,options,None).unwrap().total.complete);
    let mut invalid=good;invalid.execution_id="rejected".into();invalid.resource_id.clear();
    assert!(telemetry.personal_usage(&invalid,&usage).is_err());
    assert!(telemetry.status().failures>0);
    let report=telemetry.usage_report(start + BUCKET,options,None).unwrap();
    assert_eq!(report.total.counts.input,Some(10));
    assert!(!report.total.complete);
    assert!(report.by_resource.iter().all(|(_,usage)| !usage.complete));
    assert!(report.by_requester.iter().all(|(_,usage)| !usage.complete));
    let parts=telemetry.parts(start + BUCKET,options).unwrap();
    assert!(!parts.usage.complete);
    assert!(parts.resources.iter().all(|row|row.state!=ResourceState::Measured));
}
