//! Two threads use distinct operator-created worktrees through the production
//! owned-dispatch path (`Operation::start`). The operator registers each private
//! root and selects its opaque workspace resource for the corresponding dispatch.
//! Requesters cannot choose host directories or bootstrap commands.
//!
//! The child (the offline `hagency-execution-probe`) writes its request log
//! (`owned-dispatch.requests`) and entry markers into its OWN working directory,
//! and echoes that directory back as `thread/start.cwd` (ADR-116).
//! The two `thread/start.cwd` values and retained files prove separate working
//! directories. This does not assert automatic thread routing or OS isolation.

#[path = "../../hagency-store/tests/common/mod.rs"]
mod common;
use common::*;
use hagency_core::authority::{ProjectRequest, verify_request};
use hagency_core::tasks::*;
use hagency_execution::{
    Host, Limits, Operation, Protocol, WorktreeManager, WorktreeSpec, ordinary_launch_path,
};
use hagency_store::{DomainRepository, DomainStore, EffectOutcome};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn binary() -> PathBuf {
    env!("CARGO_BIN_EXE_hagency-execution-probe").into()
}
fn limits() -> Limits {
    Limits {
        operation_ms: 25_000,
        response_ms: 1500,
    }
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
}

fn thread_cwd(requests: &Path) -> String {
    let text = fs::read_to_string(requests).unwrap();
    let values: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    values
        .iter()
        .find(|v| v["method"] == "thread/start")
        .unwrap()["params"]["cwd"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn native_worktree_threads_use_operator_registered_workspaces() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();

    // The operator's repository is the source for the explicit local worktrees.
    let work = root.path().join("agent-workspace");
    hagency_store::private::directory(&work).unwrap();
    let work = work.canonicalize().unwrap();
    git(&work, &["init"]);
    git(&work, &["config", "user.email", "t@e.com"]);
    git(&work, &["config", "user.name", "T"]);
    fs::write(work.join("README.md"), "base\n").unwrap();
    git(&work, &["add", "README.md"]);
    git(&work, &["commit", "-m", "base"]);
    let worktrees = root.path().join("worktrees");

    // One agent's store: register, admit, approve, provision.
    let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    let pool = resource("pool", "seat", 1000);
    db.put_resource(&pool).unwrap();
    let request = request("allocation", "Worker", &pool, 100);
    let forbidden = root.path().join("requester-worktrees");
    let bootstrap_marker = root.path().join("requester-bootstrap");
    for (field, value) in [
        ("workspaceMode", json!("worktree")),
        ("worktreesDir", json!(forbidden)),
        ("worktreeBootstrap", json!(["touch", bootstrap_marker])),
    ] {
        let mut value_with_override = serde_json::to_value(&request).unwrap();
        value_with_override["agentDefinition"][field] = value;
        let hostile: ProjectRequest = serde_json::from_value(value_with_override).unwrap();
        let refusal =
            verify_request(&registration(), hostile.clone(), observation(&hostile)).unwrap_err();
        assert_eq!(refusal.0, "a request cannot set the agent's workspace");
    }
    assert!(!forbidden.exists() && !bootstrap_marker.exists());
    let proof = proof(&request);
    let engagement = db.admit(&proof, 1000).unwrap();
    db.approve("approved", &proof, 1000).unwrap();
    let effect = db.claim_effect().unwrap().unwrap();
    db.observe_effect(
        &effect.id,
        effect.fence,
        &EffectOutcome::Applied {
            receipt: "offline".into(),
        },
    )
    .unwrap();
    // Two thread sessions of the SAME engagement (differ only in thread root).
    db.register_session(&SessionBinding {
        id: "session-a".into(),
        engagement_id: engagement.id.clone(),
        room_id: "!project:example.test".into(),
        thread_root: Some("$thread-a".into()),
    })
    .unwrap();
    db.register_session(&SessionBinding {
        id: "session-b".into(),
        engagement_id: engagement.id.clone(),
        room_id: "!project:example.test".into(),
        thread_root: Some("$thread-b".into()),
    })
    .unwrap();
    // There is no requester-controlled worktree mode. Local operator APIs
    // create the roots and register them before either thread can dispatch.
    let manager = WorktreeManager::new();
    let make_worktree = |thread: &str| {
        let info = manager
            .ensure(WorktreeSpec {
                repository_path: work.to_string_lossy().into_owned(),
                worktrees_dir: worktrees.to_string_lossy().into_owned(),
                agent_id: engagement.id.clone(),
                thread_root_event_id: thread.into(),
                bootstrap: None,
            })
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&info.path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        info.path
    };
    let work_a = make_worktree("$thread-a");
    let work_b = make_worktree("$thread-b");
    fs::write(work_a.join("only-thread-a"), "thread a").unwrap();
    fs::write(work_b.join("only-thread-b"), "thread b").unwrap();
    db.register_workspace("work-a").unwrap();
    db.register_workspace("work-b").unwrap();
    let domain = DomainStore::start(db, 16).unwrap();

    // The host opens only the two private roots explicitly registered by the
    // operator. The queued dispatch selects one of these opaque resources.
    let host = |env: BTreeMap<OsString, OsString>| {
        Host::new(
            binary(),
            binary(),
            env,
            BTreeMap::from([
                ("work-a".into(), work_a.clone()),
                ("work-b".into(), work_b.clone()),
            ]),
        )
        .unwrap()
    };
    let env = || {
        let mut env = BTreeMap::from([
            (OsString::from("PATH"), OsString::from("")),
            (
                OsString::from("HAGENCY_OFFLINE_MODE"),
                OsString::from("normal"),
            ),
            (
                OsString::from("HAGENCY_OPERATION_BUDGET_MS"),
                OsString::from(limits().operation_ms.to_string()),
            ),
        ]);
        if let Some(system) = std::env::var_os("SystemRoot") {
            env.insert(OsString::from("SystemRoot"), system);
        }
        env
    };
    let dispatch = |id: &str, session: &str, task: &str, workspace: &str| DispatchInput {
        id: id.into(),
        session_id: session.into(),
        task_id: Some(task.into()),
        resources: vec![ResourceLease {
            id: workspace.into(),
            exclusive: true,
        }],
        payload: json!({"instruction": "offline"}),
    };

    // Thread A: full dispatch to a clean completion.
    domain
        .create_canonical_task(
            "task-a".into(),
            "session-a".into(),
            "Thread A".into(),
            now(),
        )
        .await
        .unwrap();
    domain
        .enqueue_dispatch(dispatch("dispatch-a", "session-a", "task-a", "work-a"))
        .await
        .unwrap();
    let cap_a = domain
        .claim_dispatch("runner-a".into(), now(), 60_000, 60_000, 1)
        .await
        .unwrap()
        .unwrap();
    let mut op_a = Operation::start(domain.clone(), cap_a, host(env()), limits()).unwrap();
    let report_a = op_a.wait().await.unwrap();
    assert_eq!(report_a.protocol, Protocol::Completed);

    // Thread B: distinct worktree, same agent.
    domain
        .create_canonical_task(
            "task-b".into(),
            "session-b".into(),
            "Thread B".into(),
            now(),
        )
        .await
        .unwrap();
    domain
        .enqueue_dispatch(dispatch("dispatch-b", "session-b", "task-b", "work-b"))
        .await
        .unwrap();
    let cap_b = domain
        .claim_dispatch("runner-b".into(), now(), 60_000, 60_000, 1)
        .await
        .unwrap()
        .unwrap();
    let mut op_b = Operation::start(domain.clone(), cap_b, host(env()), limits()).unwrap();
    let report_b = op_b.wait().await.unwrap();
    assert_eq!(report_b.protocol, Protocol::Completed);

    // Exactly two worktrees were produced, each holding its own request log.
    let mut requests = Vec::new();
    walk(&worktrees, &mut requests);
    let requests: Vec<&Path> = requests
        .iter()
        .filter(|p| p.file_name() == Some(std::ffi::OsStr::new("owned-dispatch.requests")))
        .map(|p| p.as_path())
        .collect();
    assert_eq!(requests.len(), 2, "two threads must create two worktrees");

    let cwd_a = thread_cwd(requests[0]);
    let cwd_b = thread_cwd(requests[1]);
    let shared = ordinary_launch_path(&work).unwrap();

    // The core acceptance: distinct worktrees, no cross-thread sharing.
    assert_ne!(
        cwd_a, cwd_b,
        "two threads of one agent must get distinct worktrees"
    );
    assert_ne!(
        cwd_a, shared,
        "thread A must not run in the shared workspace"
    );
    assert_ne!(
        cwd_b, shared,
        "thread B must not run in the shared workspace"
    );
    let worktrees_root = worktrees.canonicalize().unwrap();
    assert!(
        Path::new(&cwd_a).starts_with(&worktrees_root),
        "thread A cwd {cwd_a} must live under the worktrees root"
    );
    assert!(
        Path::new(&cwd_b).starts_with(&worktrees_root),
        "thread B cwd {cwd_b} must live under the worktrees root"
    );
    assert_eq!(
        BTreeSet::from([cwd_a, cwd_b]),
        BTreeSet::from([
            ordinary_launch_path(&work_a).unwrap(),
            ordinary_launch_path(&work_b).unwrap(),
        ])
    );
    assert_eq!(
        fs::read_to_string(work_a.join("only-thread-a")).unwrap(),
        "thread a"
    );
    assert_eq!(
        fs::read_to_string(work_b.join("only-thread-b")).unwrap(),
        "thread b"
    );
    assert!(!work_a.join("only-thread-b").exists());
    assert!(!work_b.join("only-thread-a").exists());
    assert!(!forbidden.exists() && !bootstrap_marker.exists());

    domain.shutdown().await.unwrap();
}
