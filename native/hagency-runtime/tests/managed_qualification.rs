use hagency_platform::Launch;
use hagency_runtime::{
    codex::{
        session::{Error, Settings},
        transport::Limits,
    },
    owned::{MANAGED_INSTRUCTIONS, MANAGED_INSTRUCTIONS_FILE, ManagedProcessState, OwnedSession},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};
fn config(path: &Path) -> Value {
    let mut features = serde_json::Map::new();
    for key in [
        "shell_tool",
        "unified_exec",
        "code_mode_host",
        "code_mode",
        "hooks",
        "plugins",
        "multi_agent",
        "skill_search",
        "skill_mcp_dependency_install",
        "shell_snapshot",
        "view_image",
        "browser_use",
        "browser_use_external",
        "browser_use_full_cdp_access",
        "browser_annotation_api",
        "computer_use",
        "in_app_browser",
        "goals",
        "image_generation",
        "apps",
        "multi_agent_v2",
        "tool_search",
        "tool_suggest",
        "web_search",
        "web_search_cached",
        "web_search_request",
        "standalone_web_search",
        "memory_tool",
        "remote_plugin",
        "remote_control",
        "memories",
    ] {
        features.insert(key.into(), false.into());
    }
    json!({"model_provider":"openai","chatgpt_base_url":"https://chatgpt.com/backend-api","model_providers":{},"model_instructions_file":path,"instructions":"","notify":[],"web_search":"disabled","project_doc_max_bytes":0,"skills":{"include_instructions":false,"bundled":{"enabled":false}},"developer_instructions":"","include_apps_instructions":false,"include_environment_context":false,"mcp_servers":{},"features":features})
}
fn fixture(mode: &str, change: impl FnMut(&mut Value)) -> (tempfile::TempDir, OwnedSession) {
    fixture_policy(mode, change, None)
}
fn fixture_policy(mode: &str, mut change: impl FnMut(&mut Value), tools: Option<bool>) -> (tempfile::TempDir, OwnedSession) {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let instructions = root.join(MANAGED_INSTRUCTIONS_FILE);
    std::fs::write(&instructions, MANAGED_INSTRUCTIONS).unwrap();
    let mut c = config(&instructions);
    change(&mut c);
    std::fs::write(root.join("config.json"), serde_json::to_vec(&c).unwrap()).unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_hagency-managed-runtime-probe"));
    let mut environment = BTreeMap::new();
    environment.insert("PATH".into(), "".into());
    if let Some(v) = std::env::var_os("SystemRoot") {
        environment.insert("SystemRoot".into(), v);
    }
    let launch = Launch {
        executable: binary.into(),
        arguments: vec![mode.into(), root.as_os_str().into()],
        directory: root.clone(),
        environment,
        require_crash_containment: false,
    };
    let settings = Settings::new(root, "fixture".into(), "medium".into()).unwrap();
    let settings = match tools { Some(yolo) => settings.with_execution_policy(yolo), None => settings.read_only() };
    let session = OwnedSession::spawn(
        binary,
        &launch,
        settings,
        Limits {
            write_timeout_ms: 500,
            event_wait_ms: 1500,
            lifetime_ms: 10_000,
        },
        1500,
    )
    .unwrap();
    (t, session)
}
#[tokio::test]
async fn managed_qualification_uses_actual_retained_account_and_never_sends_work() {
    let (t, mut p) = fixture("normal", |_| {});
    p.initialize().await.unwrap();
    let source = p.managed_process_source();
    let observed = p
        .qualify_managed_no_tools(
            &t.path()
                .canonicalize()
                .unwrap()
                .join(MANAGED_INSTRUCTIONS_FILE),
        )
        .await
        .unwrap();
    assert_eq!(observed.state(), ManagedProcessState::Qualified);
    assert!(source.same(observed.source()));
    assert!(source.is_live());
    assert_eq!(
        observed.provider_digest(),
        Some(
            hagency_core::canonical::digest(
                &json!({"provider":"openai.chatgpt","subject":"fixture_account"})
            )
            .unwrap()
            .as_str()
        )
    );
    assert!(observed.policy_digest().is_some());
    let stopped = p.stop_managed_observation();
    assert_eq!(stopped.state(), ManagedProcessState::Stopped);
    assert!(source.same(stopped.source()));
    assert!(!source.is_live());
    let methods = std::fs::read_to_string(t.path().join("methods")).unwrap();
    assert!(methods.lines().all(|v| matches!(
        v,
        "initialize" | "initialized" | "config/read" | "account/read" | "account/rateLimits/read"
    )));
    assert_eq!(
        methods
            .lines()
            .filter(|v| *v == "account/rateLimits/read")
            .count(),
        2
    );
}
#[tokio::test]
async fn managed_qualification_changed_quota_and_server_requests_fail_and_stop() {
    for (mode, expected) in [
        ("changed-account", Error::Scope),
        ("request", Error::UnsupportedRequest),
    ] {
        let (t, mut p) = fixture(mode, |_| {});
        p.initialize().await.unwrap();
        let source = p.managed_process_source();
        assert!(
            matches!(p.qualify_managed_no_tools(&t.path().canonicalize().unwrap().join(MANAGED_INSTRUCTIONS_FILE)).await,Err(e) if e==expected)
        );
        assert!(!source.is_live());
        assert_eq!(
            p.stop_managed_observation().state(),
            ManagedProcessState::Stopped
        );
    }
}
#[tokio::test]
async fn managed_qualification_rejects_enabled_tools_web_or_unknown_effective_policy() {
    for index in 0..6 {
        let (t, mut p) = fixture("normal", |c| match index {
            0 => c["features"]["shell_tool"] = true.into(),
            1 => c["web_search"] = "live".into(),
            2 => {
                c["features"]
                    .as_object_mut()
                    .unwrap()
                    .remove("code_mode_host");
            }
            3 => c["features"]["future_unknown_feature"] = true.into(),
            4 => c["mcp_servers"] = json!({"ambient":{"enabled":true}}),
            _ => c["chatgpt_base_url"] = "https://untrusted.example/".into(),
        });
        p.initialize().await.unwrap();
        assert!(matches!(
            p.qualify_managed_no_tools(
                &t.path()
                    .canonicalize()
                    .unwrap()
                    .join(MANAGED_INSTRUCTIONS_FILE)
            )
            .await,
            Err(Error::Policy)
        ));
        assert_eq!(
            p.stop_managed_observation().state(),
            ManagedProcessState::Stopped
        );
    }
}
#[tokio::test]
async fn managed_qualification_cancellation_retains_and_stops_original_process() {
    let (t, mut p) = fixture("delay", |_| {});
    p.initialize().await.unwrap();
    let source = p.managed_process_source();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            p.qualify_managed_no_tools(
                &t.path()
                    .canonicalize()
                    .unwrap()
                    .join(MANAGED_INSTRUCTIONS_FILE)
            )
        )
        .await
        .is_err()
    );
    assert!(!source.is_live());
    assert_eq!(
        p.stop_managed_observation().state(),
        ManagedProcessState::Stopped
    );
}
#[tokio::test]
async fn managed_qualification_sources_cannot_substitute_another_retained_process() {
    let (a, mut one) = fixture("normal", |_| {});
    let (b, mut two) = fixture("normal", |_| {});
    one.initialize().await.unwrap();
    two.initialize().await.unwrap();
    let proof = one
        .qualify_managed_no_tools(
            &a.path()
                .canonicalize()
                .unwrap()
                .join(MANAGED_INSTRUCTIONS_FILE),
        )
        .await
        .unwrap();
    assert!(!two.managed_process_source().same(proof.source()));
    std::fs::write(b.path().join(MANAGED_INSTRUCTIONS_FILE), "changed").unwrap();
    assert!(
        two.qualify_managed_no_tools(
            &b.path()
                .canonicalize()
                .unwrap()
                .join(MANAGED_INSTRUCTIONS_FILE)
        )
        .await
        .is_err()
    );
    assert_eq!(
        one.stop_managed_observation().state(),
        ManagedProcessState::Stopped
    );
    assert_eq!(
        two.stop_managed_observation().state(),
        ManagedProcessState::Stopped
    );
}

#[tokio::test]
async fn managed_discovery_is_metadata_only_and_binds_exact_inspector() {
    let (t, mut p) = fixture("normal", |c| {
        c["mcp_servers"] = json!({"ambient":{"enabled":true,"env":{"SECRET":"never projected"}}});
        c["features"]["future_feature"] = json!({"optional_parameter":"private, not projected"});
        c["features"]["network_proxy"] = Value::Null;
    });
    p.initialize().await.unwrap();
    let source = p.managed_process_source();
    let names = p
        .discover_managed_policy(
            &t.path()
                .canonicalize()
                .unwrap()
                .join(MANAGED_INSTRUCTIONS_FILE),
        )
        .await
        .unwrap();
    assert!(source.same(names.source()));
    assert_eq!(
        names.values(),
        &[
            "mcp_servers.ambient.enabled=false",
            "features.future_feature=false",
            "features.network_proxy=false"
        ]
    );
    assert_eq!(
        p.stop_managed_observation().state(),
        ManagedProcessState::Stopped
    );
    assert!(!names.source().is_live());
    let methods = std::fs::read_to_string(t.path().join("methods")).unwrap();
    assert!(
        methods
            .lines()
            .all(|v| matches!(v, "initialize" | "initialized" | "config/read"))
    );
}
#[tokio::test]
async fn managed_discovery_rejects_injected_names_invalid_policy_and_server_work() {
    for i in 0..5 {
        let (t, mut p) = fixture(if i == 4 { "request" } else { "normal" }, |c| match i {
            0 => c["mcp_servers"] = json!({"bad.name":{"enabled":true}}),
            1 => c["features"]["future"] = "not boolean".into(),
            2 => c["features"]["shell_tool"] = true.into(),
            3 => c["chatgpt_base_url"] = "https://wrong.invalid".into(),
            _ => (),
        });
        p.initialize().await.unwrap();
        assert!(
            p.discover_managed_policy(
                &t.path()
                    .canonicalize()
                    .unwrap()
                    .join(MANAGED_INSTRUCTIONS_FILE)
            )
            .await
            .is_err()
        );
        assert_eq!(
            p.stop_managed_observation().state(),
            ManagedProcessState::Stopped
        );
    }
}
#[tokio::test]
async fn managed_discovery_cancel_stops_actual_inspector() {
    let (t, mut p) = fixture("delay-config", |_| {});
    p.initialize().await.unwrap();
    let source = p.managed_process_source();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            p.discover_managed_policy(
                &t.path()
                    .canonicalize()
                    .unwrap()
                    .join(MANAGED_INSTRUCTIONS_FILE)
            )
        )
        .await
        .is_err()
    );
    assert!(!source.is_live());
    assert_eq!(
        p.stop_managed_observation().state(),
        ManagedProcessState::Stopped
    );
}

fn text_fixture() -> (tempfile::TempDir, OwnedSession, std::path::PathBuf) {
    text_fixture_mode("text")
}

#[tokio::test]
async fn shared_room_workspace_binds_thread_turn_and_approval_scope_without_changing_permissions() {
    use hagency_runtime::owned::{SHARED_TEXT_INSTRUCTIONS, SHARED_TOOLS_INSTRUCTIONS};
    use sha2::{Digest, Sha256};
    for (mode, tools) in [(None, None), (Some("standard"), Some(false)), (Some("yolo"), Some(true))] {
        let probe_mode = format!("text-workspace-{}", mode.unwrap_or("readonly"));
        let (t, mut process) = fixture_policy(&probe_mode, |_| {}, tools);
        let root = t.path().canonicalize().unwrap();
        let instructions = root.join("room-instructions.md");
        std::fs::write(&instructions, if tools.is_some() { SHARED_TOOLS_INSTRUCTIONS } else { SHARED_TEXT_INSTRUCTIONS }).unwrap();
        let mut c = config(&instructions);
        if tools.is_some() {
            for key in ["shell_tool", "unified_exec", "code_mode_host"] { c["features"][key] = true.into(); }
        }
        std::fs::write(root.join("config.json"), serde_json::to_vec(&c).unwrap()).unwrap();
        process.initialize().await.unwrap();
        process.qualify_shared_tools(&instructions, false, mode).await.unwrap();
        assert!(process.start_shared_text_thread_for_room("!unadmitted:server").await.is_err());
        assert!(!root.join("rooms").exists());
        let now = std::time::Instant::now();
        process.install_shared_text_activity(1, now + Duration::from_secs(5), now + Duration::from_secs(10)).unwrap();
        let room = "!actual-work-room:server:19443";
        let expected = root.join("rooms").join(format!("room_{:x}", Sha256::digest(room.as_bytes())));
        process.start_shared_text_thread_for_room(room).await.unwrap();
        // This accessor is also what host approval checks use.
        assert_eq!(process.shared_workspace(), expected);
        process.start_shared_text_turn("Offline room request".into()).await.unwrap();
        loop {
            let event = process.next_shared_text_event().await.unwrap();
            if matches!(event.observation().kind(), hagency_runtime::codex::session::ObservationKind::TurnEnded(_)) { break; }
        }
        let thread: Value = serde_json::from_slice(&std::fs::read(root.join("thread-request.json")).unwrap()).unwrap();
        let turn: Value = serde_json::from_slice(&std::fs::read(root.join("turn-request.json")).unwrap()).unwrap();
        assert_eq!(thread["cwd"], json!(expected));
        assert_eq!(turn["cwd"], json!(expected));
        assert_eq!(turn["approvalPolicy"], if tools == Some(true) { "never" } else { "on-request" });
        match tools {
            None => assert_eq!(turn["sandboxPolicy"], json!({"type":"readOnly","networkAccess":false})),
            Some(false) => assert_eq!(turn["sandboxPolicy"], json!({"type":"workspaceWrite","writableRoots":[expected],"networkAccess":false})),
            Some(true) => assert_eq!(turn["sandboxPolicy"], json!({"type":"dangerFullAccess"})),
        }
        assert!(process.start_shared_text_thread_for_room("!other:server").await.is_err());
        assert_eq!(process.shared_workspace(), expected);
        assert_eq!(process.stop_shared_text_observation().state(), ManagedProcessState::Stopped);
    }
}

#[tokio::test]
async fn shared_live_web_qualification_is_explicit_and_keeps_other_tools_disabled() {
    use hagency_runtime::owned::{SHARED_WEB_INSTRUCTIONS, shared_text_policy};
    for bad in [None, Some("shell_tool"), Some("hooks"), Some("future_tool"), Some("mcp")] {
        let (t, mut p, path) = text_fixture();
        std::fs::write(&path, SHARED_WEB_INSTRUCTIONS).unwrap();
        let mut c = config(&path);
        c["web_search"] = "live".into();
        c["features"]["standalone_web_search"] = true.into();
        c["features"]["code_mode_host"] = true.into();
        if let Some(feature) = bad {
            if feature == "mcp" { c["mcp_servers"]["test"] = json!({"enabled":true}); }
            else { c["features"][feature] = true.into(); }
        }
        std::fs::write(t.path().join("config.json"), serde_json::to_vec(&c).unwrap()).unwrap();
        p.initialize().await.unwrap();
        let result = p.qualify_shared_text_with_web(&path, true).await;
        if bad.is_some() { assert!(result.is_err()); }
        else {
            assert_eq!(result.unwrap().policy_digest(), Some(hagency_core::canonical::digest(&json!({"policy":shared_text_policy(true)})).unwrap().as_str()));
        }
        p.stop();
    }
    let (t, mut p, path) = text_fixture();
    std::fs::write(&path, SHARED_WEB_INSTRUCTIONS).unwrap();
    let mut c = config(&path);
    c["web_search"] = "live".into();
    c["features"]["standalone_web_search"] = true.into();
        c["features"]["code_mode_host"] = true.into();
    c["features"]["future_default"] = true.into();
    std::fs::write(t.path().join("config.json"), serde_json::to_vec(&c).unwrap()).unwrap();
    p.initialize().await.unwrap();
    let overrides = p.discover_shared_text_policy_with_web(&path, true).await.unwrap();
    assert_eq!(overrides.values(), &["features.future_default=false"]);
    assert!(p.qualify_shared_text(&path).await.is_err());
}
fn text_fixture_mode(mode: &str) -> (tempfile::TempDir, OwnedSession, std::path::PathBuf) {
    let (t, p) = fixture(mode, |_| {});
    let root = t.path().canonicalize().unwrap();
    let instructions = root.join(hagency_runtime::owned::SHARED_TEXT_INSTRUCTIONS_FILE);
    std::fs::write(
        &instructions,
        hagency_runtime::owned::SHARED_TEXT_INSTRUCTIONS,
    )
    .unwrap();
    std::fs::write(
        root.join("config.json"),
        serde_json::to_vec(&config(&instructions)).unwrap(),
    )
    .unwrap();
    (t, p, instructions)
}

#[tokio::test]
async fn shared_live_web_events_require_the_actual_enabled_process() {
    use hagency_runtime::owned::SHARED_WEB_INSTRUCTIONS;
    use hagency_runtime::codex::session::ObservationKind;
    for web in [false, true] {
        let (t, mut p, path) = text_fixture_mode("text-web");
        if web {
            std::fs::write(&path, SHARED_WEB_INSTRUCTIONS).unwrap();
            let mut c = config(&path);
            c["web_search"] = "live".into();
            c["features"]["standalone_web_search"] = true.into();
        c["features"]["code_mode_host"] = true.into();
            std::fs::write(t.path().join("config.json"), serde_json::to_vec(&c).unwrap()).unwrap();
        }
        p.initialize().await.unwrap();
        p.qualify_shared_text_with_web(&path, web).await.unwrap();
        let now = std::time::Instant::now();
        p.install_shared_text_activity(1, now + Duration::from_secs(5), now + Duration::from_secs(10)).unwrap();
        p.start_shared_text_thread().await.unwrap();
        p.start_shared_text_turn("Beijing weather".into()).await.unwrap();
        let mut searches = 0;
        loop {
            let event = p.next_shared_text_event().await.unwrap();
            if matches!(event.observation().kind(), ObservationKind::Tool(_)) {
                searches += 1;
                assert_eq!(event.is_authorized_web_search(), web);
            }
            if matches!(event.observation().kind(), ObservationKind::TurnEnded(_)) { break; }
        }
        assert_eq!(searches, 2);
        p.stop();
    }
}
#[tokio::test]
async fn shared_text_actual_process_has_one_turn_and_separate_metadata_policy() {
    use hagency_runtime::codex::session::Update;
    let (t, mut p, instructions) = text_fixture();
    p.initialize().await.unwrap();
    let observed = p.qualify_shared_text(&instructions).await.unwrap();
    let source = p.shared_text_process_source();
    assert!(source.same(observed.source()));
    assert!(!source.has_live_activity());
    assert!(p.start_shared_text_thread().await.is_err());
    let maximum = std::time::Instant::now() + Duration::from_secs(30);
    p.install_shared_text_activity(
        1,
        std::time::Instant::now() + Duration::from_secs(10),
        maximum,
    )
    .unwrap();
    assert!(
        p.install_shared_text_activity(
            1,
            std::time::Instant::now() + Duration::from_secs(10),
            maximum
        )
        .is_err()
    );
    p.start_shared_text_thread().await.unwrap();
    assert!(!p.managed_process_source().is_live());
    assert!(source.has_live_activity());
    p.start_shared_text_turn("Offline fixture request".into())
        .await
        .unwrap();
    let mut text = String::new();
    loop {
        match p.next_observed_update().await.unwrap().0 {
            Update::TextDelta { delta, .. } => text.push_str(&delta),
            Update::TurnEnded => break,
            _ => {}
        }
    }
    assert_eq!(text, "Offline answer.");
    assert!(p.start_shared_text_turn("second".into()).await.is_err());
    assert_eq!(
        p.stop_shared_text_observation().state(),
        ManagedProcessState::Stopped
    );
    assert!(!source.is_live());
    let methods = std::fs::read_to_string(t.path().join("methods")).unwrap();
    assert_eq!(methods.lines().filter(|m| *m == "turn/start").count(), 1);
}
#[tokio::test]
async fn shared_text_expired_activity_never_revives_or_starts_thread() {
    let (t, mut p, instructions) = text_fixture();
    p.initialize().await.unwrap();
    p.qualify_shared_text(&instructions).await.unwrap();
    let maximum = std::time::Instant::now() + Duration::from_secs(20);
    p.install_shared_text_activity(
        1,
        std::time::Instant::now() + Duration::from_millis(50),
        maximum,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        p.install_shared_text_activity(
            2,
            std::time::Instant::now() + Duration::from_secs(10),
            maximum
        )
        .is_err()
    );
    assert!(p.start_shared_text_thread().await.is_err());
    p.stop();
    assert!(
        !std::fs::read_to_string(t.path().join("methods"))
            .unwrap()
            .contains("thread/start")
    );
}
#[tokio::test]
async fn shared_text_rejects_metadata_instruction_bytes_and_foreign_source() {
    let (t, mut p) = fixture("normal", |_| {});
    p.initialize().await.unwrap();
    assert!(
        p.qualify_shared_text(
            &t.path()
                .canonicalize()
                .unwrap()
                .join(MANAGED_INSTRUCTIONS_FILE)
        )
        .await
        .is_err()
    );
    let (_a, mut a, ia) = text_fixture();
    let (_b, mut b, ib) = text_fixture();
    a.initialize().await.unwrap();
    b.initialize().await.unwrap();
    let o = a.qualify_shared_text(&ia).await.unwrap();
    b.qualify_shared_text(&ib).await.unwrap();
    assert!(!b.shared_text_process_source().same(o.source()));
    a.stop();
    b.stop();
}

#[tokio::test]
async fn shared_text_renewal_keeps_the_original_pending_read_without_cancellation() {
    let (t, mut p, instructions) = text_fixture_mode("text-block");
    p.initialize().await.unwrap();
    p.qualify_shared_text(&instructions).await.unwrap();
    let maximum = std::time::Instant::now() + Duration::from_secs(10);
    p.install_shared_text_activity(
        1,
        std::time::Instant::now() + Duration::from_secs(1),
        maximum,
    )
    .unwrap();
    let control = p.shared_text_activity_control().unwrap();
    p.start_shared_text_thread().await.unwrap();
    p.start_shared_text_turn("Offline fixture".into())
        .await
        .unwrap();
    let read = p.next_shared_text_event();
    tokio::pin!(read);
    tokio::select! {event=&mut read=>panic!("unreleased fixture returned {}",event.is_ok()),_ = tokio::time::sleep(Duration::from_millis(50))=>{}}
    control
        .renew(
            2,
            std::time::Instant::now() + Duration::from_secs(3),
            maximum,
        )
        .unwrap();
    assert!(
        control
            .renew(
                2,
                std::time::Instant::now() + Duration::from_secs(3),
                maximum
            )
            .is_err()
    );
    tokio::select! {event=&mut read=>panic!("old activity deadline terminated read {}",event.is_ok()),_ = tokio::time::sleep(Duration::from_millis(1100))=>{}}
    std::fs::write(t.path().join("release-text"), b"release").unwrap();
    assert!(read.await.is_ok());
}

#[tokio::test]
async fn no_tools_qualification_still_rejects_standard_and_yolo_execution_settings() {
    for yolo in [false, true] {
        let (t,mut process)=fixture_policy("normal",|_|{},Some(yolo));
        process.initialize().await.unwrap();
        let error=process.qualify_managed_no_tools(&t.path().canonicalize().unwrap().join(MANAGED_INSTRUCTIONS_FILE)).await.err().unwrap();
        assert_eq!(error,Error::Policy);
        assert!(!process.managed_process_source().is_live());
        let methods=std::fs::read_to_string(t.path().join("methods")).unwrap();
        assert!(methods.lines().all(|v|matches!(v,"initialize"|"initialized")));
    }
}
