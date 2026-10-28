//! Board #78 acceptance: two dispatches on two threads of ONE agent, driven
//! through the production owned-dispatch path (`Operation::start`), resolve to
//! DISTINCT per-thread worktrees and share no file visibility.
//!
//! The child (the offline `hagency-execution-probe`) writes its request log
//! (`owned-dispatch.requests`) and entry markers into its OWN working directory,
//! and echoes that directory back as `thread/start.cwd` (ADR-116). So a
//! per-thread worktree is observable: the two `thread/start.cwd` values differ,
//! live under the worktrees root, and are not the shared engagement workspace.

#[path = "../../hagency-store/tests/common/mod.rs"]
mod common;
use common::*;
use hagency_core::authority::ProjectRequest;
use hagency_core::tasks::*;
use hagency_execution::{Host, Limits, Operation, Protocol, ordinary_launch_path};
use hagency_store::{DomainRepository, DomainStore, EffectOutcome};
use serde_json::json;
use std::{
    collections::BTreeMap,
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
async fn native_worktree_two_threads_distinct_worktrees() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();

    // The agent's own workspace root. TS resolves the worktree repository from
    // the agent workdir (backend-v2.js:2057 `agent.workdir || agent.homeDir`),
    // so THIS directory is the repository `git worktree add` branches from.
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
    // TS per-agent mode (backend-v2.js:2994, resolve at :2057-2075): the
    // workspace settings ride the AGENT record through the production
    // admission path — not a host or serve-level switch.
    let mut value = serde_json::to_value(request("allocation", "Worker", &pool, 100)).unwrap();
    value["agentDefinition"]["workspaceMode"] = json!("worktree");
    value["agentDefinition"]["worktreesDir"] = json!(worktrees.to_string_lossy().into_owned());
    let request: ProjectRequest = serde_json::from_value(value).unwrap();
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
    db.register_workspace("work").unwrap();
    let domain = DomainStore::start(db, 16).unwrap();

    // The shared engagement workspace is the ONLY registered root; the
    // per-thread worktree is resolved at dispatch time from the scope's
    // thread root + the operator worktree config.
    let host = |env: BTreeMap<OsString, OsString>| {
        Host::new(
            binary(),
            binary(),
            env,
            BTreeMap::from([("work".into(), work.clone())]),
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
    let dispatch = |id: &str, session: &str, task: &str| DispatchInput {
        id: id.into(),
        session_id: session.into(),
        task_id: Some(task.into()),
        resources: vec![ResourceLease {
            id: "work".into(),
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
        .enqueue_dispatch(dispatch("dispatch-a", "session-a", "task-a"))
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
        .enqueue_dispatch(dispatch("dispatch-b", "session-b", "task-b"))
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

    domain.shutdown().await.unwrap();
}
