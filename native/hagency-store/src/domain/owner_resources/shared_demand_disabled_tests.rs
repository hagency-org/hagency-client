//! Intake-disabled builds retain all existing demand restrictions.
use super::*;
#[tokio::test]
async fn shared_demand_disabled_refuses_new_offers_and_preserves_saved_fences() {
    let f = fixture().await;
    let c = command(&f).await;
    f.registry
        .claim_shared_runtime(proof(&f, c.clone()))
        .await
        .unwrap();
    stopped(&f, &c, "stopped").await;
    let o = SharedDemandOffer {
        version: 1,
        demand_id: "saved_demand".into(),
        assignment_id: c.assignment_id.clone(),
        event_id: "$saved_event".into(),
        event_digest: "d".repeat(64),
        control_revision: 1,
        received_at_ms: graphs::now_ms().unwrap(),
        predecessor_command_id: c.command_id.clone(),
        predecessor_command_digest: digest(&c),
    };
    let e = || enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1);
    let proof = VerifiedSharedDemand::new(
        e(),
        o.clone(),
        digest(&o),
        graphs::now_ms().unwrap(),
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    assert!(matches!(
        f.registry
            .prepare_shared_demand(proof, c.clone(), None)
            .await,
        Err(Error::Unqualified)
    ));
    let saved = f
        .registry
        .prepare_shared_attempt_start(e(), c.clone(), None, "saved_start".into())
        .await
        .unwrap();
    let input = SharedDemandStartInput {
        demand_digest: digest(&o),
        expected_control_revision: 1,
        start: saved.input().clone(),
    };
    let child = c.agent_reservation_id.clone();
    let profile = f.profile.digest.clone();
    let device = c.reserve_command.executor_device_id.clone();
    let input_copy = input.clone();
    // Models rows retained by an earlier intake-enabled process. They are
    // descriptive fixture data, never reconstructed process authority.
    f.registry.0.store.call(32*1024,move|db| {
        let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rev=fixture_revision(&tx)?;
        tx.execute("INSERT INTO owner_shared_demand_controls VALUES(?1,?2,1,0)",params![child,profile])?;
        tx.execute("INSERT INTO owner_shared_demands VALUES(?1,?2,?3,?4,1,?5,?6,?7,?8)",params![o.demand_id,child,profile,device,o.event_id,canonical::encode(&serde_json::to_value(&o)?)?,digest(&o),rev])?;
        tx.execute("INSERT INTO owner_shared_demand_decisions VALUES(?1,'start_prepared',?2,?3,?4,NULL,?5)",params![o.demand_id,input_copy.start.start_intent_id,canonical::encode(&serde_json::to_value(&input_copy)?)?,digest(&input_copy),rev])?;
        tx.commit()?;Ok(())
    }).await.unwrap();
    assert!(
        f.registry
            .next_shared_attempt_start(f.profile.clone(), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.registry
            .latest_shared_attempt_start(f.profile.clone(), c.assignment_id.clone())
            .await
            .unwrap()
            .is_none()
    );
    let mut next = attempt(&f, &c).await;
    next.start_intent_id = input.start.start_intent_id;
    next.predecessor = input.start.predecessor;
    next.budget_observation = input.start.budget_observation;
    f.registry
        .stop_shared_demand_revision(cleanup(&f, &c, 1).await, 1)
        .await
        .unwrap();
    assert!(matches!(
        f.registry
            .claim_shared_successor(attempt_proof(&f, next))
            .await,
        Err(Error::LocalAuthority)
    ));
    f.registry.shutdown().await.unwrap();
}
