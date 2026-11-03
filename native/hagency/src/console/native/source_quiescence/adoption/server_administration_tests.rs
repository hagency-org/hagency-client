#[test]
fn adopted_server_administration_preserves_custody_in_an_isolated_process() {
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "console::native::source_quiescence::adoption::tests::adopted_server_administration_child",
            "--exact", "--nocapture",
        ])
        .env("HAGENCY_ADOPTED_SERVER_ADMIN_TEST", "1")
        .output().unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[tokio::test]
async fn adopted_server_administration_child() {
    if std::env::var("HAGENCY_ADOPTED_SERVER_ADMIN_TEST").as_deref() != Ok("1") {
        return;
    }
    use crate::console::native::{Command, LocalAction, ProviderAction};
    let f = Fixture::new().await;
    let (mut domain, source, resource) = f.domain();
    domain
        .put_seat(&Seat {
            id: resource.seat_id.clone(),
            declaration: None,
        })
        .unwrap();
    drop(domain);
    let (ledger, personal) = f.personal();
    drop(ledger);
    let registry =
        OwnerResourceRegistry::isolated_for_tests(&f.temp.path().join("canonical")).unwrap();
    let (work, sources) = quiesce(&f, &registry).await;
    let transfer = sources
        .prepare_resource_adoption("server_admin_original".into(), provider(), 700)
        .await
        .unwrap();
    assert_eq!(transfer.advance(provider()).await.unwrap().phase, "active");
    transfer.drain_journal().await.unwrap();
    drop(transfer);
    drop(sources);
    drop(work);
    let f = f
        .restart_adopted(registry.clone(), "server_admin_original".into())
        .await;
    assert!(f.owner.0.management.adopted_ready().await);
    let source_bytes = std::fs::read(source.join("domain.sqlite3")).unwrap();
    let personal_bytes = std::fs::read(&personal).unwrap();
    assert!(hagency_store::DomainRepository::open(&source).is_err());
    assert!(hagency_agent_local::Ledger::open(&personal, "@owner:test").is_err());

    let project = serde_json::json!({"id":"prj_original","spaceId":"!space:test","active":true,"creationPolicy":{"mode":"members"},"revision":1});
    let room = serde_json::json!({"roomId":"!room:test","projectId":"prj_original","active":true,"creationPolicy":{"mode":"inherit_project"},"revision":1});
    let agent = serde_json::json!({"id":"agt_original","ownerUserId":"uid","puppetMxid":"@_hagency_agt_original:test","displayName":"Original","state":"active","generation":1,"executionDeviceId":"fixture-device","ownerDirectRoomId":null});
    let binding = serde_json::json!({"id":"bnd_original","agentId":"agt_original","projectId":"prj_original","scopeKind":"project","roomId":"!room:test","state":"active","generation":1});
    let receipt = serde_json::json!({"creation":{"agent":agent,"binding":binding},"commandState":"active","pendingReason":null});
    let status_route = "/hagency/api/v1/commands/agent.bind/original_bind";
    {
        let mut routes = f.recipient_routes.lock().unwrap();
        routes.insert(
            "/hagency/api/v1/projects".into(),
            serde_json::json!({"projects":[project.clone()]}),
        );
        routes.insert(
            "/hagency/api/v1/projects/adopt".into(),
            serde_json::json!({"project":project}),
        );
        routes.insert(
            "/hagency/api/v1/projects/prj_original/rooms".into(),
            serde_json::json!({"rooms":[room.clone()]}),
        );
        routes.insert(
            "/hagency/api/v1/projects/prj_original/rooms/adopt".into(),
            serde_json::json!({"room":room}),
        );
        routes.insert(
            "/hagency/api/v1/agents/agt_original/bindings".into(),
            serde_json::json!({"__fixture_http_status":503,"code":"matrix_outcome_unknown"}),
        );
        routes.insert(
            status_route.into(),
            serde_json::json!({"__fixture_http_status":404,"code":"not_found"}),
        );
    }
    let projects = f.owner.execute(Command::Projects).await.unwrap();
    assert_eq!(projects["projects"][0]["id"], "prj_original");
    let adopted = f
        .owner
        .execute(Command::AdoptProject {
            space_id: "!space:test".into(),
        })
        .await
        .unwrap();
    assert_eq!(adopted["project"]["id"], "prj_original");
    let adopted = f
        .owner
        .execute(Command::AdoptRoom {
            project_id: "prj_original".into(),
            room_id: "!room:test".into(),
        })
        .await
        .unwrap();
    assert_eq!(adopted["room"]["roomId"], "!room:test");
    assert_eq!(
        f.owner
            .execute(Command::ProjectRooms {
                project_id: "prj_original".into()
            })
            .await
            .unwrap()["rooms"][0]["roomId"],
        "!room:test"
    );

    let command = Command::BindAgent {
        agent_id: "agt_original".into(),
        input: serde_json::json!({"projectId":"prj_original","roomId":"!room:test","idempotencyKey":"original_bind"}),
    };
    assert_eq!(
        f.owner.execute(command.clone()).await.unwrap_err().code,
        "matrix_outcome_unknown"
    );
    let saved = f.owner.execute(Command::AgentCommands).await.unwrap();
    let saved = &saved["commands"][0];
    assert_eq!(saved["input"]["idempotencyKey"], "original_bind");
    assert_eq!(saved["state"], "unknown");
    let saved_id = saved["id"].as_str().unwrap().to_owned();
    let journal = personal
        .parent()
        .unwrap()
        .join("agent-commands")
        .join(format!("{saved_id}.json"));
    assert!(
        journal.is_file(),
        "binding journal must retain the exact fenced profile namespace"
    );
    let durable: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    assert_eq!(durable["input"]["idempotencyKey"], "original_bind");
    // Simulate the server's accepted result whose original response was lost.
    f.recipient_routes
        .lock()
        .unwrap()
        .insert(status_route.into(), receipt);
    let f = f
        .restart_adopted(registry.clone(), "server_admin_original".into())
        .await;
    let recovered = f
        .owner
        .execute(Command::ResumeAgentCommand {
            id: saved_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(recovered["creation"]["binding"]["id"], "bnd_original");
    assert_eq!(
        f.owner.execute(command).await.unwrap()["creation"]["binding"]["id"],
        "bnd_original"
    );
    assert!(
        f.owner.execute(Command::AgentCommands).await.unwrap()["commands"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.recipient_requests
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.starts_with("POST /hagency/api/v1/agents/agt_original/bindings "))
            .count(),
        1,
        "recovery must query original receipt without repeating the mutation"
    );

    for command in [
        Command::Local {
            agent_id: "agt_original".into(),
            action: LocalAction::StartAgentService,
            input: None,
            binding_id: None,
            requester: None,
        },
        Command::Provider {
            action: ProviderAction::Status,
        },
        Command::PrepareAgentWorkspace {
            agent_id: "agt_original".into(),
        },
        Command::AssignAgentToCurrentDevice {
            agent_id: "agt_original".into(),
            expected_generation: 1,
        },
        Command::AgentConfigurationStatus {
            agent_id: "agt_original".into(),
        },
    ] {
        assert_eq!(
            f.owner.execute(command).await.unwrap_err().code,
            "source_adoption_recovery_only"
        );
    }
    let before = f.recipient_requests.lock().unwrap().len();
    let admitted = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let lock = f.owner.0.calls.lock().await;
    let owner = f.owner.clone();
    let flag = admitted.clone();
    let queued = tokio::spawn(async move {
        owner
            .execute_guarded(
                Command::Projects,
                Arc::new(move || flag.load(Ordering::Acquire)),
            )
            .await
    });
    tokio::task::yield_now().await;
    admitted.store(false, Ordering::Release);
    drop(lock);
    assert_eq!(
        queued.await.unwrap().unwrap_err().code,
        "management_scope_changed"
    );
    assert_eq!(f.recipient_requests.lock().unwrap().len(), before);
    f.live.store(false, Ordering::Release);
    assert!(
        f.owner
            .execute(Command::AdoptProject {
                space_id: "!foreign:test".into()
            })
            .await
            .is_err()
    );
    assert!(
        !f.recipient_requests
            .lock()
            .unwrap()
            .iter()
            .skip(before)
            .any(|line| line.starts_with("POST /hagency/api/v1/projects/adopt "))
    );
    f.live.store(true, Ordering::Release);
    let f = f
        .restart_adopted(registry, "server_admin_original".into())
        .await;
    let before = f.recipient_requests.lock().unwrap().len();
    let lock = f.owner.0.calls.lock().await;
    let owner = f.owner.clone();
    let queued = tokio::spawn(async move {
        owner
            .execute_guarded(Command::Projects, Arc::new(|| true))
            .await
    });
    tokio::task::yield_now().await;
    f.owner.retire();
    drop(lock);
    assert_eq!(
        queued.await.unwrap().unwrap_err().code,
        "source_adoption_recovery_only"
    );
    assert_eq!(
        f.owner.execute(Command::Projects).await.unwrap_err().code,
        "source_adoption_recovery_only"
    );
    assert_eq!(f.recipient_requests.lock().unwrap().len(), before);
    assert_eq!(
        std::fs::read(source.join("domain.sqlite3")).unwrap(),
        source_bytes
    );
    assert_eq!(std::fs::read(&personal).unwrap(), personal_bytes);
    f.stop().await;
}
