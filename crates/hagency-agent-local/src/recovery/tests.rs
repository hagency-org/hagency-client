use super::*;
use crate::inbox::{Dispatch, Finish, Limits, ServerStart};
const OWNER: &str = "@alice:test";
fn setup() -> (tempfile::TempDir, Ledger, String) {
    let temp = tempfile::tempdir().unwrap();
    let mut l = Ledger::open_scoped(temp.path().join("ledger"), OWNER, &"a".repeat(64)).unwrap();
    let scope = Scope {
        agent: "agent".into(),
        binding: "binding".into(),
        room: "!room:test".into(),
        requester: "@bob:test".into(),
        thread: "$thread:test".into(),
    };
    l.register_binding(OWNER, &scope).unwrap();
    let d = Dispatch {
        id: "original".into(),
        binding_id: scope.binding.clone(),
        agent_id: scope.agent.clone(),
        event_id: "$event:test".into(),
        room_id: scope.room,
        requester_mxid: scope.requester,
        thread_root: scope.thread,
        body: "original instruction".into(),
        state: "offered".into(),
        binding_generation: 1,
        dispatch_epoch: Some(1),
        dispatch_device_id: Some("execution-device".into()),
        execution_id: None,
        outcome: None,
    };
    l.receive_dispatch(OWNER, &d, Limits::default(), 100)
        .unwrap();
    l.acknowledge_dispatch(OWNER, &d.id).unwrap();
    let prepared = l.prepare_execution(OWNER, &d.id).unwrap();
    let mut running = d.clone();
    running.state = "running".into();
    running.execution_id = Some(prepared.execution_id.clone());
    l.confirm_execution_start(
        OWNER,
        &ServerStart {
            dispatch: running,
            newly_started: true,
        },
    )
    .unwrap()
    .unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("answer.txt"), b"existing effect").unwrap();
    l.record_execution_workspace(OWNER, &d.id, &prepared.execution_id, &workspace, true, 101)
        .unwrap();
    (temp, l, prepared.execution_id)
}
fn stop(l: &mut Ledger, execution: &str) {
    l.record_execution_stop(OWNER, "original", execution, "whole_tree", 102)
        .unwrap();
    l.finish_local_execution(OWNER, "original", execution, Finish::Unknown)
        .unwrap();
}
#[test]
fn inspection_and_owner_confirmation_retain_original_unknown_and_actual_files() {
    let (temp, mut l, execution) = setup();
    assert!(
        l.inspect_recovery(OWNER, "agent", "original", &[], 102)
            .is_err()
    );
    stop(&mut l, &execution);
    let paths = vec!["answer.txt".into(), "missing.txt".into()];
    let r = l
        .inspect_recovery(OWNER, "agent", "original", &paths, 110)
        .unwrap();
    assert_eq!(r.evidence.input, "original instruction");
    assert_eq!(r.evidence.state, State::Unknown);
    assert_eq!(
        r.evidence.files[0].sha256.as_deref(),
        Some(format!("{:x}", Sha256::digest(b"existing effect")).as_str())
    );
    assert_eq!(r.evidence.files[1].status, "missing");
    let input = Input {
        review_id: r.id.clone(),
        review_digest: r.digest,
        disposition: Disposition::OwnerConfirmed {
            note: "I checked the existing effect".into(),
        },
    };
    let d = l
        .decide_recovery(OWNER, "agent", "confirm", &input, "owner-device/1", 111)
        .unwrap();
    assert_eq!(d.action.phase, "recorded");
    assert!(d.continuation.is_none());
    assert_eq!(
        l.inbox_record(OWNER, "original").unwrap().state,
        State::Unknown
    );
    assert!(l.prepare_execution(OWNER, "original").is_err());
    assert!(
        l.decide_recovery("@other:test", "agent", "other", &input, "other-device", 112)
            .is_err()
    );
    assert!(
        l.decide_recovery(OWNER, "different", "other", &input, "owner-device/1", 112)
            .is_err()
    );
    assert!(
        l.db.execute("DELETE FROM recovery_actions WHERE id='confirm'", [])
            .is_err()
    );
    assert!(
        l.db.execute(
            "UPDATE recovery_reviews SET evidence='{}' WHERE id=?",
            [r.id]
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(temp.path().join("workspace/answer.txt")).unwrap(),
        b"existing effect"
    );
}
#[test]
fn continuation_is_armed_once_and_lost_or_restored_custody_cannot_replay() {
    let (_temp, mut l, execution) = setup();
    stop(&mut l, &execution);
    let r = l
        .inspect_recovery(OWNER, "agent", "original", &["answer.txt".into()], 110)
        .unwrap();
    let input = Input {
        review_id: r.id,
        review_digest: r.digest,
        disposition: Disposition::Continue {
            note: "reviewed unknown original".into(),
            instruction: "Verify the existing file, then finish the remaining work.".into(),
        },
    };
    let first = l
        .decide_recovery(
            OWNER,
            "agent",
            "continue-original",
            &input,
            "owner-device/1",
            111,
        )
        .unwrap();
    let permit = first.continuation.unwrap();
    assert_eq!(permit.scope.requester, "@bob:test");
    assert!(
        permit
            .instruction
            .contains("original outcome remains Unknown")
    );
    let retry = l
        .decide_recovery(
            OWNER,
            "agent",
            "continue-original",
            &input,
            "new-device/2",
            999,
        )
        .unwrap();
    assert!(retry.continuation.is_none());
    assert_eq!(retry.action.phase, "send_possible");
    assert_eq!(retry.action.deciding_device, "owner-device/1");
    let backup = l.snapshot(OWNER, &"a".repeat(64), 112).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    restored
        .restore_snapshot(OWNER, &"a".repeat(64), &backup)
        .unwrap();
    assert!(
        restored
            .decide_recovery(
                OWNER,
                "agent",
                "continue-original",
                &input,
                "recovery-device",
                113
            )
            .unwrap()
            .continuation
            .is_none()
    );
    assert_eq!(
        l.settle_continuation(OWNER, permit, Some("$new-owner-message:test"))
            .unwrap()
            .phase,
        "sent"
    );
    assert_eq!(
        l.inbox_record(OWNER, "original").unwrap().state,
        State::Unknown
    );
    let mut changed = input.clone();
    changed.disposition = Disposition::Continue {
        note: "changed".into(),
        instruction: "different".into(),
    };
    assert!(
        l.decide_recovery(
            OWNER,
            "agent",
            "continue-original",
            &changed,
            "owner-device/1",
            114
        )
        .is_err()
    );
}
#[test]
fn changed_files_stale_review_and_escaped_paths_are_refused() {
    let (temp, mut l, execution) = setup();
    stop(&mut l, &execution);
    for path in [
        "../outside",
        "/absolute",
        "a/../../outside",
        "a\\b",
        "a:stream",
    ] {
        assert!(
            l.inspect_recovery(OWNER, "agent", "original", &[path.into()], 110)
                .is_err(),
            "{path}"
        );
    }
    let r = l
        .inspect_recovery(OWNER, "agent", "original", &["answer.txt".into()], 110)
        .unwrap();
    let input = Input {
        review_id: r.id,
        review_digest: r.digest,
        disposition: Disposition::KeepBlocked {
            note: "not reviewed yet".into(),
        },
    };
    std::fs::write(temp.path().join("workspace/answer.txt"), b"changed effect").unwrap();
    assert!(
        l.decide_recovery(OWNER, "agent", "changed", &input, "owner-device", 111)
            .is_err()
    );
    let fresh = l
        .inspect_recovery(OWNER, "agent", "original", &[], 120)
        .unwrap();
    let mut stale = input;
    stale.review_id = fresh.id;
    stale.review_digest = fresh.digest;
    assert!(
        l.decide_recovery(OWNER, "agent", "expired", &stale, "owner-device", 420)
            .is_err()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            temp.path().join("ledger"),
            temp.path().join("workspace/escape"),
        )
        .unwrap();
        let r = l
            .inspect_recovery(OWNER, "agent", "original", &["escape".into()], 121)
            .unwrap();
        assert_eq!(r.evidence.files[0].status, "refused");
    }
}

#[test]
fn programming_continuation_requires_original_whole_tree_stop_evidence() {
    for scope in [None, Some("leader")] {
        let (_temp, mut l, execution) = setup();
        if let Some(scope) = scope {
            l.record_execution_stop(OWNER, "original", &execution, scope, 102)
                .unwrap();
        }
        l.finish_local_execution(OWNER, "original", &execution, Finish::Unknown)
            .unwrap();
        let r = l
            .inspect_recovery(OWNER, "agent", "original", &[], 110)
            .unwrap();
        let input = Input {
            review_id: r.id,
            review_digest: r.digest,
            disposition: Disposition::Continue {
                note: "owner reviewed".into(),
                instruction: "continue".into(),
            },
        };
        assert!(matches!(
            l.decide_recovery(OWNER, "agent", "unsafe", &input, "owner-device", 111),
            Err(Error::Denied)
        ));
        assert!(!l.has_recovery_action(OWNER, "agent", "unsafe").unwrap());
    }
}

#[test]
fn stopped_history_keeps_original_inputs_and_pages_beyond_one_hundred_requests() {
    let (_temp, mut l, execution) = setup();
    stop(&mut l, &execution);
    let original = l.inbox_record(OWNER, "original").unwrap().dispatch;
    for n in 1..105 {
        let mut dispatch = original.clone();
        dispatch.id = format!("stopped-{n:03}");
        dispatch.event_id = format!("$stopped-{n:03}:test");
        dispatch.body = format!("original input {n}");
        l.receive_dispatch(OWNER, &dispatch, Limits::default(), 100)
            .unwrap();
        l.acknowledge_dispatch(OWNER, &dispatch.id).unwrap();
        let p = l.prepare_execution(OWNER, &dispatch.id).unwrap();
        l.finish_local_execution(OWNER, &dispatch.id, &p.execution_id, Finish::Unknown)
            .unwrap();
    }
    let first = l.stopped_history(OWNER, "agent", None).unwrap();
    assert_eq!(first.records.len(), 100);
    let second = l
        .stopped_history(OWNER, "agent", first.next_cursor.as_deref())
        .unwrap();
    assert_eq!(second.records.len(), 5);
    assert!(second.next_cursor.is_none());
    assert!(
        second
            .records
            .iter()
            .all(|s| first.records.iter().all(|r| r.id != s.id))
    );
    assert!(first.records.iter().all(|r| !r.input_preview.is_empty()));
    assert!(
        l.stopped_history(OWNER, "other", first.next_cursor.as_deref())
            .is_err()
    );
}
