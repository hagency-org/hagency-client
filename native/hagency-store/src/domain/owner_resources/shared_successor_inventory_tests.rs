//! Reads of original canonical history; no provider process or live server.
use super::*;

async fn page(f: &Fixture, after: Option<String>) -> Result<SharedSuccessorPage, Error> {
    f.registry
        .shared_successor_inventory(
            f.profile.clone(),
            f.grant.executor_device_id.clone(),
            f.grant.executor_generation,
            after,
        )
        .await
}
async fn revision(f: &Fixture) -> u64 {
    f.registry
        .0
        .store
        .call(64, |db| {
            Ok(db.db.query_row(
                "SELECT revision FROM owner_resource_identity WHERE singleton=1",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn shared_successor_inventory_preserves_original_stopped_and_saved_history_across_restart() {
    let mut f = fixture().await;
    let c = command(&f).await;
    f.registry
        .claim_shared_runtime(proof(&f, c.clone()))
        .await
        .unwrap();
    stopped(&f, &c, "stopped").await;
    let before = revision(&f).await;
    let rows = page(&f, None).await.unwrap();
    assert_eq!(
        revision(&f).await,
        before,
        "listing must not mutate ledger revision"
    );
    assert_eq!(rows.items.len(), 1);
    let row = &rows.items[0];
    assert_eq!(row.assignment_id, c.assignment_id);
    assert_eq!(row.request_id, c.reserve_command.request_id);
    assert_eq!(row.agent_name, c.reserve_command.definition.name);
    assert_eq!(row.project_id, c.reserve_command.project_id);
    assert_eq!(row.process_attempt, 1);
    assert_eq!(row.process_state.as_deref(), Some("stopped"));
    assert!(!row.saved_start);
    assert!(rows.next_after.is_none());
    let saved = f
        .registry
        .prepare_shared_attempt_start(
            enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1),
            c.clone(),
            None,
            "inventory_explicit_start".into(),
        )
        .await
        .unwrap();
    assert_eq!(saved.original().assignment_id, c.assignment_id);
    assert!(page(&f, None).await.unwrap().items[0].saved_start);
    f.registry.shutdown().await.unwrap();
    f.registry = OwnerResourceRegistry::open_root(&f._temp.path().join("owner")).unwrap();
    let reopened = page(&f, None).await.unwrap();
    assert_eq!(reopened.items[0].assignment_id, c.assignment_id);
    assert!(reopened.items[0].saved_start);
    let serialized = serde_json::to_value(&reopened).unwrap();
    let keys = serialized["items"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        keys,
        std::collections::BTreeSet::from([
            "assignmentId",
            "requestId",
            "agentName",
            "projectId",
            "predecessorCommandId",
            "processAttempt",
            "processState",
            "savedStart"
        ])
    );
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_successor_inventory_latest_attempt_is_history_not_running_or_new_capacity() {
    let f = fixture().await;
    let c = command(&f).await;
    f.registry
        .claim_shared_runtime(proof(&f, c.clone()))
        .await
        .unwrap();
    stopped(&f, &c, "stopped").await;
    let next = attempt(&f, &c).await;
    f.registry
        .claim_shared_successor(attempt_proof(&f, next.clone()))
        .await
        .unwrap();
    let before = revision(&f).await;
    let rows = page(&f, None).await.unwrap();
    assert_eq!(rows.items[0].process_attempt, 2);
    assert_eq!(rows.items[0].predecessor_command_id, next.command_id);
    assert_eq!(
        rows.items[0].process_state, None,
        "a claim is not a process observation"
    );
    assert_eq!(revision(&f).await, before);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_successor_inventory_filters_before_paging_and_keeps_boundary_row() {
    let f = fixture().await;
    let mut commands = vec![];
    for i in 0..20 {
        let id = format!("inventory_{i:02}");
        let reserve_command = agent(&f, &id, 1);
        let receipt = reserve(&f, reserve_command.clone()).await;
        let now = graphs::now_ms().unwrap();
        commands.push(SharedRuntimeCommand {
            version: 1,
            capability: SHARED_EXECUTION_CAPABILITY.into(),
            operation: "agent.execution.provision".into(),
            command_id: format!("command_{id}"),
            assignment_id: format!("assignment_{i:02}"),
            assignment_generation: 1,
            agent_reservation_id: receipt.agent_reservation_id.clone().unwrap(),
            reserve_command_digest: digest(&reserve_command),
            reserve_receipt_digest: digest(&receipt),
            reserve_command,
            policy: SHARED_TEXT_POLICY.into(),
            issued_at_ms: now,
            expires_at_ms: now + 30_000,
        });
    }
    // Synthetic immutable history provides many rows without launching children.
    let profile = f.profile.digest.clone();
    f.registry.0.store.call(256*1024,move|db|{
        let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let provider:String=tx.query_row("SELECT provider_digest FROM owner_resource_accounts LIMIT 1",[],|r|r.get(0))?;
        for (i,c) in commands.into_iter().enumerate() {
            let mut c=c;
            if i==0 { c.reserve_command.executor_device_id="another_device".into();c.reserve_command_digest=digest(&c.reserve_command); }
            if i==1 { c.reserve_command.executor_generation=2;c.reserve_command_digest=digest(&c.reserve_command); }
            c.validate("@owner:a.test")?;
            let rev=fixture_revision(&tx)?;
            tx.execute("INSERT INTO owner_shared_runtime_lifetimes VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![c.agent_reservation_id,profile,c.assignment_id,c.command_id,canonical::encode(&serde_json::to_value(&c)?)?,digest(&c),"b".repeat(64),provider,rev,format!("inventory_incarnation_{i}")])?;
        }
        tx.commit()?;Ok(())
    }).await.unwrap();
    let rows = page(&f, None).await.unwrap();
    assert_eq!(rows.items.len(), 16);
    assert_eq!(rows.items[0].assignment_id, "assignment_02");
    assert_eq!(rows.items[15].assignment_id, "assignment_17");
    assert_eq!(rows.next_after.as_deref(), Some("assignment_17"));
    let last = page(&f, rows.next_after).await.unwrap();
    assert_eq!(
        last.items
            .iter()
            .map(|v| v.assignment_id.as_str())
            .collect::<Vec<_>>(),
        vec!["assignment_18", "assignment_19"]
    );
    assert!(last.next_after.is_none());
    // A cursor starts strictly after the last returned original ID.
    let suffix = page(&f, Some("assignment_15".into())).await.unwrap();
    assert_eq!(
        suffix
            .items
            .iter()
            .map(|v| v.assignment_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "assignment_16",
            "assignment_17",
            "assignment_18",
            "assignment_19"
        ]
    );
    let foreign = f
        .registry
        .shared_successor_inventory(authority('b'), f.grant.executor_device_id.clone(), 1, None)
        .await
        .unwrap();
    assert!(foreign.items.is_empty());
    assert!(page(&f, Some("bad/cursor".into())).await.is_err());
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_successor_inventory_rejects_retired_proof_and_closed_registry() {
    let f = fixture().await;
    let c = command(&f).await;
    f.registry.claim_shared_runtime(proof(&f, c)).await.unwrap();
    let live = Arc::new(AtomicBool::new(true));
    let gate = live.clone();
    let restricted = f
        .profile
        .clone()
        .with_native_gate(move || gate.load(Ordering::Acquire))
        .unwrap();
    live.store(false, Ordering::Release);
    assert!(matches!(
        f.registry
            .shared_successor_inventory(restricted, f.grant.executor_device_id.clone(), 1, None)
            .await,
        Err(Error::LocalAuthority)
    ));
    f.registry.shutdown().await.unwrap();
    assert!(page(&f, None).await.is_err());
}
