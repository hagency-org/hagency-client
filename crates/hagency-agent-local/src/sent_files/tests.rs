use super::*;
use crate::ToolPolicy;
pub(crate) static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub(crate) const OWNER: &str = "@alice:test";
pub(crate) fn ledger(path: &std::path::Path) -> (Ledger, Scope, String) {
    use crate::{Budget, Layer, Limit, Period, Policy, RequestPolicy, inbox};
    let mut l = Ledger::open_scoped(path, OWNER, &"a".repeat(64)).unwrap();
    let mut event = inbox::Dispatch {
        id: "dispatch".into(),
        binding_id: "binding".into(),
        agent_id: "agent".into(),
        event_id: "$source:test".into(),
        room_id: "!room:test".into(),
        requester_mxid: "@bob:test".into(),
        thread_root: "$root:test".into(),
        body: "Analyze original file".into(),
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
        l.set_policy(
            OWNER,
            &scope,
            layer,
            0,
            &Policy {
                budget: Budget {
                    limit: Limit::Tokens(100),
                    period: Period::Lifetime,
                },
                requests: RequestPolicy::Allow,
                high_risk: ToolPolicy::AskOwner,
            },
        )
        .unwrap();
    }
    l.receive_dispatch(OWNER, &event, inbox::Limits::default(), 10)
        .unwrap();
    l.acknowledge_dispatch(OWNER, "dispatch").unwrap();
    let p = l.prepare_execution(OWNER, "dispatch").unwrap();
    event.state = "running".into();
    event.execution_id = Some(p.execution_id.clone());
    l.confirm_execution_start(
        OWNER,
        &inbox::ServerStart {
            dispatch: event,
            newly_started: true,
        },
    )
    .unwrap()
    .unwrap();
    l.reserve(&scope, "model-call", "dispatch", 10, 20).unwrap();
    (l, scope, p.execution_id)
}
fn capture(
    sender: &Sender,
    l: &Ledger,
    scope: &Scope,
    execution: &str,
    path: &str,
    call: &str,
) -> Captured {
    sender
        .capture(
            l, scope, "dispatch", execution, path, None, None, "thread", "turn", call, 20,
        )
        .unwrap()
}
fn approve(l: &mut Ledger, c: &Captured) -> String {
    let p = c.proposal();
    let audit = l.begin_approval_audit(OWNER, p, 20).unwrap();
    l.record_approval_event(
        OWNER,
        &audit,
        crate::audit::Phase::OwnerAllowed,
        20,
        &serde_json::json!({}),
    )
    .unwrap();
    l.approve_tool(OWNER, p, 20).unwrap();
    l.record_approval_event(
        OWNER,
        &audit,
        crate::audit::Phase::Applying,
        20,
        &serde_json::json!({}),
    )
    .unwrap();
    audit
}
#[test]
fn frozen_bytes_and_actual_source_objects_precede_one_shot_durable_custody() {
    let _lock = LOCK.blocking_lock();
    let t = tempfile::tempdir().unwrap();
    let cwd = t.path().canonicalize().unwrap();
    std::fs::write(cwd.join("file.txt"), b"hello").unwrap();
    let (mut l, scope, execution) = ledger(&cwd.join("ledger"));
    let sender = Sender::open(&cwd).unwrap();
    let c = capture(&sender, &l, &scope, &execution, "file.txt", "first");
    assert_eq!(c.manifest().bytes, 5);
    let audit = approve(&mut l, &c);
    let id = c.id().to_owned();
    let custody = sender.begin(&mut l, c, &audit, 20).unwrap();
    assert_eq!(custody.bytes(), b"hello");
    assert_eq!(
        l.sent_file_summary(&scope, "dispatch", &execution)
            .unwrap()
            .unconfirmed,
        1
    );
    std::fs::write(cwd.join("file.txt"), b"changed_after_begin").unwrap();
    assert_eq!(
        custody.bytes(),
        b"hello",
        "frozen approved bytes cannot be reread from changed source"
    );
    std::fs::write(cwd.join("file.txt"), b"hello").unwrap();
    assert!(
        l.sent_file(&scope, "dispatch", &execution, &id)
            .unwrap()
            .unwrap()
            .remote
            .is_none()
    );
    assert!(
        l.sent_file(&scope, "dispatch", "foreign", &id)
            .unwrap()
            .is_none()
    );
    drop(custody);
    drop(l);
    let mut l = Ledger::open(cwd.join("ledger"), OWNER).unwrap();
    let c = capture(
        &sender,
        &l,
        &scope,
        &execution,
        "file.txt",
        "different-provider-call",
    );
    assert_eq!(c.id(), id);
    let audit = approve(&mut l, &c);
    assert!(matches!(
        sender.begin(&mut l, c, &audit, 20),
        Err(Error::Unknown)
    ));
    std::fs::write(cwd.join("file.txt"), b"changed").unwrap();
    assert!(matches!(
        sender.capture(
            &l, &scope, "dispatch", &execution, "file.txt", None, None, "thread", "turn",
            "changed", 20
        ),
        Err(Error::Unknown)
    ));
}
#[test]
fn changed_source_or_parent_after_approval_never_starts_io() {
    let _lock = LOCK.blocking_lock();
    for change_parent in [false, true] {
        let t = tempfile::tempdir().unwrap();
        let cwd = t.path().canonicalize().unwrap();
        std::fs::create_dir(cwd.join("sub")).unwrap();
        std::fs::write(cwd.join("sub/file.txt"), b"hello").unwrap();
        let (mut l, scope, execution) = ledger(&cwd.join("ledger"));
        let sender = Sender::open(&cwd).unwrap();
        let c = capture(&sender, &l, &scope, &execution, "sub/file.txt", "first");
        let audit = approve(&mut l, &c);
        if change_parent {
            std::fs::rename(cwd.join("sub"), cwd.join("old")).unwrap();
            std::fs::create_dir(cwd.join("sub")).unwrap();
            std::fs::write(cwd.join("sub/file.txt"), b"hello").unwrap();
        } else {
            std::fs::write(cwd.join("sub/file.txt"), b"other").unwrap();
        }
        assert!(sender.begin(&mut l, c, &audit, 20).is_err());
        assert_eq!(
            l.db.query_row("SELECT count(*) FROM sent_files", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}
#[test]
fn outgoing_capture_refuses_escape_links_special_objects_and_oversize() {
    let _lock = LOCK.blocking_lock();
    let t = tempfile::tempdir().unwrap();
    let cwd = t.path().canonicalize().unwrap();
    std::fs::write(cwd.join("file.txt"), b"hello").unwrap();
    std::fs::create_dir(cwd.join("sub")).unwrap();
    std::os::unix::fs::symlink(cwd.join("file.txt"), cwd.join("link")).unwrap();
    std::os::unix::fs::symlink(cwd.join("sub"), cwd.join("dir-link")).unwrap();
    std::fs::hard_link(cwd.join("file.txt"), cwd.join("hard")).unwrap();
    let big = std::fs::File::create(cwd.join("big")).unwrap();
    big.set_len(crate::file_delivery::MAX_BYTES as u64 + 1)
        .unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(cwd.join("socket")).unwrap();
    let (l, scope, execution) = ledger(&cwd.join("ledger"));
    let sender = Sender::open(&cwd).unwrap();
    for path in [
        "../file.txt",
        "/file.txt",
        "sub/../file.txt",
        "sub//file.txt",
        "link",
        "hard",
        "file.txt",
        "dir-link/a",
        "sub",
        "big",
        "socket",
    ] {
        assert!(
            sender
                .capture(
                    &l, &scope, "dispatch", &execution, path, None, None, "thread", "turn", "call",
                    20
                )
                .is_err(),
            "accepted {path}"
        );
    }
}
#[test]
fn snapshot_and_late_original_receipt_preserve_history_without_rearming() {
    let _lock = LOCK.blocking_lock();
    let t = tempfile::tempdir().unwrap();
    let cwd = t.path().canonicalize().unwrap();
    std::fs::write(cwd.join("file.txt"), b"hello").unwrap();
    let (mut l, scope, execution) = ledger(&cwd.join("ledger"));
    let sender = Sender::open(&cwd).unwrap();
    let c = capture(&sender, &l, &scope, &execution, "file.txt", "first");
    let audit = approve(&mut l, &c);
    let custody = sender.begin(&mut l, c, &audit, 20).unwrap();
    let mut r = custody.record().clone();
    drop(custody);
    let receipt = Delivery {
        id: "fil_original".into(),
        dispatch_id: "dispatch".into(),
        execution_id: execution.clone(),
        request_id: r.request_id.clone(),
        created_at_ms: 1,
        manifest: r.manifest.clone(),
        manifest_digest: r.manifest.digest().unwrap(),
        received_bytes: 5,
        state: "unknown".into(),
        upload_accepted: true,
        matrix_event_id: None,
    };
    r = l
        .observe_sent_file(&scope, "dispatch", &execution, &r.id, receipt.clone())
        .unwrap();
    let profile = "a".repeat(64);

    let snapshot = l.snapshot(OWNER, &profile, 30).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &profile).unwrap();
    restored
        .restore_snapshot(OWNER, &profile, &snapshot)
        .unwrap();
    let c = capture(
        &sender,
        &restored,
        &scope,
        &execution,
        "file.txt",
        "new-after-restore",
    );
    let audit = approve(&mut restored, &c);
    assert!(matches!(
        sender.begin(&mut restored, c, &audit, 20),
        Err(Error::Unknown)
    ));
    let other = cwd.join("replacement-workspace");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("file.txt"), b"hello").unwrap();
    let different_sender = Sender::open(&other).unwrap();
    assert!(
        matches!(
            different_sender.capture(
                &restored,
                &scope,
                "dispatch",
                &execution,
                "file.txt",
                None,
                None,
                "thread",
                "turn",
                "different-workspace",
                20
            ),
            Err(Error::Unknown)
        ),
        "restore must not rearm by changing workspace path"
    );
    let mut wrong = receipt.clone();
    wrong.state = "queued".into();
    wrong.upload_accepted = false;
    assert!(
        restored
            .observe_sent_file(&scope, "dispatch", &execution, &r.id, wrong)
            .is_err()
    );
    let mut actual = receipt;
    actual.state = "sent".into();
    actual.matrix_event_id = Some("$actualOpaqueEvent".into());
    restored
        .observe_sent_file(&scope, "dispatch", &execution, &r.id, actual.clone())
        .unwrap();
    let summary = restored
        .sent_file_summary(&scope, "dispatch", &execution)
        .unwrap();
    assert_eq!((summary.sent, summary.unconfirmed), (1, 0));
    let mut changed = actual;
    changed.matrix_event_id = Some("$other".into());
    assert!(
        restored
            .observe_sent_file(&scope, "dispatch", &execution, &r.id, changed)
            .is_err()
    );
    assert!(restored.db.execute("DELETE FROM sent_files", []).is_err());
}

#[test]
fn sender_contexts_settle_and_unknown_usage_resets_only_the_exact_scope() {
    use crate::Usage;
    for prefix in [
        crate::codex::FILE_CAPABILITY_VERSION.to_owned(),
        format!(
            "{}:{}",
            crate::codex::PROGRAMMING_CAPABILITY_VERSION,
            "a".repeat(64)
        ),
    ] {
        let t = tempfile::tempdir().unwrap();
        let (mut ledger, scope, _) = ledger(&t.path().join("ledger"));
        let base = scope.context_key(OWNER).unwrap();
        let context = format!("{prefix}:{}:{base}", CAPABILITY_VERSION);
        let usage = Usage {
            input: 3,
            output: 2,
            cached_input: 1,
            reasoning_output: 0,
            accounting_version: "fixture".into(),
        };
        assert!(
            ledger
                .settle_codex(
                    &scope,
                    "model-call",
                    &usage,
                    &format!("foreign:{base}"),
                    "{}"
                )
                .is_err()
        );
        ledger
            .settle_codex(&scope, "model-call", &usage, &context, "{\"total\":5}")
            .unwrap();
        ledger
            .db
            .execute(
                "INSERT INTO codex_usage VALUES ('foreign-context','original')",
                [],
            )
            .unwrap();
        ledger
            .reserve(&scope, "unknown-call", "dispatch", 10, 40)
            .unwrap();
        ledger.mark_unknown(&scope, "unknown-call").unwrap();
        assert_eq!(
            ledger
                .db
                .query_row(
                    "SELECT counters FROM codex_usage WHERE scope=?",
                    [&context],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "null"
        );
        assert_eq!(
            ledger
                .db
                .query_row(
                    "SELECT counters FROM codex_usage WHERE scope='foreign-context'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "original"
        );
    }
}

#[test]
fn replaced_workspace_and_local_policy_revocation_refuse_custody() {
    let _lock = LOCK.blocking_lock();
    for replace in [true, false] {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().canonicalize().unwrap();
        let cwd = base.join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        std::fs::write(cwd.join("file.txt"), b"hello").unwrap();
        let (mut l, scope, execution) = ledger(&base.join("ledger"));
        let sender = Sender::open(&cwd).unwrap();
        let c = capture(&sender, &l, &scope, &execution, "file.txt", "first");
        let audit = approve(&mut l, &c);
        if replace {
            std::fs::rename(&cwd, base.join("old")).unwrap();
            std::fs::create_dir(&cwd).unwrap();
            std::fs::write(cwd.join("file.txt"), b"hello").unwrap();
        } else {
            let snapshot = l.policy_snapshot(&scope).unwrap();
            l.set_policy(
                OWNER,
                &scope,
                crate::Layer::Agent,
                snapshot[0].revision,
                &crate::Policy {
                    budget: crate::Budget {
                        limit: crate::Limit::Tokens(100),
                        period: crate::Period::Lifetime,
                    },
                    requests: crate::RequestPolicy::Allow,
                    high_risk: ToolPolicy::Deny,
                },
            )
            .unwrap();
        }
        assert!(sender.begin(&mut l, c, &audit, 20).is_err());
        assert_eq!(
            l.db.query_row("SELECT count(*) FROM sent_files", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}
#[test]
fn pre_sender_snapshot_adds_empty_history_without_inventing_custody() {
    let t = tempfile::tempdir().unwrap();
    let (l, scope, _) = ledger(&t.path().join("ledger"));
    l.db.execute_batch("DROP TRIGGER reject_absent_file_update; DROP TABLE sent_file_absence; DROP TABLE sent_files").unwrap();
    let profile = "a".repeat(64);
    let snapshot = l.snapshot(OWNER, &profile, 30).unwrap();
    let mut target = Ledger::open_scoped(":memory:", OWNER, &profile).unwrap();
    target.restore_snapshot(OWNER, &profile, &snapshot).unwrap();
    assert!(
        target
            .sent_file(&scope, "dispatch", "execution", "missing")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        target
            .db
            .query_row("SELECT count(*) FROM sent_files", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn capture_permits_bound_live_bytes_and_release_on_drop() {
    let _lock = LOCK.blocking_lock();
    let t = tempfile::tempdir().unwrap();
    let cwd = t.path().canonicalize().unwrap();
    std::fs::write(cwd.join("file.txt"), b"hello").unwrap();
    let (l, scope, execution) = ledger(&cwd.join("ledger"));
    let sender = Sender::open(&cwd).unwrap();
    let mut captures = (0..4)
        .map(|i| {
            capture(
                &sender,
                &l,
                &scope,
                &execution,
                "file.txt",
                &format!("call-{i}"),
            )
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        sender.capture(
            &l, &scope, "dispatch", &execution, "file.txt", None, None, "thread", "turn", "fifth",
            20
        ),
        Err(Error::Invalid("outgoing capture capacity"))
    ));
    captures.pop();
    let again = capture(&sender, &l, &scope, &execution, "file.txt", "released");
    assert_eq!(again.manifest().bytes, 5);
}
#[test]
fn actual_unresolved_byte_quota_is_retained_without_storing_file_blobs() {
    use std::io::Write;
    let _lock = LOCK.blocking_lock();
    let t = tempfile::tempdir().unwrap();
    let cwd = t.path().canonicalize().unwrap();
    let (mut l, scope, execution) = ledger(&cwd.join("ledger"));
    let sender = Sender::open(&cwd).unwrap();
    for i in 0u8..17 {
        let name = format!("file-{i}.bin");
        let mut file = std::fs::File::create(cwd.join(&name)).unwrap();
        file.set_len(crate::file_delivery::MAX_BYTES as u64)
            .unwrap();
        file.write_all(&[i]).unwrap();
        drop(file);
        let c = capture(&sender, &l, &scope, &execution, &name, &format!("call-{i}"));
        let audit = approve(&mut l, &c);
        let result = sender.begin(&mut l, c, &audit, 20);
        if i == 16 {
            assert!(matches!(result, Err(Error::Invalid("sent file quota"))));
        } else {
            drop(result.unwrap());
        }
    }
    assert_eq!(
        l.db.query_row("SELECT count(*) FROM sent_files", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        16
    );
}

fn lookup(r: &Record, execution: &str, closed: bool) -> FileObservation {
    FileObservation {
        request: FileLookup {
            dispatch_id: "dispatch".into(),
            execution_id: execution.into(),
            request_id: r.request_id.clone(),
        },
        delivery: None,
        preparation_closed: closed,
    }
}
#[test]
fn history_final_absence_is_permanent_scoped_and_never_a_new_send_grant() {
    let _lock = LOCK.blocking_lock();
    let t = tempfile::tempdir().unwrap();
    let cwd = t.path().canonicalize().unwrap();
    std::fs::write(cwd.join("file.txt"), b"hello").unwrap();
    let (mut l, scope, execution) = ledger(&cwd.join("ledger"));
    let sender = Sender::open(&cwd).unwrap();
    let c = capture(&sender, &l, &scope, &execution, "file.txt", "first");
    let audit = approve(&mut l, &c);
    let custody = sender.begin(&mut l, c, &audit, 20).unwrap();
    let r = custody.record().clone();
    drop(custody);
    assert!(l.sent_file_record("@foreign:test", "agent", &r.id).is_err());
    assert!(l.sent_file_record(OWNER, "other-agent", &r.id).is_err());
    let temporary = l
        .observe_file_lookup(
            &scope,
            "dispatch",
            &execution,
            &r.id,
            lookup(&r, &execution, false),
            30,
        )
        .unwrap();
    assert!(temporary.absence.is_none());
    assert_eq!(
        l.sent_file_summary(&scope, "dispatch", &execution)
            .unwrap()
            .unconfirmed,
        1
    );
    let mut wrong = lookup(&r, &execution, true);
    wrong.request.request_id = "other".into();
    assert!(
        l.observe_file_lookup(&scope, "dispatch", &execution, &r.id, wrong, 31)
            .is_err()
    );
    let final_record = l
        .observe_file_lookup(
            &scope,
            "dispatch",
            &execution,
            &r.id,
            lookup(&r, &execution, true),
            32,
        )
        .unwrap();
    assert!(final_record.remote.is_none());
    assert_eq!(final_record.absence.as_ref().unwrap().observed_at, 32);
    assert_eq!(
        l.sent_file_summary(&scope, "dispatch", &execution)
            .unwrap()
            .not_prepared,
        1
    );
    assert_eq!(
        l.sent_file_summary(&scope, "dispatch", &execution)
            .unwrap()
            .unconfirmed,
        0
    );
    assert!(
        l.observe_file_lookup(
            &scope,
            "dispatch",
            &execution,
            &r.id,
            lookup(&r, &execution, false),
            33
        )
        .is_err()
    );
    assert_eq!(
        l.observe_file_lookup(
            &scope,
            "dispatch",
            &execution,
            &r.id,
            lookup(&r, &execution, true),
            34
        )
        .unwrap()
        .absence
        .unwrap()
        .observed_at,
        32
    );
    for sql in [
        "DELETE FROM sent_file_absence",
        "UPDATE sent_file_absence SET observed_at=33",
        "UPDATE sent_files SET remote='{}'",
    ] {
        assert!(l.db.execute(sql, []).is_err());
    }
    let snapshot = l.snapshot(OWNER, &"a".repeat(64), 35).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    restored
        .restore_snapshot(OWNER, &"a".repeat(64), &snapshot)
        .unwrap();
    assert!(
        restored
            .sent_file_record(OWNER, "agent", &r.id)
            .unwrap()
            .record
            .absence
            .is_some()
    );
    let c = capture(
        &sender, &restored, &scope, &execution, "file.txt", "new-call",
    );
    assert_eq!(c.id(), r.id);
    let audit = approve(&mut restored, &c);
    assert!(matches!(
        sender.begin(&mut restored, c, &audit, 36),
        Err(Error::Unknown)
    ));
    drop(l);
    let reopened = Ledger::open_scoped(cwd.join("ledger"), OWNER, &"a".repeat(64)).unwrap();
    assert!(
        reopened
            .sent_file_record(OWNER, "agent", &r.id)
            .unwrap()
            .record
            .absence
            .is_some()
    );
}
#[test]
fn pre_history_snapshot_preserves_existing_sender_witness_and_adds_no_absence() {
    let _lock = LOCK.blocking_lock();
    let t = tempfile::tempdir().unwrap();
    let cwd = t.path().canonicalize().unwrap();
    std::fs::write(cwd.join("file.txt"), b"hello").unwrap();
    let (mut l, scope, execution) = ledger(&cwd.join("ledger"));
    let sender = Sender::open(&cwd).unwrap();
    let c = capture(&sender, &l, &scope, &execution, "file.txt", "first");
    let audit = approve(&mut l, &c);
    let r = sender
        .begin(&mut l, c, &audit, 20)
        .unwrap()
        .record()
        .clone();
    l.db.execute_batch("DROP TRIGGER reject_absent_file_update; DROP TABLE sent_file_absence")
        .unwrap();
    let snapshot = l.snapshot(OWNER, &"a".repeat(64), 30).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    restored
        .restore_snapshot(OWNER, &"a".repeat(64), &snapshot)
        .unwrap();
    let old = restored.sent_file_record(OWNER, "agent", &r.id).unwrap();
    assert!(old.record.absence.is_none());
    assert!(old.record.remote.is_none());
    assert_eq!(old.record.request_id, r.request_id);
    assert_eq!(old.record.audit, r.audit);
    let c = capture(
        &sender,
        &restored,
        &scope,
        &execution,
        "file.txt",
        "after-restore",
    );
    let audit = approve(&mut restored, &c);
    assert!(matches!(
        sender.begin(&mut restored, c, &audit, 31),
        Err(Error::Unknown)
    ));
}
#[test]
fn file_history_pages_preserve_scope_order_and_exact_original_receipts() {
    let _lock = LOCK.blocking_lock();
    let t = tempfile::tempdir().unwrap();
    let cwd = t.path().canonicalize().unwrap();
    let (mut l, scope, execution) = ledger(&cwd.join("ledger"));
    let sender = Sender::open(&cwd).unwrap();
    for i in 0..101 {
        let name = format!("file-{i}.txt");
        std::fs::write(cwd.join(&name), format!("original-{i}")).unwrap();
        let c = capture(&sender, &l, &scope, &execution, &name, &format!("call-{i}"));
        let audit = approve(&mut l, &c);
        drop(sender.begin(&mut l, c, &audit, 20).unwrap());
    }
    let first = l.sent_file_history(OWNER, "agent", None).unwrap();
    assert_eq!(first.records.len(), 100);
    let second = l
        .sent_file_history(OWNER, "agent", first.next_cursor.as_deref())
        .unwrap();
    assert_eq!(second.records.len(), 1);
    assert!(second.next_cursor.is_none());
    assert!(
        first
            .records
            .windows(2)
            .all(|p| p[0].record.id > p[1].record.id)
    );
    assert!(
        first.records.iter().all(|r| r.scope == scope
            && r.execution_id == execution
            && r.dispatch_id == "dispatch")
    );
    assert!(
        l.sent_file_history(OWNER, "other-agent", first.next_cursor.as_deref())
            .is_err()
    );
}

#[test]
fn final_absence_releases_actual_capacity_but_not_permanent_identity() {
    use std::io::Write;
    let _lock = LOCK.blocking_lock();
    let t = tempfile::tempdir().unwrap();
    let cwd = t.path().canonicalize().unwrap();
    let (mut l, scope, execution) = ledger(&cwd.join("ledger"));
    let sender = Sender::open(&cwd).unwrap();
    let mut original = None;
    for i in 0u8..16 {
        let name = format!("large-{i}.bin");
        let mut f = std::fs::File::create(cwd.join(&name)).unwrap();
        f.set_len(crate::file_delivery::MAX_BYTES as u64).unwrap();
        f.write_all(&[i]).unwrap();
        drop(f);
        let c = capture(&sender, &l, &scope, &execution, &name, &format!("call-{i}"));
        let audit = approve(&mut l, &c);
        let custody = sender.begin(&mut l, c, &audit, 20).unwrap();
        if i == 0 {
            original = Some(custody.record().clone());
        }
    }
    let r = original.unwrap();
    std::fs::write(cwd.join("new.txt"), b"new approved original").unwrap();
    l.observe_file_lookup(
        &scope,
        "dispatch",
        &execution,
        &r.id,
        lookup(&r, &execution, false),
        30,
    )
    .unwrap();
    let c = capture(&sender, &l, &scope, &execution, "new.txt", "quota-held");
    let audit = approve(&mut l, &c);
    assert!(matches!(
        sender.begin(&mut l, c, &audit, 30),
        Err(Error::Invalid("sent file quota"))
    ));
    l.observe_file_lookup(
        &scope,
        "dispatch",
        &execution,
        &r.id,
        lookup(&r, &execution, true),
        31,
    )
    .unwrap();
    let c = capture(&sender, &l, &scope, &execution, "new.txt", "quota-released");
    let audit = approve(&mut l, &c);
    drop(sender.begin(&mut l, c, &audit, 31).unwrap());
    assert_eq!(
        l.sent_file_history(OWNER, "agent", None)
            .unwrap()
            .records
            .len(),
        17
    );
    assert!(
        l.sent_file_record(OWNER, "agent", &r.id)
            .unwrap()
            .record
            .absence
            .is_some()
    );
}
