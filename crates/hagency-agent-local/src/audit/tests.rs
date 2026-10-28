use super::*;
const OWNER: &str = "@alice:example.org";
fn ledger() -> (Ledger, ToolProposal) {
    let mut ledger = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    let scope = Scope {
        agent: "agent".into(),
        binding: "binding".into(),
        room: "!room:example.org".into(),
        requester: OWNER.into(),
        thread: "thread".into(),
    };
    ledger.register_binding(OWNER, &scope).unwrap();
    (
        ledger,
        ToolProposal {
            scope,
            dispatch: "dispatch".into(),
            tool: "codex.command".into(),
            arguments: value!({"command":"echo hello","itemId":"item"}),
            canonical_directory: "/workspace".into(),
            risk: "high".into(),
            policy_revision: [1, 2, 3],
            expires: 500,
        },
    )
}
#[test]
fn exact_decision_application_and_completion_are_distinct_durable_evidence() {
    let (mut l, p) = ledger();
    let id = l.begin_approval_audit(OWNER, &p, 100).unwrap();
    let another = l.begin_approval_audit(OWNER, &p, 100).unwrap();
    assert_ne!(id, another);
    assert_eq!(l.approval_phase(OWNER, &id).unwrap(), Phase::Offered);
    assert!(
        l.record_approval_event(OWNER, &id, Phase::Completed, 101, &value!({}))
            .is_err()
    );
    for (phase, at) in [
        (Phase::OwnerAllowed, 102),
        (Phase::Applying, 103),
        (Phase::Submitted, 104),
    ] {
        l.record_approval_event(OWNER, &id, phase, at, &value!({"original":true}))
            .unwrap();
    }
    l.record_approval_event(
        OWNER,
        &id,
        Phase::Submitted,
        999,
        &value!({"different":true}),
    )
    .unwrap();
    l.record_approval_event(
        OWNER,
        &id,
        Phase::Completed,
        105,
        &value!({"itemId":"item","exitCode":0}),
    )
    .unwrap();
    let record = l.approval_record(OWNER, "agent", &id).unwrap();
    assert_eq!(record.events.len(), 5);
    assert_eq!(record.events[3].observed_at, 104);
    assert_eq!(record.proposal, p);
    assert!(
        l.record_approval_event(OWNER, &id, Phase::Applying, 106, &value!({}))
            .is_err()
    );
    assert!(
        l.record_approval_event(OWNER, &id, Phase::Offered, 107, &value!({}))
            .is_err()
    );
    assert!(l.approval_record("@bob:example.org", "agent", &id).is_err());
    assert!(l.approval_record(OWNER, "other", &id).is_err());
    let mut changed = p.clone();
    changed.arguments = value!({"command":"changed"});
    assert!(l.verify_approval_audit(OWNER, &id, &changed).is_err());
    assert!(
        l.db.execute("UPDATE approval_audit SET proposal='{}' WHERE id=?", [&id])
            .is_err()
    );
    assert!(
        l.db.execute(
            "DELETE FROM approval_audit_events WHERE approval_id=?",
            [&id]
        )
        .is_err()
    );
}
#[test]
fn expiry_never_implies_execution_and_stopped_grants_remain_unknown() {
    let (mut l, p) = ledger();
    let offered = l.begin_approval_audit(OWNER, &p, 100).unwrap();
    let allowed = l.begin_approval_audit(OWNER, &p, 100).unwrap();
    l.record_approval_event(OWNER, &allowed, Phase::OwnerAllowed, 101, &value!({}))
        .unwrap();
    l.record_approval_event(OWNER, &allowed, Phase::Applying, 102, &value!({}))
        .unwrap();
    assert_eq!(
        l.close_approval_audits(OWNER, "agent", false, 501).unwrap(),
        1
    );
    assert_eq!(l.approval_phase(OWNER, &offered).unwrap(), Phase::Expired);
    assert_eq!(l.approval_phase(OWNER, &allowed).unwrap(), Phase::Applying);
    assert_eq!(
        l.close_approval_audits(OWNER, "agent", true, 502).unwrap(),
        1
    );
    assert_eq!(l.approval_phase(OWNER, &allowed).unwrap(), Phase::Unknown);
    assert_eq!(
        l.close_approval_audits(OWNER, "agent", true, 503).unwrap(),
        0
    );
    assert!(
        l.record_approval_event(OWNER, &allowed, Phase::Completed, 504, &value!({}))
            .is_err()
    );
}
#[test]
fn owner_audit_history_paginates_without_dropping_identical_proposals() {
    let (mut l, p) = ledger();
    for _ in 0..105 {
        l.begin_approval_audit(OWNER, &p, 100).unwrap();
    }
    let first = l.approval_history(OWNER, "agent", None).unwrap();
    assert_eq!(first.records.len(), 100);
    let second = l
        .approval_history(OWNER, "agent", first.next_cursor.as_deref())
        .unwrap();
    assert_eq!(second.records.len(), 5);
    assert!(second.next_cursor.is_none());
    assert!(
        second
            .records
            .iter()
            .all(|a| first.records.iter().all(|b| a.id != b.id))
    );
    assert!(
        l.approval_history(OWNER, "other", first.next_cursor.as_deref())
            .is_err()
    );
}
#[test]
fn owner_snapshot_keeps_exact_audit_across_restore() {
    let (mut source, p) = ledger();
    let id = source.begin_approval_audit(OWNER, &p, 100).unwrap();
    source
        .record_approval_event(
            OWNER,
            &id,
            Phase::OwnerAllowed,
            101,
            &value!({"device":"original"}),
        )
        .unwrap();
    let backup = source.snapshot(OWNER, &"a".repeat(64), 200).unwrap();
    let mut target = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    target
        .restore_snapshot(OWNER, &"a".repeat(64), &backup)
        .unwrap();
    let record = target.approval_record(OWNER, "agent", &id).unwrap();
    assert_eq!(record.events.len(), 2);
    assert_eq!(record.events[1].detail["device"], "original");
    assert_eq!(record.summary.phase, Phase::OwnerAllowed);
    target
        .close_approval_audits(OWNER, "agent", true, 201)
        .unwrap();
    assert_eq!(target.approval_phase(OWNER, &id).unwrap(), Phase::Unknown);
}
