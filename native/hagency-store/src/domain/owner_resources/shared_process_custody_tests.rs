//! Real supervised offline children: no provider credentials or model calls.
use super::*;
use hagency_runtime::{
    codex::{session::Settings, transport::Limits},
    owned::{
        MANAGED_DISABLED_FEATURES, OwnedSession, SHARED_TEXT_INSTRUCTIONS,
        SHARED_TEXT_INSTRUCTIONS_FILE,
    },
};

async fn text_fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let registry = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let profile = authority('a');
    let provider = VerifiedProviderAccount::new(
        "codex",
        "openai.chatgpt",
        "fixture_account",
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap();
    let resource = registry
        .prepare_enrollment(
            profile.clone(),
            "text_provider".into(),
            provider,
            "gpt-5.6-sol".into(),
            Some("medium".into()),
            1000,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap()
        .approve()
        .await
        .unwrap()
        .resource_id;
    let grant = grant(&registry, &profile, &resource, "parent", 700)
        .await
        .unwrap();
    let project = super::super::command(&grant, "project", 400);
    let receipt = apply(&registry, &profile, project.clone()).await;
    Fixture {
        _temp: temp,
        registry,
        profile,
        resource,
        grant,
        project,
        receipt,
    }
}
async fn another_command(f: &Fixture, id: &str) -> SharedRuntimeCommand {
    let reserve_command = agent(f, id, 150);
    let saved = reserve(f, reserve_command.clone()).await;
    let now = graphs::now_ms().unwrap();
    SharedRuntimeCommand {
        version: 1,
        capability: SHARED_EXECUTION_CAPABILITY.into(),
        operation: "agent.execution.provision".into(),
        command_id: format!("shared_{id}"),
        assignment_id: format!("assignment_{id}"),
        assignment_generation: 1,
        agent_reservation_id: saved.agent_reservation_id.clone().unwrap(),
        reserve_command_digest: digest(&reserve_command),
        reserve_receipt_digest: digest(&saved),
        reserve_command,
        policy: SHARED_TEXT_POLICY.into(),
        issued_at_ms: now,
        expires_at_ms: now + 30_000,
    }
}
async fn spawn(f: &Fixture, id: &str) -> (OwnedSession, PathBuf) {
    let binary = PathBuf::from(
        std::env::var_os("HAGENCY_TEST_RUNTIME_PROBE").expect("supervised offline fixture"),
    );
    let root = f._temp.path().join(id);
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let instructions = root.join(SHARED_TEXT_INSTRUCTIONS_FILE);
    std::fs::write(&instructions, SHARED_TEXT_INSTRUCTIONS).unwrap();
    let features: serde_json::Map<String, serde_json::Value> = MANAGED_DISABLED_FEATURES
        .iter()
        .map(|key| ((*key).into(), false.into()))
        .collect();
    let config = json!({"model_provider":"openai","chatgpt_base_url":"https://chatgpt.com/backend-api","model_providers":{},"model_instructions_file":instructions,"instructions":"","notify":[],"web_search":"disabled","project_doc_max_bytes":0,"skills":{"include_instructions":false,"bundled":{"enabled":false}},"developer_instructions":"","include_apps_instructions":false,"include_environment_context":false,"mcp_servers":{},"features":features});
    std::fs::write(
        root.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let launch = hagency_platform::Launch {
        executable: binary.clone(),
        arguments: vec!["text".into(), root.as_os_str().into()],
        directory: root.clone(),
        environment: std::collections::BTreeMap::new(),
        require_crash_containment: false,
    };
    let settings = Settings::new(root, "fixture".into(), "medium".into())
        .unwrap()
        .read_only();
    let mut process = OwnedSession::spawn(
        &binary,
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
    process.initialize().await.unwrap();
    (process, instructions)
}
fn no_model_calls(instructions: &std::path::Path) {
    let methods = std::fs::read_to_string(instructions.parent().unwrap().join("methods")).unwrap();
    assert!(!methods.contains("thread/start") && !methods.contains("turn/start"));
}
async fn cleanup(
    f: &Fixture,
    c: &SharedRuntimeCommand,
    generation: u64,
) -> ManagedCleanupAuthority {
    f.registry
        .shared_cleanup_authority(
            enrollment_from(
                &f.profile,
                &f.resource,
                &f.grant.resource_revision,
                generation,
            ),
            c.clone(),
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires HAGENCY_TEST_RUNTIME_PROBE built from the supervised offline runtime fixture"]
async fn shared_process_custody_source_can_belong_to_only_one_original_child() {
    let f = text_fixture().await;
    let first = command(&f).await;
    let second = another_command(&f, "second").await;
    let SharedRuntimeClaim::Issued(first_custody) = f
        .registry
        .claim_shared_runtime(proof(&f, first.clone()))
        .await
        .unwrap()
    else {
        panic!("first custody")
    };
    let SharedRuntimeClaim::Issued(second_custody) = f
        .registry
        .claim_shared_runtime(proof(&f, second.clone()))
        .await
        .unwrap()
    else {
        panic!("second custody")
    };
    let (mut process, instructions) = spawn(&f, "first_process").await;
    let source = process.shared_text_process_source();
    let incarnation = first_custody.incarnation().to_owned();
    let sink = (*first_custody).bind_process(source.clone()).unwrap();
    assert!(source.has_managed_custody(&digest(&first), &incarnation));
    assert!(
        source
            .claim_managed_custody(&digest(&first), &incarnation)
            .is_err(),
        "an exact clone is not another custody claim"
    );
    assert!(
        (*second_custody).bind_process(source).is_err(),
        "different child on same provider cannot reuse actual process"
    );
    let (mut foreign, foreign_instructions) = spawn(&f, "foreign_process").await;
    assert!(
        sink.record(foreign.stop_shared_text_observation())
            .await
            .is_err(),
        "foreign stop is not proof for the retained source"
    );
    let qualified = sink
        .record(process.qualify_shared_text(&instructions).await.unwrap())
        .await
        .unwrap();
    let stopped = sink
        .record(process.stop_shared_text_observation())
        .await
        .unwrap();
    assert_eq!(stopped.state, "stopped");
    assert!(stopped.ledger_revision > qualified.ledger_revision);
    assert!(
        f.registry
            .recover_shared_process(cleanup(&f, &second, 1).await, second.clone(), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        matches!(
            f.registry
                .claim_shared_runtime(proof(&f, second))
                .await
                .unwrap(),
            SharedRuntimeClaim::History(_)
        ),
        "lost second custody never respawns"
    );
    let saved = f
        .registry
        .recover_shared_process(
            cleanup(&f, &first, 1).await,
            first,
            Some(qualified.observation_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(digest(&saved), digest(&qualified));
    no_model_calls(&instructions);
    no_model_calls(&foreign_instructions);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires HAGENCY_TEST_RUNTIME_PROBE built from the supervised offline runtime fixture"]
async fn shared_process_custody_expired_before_bind_keeps_actual_stop_evidence_only() {
    let mut f = text_fixture().await;
    let c = command(&f).await;
    let deadline = Instant::now() + Duration::from_secs(2);
    let delivery = VerifiedSharedRuntimeDelivery::new(
        enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1),
        c.clone(),
        digest(&c),
        true,
        deadline,
    )
    .unwrap();
    let SharedRuntimeClaim::Issued(custody) =
        f.registry.claim_shared_runtime(delivery).await.unwrap()
    else {
        panic!("original custody")
    };
    let (mut process, instructions) = spawn(&f, "late_process").await;
    let qualified = process.qualify_shared_text(&instructions).await.unwrap();
    tokio::time::sleep_until(tokio::time::Instant::from_std(
        deadline + Duration::from_millis(1),
    ))
    .await;
    assert!(custody.check().is_err());
    assert!(process.shared_text_process_source().is_live());
    let sink = (*custody)
        .bind_process(process.shared_text_process_source())
        .unwrap();
    assert!(
        sink.record(qualified).await.is_err(),
        "late binding cannot qualify the expired process"
    );
    f.profile.retire();
    let saved = sink
        .record(process.stop_shared_text_observation())
        .await
        .unwrap();
    assert_eq!(saved.state, "stopped");
    f.profile = authority('a');
    let recovered = f
        .registry
        .recover_shared_process(cleanup(&f, &c, 2).await, c, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(digest(&recovered), digest(&saved));
    let count = f
        .registry
        .0
        .store
        .call(1024, |db| {
            Ok(db.db.query_row(
                "SELECT count(*) FROM owner_shared_process_observations",
                [],
                |r| r.get::<_, u64>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(count, 1, "no qualified receipt was saved after expiry");
    no_model_calls(&instructions);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires HAGENCY_TEST_RUNTIME_PROBE built from the supervised offline runtime fixture"]
async fn shared_process_custody_already_stopped_before_bind_retains_cleanup_after_retirement() {
    let mut f = text_fixture().await;
    let c = command(&f).await;
    let SharedRuntimeClaim::Issued(custody) = f
        .registry
        .claim_shared_runtime(proof(&f, c.clone()))
        .await
        .unwrap()
    else {
        panic!("original custody")
    };
    let (mut process, instructions) = spawn(&f, "stopped_before_bind").await;
    let stopped = process.stop_shared_text_observation();
    assert!(!process.shared_text_process_source().is_live());
    f.profile.retire();
    assert!(custody.check().is_err());
    let sink = (*custody)
        .bind_process(process.shared_text_process_source())
        .unwrap();
    let saved = sink.record(stopped).await.unwrap();
    assert_eq!(saved.state, "stopped");
    f.profile = authority('a');
    let recovered = f
        .registry
        .recover_shared_process(cleanup(&f, &c, 2).await, c, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(digest(&recovered), digest(&saved));
    no_model_calls(&instructions);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires HAGENCY_TEST_RUNTIME_PROBE built from the supervised offline runtime fixture"]
async fn shared_process_custody_queued_qualification_rechecks_retirement_and_retains_stop() {
    let mut f = text_fixture().await;
    let c = command(&f).await;
    let SharedRuntimeClaim::Issued(custody) = f
        .registry
        .claim_shared_runtime(proof(&f, c.clone()))
        .await
        .unwrap()
    else {
        panic!("original custody")
    };
    let (mut process, instructions) = spawn(&f, "queued_process").await;
    let sink = (*custody)
        .bind_process(process.shared_text_process_source())
        .unwrap();
    let observed = process.qualify_shared_text(&instructions).await.unwrap();
    let store = f.registry.0.store.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (unblock, blocked) = std::sync::mpsc::channel();
    let blocker = tokio::spawn(async move {
        store
            .call(1024, move |_| {
                started.send(()).unwrap();
                blocked.recv_timeout(Duration::from_secs(10)).unwrap();
                Ok(())
            })
            .await
            .unwrap();
    });
    ready.await.unwrap();
    let mut pending = Box::pin(sink.record(observed));
    tokio::select! { biased; _=&mut pending => panic!("qualification bypassed writer"), _=tokio::task::yield_now()=>{} }
    f.profile.retire();
    unblock.send(()).unwrap();
    blocker.await.unwrap();
    assert!(pending.await.is_err());
    let stopped = sink
        .record(process.stop_shared_text_observation())
        .await
        .unwrap();
    assert_eq!(stopped.state, "stopped");
    f.profile = authority('a');
    let recovered = f
        .registry
        .recover_shared_process(cleanup(&f, &c, 2).await, c, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(digest(&recovered), digest(&stopped));
    let count = f
        .registry
        .0
        .store
        .call(1024, |db| {
            Ok(db.db.query_row(
                "SELECT count(*) FROM owner_shared_process_observations",
                [],
                |r| r.get::<_, u64>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
    no_model_calls(&instructions);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires HAGENCY_TEST_RUNTIME_PROBE; real supervised offline process, no provider"]
async fn managed_settlement_waits_for_its_own_original_process_exit() {
    use super::super::{custody::settlement_tests, fences};
    let f = text_fixture().await;
    let c = another_command(&f, "settlement_original").await;
    let SharedRuntimeClaim::Issued(custody) = f
        .registry
        .claim_shared_runtime(proof(&f, c.clone()))
        .await
        .unwrap()
    else {
        panic!("first custody")
    };
    let (mut process, instructions) = spawn(&f, "settlement_original_process").await;
    let sink = (*custody)
        .bind_process(process.shared_text_process_source())
        .unwrap();
    sink.record(process.qualify_shared_text(&instructions).await.unwrap())
        .await
        .unwrap();
    let fc = fences::command(&f, c.reserve_command.clone(), "settlement_original_fence");
    let fr = fences::apply_fence(&f, fc.clone()).await;
    let pending_command = settlement_tests::command(&fc, &fr, "settlement_original_pending");
    let pending = settlement_tests::settle(&f, pending_command.clone()).await;
    assert!(!pending.usage_complete);
    assert_eq!(pending.retained_tokens, 150);
    assert_eq!(pending.process_state, "unknown");
    sink.record(process.stop_shared_text_observation())
        .await
        .unwrap();
    let done = settlement_tests::settle(
        &f,
        settlement_tests::command(&fc, &fr, "settlement_original_done"),
    )
    .await;
    assert!(done.usage_complete);
    assert_eq!(done.retained_tokens, 0);
    assert_eq!(done.process_state, "stopped");
    assert_eq!(
        digest(&settlement_tests::settle(&f, pending_command).await),
        digest(&pending)
    );
    no_model_calls(&instructions);
}
