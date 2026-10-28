use super::*;
fn challenge(
    c: ManagedAgentReserveCommand,
    r: &ManagedAgentReservationReceipt,
) -> ManagedAgentMatrixChallenge {
    let now = graphs::now_ms().unwrap();
    ManagedAgentMatrixChallenge {
        version: 1,
        capability: AGENT_MATRIX_CAPABILITY.into(),
        operation: "agent.matrix.check".into(),
        command_id: "matrix_check".into(),
        assignment_id: "assignment".into(),
        assignment_generation: 1,
        agent_reservation_id: r.agent_reservation_id.clone().unwrap(),
        reserve_command_digest: canonical::digest(&serde_json::to_value(&c).unwrap()).unwrap(),
        reserve_receipt_digest: canonical::digest(&serde_json::to_value(r).unwrap()).unwrap(),
        reserve_command: c,
        space_id: "!space:a.test".into(),
        puppet_mxid: "@_hagency_agent:a.test".into(),
        issued_at_ms: now,
        expires_at_ms: now + 20_000,
    }
}
fn proof(f: &Fixture, c: ManagedAgentMatrixChallenge) -> VerifiedManagedMatrixCheck {
    let digest = canonical::digest(&serde_json::to_value(&c).unwrap()).unwrap();
    VerifiedManagedMatrixCheck::new(
        enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1),
        c,
        digest,
        true,
        Instant::now() + Duration::from_secs(20),
    )
    .unwrap()
}
fn files(f: &Fixture) -> Vec<Option<Vec<u8>>> {
    ["domain.sqlite3", "domain.sqlite3-wal"]
        .iter()
        .map(|p| std::fs::read(f._temp.path().join("owner").join(p)).ok())
        .collect()
}
#[tokio::test]
async fn managed_matrix_readonly_snapshot_keeps_main_wal_and_runtime_unissued() {
    let f = fixture().await;
    let c = agent(&f, "matrix", 100);
    let r = reserve(&f, c.clone()).await;
    let q = challenge(c, &r);
    let before = files(&f);
    let value = f
        .registry
        .check_managed_matrix_reservation(proof(&f, q.clone()))
        .await
        .unwrap();
    value.validate_for(&q).unwrap();
    assert!(value.eligible);
    assert_eq!(value.observed_ledger_revision, r.ledger_revision);
    assert_eq!(files(&f), before);
    f.registry
        .0
        .store
        .call(1024, |db| {
            assert_eq!(
                db.db.query_row(
                    "SELECT count(*) FROM owner_managed_assignment_pins",
                    [],
                    |r| r.get::<_, u64>(0)
                )?,
                0
            );
            assert_eq!(
                db.db
                    .query_row("SELECT state FROM owner_managed_effects", [], |r| r
                        .get::<_, String>(0))?,
                "pending"
            );
            Ok(())
        })
        .await
        .unwrap();
}
#[tokio::test]
async fn managed_matrix_fenced_child_or_parent_is_not_eligible_and_snapshot_cannot_reopen() {
    let f = fixture().await;
    let c = agent(&f, "matrix_fenced", 100);
    let r = reserve(&f, c.clone()).await;
    let q = challenge(c.clone(), &r);
    let fence = ManagedAgentFenceCommand {
        version: 1,
        capability: AGENT_FENCE_CAPABILITY.into(),
        operation: "agent.fence".into(),
        command_id: "matrix_fence".into(),
        reserve_command: c,
        reserve_command_digest: q.reserve_command_digest.clone(),
        issued_at_ms: graphs::now_ms().unwrap(),
    };
    let digest = canonical::digest(&serde_json::to_value(&fence).unwrap()).unwrap();
    f.registry
        .apply_managed_agent_fence(
            VerifiedManagedAgentFence::new(
                enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1),
                fence,
                digest,
                f.project.clone(),
                true,
                Instant::now() + Duration::from_secs(20),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let before = files(&f);
    let observation = f
        .registry
        .check_managed_matrix_reservation(proof(&f, q))
        .await
        .unwrap();
    assert!(!observation.eligible);
    assert_eq!(
        observation.reason.as_deref(),
        Some("native_reservation_unavailable")
    );
    assert_eq!(files(&f), before);
    let g = fixture().await;
    let c = agent(&g, "closed_parent", 100);
    let r = reserve(&g, c.clone()).await;
    g.registry
        .0
        .store
        .call(1024, |db| {
            db.db
                .execute("UPDATE owner_capacity_grants SET active=0", [])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        !g.registry
            .check_managed_matrix_reservation(proof(&g, challenge(c, &r)))
            .await
            .unwrap()
            .eligible
    );
}
#[tokio::test]
async fn managed_matrix_wrong_receipt_profile_generation_expiry_or_historical_proof_is_denied() {
    let f = fixture().await;
    let c = agent(&f, "matrix_denied", 100);
    let r = reserve(&f, c.clone()).await;
    let q = challenge(c, &r);
    let mut bad = q.clone();
    bad.reserve_receipt_digest = "f".repeat(64);
    assert!(
        f.registry
            .check_managed_matrix_reservation(proof(&f, bad))
            .await
            .is_err()
    );
    let wrong = VerifiedManagedMatrixCheck::new(
        enrollment_from(&authority('b'), &f.resource, &f.grant.resource_revision, 1),
        q.clone(),
        canonical::digest(&serde_json::to_value(&q).unwrap()).unwrap(),
        true,
        Instant::now() + Duration::from_secs(20),
    );
    assert!(wrong.is_err());
    assert!(
        VerifiedManagedMatrixCheck::new(
            enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 2),
            q.clone(),
            canonical::digest(&serde_json::to_value(&q).unwrap()).unwrap(),
            true,
            Instant::now() + Duration::from_secs(20)
        )
        .is_err()
    );
    assert!(
        VerifiedManagedMatrixCheck::new(
            enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1),
            q.clone(),
            canonical::digest(&serde_json::to_value(&q).unwrap()).unwrap(),
            false,
            Instant::now() + Duration::from_secs(20)
        )
        .is_err()
    );
    assert!(
        VerifiedManagedMatrixCheck::new(
            enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1),
            q.clone(),
            canonical::digest(&serde_json::to_value(&q).unwrap()).unwrap(),
            true,
            Instant::now()
        )
        .is_err()
    );
}

#[tokio::test]
async fn managed_matrix_existing_runtime_pin_must_match_but_check_cannot_acquire_it() {
    let f = fixture().await;
    let c = agent(&f, "pinned", 100);
    let r = reserve(&f, c.clone()).await;
    let mut q = challenge(c, &r);
    let a = ManagedAssignmentAuthority::fixture(
        enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1),
        q.agent_reservation_id.clone(),
        "original_assignment".into(),
        1,
        Instant::now() + Duration::from_secs(20),
    );
    assert!(matches!(
        f.registry.claim_managed_provision(a).await.unwrap(),
        ManagedProvisionClaim::Issued(_)
    ));
    let before = files(&f);
    assert!(matches!(
        f.registry
            .check_managed_matrix_reservation(proof(&f, q.clone()))
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(files(&f), before);
    q.assignment_id = "original_assignment".into();
    assert!(
        f.registry
            .check_managed_matrix_reservation(proof(&f, q))
            .await
            .unwrap()
            .eligible
    );
    assert_eq!(files(&f), before);
}

#[test]
fn managed_matrix_canonical_fixture_is_closed_and_matches_central() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "capacity/managed_agents/management-agent-matrix-v1.json"
    ))
    .unwrap();
    let c: ManagedAgentMatrixChallenge =
        serde_json::from_value(fixture["challenge"].clone()).unwrap();
    c.validate("@owner:example.test").unwrap();
    let r: ManagedAgentMatrixObservation =
        serde_json::from_value(fixture["observation"].clone()).unwrap();
    r.validate_for(&c).unwrap();
    assert_eq!(
        canonical::digest(&serde_json::to_value(&c).unwrap()).unwrap(),
        fixture["challengeDigest"]
    );
    assert_eq!(
        canonical::digest(&serde_json::to_value(&r).unwrap()).unwrap(),
        fixture["observationDigest"]
    );
    let mut bad = fixture["challenge"].clone();
    bad["runtimeReady"] = json!(true);
    assert!(serde_json::from_value::<ManagedAgentMatrixChallenge>(bad).is_err());
}
