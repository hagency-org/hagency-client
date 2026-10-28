use super::*;
use crate::{
    Budget, Layer, Ledger, Limit, Period, Policy, RequestPolicy, Scope, ToolPolicy,
    attachments::Attachment, inbox,
};
use serde_json::json;
const OWNER: &str = "@alice:test";
const PROFILE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
struct Fixture {
    temp: tempfile::TempDir,
    ledger: Ledger,
    receiver: Receiver,
    scope: Scope,
    execution: String,
}
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let mut ledger = Ledger::open_scoped(temp.path().join("ledger"), OWNER, PROFILE).unwrap();
    let event = inbox::Dispatch {
        id: "dispatch".into(),
        binding_id: "binding".into(),
        agent_id: "agent".into(),
        event_id: "$source:test".into(),
        room_id: "!room:test".into(),
        requester_mxid: "@bob:test".into(),
        thread_root: "$thread:test".into(),
        body: "Analyze attachment".into(),
        state: "offered".into(),
        binding_generation: 1,
        dispatch_epoch: Some(1),
        dispatch_device_id: Some("device".into()),
        execution_id: None,
        outcome: None,
    };
    let scope = event.scope();
    ledger.register_binding(OWNER, &scope).unwrap();
    for layer in [Layer::Agent, Layer::Room, Layer::Requester] {
        ledger
            .set_policy(
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
    ledger
        .receive_dispatch(OWNER, &event, inbox::Limits::default(), 10)
        .unwrap();
    ledger.acknowledge_dispatch(OWNER, &event.id).unwrap();
    let p = ledger.prepare_execution(OWNER, &event.id).unwrap();
    let execution = p.execution_id;
    let mut running = event;
    running.state = "running".into();
    running.execution_id = Some(execution.clone());
    ledger
        .confirm_execution_start(
            OWNER,
            &inbox::ServerStart {
                dispatch: running,
                newly_started: true,
            },
        )
        .unwrap()
        .unwrap();
    ledger
        .reserve(&scope, "model-call", "dispatch", 10, 20)
        .unwrap();
    let receiver = Receiver::open(&cwd.canonicalize().unwrap()).unwrap();
    Fixture {
        temp,
        ledger,
        receiver,
        scope,
        execution,
    }
}
fn source() -> Attachment {
    Attachment::from_content(&json!({"msgtype":"m.file","body":"../../untrusted.txt","url":"mxc://media.test/original","info":{"size":5}})).unwrap()
}
fn prepared(f: &Fixture, call: &str) -> Prepared {
    f.receiver
        .prepare(
            &f.ledger,
            &f.scope,
            "dispatch",
            &f.execution,
            "$source:test",
            source(),
            "codex-thread",
            "turn",
            call,
            30,
        )
        .unwrap()
}
fn approved(f: &mut Fixture, call: &str) -> Prepared {
    let p = prepared(f, call);
    f.ledger.approve_tool(OWNER, p.proposal(), 31).unwrap();
    p
}
fn fresh(f: &mut Fixture, call: &str) -> Custody {
    let p = approved(f, call);
    let Begin::Fresh(c) = f.receiver.begin(&mut f.ledger, p, 32).unwrap() else {
        panic!("not original custody")
    };
    c
}
#[test]
fn actual_private_bytes_are_atomically_published_and_known_retry_only_revalidates() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = fixture();
    let c = fresh(&mut f, "first");
    let receipt = f.receiver.publish(&mut f.ledger, c, b"hello", 33).unwrap();
    let cwd = f.temp.path().join("workspace");
    let path = cwd.join(&receipt.relative_path);
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    assert_eq!(receipt.bytes, 5);
    assert_eq!(
        receipt.sha256,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert!(!receipt.relative_path.contains("untrusted"));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let p = approved(&mut f, "retry");
    let Begin::Ready(replayed) = f.receiver.begin(&mut f.ledger, p, 34).unwrap() else {
        panic!("must not mint custody")
    };
    assert_eq!(*replayed, receipt);
    std::fs::write(&path, b"other").unwrap();
    let p = approved(&mut f, "changed");
    assert!(f.receiver.begin(&mut f.ledger, p, 35).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"other");
}
#[test]
fn missing_approval_wrong_request_and_replaced_workspace_are_refused_before_any_directory_write() {
    let mut f = fixture();
    let p = prepared(&f, "no-grant");
    assert!(f.receiver.begin(&mut f.ledger, p, 32).is_err());
    assert!(!f.temp.path().join("workspace/.hagency-received").exists());
    assert!(
        f.receiver
            .prepare(
                &f.ledger,
                &f.scope,
                "dispatch",
                "foreign-execution",
                "$source:test",
                source(),
                "thread",
                "turn",
                "call",
                30
            )
            .is_err()
    );
    assert!(
        f.receiver
            .prepare(
                &f.ledger,
                &f.scope,
                "dispatch",
                &f.execution,
                "$other:test",
                source(),
                "thread",
                "turn",
                "call",
                30
            )
            .is_err()
    );
    let c = fresh(&mut f, "before-rename");
    let cwd = f.temp.path().join("workspace");
    std::fs::rename(&cwd, f.temp.path().join("old-workspace")).unwrap();
    std::fs::create_dir(&cwd).unwrap();
    assert!(f.receiver.publish(&mut f.ledger, c, b"hello", 33).is_err());
    assert!(!cwd.join(".hagency-received").exists());
}
#[test]
fn lost_original_custody_and_restored_snapshot_cannot_rearm_receive() {
    let mut f = fixture();
    drop(fresh(&mut f, "first"));
    let snapshot = f.ledger.snapshot(OWNER, PROFILE, 33).unwrap();
    let mut restored = Ledger::open_scoped(f.temp.path().join("restored"), OWNER, PROFILE).unwrap();
    restored
        .restore_snapshot(OWNER, PROFILE, &snapshot)
        .unwrap();
    let p = approved(&mut f, "after-lost-result");
    assert!(matches!(
        f.receiver.begin(&mut f.ledger, p, 34),
        Err(crate::Error::Unknown)
    ));
    f.ledger = restored;
    let p = approved(&mut f, "after-restore");
    assert!(matches!(
        f.receiver.begin(&mut f.ledger, p, 35),
        Err(crate::Error::Unknown)
    ));
    assert!(!f.temp.path().join("workspace/.hagency-received").exists());
}
#[test]
fn publication_uncertainty_never_overwrites_existing_or_reconstructs_a_receipt() {
    let mut f = fixture();
    let p = approved(&mut f, "first");
    let relative = p.proposal().arguments["relativePath"]
        .as_str()
        .unwrap()
        .to_owned();
    let Begin::Fresh(c) = f.receiver.begin(&mut f.ledger, p, 32).unwrap() else {
        panic!("custody")
    };
    let path = f.temp.path().join("workspace").join(relative);
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(
        parent.parent().unwrap(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(&path, b"original").unwrap();
    assert!(matches!(
        f.receiver.publish(&mut f.ledger, c, b"hello", 33),
        Err(crate::Error::Unknown)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    let p = approved(&mut f, "second");
    assert!(matches!(
        f.receiver.begin(&mut f.ledger, p, 34),
        Err(crate::Error::Unknown)
    ));
    assert!(
        f.ledger
            .db
            .execute("DELETE FROM received_files", [])
            .is_err()
    );
    assert!(
        f.ledger
            .db
            .execute("UPDATE received_files SET state='reserved'", [])
            .is_err()
    );
}
#[test]
fn revoked_policy_symlinked_target_and_bad_length_do_not_publish() {
    for variant in ["policy", "symlink", "length"] {
        let mut f = fixture();
        let c = fresh(&mut f, "first");
        if variant == "policy" {
            let prior = f.ledger.policy_snapshot(&f.scope).unwrap()[0].clone();
            let mut policy = prior.policy;
            policy.high_risk = ToolPolicy::Deny;
            f.ledger
                .set_policy(OWNER, &f.scope, Layer::Agent, prior.revision, &policy)
                .unwrap();
        }
        if variant == "symlink" {
            let outside = f.temp.path().join("outside");
            std::fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, f.temp.path().join("workspace/.hagency-received"))
                .unwrap();
        }
        assert!(
            f.receiver
                .publish(
                    &mut f.ledger,
                    c,
                    if variant == "length" {
                        b"wrong size"
                    } else {
                        b"hello"
                    },
                    33
                )
                .is_err()
        );
        assert_eq!(
            std::fs::read_dir(f.temp.path().join("workspace"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|v| v == "bin"))
                .count(),
            0
        );
    }
}
#[test]
fn legacy_pre_receive_snapshot_migrates_empty_without_inventing_custody() {
    let f = fixture();
    f.ledger
        .db
        .execute_batch("DROP TRIGGER reject_absent_file_update; DROP TABLE sent_file_absence; DROP TABLE sent_files; DROP TABLE received_files")
        .unwrap();
    let snapshot = f.ledger.snapshot(OWNER, PROFILE, 33).unwrap();
    let mut target = Ledger::open_scoped(":memory:", OWNER, PROFILE).unwrap();
    target.restore_snapshot(OWNER, PROFILE, &snapshot).unwrap();
    assert_eq!(
        target
            .db
            .query_row("SELECT count(*) FROM received_files", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn receive_contexts_settle_and_unknown_usage_resets_only_the_exact_scope() {
    use crate::Usage;
    for prefix in [
        crate::codex::FILE_CAPABILITY_VERSION.to_owned(),
        format!(
            "{}:{}",
            crate::codex::PROGRAMMING_CAPABILITY_VERSION,
            "a".repeat(64)
        ),
    ] {
        let mut f = fixture();
        let base = f.scope.context_key(OWNER).unwrap();
        let context = format!("{prefix}:{}:{base}", CAPABILITY_VERSION);
        let usage = Usage {
            input: 3,
            output: 2,
            cached_input: 1,
            reasoning_output: 0,
            accounting_version: "fixture".into(),
        };
        assert!(
            f.ledger
                .settle_codex(
                    &f.scope,
                    "model-call",
                    &usage,
                    &format!("foreign:{base}"),
                    "{}"
                )
                .is_err()
        );
        f.ledger
            .settle_codex(&f.scope, "model-call", &usage, &context, "{\"total\":5}")
            .unwrap();
        f.ledger
            .db
            .execute(
                "INSERT INTO codex_usage VALUES ('foreign-context','original')",
                [],
            )
            .unwrap();
        f.ledger
            .reserve(&f.scope, "unknown-call", "dispatch", 10, 40)
            .unwrap();
        f.ledger.mark_unknown(&f.scope, "unknown-call").unwrap();
        assert_eq!(
            f.ledger
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
            f.ledger
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
