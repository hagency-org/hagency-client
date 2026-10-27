use super::*;
use crate::{Budget, Layer, Limit, Period, Policy, RequestPolicy, ToolPolicy, inbox};
pub(super) const OWNER: &str = "@alice:test";
pub(super) fn setup(path: impl AsRef<std::path::Path>) -> (Ledger, Request) {
    let mut l = Ledger::open_scoped(path, OWNER, &"a".repeat(64)).unwrap();
    let mut event = inbox::Dispatch {
        id: "dispatch".into(),
        binding_id: "binding".into(),
        agent_id: "agent".into(),
        event_id: "$source".into(),
        room_id: "!project:test".into(),
        requester_mxid: "@bob:test".into(),
        thread_root: "$thread".into(),
        body: "Do the original task".into(),
        state: "offered".into(),
        binding_generation: 1,
        dispatch_epoch: Some(1),
        dispatch_device_id: Some("device".into()),
        execution_id: None,
        outcome: None,
    };
    let scope = event.scope();
    l.register_binding(OWNER, &scope).unwrap();
    for layer in [Layer::Agent, Layer::Room, Layer::Requester] {
        l.set_policy(OWNER, &scope, layer, 0, &policy()).unwrap();
    }
    l.receive_dispatch(OWNER, &event, inbox::Limits::default(), 10)
        .unwrap();
    l.acknowledge_dispatch(OWNER, "dispatch").unwrap();
    let prepared = l.prepare_execution(OWNER, "dispatch").unwrap();
    event.state = "running".into();
    event.execution_id = Some(prepared.execution_id.clone());
    l.confirm_execution_start(
        OWNER,
        &inbox::ServerStart {
            dispatch: event,
            newly_started: true,
        },
    )
    .unwrap()
    .unwrap();
    let proposal = ToolProposal {
        scope,
        dispatch: "dispatch".into(),
        tool: "codex.command".into(),
        arguments: value!({"command":"echo original","itemId":"item"}),
        canonical_directory: "/workspace".into(),
        risk: "high".into(),
        policy_revision: [1, 1, 1],
        expires: 500,
    };
    let approval_id = l.begin_approval_audit(OWNER, &proposal, 20).unwrap();
    let request = Request {
        version: 1,
        owner: OWNER.into(),
        profile: "a".repeat(64),
        approval_id,
        approval_room: "!private:test".into(),
        execution_id: prepared.execution_id,
        binding_generation: 1,
        dispatch_epoch: 1,
        execution_device: ExecutionDevice {
            id: "device".into(),
            generation: 1,
        },
        publisher_matrix_device: "HOST".into(),
        proposal_digest: hash(&proposal).unwrap(),
        proposal,
    };
    (l, request)
}
fn policy() -> Policy {
    Policy {
        budget: Budget {
            limit: Limit::Tokens(100),
            period: Period::Lifetime,
        },
        requests: RequestPolicy::Allow,
        high_risk: ToolPolicy::AskOwner,
    }
}
fn observed(publication: &Publication) -> ObservedVerdict {
    ObservedVerdict {
        observed_at: 32,
        sender: OWNER.into(),
        sender_device: "REMOTE".into(),
        sender_ed25519: "A".repeat(43),
        room: publication.request.approval_room.clone(),
        event_id: "$verdict".into(),
        request_event_id: "$request".into(),
        verdict: Verdict {
            approval_id: publication.request.approval_id.clone(),
            request_digest: publication.digest.clone(),
            approved: true,
        },
    }
}
#[test]
fn remote_publication_retained_receipt_custody_cannot_mint_a_second_send_permit() {
    let (mut ledger, request) = setup(":memory:");
    let mut publication = ledger
        .reserve_remote_approval(OWNER, &request, || 30)
        .unwrap();
    let permit = publication.begin_publication().unwrap();
    assert!(permit.request() == publication.request());
    assert_eq!(permit.transaction_id(), publication.transaction_id());
    assert!(matches!(
        publication.begin_publication(),
        Err(Error::Unknown)
    ));
    drop(permit); // A cancelled or failed original attempt cannot rearm it.
    assert!(matches!(
        publication.begin_publication(),
        Err(Error::Unknown)
    ));
    ledger
        .record_remote_approval_receipt(OWNER, &publication, "$request", 31)
        .unwrap();
    assert!(matches!(
        publication.begin_publication(),
        Err(Error::Unknown)
    ));
    assert!(matches!(
        ledger.reserve_remote_approval(OWNER, &request, || 32),
        Err(Error::Unknown)
    ));
    assert_eq!(
        ledger
            .remote_approval_record(OWNER, "agent", &request.approval_id)
            .unwrap()
            .unwrap()
            .request_event_id
            .as_deref(),
        Some("$request")
    );
}
#[test]
fn remote_publication_commits_once_and_reopen_cannot_recreate_custody() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ledger.sqlite");
    let (mut l, r) = setup(&path);
    let p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    assert!(matches!(
        l.reserve_remote_approval(OWNER, &r, || 31),
        Err(Error::Unknown)
    ));
    let record = l
        .remote_approval_record(OWNER, "agent", &r.approval_id)
        .unwrap()
        .unwrap();
    assert!(record.request == r);
    assert!(record.request_event_id.is_none());
    let txid = p.transaction_id();
    assert!(!txid.contains("original"));
    drop(l);
    let mut l = Ledger::open_scoped(path, OWNER, &"a".repeat(64)).unwrap();
    assert!(matches!(
        l.reserve_remote_approval(OWNER, &r, || 32),
        Err(Error::Unknown)
    ));
    assert!(
        l.record_remote_approval_receipt(OWNER, &p, "$request", 32)
            .is_err(),
        "another Ledger handle has no original custody"
    );
}
#[test]
fn remote_decision_is_atomic_single_use_original_device_and_audit_evidence() {
    let (mut l, r) = setup(":memory:");
    let p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    l.record_remote_approval_receipt(OWNER, &p, "$request", 31)
        .unwrap();
    let decision = l
        .record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 32)
        .unwrap();
    assert_eq!(decision.approval_id(), r.approval_id);
    assert!(decision.approved());
    assert_eq!(
        l.approval_phase(OWNER, &r.approval_id).unwrap(),
        Phase::OwnerAllowed
    );
    assert!(
        l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 33)
            .is_err()
    );
    let record = l
        .remote_approval_record(OWNER, "agent", &r.approval_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.decision.unwrap().sender_device, "REMOTE");
    let audit = l.approval_record(OWNER, "agent", &r.approval_id).unwrap();
    assert_eq!(audit.events.len(), 2);
    assert_eq!(audit.events[1].detail["origin"], "verified_matrix_device");
    assert_eq!(audit.events[1].detail["eventId"], "$verdict");
    assert_eq!(
        l.db.query_row("SELECT count(*) FROM approvals", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        0,
        "an owner decision does not mint tool permission"
    );
}
#[test]
fn remote_wrong_owner_scope_device_receipt_and_digest_never_consume_original() {
    let (mut l, r) = setup(":memory:");
    let p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    assert!(
        l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 31)
            .is_err(),
        "no original send receipt"
    );
    l.record_remote_approval_receipt(OWNER, &p, "$request", 31)
        .unwrap();
    for change in 0..7 {
        let mut observation = observed(&p);
        match change {
            0 => observation.sender = "@mallory:test".into(),
            1 => observation.room = "!other:test".into(),
            2 => observation.sender_device = "HOST".into(),
            3 => observation.request_event_id = "$other".into(),
            4 => observation.verdict.request_digest = "b".repeat(64),
            5 => observation.verdict.approval_id = "c".repeat(64),
            _ => observation.event_id = "$request".into(),
        };
        assert!(
            l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observation, || 32)
                .is_err()
        );
        assert_eq!(
            l.approval_phase(OWNER, &r.approval_id).unwrap(),
            Phase::Offered
        );
    }
    let changed = ExecutionDevice {
        generation: 2,
        ..r.execution_device.clone()
    };
    assert!(
        l.record_remote_approval_decision(OWNER, &p, &changed, &observed(&p), || 32)
            .is_err()
    );
    l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 32)
        .unwrap();
}
#[test]
fn remote_policy_change_and_expiry_preserve_offered_evidence_without_deciding() {
    let (mut l, r) = setup(":memory:");
    let p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    l.record_remote_approval_receipt(OWNER, &p, "$request", 31)
        .unwrap();
    assert!(
        l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 500)
            .is_err()
    );
    l.set_policy(OWNER, &r.proposal.scope, Layer::Room, 1, &policy())
        .unwrap();
    assert!(
        l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 32)
            .is_err()
    );
    assert_eq!(
        l.approval_phase(OWNER, &r.approval_id).unwrap(),
        Phase::Offered
    );
}
#[test]
fn remote_late_receipt_does_not_reopen_stopped_or_locally_decided_approval() {
    let (mut l, r) = setup(":memory:");
    let p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    l.record_approval_event(
        OWNER,
        &r.approval_id,
        Phase::OwnerDenied,
        31,
        &value!({"origin":"local"}),
    )
    .unwrap();
    l.record_remote_approval_receipt(OWNER, &p, "$request", 900)
        .unwrap();
    assert!(
        l.record_remote_approval_receipt(OWNER, &p, "$replacement", 901)
            .is_err()
    );
    assert!(
        l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 32)
            .is_err()
    );
    assert_eq!(
        l.approval_phase(OWNER, &r.approval_id).unwrap(),
        Phase::OwnerDenied
    );
}
#[test]
fn remote_snapshot_restores_all_witnesses_without_copying_live_publication_custody() {
    let (mut l, r) = setup(":memory:");
    let p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    l.record_remote_approval_receipt(OWNER, &p, "$request", 31)
        .unwrap();
    let snap = l.snapshot(OWNER, &r.profile, 32).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &r.profile).unwrap();
    restored.restore_snapshot(OWNER, &r.profile, &snap).unwrap();
    let record = restored
        .remote_approval_record(OWNER, "agent", &r.approval_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.request_event_id.as_deref(), Some("$request"));
    assert!(matches!(
        restored.reserve_remote_approval(OWNER, &r, || 33),
        Err(Error::Unknown)
    ));
    assert!(
        restored
            .record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 33)
            .is_err()
    );
    l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 33)
        .unwrap();
}
#[test]
fn remote_request_cannot_change_original_proposal_or_replace_publication_destination() {
    let (mut l, r) = setup(":memory:");
    let mut changed = r.clone();
    changed.proposal.arguments = value!({"command":"different"});
    changed.proposal_digest = hash(&changed.proposal).unwrap();
    assert!(l.reserve_remote_approval(OWNER, &changed, || 30).is_err());
    let _p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    for room in ["!other:test", "!project:test"] {
        let mut changed = r.clone();
        changed.approval_room = room.into();
        assert!(l.reserve_remote_approval(OWNER, &changed, || 31).is_err());
    }
    let mut large = r.clone();
    large.proposal.arguments = value!({"large":"x".repeat(MAX_REQUEST_BYTES)});
    large.proposal_digest = hash(&large.proposal).unwrap();
    assert!(large.validate().is_err());
    assert!(
        l.remote_approval_record(OWNER, "other-agent", &r.approval_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn pre_remote_snapshot_preserves_original_audit_and_adds_no_remote_witness() {
    let (l, r) = setup(":memory:");
    let original = l.approval_record(OWNER, "agent", &r.approval_id).unwrap();
    l.db.execute_batch("DROP TABLE remote_verdict_receipts; DROP TABLE remote_verdict_publications; DROP TABLE remote_approval_decisions; DROP TABLE remote_approval_receipts; DROP TABLE remote_approval_publications").unwrap();
    let snap = l.snapshot(OWNER, &r.profile, 40).unwrap();
    let inspected =
        crate::backup::Snapshot::inspect(snap.bytes().to_vec(), OWNER, &r.profile).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &r.profile).unwrap();
    restored
        .restore_snapshot(OWNER, &r.profile, &inspected)
        .unwrap();
    assert!(
        restored
            .remote_approval_record(OWNER, "agent", &r.approval_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        serde_json::to_value(
            restored
                .approval_record(OWNER, "agent", &r.approval_id)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(original).unwrap()
    );
}
#[test]
fn remote_proof_age_execution_epoch_and_unknown_stop_are_not_grants() {
    let (mut l, r) = setup(":memory:");
    let p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    assert!(
        l.record_remote_approval_receipt(OWNER, &p, "$request", 29)
            .is_err(),
        "receipt cannot predate the publication"
    );
    l.record_remote_approval_receipt(OWNER, &p, "$request", 31)
        .unwrap();
    assert!(
        l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 38)
            .is_err()
    );
    let mut future = observed(&p);
    future.observed_at = 34;
    assert!(
        l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &future, || 33)
            .is_err()
    );
    let mut changed = r.clone();
    changed.dispatch_epoch = 2;
    assert!(l.reserve_remote_approval(OWNER, &changed, || 33).is_err());
    l.db.execute(
        "UPDATE local_inbox SET state='unknown' WHERE id='dispatch'",
        [],
    )
    .unwrap();
    assert!(
        l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 33)
            .is_err()
    );
    assert_eq!(
        l.approval_phase(OWNER, &r.approval_id).unwrap(),
        Phase::Offered
    );
}
#[test]
fn remote_decision_retains_denial_and_permanent_witnesses_through_restore() {
    let (mut l, r) = setup(":memory:");
    let p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    l.record_remote_approval_receipt(OWNER, &p, "$request", 31)
        .unwrap();
    let mut observation = observed(&p);
    observation.verdict.approved = false;
    assert!(
        !l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observation, || 32)
            .unwrap()
            .approved()
    );
    for table in [
        "remote_approval_decisions",
        "remote_approval_receipts",
        "remote_approval_publications",
    ] {
        assert!(l.db.execute(&format!("DELETE FROM {table}"), []).is_err());
    }
    let snapshot = l.snapshot(OWNER, &r.profile, 40).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &r.profile).unwrap();
    restored
        .restore_snapshot(OWNER, &r.profile, &snapshot)
        .unwrap();
    let record = restored
        .remote_approval_record(OWNER, "agent", &r.approval_id)
        .unwrap()
        .unwrap();
    assert!(!record.decision.unwrap().approved);
    assert_eq!(
        restored.approval_phase(OWNER, &r.approval_id).unwrap(),
        Phase::OwnerDenied
    );
}

#[test]
fn remote_policy_update_during_sqlite_writer_wait_cannot_commit_a_stale_decision() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ledger.sqlite");
    let (mut l, r) = setup(&path);
    let p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    l.record_remote_approval_receipt(OWNER, &p, "$request", 31)
        .unwrap();
    let key = r.proposal.scope.layers().unwrap()[1].clone();
    let (ready, rx) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        db.execute(
            "UPDATE policies SET revision=revision+1 WHERE scope=?",
            [key],
        )
        .unwrap();
        ready.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        db.execute_batch("COMMIT").unwrap();
    });
    rx.recv().unwrap();
    let result =
        l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || 32);
    writer.join().unwrap();
    assert!(
        result.is_err(),
        "must recheck the actual policy after obtaining the writer lock"
    );
    assert_eq!(
        l.approval_phase(OWNER, &r.approval_id).unwrap(),
        Phase::Offered
    );
}

#[test]
fn remote_actual_serialized_capacity_is_permanent_and_failed_reservation_is_not_a_publication() {
    let (mut l, mut r) = setup(":memory:");
    r.proposal.arguments = value!({"preview":"x".repeat(28000)});
    r.proposal_digest = hash(&r.proposal).unwrap();
    let mut accepted = 0;
    loop {
        r.approval_id = l.begin_approval_audit(OWNER, &r.proposal, 20).unwrap();
        match l.reserve_remote_approval(OWNER, &r, || 30) {
            Ok(_) => accepted += 1,
            Err(Error::Invalid("remote approval byte capacity")) => break,
            _ => panic!("unexpected bounded remote publication result"),
        }
    }
    assert!(accepted > 100 && accepted < MAX_PUBLICATIONS);
    assert!(
        l.remote_approval_record(OWNER, "agent", &r.approval_id)
            .unwrap()
            .is_none()
    );
    let bytes: usize =
        l.db.query_row(
            "SELECT sum(length(CAST(request AS BLOB))) FROM remote_approval_publications",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(bytes <= MAX_PUBLICATION_BYTES && bytes > MAX_PUBLICATION_BYTES - 32768);
    assert!(
        l.db.execute("DELETE FROM remote_approval_publications", [])
            .is_err()
    );
}

#[test]
fn remote_clock_sample_after_actual_writer_wait_rejects_expiry_without_moving_deadline() {
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ledger.sqlite");
    let (mut l, r) = setup(&path);
    let p = l.reserve_remote_approval(OWNER, &r, || 30).unwrap();
    l.record_remote_approval_receipt(OWNER, &p, "$request", 31)
        .unwrap();
    let clock = Arc::new(AtomicI64::new(32));
    let other = clock.clone();
    let (ready, rx) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        ready.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        other.store(500, Ordering::SeqCst);
        db.execute_batch("COMMIT").unwrap();
    });
    rx.recv().unwrap();
    assert!(
        l.record_remote_approval_decision(OWNER, &p, &r.execution_device, &observed(&p), || clock
            .load(Ordering::SeqCst))
            .is_err()
    );
    writer.join().unwrap();
    assert_eq!(
        l.approval_phase(OWNER, &r.approval_id).unwrap(),
        Phase::Offered
    );
}
