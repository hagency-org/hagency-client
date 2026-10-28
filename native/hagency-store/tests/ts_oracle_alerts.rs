//! TS-oracle tests for `tests/alert-store.test.js` (board #63).
//!
//! The retained alert store (`lib/alert-store.js`) is a five-state, many-kind,
//! note- and patch-bearing store with an auto-resolve-by-prefix path. The native
//! ceiling-alert store is deliberately narrower (ADR-124): ONE kind
//! (`agent_ceiling_overrun`), a four-state map without `assigned`, no notes, no
//! delete and no PATCH — each of those needs a column or an authority the
//! native boundary does not have. So most retained cases land as parity gaps
//! (kept as the oracle, `#[ignore]`d), and the ones native DOES carry are
//! asserted green.
mod common;
use common::*;
use hagency_core::project::Resource;
use hagency_store::{AlertTransition, DomainRepository, EffectOutcome, allowed_transitions};
use rusqlite::Connection;
use std::path::PathBuf;

const GENEROUS: u64 = 9_000_000_000_000_000;

struct Alarm {
    root: tempfile::TempDir,
    db: DomainRepository,
}

fn open(ceiling: u64) -> Alarm {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    let mut pool: Resource = resource("oracle_alarm_pool", "oracle_alarm_seat", ceiling);
    pool.framework = "codex".into();
    pool.model = "gpt-5.6-sol".into();
    pool.reasoning = Some("medium".into());
    db.put_resource(&pool).unwrap();
    Alarm { root, db }
}

fn set_ceiling(alarm: &mut Alarm, ceiling: u64) {
    let mut pool: Resource = resource("oracle_alarm_pool", "oracle_alarm_seat", ceiling);
    pool.framework = "codex".into();
    pool.model = "gpt-5.6-sol".into();
    pool.reasoning = Some("medium".into());
    alarm.db.put_resource(&pool).unwrap();
}

fn engaged(alarm: &mut Alarm, id: &str, tokens: u64, at: u64) -> String {
    let pool = resource("oracle_alarm_pool", "oracle_alarm_seat", GENEROUS);
    let ask = request(id, "OracleWorker", &pool, tokens);
    let proof = proof(&ask);
    alarm.db.admit(&proof, at).unwrap();
    alarm
        .db
        .approve(&format!("approve_{id}"), &proof, at)
        .unwrap();
    let effect = alarm.db.claim_effect().unwrap().unwrap();
    alarm
        .db
        .observe_effect(
            &effect.id,
            effect.fence,
            &EffectOutcome::Applied {
                receipt: "oracle fixture".into(),
            },
        )
        .unwrap();
    ask.engagement_id().unwrap()
}

fn state(alarm: &Alarm) -> PathBuf {
    alarm.root.path().join("state/domain.sqlite3")
}

fn occurrences(alarm: &Alarm) -> i64 {
    Connection::open(state(alarm))
        .unwrap()
        .query_row("SELECT occurrences FROM ceiling_alerts", [], |r| r.get(0))
        .unwrap()
}

/// TS: `deduplicates alerts by dedupeKey` — repeated occurrences ride one row
/// and the counter increments (`list.body.length` 1, `occurrences` 2).
#[test]
fn ts_oracle_alerts_dedupe_by_key() {
    let mut alarm = open(GENEROUS);
    engaged(&mut alarm, "dedupe_me", 1_500_000, 1000);
    set_ceiling(&mut alarm, 1_000_000);
    alarm.db.sweep_ceiling_overruns(1_000_000).unwrap();
    alarm.db.sweep_ceiling_overruns(4_600_000).unwrap();
    let rows = alarm.db.open_ceiling_alerts(200).unwrap();
    assert_eq!(rows.len(), 1, "one row for one dedupe key");
    assert_eq!(occurrences(&alarm), 2);
}

/// TS: `auto-resolves alert on recovery event` — when the draw falls back under
/// the ceiling the alert resolves on its own (`lib/alert-store.js:336-381`).
#[test]
fn ts_oracle_alerts_auto_resolve_on_recovery() {
    let mut alarm = open(GENEROUS);
    engaged(&mut alarm, "recovers", 1_500_000, 1000);
    set_ceiling(&mut alarm, 1_000_000);
    alarm.db.sweep_ceiling_overruns(1_000_000).unwrap();
    assert_eq!(alarm.db.open_ceiling_alerts(200).unwrap().len(), 1);
    // The ceiling rises back above the draw: the sweep auto-resolves.
    set_ceiling(&mut alarm, GENEROUS);
    let outcome = alarm.db.sweep_ceiling_overruns(4_600_000).unwrap();
    assert_eq!(outcome.resolved, 1);
    assert!(alarm.db.open_ceiling_alerts(200).unwrap().is_empty());
    let resolved_by: Option<String> = Connection::open(state(&alarm))
        .unwrap()
        .query_row("SELECT resolved_by FROM ceiling_alerts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        resolved_by.as_deref(),
        Some("system"),
        "auto-resolve is the system's act, exactly as the retained store"
    );
}

/// TS: `transitions through the state machine correctly` — the legal pairs and
/// `resolved` terminal. Native's map is the four-state subset; `assigned` is a
/// gap (below).
#[test]
fn ts_oracle_alerts_transition_map_legal_pairs_and_terminal() {
    let mut alarm = open(GENEROUS);
    engaged(&mut alarm, "transitions", 1_500_000, 1000);
    set_ceiling(&mut alarm, 1_000_000);
    alarm.db.sweep_ceiling_overruns(1_000_000).unwrap();
    let key = alarm.db.open_ceiling_alerts(200).unwrap()[0]
        .dedupe_key
        .clone();
    // open → acknowledged
    let alert = alarm
        .db
        .transition_ceiling_alert(AlertTransition {
            key: key.clone(),
            to: "acknowledged",
            actor: "operator".into(),
            note: None,
            assignee: None,
            suppress_until_ms: None,
            now: 2_000_000,
        })
        .unwrap();
    assert_eq!(alert.status, "acknowledged");
    // acknowledged → resolved
    let alert = alarm
        .db
        .transition_ceiling_alert(AlertTransition {
            key: key.clone(),
            to: "resolved",
            actor: "operator".into(),
            note: None,
            assignee: None,
            suppress_until_ms: None,
            now: 2_100_000,
        })
        .unwrap();
    assert_eq!(alert.status, "resolved");
    // resolved is TERMINAL — the retained store refuses with `bad_transition`,
    // which the console maps to 400.
    assert!(
        alarm
            .db
            .transition_ceiling_alert(AlertTransition {
                key,
                to: "open",
                actor: "operator".into(),
                note: None,
                assignee: None,
                suppress_until_ms: None,
                now: 2_200_000,
            })
            .is_err()
    );
    // The served map agrees: resolved offers nothing.
    assert!(allowed_transitions("resolved").is_empty());
}

/// TS: `server_online resolves only the matching server_offline alert`, and
/// every other case that depends on a NON-ceiling alert kind or its
/// auto-resolve-by-prefix path.
///
/// PARITY GAP: native raises exactly one kind, `agent_ceiling_overrun`
/// (`ceiling_alerts.rs`), so `mcp_missing`, `server_offline`, `server_online`,
/// `swap_high`, `agent_blocked`, `bridge_warning` and the prefix auto-resolve
/// have no native counterpart.
#[test]
#[ignore = "parity gap: native raises only agent_ceiling_overrun; server_online/prefix resolve absent"]
fn ts_oracle_alerts_server_online_resolves_only_the_matching_offline() {
    // The retained store resolves by `server_online`'s correlation; native has
    // no such ingest point.
    panic!("no native server_offline/server_online ingest path");
}

/// TS: `transitions through the state machine correctly` (the `assigned` legs)
/// and `agent can resolve their assigned alert via agent-token`.
///
/// PARITY GAP: `assigned` needs an assignee column and the retained
/// agent-token authority (`backend-v2.js:16104-16113`), which the native
/// boundary does not have (ADR-124 dropped it).
#[test]
#[ignore = "parity gap: no `assigned` state natively (no assignee column, no agent-token authority)"]
fn ts_oracle_alerts_assigned_state_and_agent_token_resolve() {
    panic!("native carries no assigned state");
}

/// TS: `lists alerts with filters and returns stats` (`/api/alerts?sourceAgent=`
/// + `/api/alerts/stats`).
///
/// PARITY GAP: native's read is open-rows-only with no filter query and no
/// stats route (the console ceiling-alert read is the whole surface).
#[test]
#[ignore = "parity gap: no alert filters or /api/alerts/stats route natively"]
fn ts_oracle_alerts_list_filters_and_stats() {
    panic!("native has no alert stats route or filter query");
}

/// TS: `adds and retrieves notes` (`POST /api/alerts/:id/notes`).
///
/// PARITY GAP: native carries a single `note` string on a transition, not a
/// note list with author+text.
#[test]
#[ignore = "parity gap: native has one note string, not a notes list"]
fn ts_oracle_alerts_adds_and_retrieves_notes() {
    panic!("native has no notes list");
}

/// TS: `deletes an alert` (`DELETE /api/alerts/:id`).
///
/// PARITY GAP: native has no delete route.
#[test]
#[ignore = "parity gap: no alert delete route natively"]
fn ts_oracle_alerts_deletes_an_alert() {
    panic!("native has no delete route");
}

/// TS: `patch updates actionable metadata and can restore original severity`
/// (`PATCH /api/alerts/:id`).
///
/// PARITY GAP: native has no alert PATCH route and no owner/runbook/impact/
/// recoveryCondition patch surface.
#[test]
#[ignore = "parity gap: no alert PATCH route natively"]
fn ts_oracle_alerts_patch_restores_original_severity() {
    panic!("native has no alert patch route");
}

/// TS: `suppressed alert reopens on new occurrence after suppressUntil expires`
/// and `... stays suppressed when suppressUntil has not passed`.
///
/// PARITY GAP: native suppression is operator-released only (`suppressed→open`)
/// with NO window — the retained 24h `suppressUntil` expiry is the named
/// divergence (brief-24 §2.6).
#[test]
#[ignore = "parity gap: native suppression has no suppressUntil window"]
fn ts_oracle_alerts_suppressed_reopens_after_window() {
    panic!("native suppression has no window");
}

/// TS: `downgrades incomplete paging alerts to diagnostic info` and `keeps
/// complete paging alerts actionable...`.
///
/// PARITY GAP: the actionability downgrade (severity→info, missing-field list)
/// applies to paging alert kinds native never raises.
#[test]
#[ignore = "parity gap: actionability downgrade needs non-ceiling alert kinds"]
fn ts_oracle_alerts_downgrades_incomplete_paging_alerts() {
    panic!("native raises no paging-kind alerts");
}

/// TS: the three rollback cases (`ingest rolls back...`, `transition rollback
/// keeps status...`, `prefix auto-resolve rollback...`).
///
/// PARITY GAP: native's alert writes are single `Immediate` transactions, so a
/// refused transition already writes nothing — asserted instead by
/// `ts_oracle_alerts_transition_map_legal_pairs_and_terminal`. The retained
/// cases inject a `save()` failure the native store has no seam for.
#[test]
#[ignore = "parity gap: no persistence-failure injection seam in the native alert store"]
fn ts_oracle_alerts_write_rollbacks() {
    panic!("native alert writes are already atomic");
}
