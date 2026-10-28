//! Real authenticated loopback HTTP, canonical SQLite and retained probe trees.
use super::*;
use crate::console::native::{
    ManagedRuntimeProgress, ManagedRuntimeTransport, managed_runtime::PreparedLaunch,
};
use hagency_runtime::owned::{
    MANAGED_DISABLED_FEATURES, MANAGED_INSTRUCTIONS, MANAGED_INSTRUCTIONS_FILE,
};
pub(super) fn management(path: &str, body: &Value, s: &mut State) -> Option<Value> {
    let at = now();
    let c = s.runtime_command.clone().unwrap();
    let id = c.id();
    let hash = digest(&c).unwrap();
    if path.ends_with("/result") {
        let r: ManagedRuntimeReceipt = serde_json::from_value(body["receipt"].clone()).unwrap();
        r.validate_for(&c).unwrap();
        assert_eq!(body["commandDigest"], hash);
        if let Some(prior) = s.runtime_receipts.get(&r.observation_id) {
            assert_eq!(digest(prior).unwrap(), digest(&r).unwrap());
        } else {
            s.runtime_writes += 1;
            s.runtime_receipts
                .insert(r.observation_id.clone(), r.clone());
        }
        if s.invalidate_at_result {
            s.source.as_ref().unwrap().store(false, Ordering::Release);
        }
        if std::mem::take(&mut s.lose_result) {
            return None;
        }
        return Some(
            json!({"commandId":id,"commandDigest":hash,"receiptDigest":if s.wrong_ack{"f".repeat(64)}else{digest(&r).unwrap()},
            "runtime":{"assignmentId":c.original().assignment_id,"commandId":c.original().command_id,"state":r.state,"processState":r.process_state,
                "provisionExpiresAtMs":c.original().expires_at_ms,"latest":r,"workAvailable":false,"capacityReleased":false},"observedAtMs":at}),
        );
    }
    let historical = path.ends_with("/recover");
    if historical {
        assert_eq!(body["commandDigest"], hash);
    }
    let requested = body["observationId"].as_str();
    let recorded = s
        .runtime_receipts
        .values()
        .filter(|r| r.command_id == id && requested.is_none_or(|v| v == r.observation_id))
        .max_by_key(|r| r.ledger_revision)
        .cloned();
    let executable = if historical {
        s.historical_executable
    } else {
        match &c {
            ManagedRuntimeCommand::Provision(_) => recorded.is_none(),
            ManagedRuntimeCommand::Stop(_) => {
                recorded.as_ref().is_none_or(|r| r.state == "unknown")
            }
        }
    };
    let until = if s.expired_ticket {
        at - 1
    } else if s.short_ticket {
        at + 1000
    } else {
        at + 20000
    };
    let until = if executable && matches!(c, ManagedRuntimeCommand::Provision(_)) {
        until.min(c.original().expires_at_ms)
    } else {
        until
    };
    Some(
        json!({"delivery":{"command":c,"commandDigest":if s.tamper_digest{"f".repeat(64)}else{hash},"token":"e".repeat(64),
        "leaseUntilMs":until,"executionAllowed":executable,"recorded":recorded},"observedAtMs":at}),
    )
}
async fn fixture() -> (
    Fixture,
    ManagedRuntimeTransport,
    ManagedRuntimeProvisionCommand,
) {
    let (f, t, c) =
        prepared_with(Fixture::with_provider("openai.chatgpt", "fixture_account").await).await;
    t.poll(&f.registry, None).await.unwrap();
    let r: ManagedAgentReservationReceipt =
        serde_json::from_value(f.state.lock().unwrap().agent_receipts[&c.command_id].clone())
            .unwrap();
    let q = ManagedRuntimeProvisionCommand {
        version: 1,
        capability: AGENT_RUNTIME_CAPABILITY.into(),
        operation: "agent.runtime.provision".into(),
        command_id: "runtime_fixture".into(),
        assignment_id: "assignment_fixture".into(),
        assignment_generation: 1,
        agent_reservation_id: r.agent_reservation_id.clone().unwrap(),
        reserve_command: c.clone(),
        reserve_command_digest: digest(&c).unwrap(),
        reserve_receipt_digest: digest(&r).unwrap(),
        policy: "codex_no_tools_v1".into(),
        issued_at_ms: now(),
        expires_at_ms: now() + 30000,
    };
    f.state.lock().unwrap().runtime_command = Some(ManagedRuntimeCommand::Provision(q.clone()));
    let runtime = ManagedRuntimeTransport::fixture(f.owner.management(package(1)).await.unwrap());
    (f, runtime, q)
}
fn stop(f: &Fixture, q: &ManagedRuntimeProvisionCommand) {
    let s = ManagedRuntimeStopCommand {
        version: 1,
        capability: AGENT_RUNTIME_CAPABILITY.into(),
        operation: "agent.runtime.stop".into(),
        command_id: format!("stop_{}", q.command_id),
        provision_command: q.clone(),
        provision_command_digest: digest(q).unwrap(),
        issued_at_ms: now(),
    };
    f.state.lock().unwrap().runtime_command = Some(ManagedRuntimeCommand::Stop(s));
}
fn launch(f: &Fixture, mode: &str) -> PreparedLaunch {
    let binary = std::path::PathBuf::from(
        std::env::var_os("HAGENCY_TEST_RUNTIME_PROBE").expect("explicit supervised fixture binary"),
    );
    let root = f._root.path().join("runtime-probe");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let instructions = root.join(MANAGED_INSTRUCTIONS_FILE);
    std::fs::write(&instructions, MANAGED_INSTRUCTIONS).unwrap();
    let features: serde_json::Map<String, Value> = MANAGED_DISABLED_FEATURES
        .iter()
        .map(|k| ((*k).into(), false.into()))
        .collect();
    let config = json!({"model_provider":"openai","chatgpt_base_url":"https://chatgpt.com/backend-api","model_providers":{},"model_instructions_file":instructions,"instructions":"","notify":[],"web_search":"disabled","project_doc_max_bytes":0,"skills":{"include_instructions":false,"bundled":{"enabled":false}},"developer_instructions":"","include_apps_instructions":false,"include_environment_context":false,"mcp_servers":{},"features":features});
    std::fs::write(
        root.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    PreparedLaunch::fixture(binary, root, mode)
}
fn rows(f: &Fixture, table: &str) -> u64 {
    let db = rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
    db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
#[tokio::test]
async fn native_managed_runtime_transport_recovery_tamper_expiry_never_mints_custody() {
    let (f, t, q) = fixture().await;
    assert!(matches!(
        t.recover(&f.registry, q.command_id.clone(), digest(&q).unwrap(), None)
            .await
            .unwrap(),
        ManagedRuntimeProgress::HistoricalEvidenceMissing { .. }
    ));
    for field in ["historical", "digest", "expiry"] {
        {
            let mut s = f.state.lock().unwrap();
            s.historical_executable = field == "historical";
            s.tamper_digest = field == "digest";
            s.expired_ticket = field == "expiry";
        }
        assert!(
            t.recover(&f.registry, q.command_id.clone(), digest(&q).unwrap(), None)
                .await
                .is_err()
        );
    }
    assert_eq!(rows(&f, "owner_managed_runtime_lifetimes"), 0);
    assert_eq!(f.state.lock().unwrap().runtime_writes, 0);
    f.stop().await;
}
#[tokio::test]
async fn native_managed_runtime_transport_stop_before_provision_is_permanent_and_no_process_claim()
{
    let (f, t, q) = fixture().await;
    stop(&f, &q);
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="not_started")
    );
    assert_eq!(rows(&f, "owner_managed_runtime_lifetimes"), 1);
    f.state.lock().unwrap().runtime_command = Some(ManagedRuntimeCommand::Provision(q.clone()));
    assert!(matches!(
        t.recover(&f.registry, q.command_id.clone(), digest(&q).unwrap(), None)
            .await
            .unwrap(),
        ManagedRuntimeProgress::HistoricalEvidenceMissing { .. }
    ));
    assert_eq!(f.state.lock().unwrap().runtime_writes, 1);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built supervised probe"]
async fn native_managed_runtime_transport_qualified_reply_loss_recovers_then_exact_stop_drains() {
    let (f, t, q) = fixture().await;
    f.state.lock().unwrap().lose_result = true;
    assert!(
        t.poll_fixture(&f.registry, launch(&f, "normal"))
            .await
            .is_err()
    );
    assert_eq!(f.state.lock().unwrap().runtime_writes, 1);
    let original = f
        .state
        .lock()
        .unwrap()
        .runtime_receipts
        .values()
        .next()
        .unwrap()
        .clone();
    assert_eq!(original.state, "qualified");
    assert!(matches!(
        t.recover(
            &f.registry,
            q.command_id.clone(),
            digest(&q).unwrap(),
            Some(original.observation_id)
        )
        .await
        .unwrap(),
        ManagedRuntimeProgress::Recorded { .. }
    ));
    assert_eq!(f.state.lock().unwrap().runtime_writes, 1);
    stop(&f, &q);
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="stopped")
    );
    f.owner.0.managed_runtime.drain().await.unwrap();
    assert_eq!(f.state.lock().unwrap().runtime_writes, 2);
    let methods = std::fs::read_to_string(f._root.path().join("runtime-probe/methods")).unwrap();
    assert_eq!(methods.lines().filter(|v| *v == "initialize").count(), 1);
    assert!(!methods.contains("turn/") && !methods.contains("thread/"));
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built supervised probe"]
async fn native_managed_runtime_transport_cancelled_wait_retains_original_process_and_drain_result()
{
    let (f, t, q) = fixture().await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(350),
            t.poll_fixture(&f.registry, launch(&f, "delay"))
        )
        .await
        .is_err()
    );
    f.owner.0.managed_runtime.drain().await.unwrap();
    stop(&f, &q);
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="stopped")
    );
    assert_eq!(rows(&f, "owner_managed_runtime_lifetimes"), 1);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built supervised probe"]
async fn native_managed_runtime_transport_monotonic_expiry_stops_and_cannot_respawn() {
    let (f, t, mut q) = fixture().await;
    // Require qualification first, then expire the original immutable deadline.
    // A startup refusal must not silently substitute for post-start coverage.
    q.expires_at_ms = now() + 8000;
    f.state.lock().unwrap().runtime_command = Some(ManagedRuntimeCommand::Provision(q.clone()));
    let until = std::time::Instant::now() + Duration::from_millis(8150);
    assert!(
        matches!(t.poll_fixture(&f.registry, launch(&f, "normal")).await.unwrap(), ManagedRuntimeProgress::Recorded { process_state, .. } if process_state == "live")
    );
    tokio::time::sleep_until(until.into()).await;
    stop(&f, &q);
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="stopped")
    );
    f.state.lock().unwrap().runtime_command = Some(ManagedRuntimeCommand::Provision(q.clone()));
    t.poll_fixture(&f.registry, launch(&f, "normal"))
        .await
        .unwrap();
    let methods = std::fs::read_to_string(f._root.path().join("runtime-probe/methods")).unwrap();
    assert_eq!(methods.lines().filter(|v| *v == "initialize").count(), 1);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built supervised probe"]
async fn native_managed_runtime_transport_startup_expiry_refuses_and_stops_original() {
    let (f, t, q) = fixture().await;
    f.state.lock().unwrap().short_ticket = true;
    let error = t
        .poll_fixture(&f.registry, launch(&f, "delay-config"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "managed_runtime_lease_expired");
    stop(&f, &q);
    f.state.lock().unwrap().short_ticket = false;
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="stopped")
    );
    f.state.lock().unwrap().runtime_command = Some(ManagedRuntimeCommand::Provision(q.clone()));
    t.poll_fixture(&f.registry, launch(&f, "normal"))
        .await
        .unwrap();
    let methods = std::fs::read_to_string(f._root.path().join("runtime-probe/methods")).unwrap();
    assert_eq!(methods.lines().filter(|v| *v == "initialize").count(), 1);
    f.owner.0.managed_runtime.drain().await.unwrap();
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built supervised probe"]
async fn native_managed_runtime_transport_retirement_records_stop_before_store_shutdown() {
    let (f, t, _) = fixture().await;
    t.poll_fixture(&f.registry, launch(&f, "normal"))
        .await
        .unwrap();
    f.owner.retire();
    f.owner.0.managed_runtime.drain().await.unwrap();
    let db = rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
    let stopped:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_managed_runtime_observations WHERE json_extract(payload,'$.evidence.state')='stopped')",[],|r|r.get(0)).unwrap();
    assert!(stopped);
    drop(db);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built supervised probe"]
async fn native_managed_runtime_transport_unknown_launch_keeps_drain_failed_and_capacity_held() {
    let (f, t, q) = fixture().await;
    let p = PreparedLaunch::fixture(
        f._root.path().join("missing-binary"),
        f._root.path().canonicalize().unwrap(),
        "normal",
    );
    assert!(t.poll_fixture(&f.registry, p).await.is_err());
    assert!(f.owner.0.managed_runtime.drain().await.is_err());
    assert!(f.owner.0.managed_runtime.drain().await.is_err());
    stop(&f, &q);
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="unknown")
    );
    // Central still sends an executable cleanup with its prior unknown result.
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="unknown")
    );
    assert_eq!(f.state.lock().unwrap().runtime_writes, 1);
    f.stop().await;
}

#[tokio::test]
async fn native_managed_runtime_transport_absent_manager_after_issued_history_stays_unknown() {
    let (f, t, q) = fixture().await;
    let scope = f
        .transport
        .scope(Permission::ExecuteManagedAgents)
        .await
        .unwrap();
    let grant = f
        .state
        .lock()
        .unwrap()
        .allocation
        .as_ref()
        .unwrap()
        .grant_receipt
        .clone();
    let enrollment = f.transport.enrollment(&scope, &grant, true).await.unwrap();
    let command = ManagedRuntimeCommand::Provision(q.clone());
    let claim = f
        .registry
        .claim_managed_runtime(
            VerifiedManagedRuntimeDelivery::new(
                enrollment,
                command.clone(),
                digest(&command).unwrap(),
                true,
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(claim, ManagedRuntimeClaim::Issued(_)));
    drop(claim);
    // A missing in-memory task is not evidence that the issued effect did not
    // start. Only canonical native evidence decides the stop receipt.
    f.owner
        .0
        .managed_runtime
        .stop(&q.agent_reservation_id, &digest(&q).unwrap())
        .await
        .unwrap();
    stop(&f, &q);
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="unknown")
    );
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="unknown")
    );
    assert_eq!(f.state.lock().unwrap().runtime_writes, 1);
    f.stop().await;
}

#[cfg(not(feature = "managed-runtime"))]
#[tokio::test]
async fn native_managed_runtime_transport_is_disabled_by_default_before_http_or_custody() {
    let (f, _, _) = fixture().await;
    let disabled = ManagedRuntimeTransport::new(f.owner.management(package(1)).await.unwrap());
    assert_eq!(
        disabled.poll(&f.registry, None).await.unwrap_err().code,
        "managed_runtime_disabled"
    );
    assert_eq!(rows(&f, "owner_managed_runtime_lifetimes"), 0);
    assert_eq!(f.state.lock().unwrap().runtime_writes, 0);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicitly built supervised probe"]
async fn native_managed_runtime_transport_stop_racing_qualification_joins_original_child() {
    let (f, t, q) = fixture().await;
    let t = Arc::new(t);
    let transport = t.clone();
    let registry = f.registry.clone();
    let prepared = launch(&f, "delay");
    let pending = tokio::spawn(async move { transport.poll_fixture(&registry, prepared).await });
    tokio::time::sleep(Duration::from_millis(350)).await;
    stop(&f, &q);
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="stopped")
    );
    assert!(pending.await.unwrap().is_err());
    f.owner.0.managed_runtime.drain().await.unwrap();
    assert_eq!(rows(&f, "owner_managed_runtime_lifetimes"), 1);
    f.stop().await;
}

#[tokio::test]
async fn native_managed_runtime_transport_new_generation_only_cleans_original_command() {
    let (f, t, q) = fixture().await;
    explicit_new_generation(&f, 2).await;
    assert!(t.poll(&f.registry, None).await.is_err());
    assert_eq!(rows(&f, "owner_managed_runtime_lifetimes"), 0);
    stop(&f, &q);
    assert!(
        matches!(t.poll(&f.registry,None).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="not_started")
    );
    f.stop().await;
}

#[tokio::test]
#[ignore = "explicit supervised fixture binary"]
async fn native_managed_runtime_transport_shared_inspection_stops_then_qualifies_independently() {
    let (f, t, q) = fixture().await;
    let launch = launch(&f, "normal").shared_fixture();
    let root = f._root.path().join("runtime-probe");
    let mut c: Value =
        serde_json::from_slice(&std::fs::read(root.join("config.json")).unwrap()).unwrap();
    c["features"]["future_feature"] = json!({"optional_config":true});
    c["features"]["network_proxy"] = Value::Null;
    c["mcp_servers"] = json!({"ambient":{"enabled":true}});
    std::fs::write(root.join("config.json"), serde_json::to_vec(&c).unwrap()).unwrap();
    assert!(matches!(
        t.poll_fixture(&f.registry, launch).await.unwrap(),
        ManagedRuntimeProgress::Recorded { .. }
    ));
    let methods = std::fs::read_to_string(root.join("methods")).unwrap();
    assert_eq!(methods.lines().filter(|s| *s == "initialize").count(), 2);
    assert_eq!(
        methods
            .lines()
            .filter(|s| *s == "account/rateLimits/read")
            .count(),
        2
    );
    assert_eq!(rows(&f, "owner_managed_runtime_lifetimes"), 1);
    stop(&f, &q);
    t.poll(&f.registry, None).await.unwrap();
    f.owner.0.managed_runtime.drain().await.unwrap();
    f.stop().await;
}
#[tokio::test]
#[ignore = "explicit supervised fixture binary"]
async fn native_managed_runtime_transport_shared_inspector_cancellation_never_issues() {
    let (f, t, _) = fixture().await;
    let prepared = launch(&f, "delay-config").shared_fixture();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(300),
            t.poll_fixture(&f.registry, prepared)
        )
        .await
        .is_err()
    );
    f.owner.retire();
    f.owner.0.managed_runtime.drain().await.unwrap();
    assert_eq!(rows(&f, "owner_managed_runtime_lifetimes"), 0);
    let methods = std::fs::read_to_string(f._root.path().join("runtime-probe/methods")).unwrap();
    assert!(
        methods
            .lines()
            .all(|s| matches!(s, "initialize" | "initialized" | "config/read"))
    );
    f.stop().await;
}

#[tokio::test]
#[ignore = "explicit supervised fixture binary"]
async fn native_managed_runtime_transport_shared_override_bound_fails_before_issuance() {
    let (f, t, _) = fixture().await;
    let launch = launch(&f, "normal").shared_fixture();
    let root = f._root.path().join("runtime-probe");
    let mut c: Value =
        serde_json::from_slice(&std::fs::read(root.join("config.json")).unwrap()).unwrap();
    let servers: serde_json::Map<String, Value> = (0..128)
        .map(|i| (format!("ambient_{i}"), json!({"enabled":true})))
        .collect();
    c["mcp_servers"] = servers.into();
    std::fs::write(root.join("config.json"), serde_json::to_vec(&c).unwrap()).unwrap();
    assert_eq!(
        t.poll_fixture(&f.registry, launch).await.unwrap_err().code,
        "managed_runtime_policy_bounds"
    );
    f.owner.0.managed_runtime.drain().await.unwrap();
    assert_eq!(rows(&f, "owner_managed_runtime_lifetimes"), 0);
    f.stop().await;
}

#[tokio::test]
#[ignore = "explicit supervised fixture binary"]
async fn native_managed_runtime_transport_network_metadata_uses_original_lease_not_two_seconds() {
    let (f, t, q) = fixture().await;
    let before = std::time::Instant::now();
    assert!(
        matches!(t.poll_fixture(&f.registry,launch(&f,"slow-quota")).await.unwrap(),ManagedRuntimeProgress::Recorded {process_state,..} if process_state=="live")
    );
    assert!(before.elapsed() >= Duration::from_millis(4400));
    stop(&f, &q);
    t.poll(&f.registry, None).await.unwrap();
    f.owner.0.managed_runtime.drain().await.unwrap();
    f.stop().await;
}
