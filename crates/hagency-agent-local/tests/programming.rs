//! Real process-tree and local profile persistence tests for programming mode.
#[cfg(unix)]
use hagency_agent_local::codex::{Profile, Programming};
use hagency_agent_local::{Ledger, ModelProfile, codex::McpServer};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::{path::PathBuf, time::Duration};

#[cfg(unix)]
#[tokio::test]
async fn dropping_programming_during_initialization_stops_original_descendants() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for name in ["home", "provider", "work"] {
        std::fs::create_dir(root.join(name)).unwrap();
        std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let actor = root.join("actor");
    let source = format!(
        r#"#!/usr/bin/python3
import os,subprocess,time
child=subprocess.Popen(['/bin/sleep','60'])
with open({},'w') as f:
 f.write(str(child.pid))
while True: time.sleep(1)
"#,
        serde_json::to_string(&root.join("child.pid")).unwrap()
    );
    std::fs::write(&actor, source).unwrap();
    std::fs::set_permissions(&actor, std::fs::Permissions::from_mode(0o700)).unwrap();
    let profile = Profile {
        executable: actor,
        home: root.join("home"),
        codex_home: root.join("provider"),
        cwd: root.join("work"),
        model: "fixture".into(),
        effort: "low".into(),
        shared_auth: false,
        web_search: false,
        programming: Programming {
            enabled: true,
            ..Default::default()
        },
    };
    let guardian = PathBuf::from(env!("CARGO_BIN_EXE_hagency-local-probe"));
    let process = profile.spawn_with_guardian(&guardian).await.unwrap();
    let pid = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(root.join("child.pid")) {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    drop(process); // Cancellation before initialize/open_context, not only normal turn completion.
    let gone = std::process::Command::new("/bin/kill")
        .args(["-0", pid.trim()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(
        !gone.success(),
        "programming descendant survived owner drop"
    );
}
#[test]
fn programming_profile_migrates_without_resetting_and_persists_exact_mcp_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("ledger");
    let owner = "@owner:test";
    let mut ledger = Ledger::open(&db, owner).unwrap();
    let profile = ModelProfile {
        model: "fixture".into(),
        credential_ref: "keychain:fixture".into(),
        workspace_root: temp.path().to_str().unwrap().into(),
        reasoning_effort: "low".into(),
        web_search: false,
        programming: Default::default(),
    };
    ledger
        .set_agent_model_profile(owner, "agent", &profile)
        .unwrap();
    drop(ledger);
    let sql = rusqlite::Connection::open(&db).unwrap();
    sql.execute_batch("ALTER TABLE model_profiles DROP COLUMN programming")
        .unwrap();
    drop(sql);
    let mut ledger = Ledger::open(&db, owner).unwrap();
    assert_eq!(
        ledger.agent_model_profile(owner, "agent").unwrap(),
        Some(profile.clone())
    );
    let mut enabled = profile.clone();
    enabled.programming.enabled = true;
    enabled.programming.mcp_servers.insert(
        "fixture".into(),
        McpServer::Http {
            url: "https://example.test/mcp".into(),
        },
    );
    ledger
        .set_agent_model_profile(owner, "agent", &enabled)
        .unwrap();
    drop(ledger);
    let ledger = Ledger::open(&db, owner).unwrap();
    assert_eq!(
        ledger.agent_model_profile(owner, "agent").unwrap(),
        Some(enabled)
    );
    assert!(Ledger::open(&db, "@other:test").is_err());
}

#[test]
fn programming_refuses_ambient_prefix_rules_instead_of_bypassing_owner_policy() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for name in ["home", "provider", "work"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    #[cfg(unix)]
    std::fs::set_permissions(
        root.join("provider"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let profile = hagency_agent_local::codex::Profile {
        executable: std::env::current_exe().unwrap().canonicalize().unwrap(),
        home: root.join("home"),
        codex_home: root.join("provider"),
        cwd: root.join("work"),
        model: "fixture".into(),
        effort: "low".into(),
        shared_auth: false,
        web_search: false,
        programming: hagency_agent_local::codex::Programming {
            enabled: true,
            ..Default::default()
        },
    };
    profile.validate().unwrap();
    std::fs::create_dir(profile.codex_home.join("rules")).unwrap();
    std::fs::write(
        profile.codex_home.join("rules/ambient.rules"),
        r#"prefix_rule(pattern=["sh"], decision="allow")"#,
    )
    .unwrap();
    assert!(matches!(
        profile.validate(),
        Err(hagency_agent_local::codex::Error::Profile(
            "ambient execution rules are unsupported"
        ))
    ));
}
