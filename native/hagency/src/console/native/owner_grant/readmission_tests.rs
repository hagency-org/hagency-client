use super::*;
use hagency_store::owner_resources::{GrantClosureCommand, VerifiedParentDelivery};

fn next_admission(f: &Fixture) {
    let mut reply = f.admission.lock().unwrap();
    reply["admission"]["contribution"]["requestId"] = json!("second_admission_request");
    reply["admission"]["contribution"]["revision"] = json!(5);
    reply["admission"]["request"]["id"] = json!("second_admission_request");
}

#[tokio::test]
async fn admission_v2_retains_original_and_requires_pending_intent_cancellation() {
    let mut f = Fixture::new().await;
    f.enable_factory().await;
    f.publications.lock().unwrap().admission_receipts = true;
    let first = f.service.prepare(&f.registry, f.factory().await.unwrap(), Fixture::choices()).await.unwrap();
    let id = first.summary().intent_id;
    assert_eq!(first.original.receipt_version, 2);
    drop(first);
    let path = f.enrollment_path().await;
    let first_enrollment = std::fs::read(&path).unwrap();
    let original = f.history().await.data.records[0].original.clone();
    next_admission(&f);
    assert_eq!(f.factory().await.err().expect("pending intent must be resolved").code,
        "owner_grant_prior_intent_required");
    assert_eq!(std::fs::read(&path).unwrap(), first_enrollment);
    let review = match f.service.review_cancellation(&f.registry, id.clone()).await.unwrap() {
        OwnerGrantCancellationResult::Review(review) => *review,
        _ => panic!("expected cancellation review"),
    };
    f.service.confirm_cancellation(&f.registry, review).await.unwrap();
    let proof = f.factory().await.unwrap();
    let second = f.service.prepare(&f.registry, proof, Fixture::choices()).await.unwrap();
    assert_ne!(second.original.binding.enrollment_id, original.binding.enrollment_id);
    assert_eq!(second.original.admission.as_ref().unwrap().request_id, "second_admission_request");
    let second_id = second.summary().intent_id;
    f.service.confirm(&f.registry, second).await.unwrap();
    let history = f.history().await;
    assert_eq!(history.data.records.len(), 2);
    assert_eq!(history.data.records[0].original, original);
    assert_eq!(history.data.records[0].state, State::Declined);
    let receipt: NativeGrantReceipt = serde_json::from_str(history.data.records[1].receipt.as_deref().unwrap()).unwrap();
    assert_eq!(receipt.version, 2);
    assert_eq!(receipt.admission_request_id.as_deref(), Some("second_admission_request"));
    let mut f = f.restart().await;
    f.enable_factory().await;
    assert!(matches!(f.service.recover(&f.registry, second_id).await.unwrap().state, OwnerGrantState::Recorded));
    assert_eq!(f.count("owner_capacity_grants"), 1);
    f.stop().await;
}

#[tokio::test]
async fn admission_v2_closed_original_keeps_receipt_and_accepts_distinct_round() {
    let mut f = Fixture::new().await;
    f.enable_factory().await;
    f.publications.lock().unwrap().admission_receipts = true;
    let first = f.service.prepare(&f.registry, f.factory().await.unwrap(), Fixture::choices()).await.unwrap();
    let first_id = first.summary().intent_id;
    f.service.confirm(&f.registry, first).await.unwrap();
    let history = f.history().await;
    let original = serde_json::to_value(&history.data.records[0]).unwrap();
    let receipt: NativeGrantReceipt = serde_json::from_str(history.data.records[0].receipt.as_deref().unwrap()).unwrap();
    let cookie = f.service.cookie().await.unwrap();
    let io = f.service.io(&cookie, Permission::ConsentGrants).await.unwrap();
    assert_eq!(reoffer::check_originals(&io, &f.registry, "fixture_contribution").await.unwrap_err().code,
        "owner_grant_prior_intent_required");
    let proof = VerifiedManagementEnrollment::new(io.profile.clone(), receipt.enrollment_id.clone(),
        receipt.contribution_id.clone(), receipt.resource_id.clone(), receipt.resource_revision.clone(),
        receipt.coordinator_mxid.clone(), receipt.executor_device_id.clone(), receipt.executor_generation,
        io.deadline).unwrap().with_admission_request(receipt.admission_request_id.clone().unwrap()).unwrap();
    let closure = GrantClosureCommand {
        version: 1, command_id: "close_original_admission".into(), operation: "grant.close".into(),
        grant_id: receipt.grant_id.clone(), grant_receipt_digest: digest(&receipt).unwrap(),
        previous_settlement_digest: None, reason: "owner_withdrawal".into(), requested_at_ms: now_ms().unwrap(),
    };
    let closure_digest = digest(&closure).unwrap();
    f.registry.apply_parent_settlement(VerifiedParentDelivery::new(proof, closure, closure_digest, true, io.deadline).unwrap()).await.unwrap();
    reoffer::check_originals(&io, &f.registry, "fixture_contribution").await.unwrap();
    drop(io);
    let closed = f.service.recover(&f.registry, first_id.clone()).await.unwrap();
    assert!(matches!(closed.state, OwnerGrantState::Recorded));
    let closure = closed.closure.unwrap();
    assert_eq!((closure.retained_tokens, closure.known_consumed_tokens, closure.unresolved_tokens), (0, 0, 0));
    assert!(closure.observed_at_ms > 0);
    assert_eq!(f.service.check_new_admission(&f.registry, "fixture_contribution".into()).await.unwrap_err().code,
        "owner_grant_new_admission_required");
    next_admission(&f);
    let enrollment_path = f.enrollment_path().await;
    let enrollment_before = std::fs::read(&enrollment_path).unwrap();
    f.service.check_new_admission(&f.registry, "fixture_contribution".into()).await.unwrap();
    assert_eq!(std::fs::read(enrollment_path).unwrap(), enrollment_before);
    assert_eq!(f.count("owner_capacity_grants"), 1);

    let second = f.service.prepare(&f.registry, f.factory().await.unwrap(), Fixture::choices()).await.unwrap();
    let second_id = second.summary().intent_id;
    assert_ne!(second_id, first_id);
    f.service.confirm(&f.registry, second).await.unwrap();
    assert_eq!(serde_json::to_value(&f.history().await.data.records[0]).unwrap(), original);
    assert_eq!(f.count("owner_capacity_grants"), 2);
    let mut f = f.restart().await;
    f.enable_factory().await;
    let again = f.binding(f.factory().await.unwrap()).await;
    assert_ne!(again.enrollment_id, receipt.enrollment_id);
    let old = f.service.recover(&f.registry, first_id).await.unwrap();
    let current = f.service.recover(&f.registry, second_id).await.unwrap();
    assert!(matches!(old.state, OwnerGrantState::Recorded) && old.closure.is_some());
    assert!(matches!(current.state, OwnerGrantState::Recorded) && current.closure.is_none());

    assert_eq!(serde_json::to_value(&f.history().await.data.records[0]).unwrap(), original);
    f.stop().await;
}

#[tokio::test]
async fn admission_v2_unknown_attempt_never_mints_replacement_enrollment() {
    let mut f = Fixture::new().await;
    f.enable_factory().await;
    f.publications.lock().unwrap().admission_receipts = true;
    let review = f.service.prepare(&f.registry, f.factory().await.unwrap(), Fixture::choices()).await.unwrap();
    drop(review);
    let mut history = f.history().await;
    history.data.records[0].state = State::Applying;
    history.save().unwrap();
    let before = std::fs::read(&history.path).unwrap();
    let path = f.enrollment_path().await;
    let enrollment = std::fs::read(&path).unwrap();
    next_admission(&f);
    assert_eq!(f.factory().await.err().expect("unknown is not zero effect").code,
        "owner_grant_prior_intent_required");
    assert_eq!(std::fs::read(path).unwrap(), enrollment);
    assert_eq!(std::fs::read(history.path).unwrap(), before);
    assert_eq!(f.count("owner_capacity_grants"), 0);
    f.stop().await;
}

#[tokio::test]
async fn admission_v2_late_original_hold_is_a_known_refusal_and_keeps_review_cancellable() {
    let f = Fixture::new().await;
    f.publications.lock().unwrap().admission_receipts = true;
    let review = f.review().await;
    let id = review.summary().intent_id;
    f.registry.prepare_capacity_grant(f.enrollment().await, OwnerGrantSelection {
        command_id: "racing_old_consent".into(), grant_id: "racing_old_grant".into(),
        coordinator_mxid: "@owner:test".into(), delegation_expires_at_ms: now_ms().unwrap() + 60000,
        allow_self_approval: false, eligible_managers: vec!["@manager:test".into()], allocated_tokens: 100,
    }, Instant::now() + Duration::from_secs(30)).unwrap().approve().await.unwrap();
    assert_eq!(f.service.confirm(&f.registry, review).await.unwrap_err().code, "owner_grant_review_changed");
    assert_eq!(f.history().await.data.records[0].state, State::Review);
    assert_eq!(f.count("owner_capacity_grants"), 1);
    let review = match f.service.review_cancellation(&f.registry, id).await.unwrap() {
        OwnerGrantCancellationResult::Review(review) => *review,
        _ => panic!("known no-effect refusal must remain cancellable"),
    };
    f.service.confirm_cancellation(&f.registry, review).await.unwrap();
    assert_eq!(f.history().await.data.records[0].state, State::Declined);
    assert_eq!(f.count("owner_capacity_grants"), 1);
    f.stop().await;
}

#[tokio::test]
async fn admission_policy_disabled_never_falls_back_to_legacy_capacity_grant() {
    for supports_v2 in [false, true] {
    let f = Fixture::new().await;
    f.publications.lock().unwrap().allocations_disabled = true;
    let error = f.service.prepare(&f.registry, f.enrollment().await, Fixture::choices())
        .await.err().expect("disabled policy is not a legacy protocol");
    assert_eq!(error.code, "management_admission_receipts_required");
    assert!(f.history().await.data.records.is_empty());
    assert_eq!(f.count("owner_capacity_grants"), 0);
    f.publications.lock().unwrap().allocations_disabled = false;
    f.publications.lock().unwrap().admission_receipts = supports_v2;
    let review = f.review().await;
    let id = review.summary().intent_id;
    f.publications.lock().unwrap().allocations_disabled = true;
    assert_eq!(f.service.confirm(&f.registry, review).await.unwrap_err().code,
        "management_admission_receipts_required");
    assert_eq!(f.history().await.data.records[0].state, State::Review);
    assert_eq!(f.count("owner_capacity_grants"), 0);
    let review = match f.service.review_cancellation(&f.registry, id).await.unwrap() {
        OwnerGrantCancellationResult::Review(review) => *review,
        _ => panic!("disabling issuance must not block cancellation"),
    };
    f.service.confirm_cancellation(&f.registry, review).await.unwrap();
    assert_eq!(f.history().await.data.records[0].state, State::Declined);
    f.stop().await;
    }
}

#[tokio::test]
async fn reoffer_preflight_requires_original_intent_resolution_without_changing_capacity() {
    let f = Fixture::new().await;
    let review = f.review().await;
    let id = review.summary().intent_id;
    drop(review);
    let cookie = f.service.cookie().await.unwrap();
    let io = f.service.io(&cookie, Permission::ConsentGrants).await.unwrap();
    assert_eq!(reoffer::check_originals(&io, &f.registry, "fixture_contribution").await.unwrap_err().code,
        "owner_grant_prior_intent_required");
    let mut history = f.history().await;
    assert_eq!(history.data.records[0].state, State::Review);
    history.data.records[0].state = State::Applying;
    history.save().unwrap();
    let unknown = std::fs::read(&history.path).unwrap();
    assert_eq!(reoffer::check_originals(&io, &f.registry, "fixture_contribution").await.unwrap_err().code,
        "owner_grant_prior_intent_required");
    assert_eq!(std::fs::read(&history.path).unwrap(), unknown);
    // Restore the synthetic pending state to exercise explicit human cancellation.
    history.data.records[0].state = State::Review;
    history.save().unwrap();
    let review = match f.service.review_cancellation(&f.registry, id).await.unwrap() {
        OwnerGrantCancellationResult::Review(review) => *review,
        _ => panic!("pending intent must be cancellable"),
    };
    f.service.confirm_cancellation(&f.registry, review).await.unwrap();
    let cancelled = std::fs::read(&history.path).unwrap();
    reoffer::check_originals(&io, &f.registry, "fixture_contribution").await.unwrap();
    assert_eq!(std::fs::read(&history.path).unwrap(), cancelled);
    assert_eq!(f.count("owner_capacity_grants"), 0);
    drop(io);
    f.stop().await;
}

#[tokio::test]
async fn admission_v2_first_grant_requires_no_preexisting_allocation() {
    let mut f = Fixture::new().await;
    f.enable_factory().await;
    f.publications.lock().unwrap().admission_receipts = true;
    assert_eq!(f.count("owner_capacity_grants"), 0);
    let review = f.service.prepare(&f.registry, f.factory().await.unwrap(), Fixture::choices()).await.unwrap();
    assert_eq!(review.original.receipt_version, 2);
    assert_eq!(f.count("owner_capacity_grants"), 0);
    f.service.confirm(&f.registry, review).await.unwrap();
    assert_eq!(f.count("owner_capacity_grants"), 1);
    assert_eq!(f.publications.lock().unwrap().posts, 0);
    f.stop().await;
}
