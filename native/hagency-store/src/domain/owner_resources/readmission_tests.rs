use super::*;

async fn consent(
    registry: &OwnerResourceRegistry, a: &ProfileAuthority, resource: &str,
    round: &str, id: &str, tokens: u64, expires: u64,
) -> OwnerGrantConsent {
    let revision = registry.resource(a, resource.into()).await.unwrap().revision;
    let proof = VerifiedManagementEnrollment::new(a.clone(), format!("enrollment_{round}"),
        "contribution_a".into(), resource.into(), revision, "@coordinator:a.test".into(),
        "device_a".into(), 1, Instant::now() + Duration::from_secs(30)).unwrap();
    registry.prepare_capacity_grant(proof, OwnerGrantSelection {
        command_id: format!("consent_{id}"), grant_id: id.into(),
        coordinator_mxid: "@coordinator:a.test".into(), delegation_expires_at_ms: expires,
        allow_self_approval: false, eligible_managers: vec!["@manager:a.test".into()], allocated_tokens: tokens,
    }, Instant::now() + Duration::from_secs(30)).unwrap()
        .with_admission_request(round.into()).unwrap().require_closed_prior_admissions().unwrap()
}

async fn close_original(registry: &OwnerResourceRegistry, a: &ProfileAuthority, g: &NativeGrantReceipt) -> NativeGrantSettlementReceipt {
    let command = GrantClosureCommand {
        version: 1, command_id: format!("close_{}", g.grant_id), operation: "grant.close".into(),
        grant_id: g.grant_id.clone(), grant_receipt_digest: canonical::digest(&serde_json::to_value(g).unwrap()).unwrap(),
        previous_settlement_digest: None, reason: "owner_withdrawal".into(), requested_at_ms: graphs::now_ms().unwrap(),
    };
    let mut proof = VerifiedManagementEnrollment::new(a.clone(), g.enrollment_id.clone(),
        g.contribution_id.clone(), g.resource_id.clone(), g.resource_revision.clone(),
        g.coordinator_mxid.clone(), g.executor_device_id.clone(), g.executor_generation,
        Instant::now() + Duration::from_secs(30)).unwrap();
    if let Some(id) = &g.admission_request_id { proof = proof.with_admission_request(id.clone()).unwrap(); }
    let digest = canonical::digest(&serde_json::to_value(&command).unwrap()).unwrap();
    registry.apply_parent_settlement(VerifiedParentDelivery::new(proof, command, digest, true,
        Instant::now() + Duration::from_secs(30)).unwrap()).await.unwrap()
}

#[tokio::test]
async fn new_admission_preserves_legacy_hold_and_retained_child_liability() {
    let temp = tempfile::tempdir().unwrap();
    let registry = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let a = authority('a');
    let resource = enroll(&registry, &a, "enroll").await;
    let original = grant(&registry, &a, &resource.resource_id, "old_parent", 700).await.unwrap();
    let bytes = serde_json::to_value(&original).unwrap();
    assert!(bytes.get("admissionRequestId").is_none());
    let child = apply(&registry, &a, command(&original, "original_child", 200)).await;
    assert_eq!(child.state, "reserved");
    let expires = graphs::now_ms().unwrap() + 60_000;
    assert!(consent(&registry, &a, &resource.resource_id, "next_request", "next_parent", 500, expires).await.approve().await.is_err());
    let settled = close_original(&registry, &a, &original).await;
    assert_eq!((settled.state.as_str(), settled.retained_tokens, settled.unresolved_tokens), ("retained", 200, 200));
    assert!(consent(&registry, &a, &resource.resource_id, "next_request", "too_large", 900, expires).await.approve().await.is_err());
    let next = consent(&registry, &a, &resource.resource_id, "next_request", "next_parent", 500, expires).await.approve().await.unwrap();
    assert_eq!(next.version, 2);
    assert_eq!(next.admission_request_id.as_deref(), Some("next_request"));
    assert_eq!(registry.resource(&a, resource.resource_id.clone()).await.unwrap().budget.pool.remaining.map(u64::from), Some(300));
    let recovered = registry.recover_capacity_grant(a.clone(), "device_a".into(), original.grant_id.clone()).await.unwrap().unwrap();
    assert_eq!(serde_json::to_value(recovered).unwrap(), bytes);
    assert_eq!(registry.parent_settlement(a.clone(), "device_a".into(), original.grant_id).await.unwrap().unwrap().retained_tokens, 200);
    let replay = consent(&registry, &a, &resource.resource_id, "next_request", "next_parent", 500, expires).await.approve().await.unwrap();
    assert_eq!(serde_json::to_value(replay).unwrap(), serde_json::to_value(&next).unwrap());
    assert!(consent(&registry, &a, &resource.resource_id, "different_request", "next_parent", 500, expires).await.approve().await.is_err());
    registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn next_admission_requires_committed_fence_and_writer_rechecks_late_old_grant() {
    let temp = tempfile::tempdir().unwrap();
    let registry = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let a = authority('a');
    let resource = enroll(&registry, &a, "enroll").await;
    let expires = graphs::now_ms().unwrap() + 60_000;
    let pending = consent(&registry, &a, &resource.resource_id, "new_request", "new_parent", 100, expires).await;
    let original = grant(&registry, &a, &resource.resource_id, "late_old_parent", 100).await.unwrap();
    assert!(pending.approve().await.is_err());
    registry.0.store.call(1024, |db| {
        db.db.execute("UPDATE owner_capacity_grants SET active=0", [])?;
        Ok(())
    }).await.unwrap();
    assert!(consent(&registry, &a, &resource.resource_id, "new_request", "new_parent", 100, expires).await.approve().await.is_err());
    close_original(&registry, &a, &original).await;
    let next = consent(&registry, &a, &resource.resource_id, "new_request", "new_parent", 100, expires).await.approve().await.unwrap();
    close_original(&registry, &a, &next).await;
    assert!(consent(&registry, &a, &resource.resource_id, "new_request", "same_round_again", 100, expires).await.approve().await.is_err());
    assert!(consent(&registry, &a, &resource.resource_id, "third_request", "third_parent", 100, expires).await.approve().await.is_ok());
    registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn individual_closure_observation_preserves_retained_history_beside_active_new_grant() {
    let temp = tempfile::tempdir().unwrap();
    let registry = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let a = authority('a');
    let resource = enroll(&registry, &a, "enroll").await;
    let original = grant(&registry, &a, &resource.resource_id, "old_parent", 700).await.unwrap();
    assert!(registry.capacity_grant_closure(a.clone(), "device_a".into(), original.clone()).await.unwrap().is_none());
    apply(&registry, &a, command(&original, "original_child", 200)).await;
    let settled = close_original(&registry, &a, &original).await;
    let next = consent(&registry, &a, &resource.resource_id, "new_request", "new_parent", 500,
        graphs::now_ms().unwrap() + 60_000).await.approve().await.unwrap();
    let observed = registry.capacity_grant_closure(a.clone(), "device_a".into(), original.clone()).await.unwrap().unwrap();
    assert_eq!(serde_json::to_value(&observed).unwrap(), serde_json::to_value(&settled).unwrap());
    assert_eq!((observed.retained_tokens, observed.known_consumed_tokens, observed.unresolved_tokens), (200, 0, 200));
    assert!(registry.capacity_grant_closure(a.clone(), "device_a".into(), next).await.unwrap().is_none());
    assert_eq!(registry.resource(&a, resource.resource_id).await.unwrap().budget.pool.remaining.map(u64::from), Some(300));
    assert!(registry.capacity_grant_closure(authority('b'), "device_a".into(), original.clone()).await.is_err());
    assert!(registry.capacity_grant_closure(a.clone(), "another_device".into(), original.clone()).await.is_err());
    let mut changed = original.clone();changed.allocated_tokens -= 1;
    assert!(registry.capacity_grant_closure(a.clone(), "device_a".into(), changed).await.is_err());
    let mut missing = original;missing.grant_id = "missing_original".into();
    assert!(registry.capacity_grant_closure(a, "device_a".into(), missing).await.is_err());
    registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn individual_closure_observation_never_treats_an_inactive_bit_as_settlement() {
    let temp = tempfile::tempdir().unwrap();
    let registry = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let a = authority('a');
    let resource = enroll(&registry, &a, "enroll").await;
    let original = grant(&registry, &a, &resource.resource_id, "old_parent", 700).await.unwrap();
    registry.0.store.call(1024, |db| {db.db.execute("UPDATE owner_capacity_grants SET active=0", [])?;Ok(())}).await.unwrap();
    assert!(registry.capacity_grant_closure(a.clone(), "device_a".into(), original.clone()).await.is_err());
    close_original(&registry, &a, &original).await;
    assert_eq!(registry.capacity_grant_closure(a.clone(), "device_a".into(), original.clone()).await.unwrap().unwrap().retained_tokens, 0);
    registry.0.store.call(1024, |db| {db.db.execute("UPDATE owner_capacity_grants SET active=1", [])?;Ok(())}).await.unwrap();
    // An inconsistent legacy flag cannot hide a committed original settlement.
    assert_eq!(registry.capacity_grant_closure(a.clone(), "device_a".into(), original.clone()).await.unwrap().unwrap().retained_tokens, 0);
    // Descriptive recovery does not relax the separate new-admission gate.
    assert!(registry.closed_contribution_grants(a, "device_a".into(), original.contribution_id).await.is_err());
    registry.shutdown().await.unwrap();
}
