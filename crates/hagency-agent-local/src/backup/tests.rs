use super::*;
use crate::{Budget, Layer, Limit, Period, Policy, RequestPolicy, Scope, ToolPolicy, Usage};
const OWNER: &str = "@alice:example.org";
fn profile() -> String {
    "a".repeat(64)
}
fn scope() -> Scope {
    Scope {
        agent: "agent".into(),
        binding: "binding".into(),
        room: "!room:example.org".into(),
        requester: OWNER.into(),
        thread: "main".into(),
    }
}
fn populated(path: &std::path::Path) -> Ledger {
    let mut ledger = Ledger::open_scoped(path, OWNER, &profile()).unwrap();
    ledger
        .db
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    ledger.register_binding(OWNER, &scope()).unwrap();
    for layer in [Layer::Agent, Layer::Room, Layer::Requester] {
        ledger
            .set_policy(
                OWNER,
                &scope(),
                layer,
                0,
                &Policy {
                    budget: Budget {
                        limit: Limit::Tokens(5000),
                        period: Period::Lifetime,
                    },
                    requests: RequestPolicy::Allow,
                    high_risk: ToolPolicy::AskOwner,
                },
            )
            .unwrap();
    }
    ledger
        .reserve(&scope(), "settled", "dispatch1", 100, 100)
        .unwrap();
    ledger
        .settle(
            &scope(),
            "settled",
            &Usage {
                input: 25,
                output: 10,
                cached_input: 5,
                reasoning_output: 2,
                accounting_version: "fixture-v1".into(),
            },
        )
        .unwrap();
    ledger
        .reserve(&scope(), "pending", "dispatch2", 200, 101)
        .unwrap();
    ledger
        .db
        .execute(
            "INSERT INTO approvals VALUES ('approval','binding','original proposal',900,1)",
            [],
        )
        .unwrap();
    // Preserve the exact durable inbox/reply proof, not a regenerated response.
    ledger.db.execute("INSERT INTO local_inbox VALUES ('inbox','binding','$original','immutable','{}','reply_ready','execution','original reply','reply-digest',101)",[]).unwrap();
    ledger
}
fn rows(db: &Connection, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let mut stmt = db
        .prepare(&format!("SELECT * FROM {} ORDER BY rowid", quoted(table)))
        .unwrap();
    let n = stmt.column_count();
    stmt.query_map([], |r| (0..n).map(|i| r.get(i)).collect())
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
}
#[test]
fn wal_snapshot_restores_all_tables_and_pending_custody_without_reexecution() {
    let tmp = tempfile::tempdir().unwrap();
    let source = populated(&tmp.path().join("source.sqlite"));
    let snapshot = source.snapshot(OWNER, &profile(), 123).unwrap();
    assert_eq!(snapshot.summary.calls, 2);
    assert_eq!(snapshot.summary.pending_calls, 1);
    assert_eq!(snapshot.summary.pending_replies, 1);
    assert!(snapshot.summary.database_bytes > 0);
    let read = Snapshot::inspect(snapshot.bytes().to_vec(), OWNER, &profile()).unwrap();
    assert_eq!(read.digest(), snapshot.digest());
    let target = tmp.path().join("target.sqlite");
    let mut restored = Ledger::open_scoped(&target, OWNER, &profile()).unwrap();
    restored.restore_snapshot(OWNER, &profile(), &read).unwrap();
    for (_, table, _) in objects(&source.db)
        .unwrap()
        .into_iter()
        .filter(|(kind, _, _)| kind == "table")
    {
        assert_eq!(
            rows(&source.db, &table),
            rows(&restored.db, &table),
            "{table}"
        );
    }
    drop(restored);
    let restored = Ledger::open_scoped(target, OWNER, &profile()).unwrap();
    assert_eq!(
        restored
            .account(&scope(), Layer::Agent, Period::Lifetime, 200)
            .unwrap(),
        (35, 200)
    );
    assert_eq!(
        restored
            .db
            .query_row("SELECT state FROM calls WHERE id='pending'", [], |r| r
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "pending"
    );
    assert_eq!(
        restored
            .db
            .query_row("SELECT consumed FROM approvals", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn corrupt_or_wrong_identity_snapshots_never_change_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let source = populated(&tmp.path().join("source.sqlite"));
    let snapshot = source.snapshot(OWNER, &profile(), 123).unwrap();
    for length in [0, 7, 11, 12, 40, snapshot.bytes.len() - 1] {
        assert!(Snapshot::inspect(snapshot.bytes[..length].to_vec(), OWNER, &profile()).is_err());
    }
    let mut corrupt = snapshot.bytes.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(Snapshot::inspect(corrupt, OWNER, &profile()).is_err());
    assert!(matches!(
        Snapshot::inspect(snapshot.bytes.clone(), "@bob:example.org", &profile()),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        Snapshot::inspect(snapshot.bytes.clone(), OWNER, &"b".repeat(64)),
        Err(Error::Unauthorized)
    ));
    let mut destination = Ledger::open_scoped(":memory:", OWNER, &"b".repeat(64)).unwrap();
    assert!(
        destination
            .restore_snapshot(OWNER, &"b".repeat(64), &snapshot)
            .is_err()
    );
    assert_eq!(
        destination
            .db
            .query_row("SELECT count(*) FROM calls", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn old_backup_cannot_erase_newer_charges_approvals_or_deduplication() {
    let tmp = tempfile::tempdir().unwrap();
    let mut source = populated(&tmp.path().join("source.sqlite"));
    let snapshot = source.snapshot(OWNER, &profile(), 123).unwrap();
    source
        .settle(
            &scope(),
            "pending",
            &Usage {
                input: 60,
                output: 20,
                cached_input: 0,
                reasoning_output: 0,
                accounting_version: "fixture-v1".into(),
            },
        )
        .unwrap();
    let before = rows(&source.db, "calls");
    assert!(matches!(
        source.restore_snapshot(OWNER, &profile(), &snapshot),
        Err(Error::Conflict)
    ));
    assert_eq!(rows(&source.db, "calls"), before);
    assert_eq!(
        source
            .account(&scope(), Layer::Agent, Period::Lifetime, 200)
            .unwrap(),
        (115, 0)
    );
}
#[test]
fn foreign_schema_or_inconsistent_rows_are_rejected_before_publication() {
    let mut source = Ledger::open_scoped(":memory:", OWNER, &profile()).unwrap();
    source
        .db
        .execute_batch("CREATE TABLE unexpected(secret TEXT)")
        .unwrap();
    assert!(matches!(
        source.snapshot(OWNER, &profile(), 123),
        Err(Error::ForeignDatabase)
    ));
    source.db.execute_batch("DROP TABLE unexpected; PRAGMA foreign_keys=OFF; INSERT INTO calls VALUES ('bad','missing','{}','digest',1,'[]','pending',NULL)").unwrap();
    assert!(source.snapshot(OWNER, &profile(), 123).is_err());
    // Destination insert failure rolls back every table, not a partial import.
    source = Ledger::open_scoped(":memory:", OWNER, &profile()).unwrap();
    source
        .db
        .execute("UPDATE inbox_limits SET max_records=20,max_bytes=4000", [])
        .unwrap();
    let snap = source.snapshot(OWNER, &profile(), 123).unwrap();
    let mut target = Ledger::open_scoped(":memory:", OWNER, &profile()).unwrap();
    target.restore_snapshot(OWNER, &profile(), &snap).unwrap();
    assert_eq!(
        rows(&source.db, "inbox_limits"),
        rows(&target.db, "inbox_limits")
    );
}

#[test]
fn revoked_proof_before_commit_rolls_back_every_imported_row() {
    let temp = tempfile::tempdir().unwrap();
    let source = populated(&temp.path().join("source.sqlite"));
    let snap = source.snapshot(OWNER, &profile(), 123).unwrap();
    let mut target = Ledger::open_scoped(":memory:", OWNER, &profile()).unwrap();
    let mut checks = 0;
    let result = target.restore_snapshot_checked(OWNER, &profile(), &snap, || {
        checks += 1;
        if checks == 2 {
            Err(Error::Unauthorized)
        } else {
            Ok(())
        }
    });
    assert!(matches!(result, Err(Error::Unauthorized)));
    assert_eq!(checks, 2);
    for table in data_tables(&target.db).unwrap() {
        assert!(rows(&target.db, &table).is_empty(), "{table}");
    }
    assert_eq!(rows(&source.db, "identity"), rows(&target.db, "identity"));
    target.restore_snapshot(OWNER, &profile(), &snap).unwrap();
}

#[test]
fn concurrent_uncommitted_wal_write_is_not_part_of_the_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source.sqlite");
    let source = populated(&path);
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch("BEGIN IMMEDIATE; UPDATE accounts SET spent=spent+100")
        .unwrap();
    let snapshot = source.snapshot(OWNER, &profile(), 123).unwrap();
    let mut target = Ledger::open_scoped(":memory:", OWNER, &profile()).unwrap();
    target
        .restore_snapshot(OWNER, &profile(), &snapshot)
        .unwrap();
    writer.execute_batch("COMMIT").unwrap();
    assert_eq!(
        target
            .account(&scope(), Layer::Agent, Period::Lifetime, 200)
            .unwrap(),
        (35, 200)
    );
    assert_eq!(
        source
            .account(&scope(), Layer::Agent, Period::Lifetime, 200)
            .unwrap(),
        (135, 200)
    );
}

#[test]
fn original_v1_snapshot_restores_without_fabricating_usage_dates() {
    let tmp = tempfile::tempdir().unwrap();
    let source = populated(&tmp.path().join("original.sqlite"));
    source
        .db
        .execute_batch("DROP TABLE received_files; DROP TABLE call_observations; DROP TABLE usage_alerts; DROP TABLE execution_stops; DROP TABLE execution_inputs; DROP TABLE recovery_actions; DROP TABLE recovery_reviews; DROP TABLE execution_workspaces; DROP TABLE approval_audit_events; DROP TABLE approval_audit")
        .unwrap();
    let old = source.snapshot(OWNER, &profile(), 123).unwrap();
    let mut target = Ledger::open_scoped(":memory:", OWNER, &profile()).unwrap();
    target.restore_snapshot(OWNER, &profile(), &old).unwrap();
    let report = target
        .usage_report(
            OWNER,
            &crate::usage::Query {
                agent: "agent".into(),
                binding: None,
                requester: None,
                from: 0,
                until: 1000,
                group: crate::usage::Group::Day,
                alerts_before: None,
            },
            200,
        )
        .unwrap();
    assert_eq!(report.totals.calls, 0);
    assert_eq!(report.undated.calls, 2);
    assert_eq!(report.undated.input, 25);
    assert_eq!(report.undated.reserved, 200);
}

#[test]
fn pre_audit_snapshot_keeps_usage_and_does_not_invent_approval_history() {
    let tmp = tempfile::tempdir().unwrap();
    let source = populated(&tmp.path().join("usage-only.sqlite"));
    source
        .db
        .execute_batch("DROP TABLE received_files; DROP TABLE execution_stops; DROP TABLE execution_inputs; DROP TABLE recovery_actions; DROP TABLE recovery_reviews; DROP TABLE execution_workspaces; DROP TABLE approval_audit_events; DROP TABLE approval_audit")
        .unwrap();
    let original_dates = rows(&source.db, "call_observations");
    assert_eq!(original_dates.len(), 2);
    let old = source.snapshot(OWNER, &profile(), 123).unwrap();
    let mut target = Ledger::open_scoped(":memory:", OWNER, &profile()).unwrap();
    target.restore_snapshot(OWNER, &profile(), &old).unwrap();
    assert_eq!(rows(&target.db, "call_observations"), original_dates);
    assert!(
        target
            .approval_history(OWNER, "agent", None)
            .unwrap()
            .records
            .is_empty()
    );
    assert_eq!(
        target
            .account(&scope(), Layer::Agent, Period::Lifetime, 200)
            .unwrap(),
        (35, 200)
    );
}

#[test]
fn published_audit_snapshot_migrates_empty_recovery_without_inventing_stop_proofs() {
    let tmp = tempfile::tempdir().unwrap();
    let mut source = populated(&tmp.path().join("audit-only.sqlite"));
    let proposal = crate::ToolProposal {
        scope: scope(),
        dispatch: "original".into(),
        tool: "codex.command".into(),
        arguments: serde_json::json!({"command":"echo original"}),
        canonical_directory: "/workspace".into(),
        risk: "high".into(),
        policy_revision: [1; 3],
        expires: 500,
    };
    let id = source.begin_approval_audit(OWNER, &proposal, 100).unwrap();
    source
        .record_approval_event(
            OWNER,
            &id,
            crate::audit::Phase::OwnerAllowed,
            101,
            &serde_json::json!({"device":"original"}),
        )
        .unwrap();
    source.db.execute_batch("DROP TABLE received_files; DROP TABLE recovery_actions; DROP TABLE recovery_reviews; DROP TABLE execution_inputs; DROP TABLE execution_stops; DROP TABLE execution_workspaces").unwrap();
    let snapshot = source.snapshot(OWNER, &profile(), 123).unwrap();
    let mut target = Ledger::open_scoped(":memory:", OWNER, &profile()).unwrap();
    target
        .restore_snapshot(OWNER, &profile(), &snapshot)
        .unwrap();
    assert_eq!(
        target
            .approval_record(OWNER, "agent", &id)
            .unwrap()
            .proposal,
        proposal
    );
    assert_eq!(
        target.approval_phase(OWNER, &id).unwrap(),
        crate::audit::Phase::OwnerAllowed
    );
    assert!(rows(&target.db, "execution_stops").is_empty());
    assert!(rows(&target.db, "execution_inputs").is_empty());
    assert!(rows(&target.db, "recovery_actions").is_empty());
    assert_eq!(
        target
            .account(&scope(), Layer::Agent, Period::Lifetime, 200)
            .unwrap(),
        (35, 200)
    );
}
