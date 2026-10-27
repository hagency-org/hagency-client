use super::*;
const OWNER: &str = super::super::tests::OWNER;
fn observed() -> ObservedRequest {
    let (_, request) = super::super::tests::setup(":memory:");
    ObservedRequest {
        observed_at: 30,
        event_id: "$request".into(),
        sender_ed25519: "A".repeat(43),
        reviewer_matrix_device: "REMOTE".into(),
        request,
    }
}
#[test]
fn remote_reviewer_original_publication_cannot_rearm_after_drop_reopen_or_conflicting_choice() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("review.sqlite");
    let mut l = Ledger::open_scoped(&path, OWNER, &"a".repeat(64)).unwrap();
    let o = observed();
    let digest = o.request.digest().unwrap();
    let mut publication = l
        .reserve_remote_verdict(OWNER, &o, &digest, true, || 31)
        .unwrap();
    let permit = publication.begin_publication().unwrap();
    assert_eq!(
        permit.content().unwrap()["m.relates_to"]["event_id"],
        "$request"
    );
    assert!(publication.begin_publication().is_err());
    drop(permit);
    assert!(
        l.reserve_remote_verdict(OWNER, &o, &digest, false, || 32)
            .is_err()
    );
    let mut reopened = Ledger::open_scoped(&path, OWNER, &"a".repeat(64)).unwrap();
    assert!(
        reopened
            .reserve_remote_verdict(OWNER, &o, &digest, true, || 32)
            .is_err()
    );
    assert!(
        reopened
            .record_remote_verdict_receipt(OWNER, &publication, "$verdict", 33)
            .is_err()
    );
    l.record_remote_verdict_receipt(OWNER, &publication, "$verdict", 1000)
        .unwrap();
    l.record_remote_verdict_receipt(OWNER, &publication, "$verdict", 1001)
        .unwrap();
    assert!(
        l.record_remote_verdict_receipt(OWNER, &publication, "$other", 1002)
            .is_err()
    );
    let r = l
        .remote_verdict_record(OWNER, &o.request.approval_id)
        .unwrap()
        .unwrap();
    assert!(r.approved);
    assert_eq!(r.event_id.as_deref(), Some("$verdict"));
    assert_eq!(
        l.db.query_row("SELECT COUNT(*) FROM approvals", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(
        l.db.execute("DELETE FROM remote_verdict_publications", [])
            .is_err()
    );
}
#[test]
fn remote_reviewer_fresh_verified_scope_and_exact_reviewed_digest_precede_any_reservation() {
    let mut l = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    let o = observed();
    let digest = o.request.digest().unwrap();
    for wrong in 0..7 {
        let mut o = observed();
        match wrong {
            0 => o.request.owner = "@foreign:test".into(),
            1 => o.request.profile = "b".repeat(64),
            2 => o.reviewer_matrix_device = "HOST".into(),
            3 => o.observed_at = 20,
            4 => o.observed_at = 40,
            5 => o.request.proposal.expires = 31,
            _ => o.event_id = "plain".into(),
        };
        assert!(
            l.reserve_remote_verdict(OWNER, &o, &digest, true, || 31)
                .is_err()
        );
    }
    assert!(
        l.reserve_remote_verdict(OWNER, &o, &"0".repeat(64), true, || 31)
            .is_err()
    );
    assert!(
        l.remote_verdict_record(OWNER, &o.request.approval_id)
            .unwrap()
            .is_none()
    );
    assert!(
        l.reserve_remote_verdict(OWNER, &o, &digest, false, || 31)
            .is_ok()
    );
}
#[test]
fn remote_reviewer_snapshot_retains_unknown_decision_and_previous_schema_migrates_empty() {
    let mut l = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    let o = observed();
    let digest = o.request.digest().unwrap();
    let mut p = l
        .reserve_remote_verdict(OWNER, &o, &digest, false, || 31)
        .unwrap();
    drop(p.begin_publication().unwrap());
    let snapshot = l.snapshot(OWNER, &"a".repeat(64), 32).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    restored
        .restore_snapshot(OWNER, &"a".repeat(64), &snapshot)
        .unwrap();
    assert!(
        restored
            .reserve_remote_verdict(OWNER, &o, &digest, true, || 33)
            .is_err()
    );
    let r = restored
        .remote_verdict_record(OWNER, &o.request.approval_id)
        .unwrap()
        .unwrap();
    assert!(!r.approved);
    assert!(r.event_id.is_none());
    let old = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    old.db
        .execute_batch("DROP TABLE encrypted_outputs;DROP TABLE encrypted_inputs; DROP TABLE remote_verdict_receipts; DROP TABLE remote_verdict_publications")
        .unwrap();
    let snapshot = old.snapshot(OWNER, &"a".repeat(64), 32).unwrap();
    let mut migrated = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    migrated
        .restore_snapshot(OWNER, &"a".repeat(64), &snapshot)
        .unwrap();
    assert!(
        migrated
            .remote_verdict_record(OWNER, &o.request.approval_id)
            .unwrap()
            .is_none()
    );
}
#[test]
fn remote_reviewer_capacity_counts_actual_utf8_and_never_prunes_for_a_retry() {
    let mut l = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    let mut o = observed();
    o.request.proposal.arguments = value!({"command":"界".repeat(9000)});
    o.request.proposal_digest = hash(&o.request.proposal).unwrap();
    let mut accepted = 0;
    loop {
        o.request.approval_id = hash(&value!({"index":accepted})).unwrap();
        o.event_id = format!("$request{accepted}");
        let digest = o.request.digest().unwrap();
        if l.reserve_remote_verdict(OWNER, &o, &digest, true, || 31)
            .is_err()
        {
            break;
        }
        accepted += 1;
        assert!(accepted < MAX_PUBLICATIONS);
    }
    assert!(accepted > 200);
    let count: i64 =
        l.db.query_row(
            "SELECT COUNT(*) FROM remote_verdict_publications",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, accepted as i64);
    assert!(
        l.remote_verdict_record(OWNER, &o.request.approval_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn remote_reviewer_clock_after_actual_writer_wait_cannot_extend_request_or_sdk_proof() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("review.sqlite");
    let mut l = Ledger::open_scoped(&path, OWNER, &"a".repeat(64)).unwrap();
    let o = observed();
    let digest = o.request.digest().unwrap();
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let clock = Arc::new(AtomicI64::new(31));
    let at = clock.clone();
    let started = Arc::new(AtomicBool::new(false));
    let entered = started.clone();
    let job = std::thread::spawn(move || {
        entered.store(true, Ordering::SeqCst);
        l.reserve_remote_verdict(OWNER, &o, &digest, true, || at.load(Ordering::SeqCst))
            .is_err()
    });
    while !started.load(Ordering::SeqCst) {
        std::thread::yield_now();
    }
    std::thread::sleep(std::time::Duration::from_millis(40));
    assert!(!job.is_finished());
    clock.store(500, Ordering::SeqCst);
    writer.execute_batch("COMMIT").unwrap();
    assert!(job.join().unwrap());
    let l = Ledger::open_scoped(&path, OWNER, &"a".repeat(64)).unwrap();
    assert!(
        l.remote_verdict_record(OWNER, &observed().request.approval_id)
            .unwrap()
            .is_none()
    );
}
