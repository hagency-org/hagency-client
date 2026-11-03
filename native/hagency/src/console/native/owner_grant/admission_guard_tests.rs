#[tokio::test]
async fn native_owner_grant_cached_confirmation_refuses_changed_admission_without_hold() {
    for change in ["request", "contribution_revision", "request_revision", "revoked"] {
        let f = Fixture::new().await;
        let review = f.review().await;
        let path = f.history().await.path;
        let before = std::fs::read(&path).unwrap();
        {
            let mut admission = f.admission.lock().unwrap();
            match change {
                "request" => {
                    admission["admission"]["contribution"]["requestId"] = json!("new_request");
                    admission["admission"]["request"]["id"] = json!("new_request");
                }
                "contribution_revision" => admission["admission"]["contribution"]["revision"] = json!(5),
                "request_revision" => admission["admission"]["request"]["revision"] = json!(5),
                "revoked" => admission["admission"]["contribution"]["state"] = json!("revoked"),
                _ => unreachable!(),
            }
        }
        assert!(f.service.confirm(&f.registry, review).await.is_err(), "{change}: an old confirmation must not reserve a new hold");
        assert_eq!(f.count("owner_capacity_grants"), 0, "{change}");
        assert_eq!(std::fs::read(&path).unwrap(), before, "{change}: retain original review and IDs");
        f.stop().await;
    }
}

#[tokio::test]
async fn native_owner_grant_cached_enrollment_cannot_prepare_for_new_round() {
    let mut f = Fixture::new().await;
    f.enable_factory().await;
    let proof = f.factory().await.unwrap();
    let path = f.enrollment_path().await;
    let before = std::fs::read(&path).unwrap();
    {
        let mut a = f.admission.lock().unwrap();
        a["admission"]["contribution"]["requestId"] = json!("new_request");
        a["admission"]["request"]["id"] = json!("new_request");
        a["admission"]["contribution"]["revision"] = json!(5);
        a["admission"]["request"]["revision"] = json!(5);
    }
    assert_eq!(f.service.prepare(&f.registry, proof, Fixture::choices()).await.err().unwrap().code, "management_enrollment_changed");
    assert!(f.history().await.data.records.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(f.count("owner_capacity_grants"), 0);
    f.stop().await;
}

impl Fixture {
    async fn cancellation(&self, id: &str) -> OwnerGrantCancellationReview {
        match self.service.review_cancellation(&self.registry, id.into()).await.unwrap() {
            OwnerGrantCancellationResult::Review(review) => *review,
            _ => panic!("expected cancellation review"),
        }
    }
}

#[tokio::test]
async fn native_owner_grant_same_admission_can_refresh_revision_without_replacing_intent() {
    let mut f = Fixture::new().await;
    f.enable_factory().await;
    let review = f.service.prepare(&f.registry, f.factory().await.unwrap(), Fixture::choices()).await.unwrap();
    let original = review.original.clone();
    let before = f.history().await.data.records[0].original_digest.clone();
    {
        let mut a = f.admission.lock().unwrap();
        // A connection verification changes revisions, not the approval's ID.
        a["admission"]["contribution"]["revision"] = json!(3);
        a["admission"]["request"]["revision"] = json!(3);
    }
    assert!(f.service.confirm(&f.registry, review).await.is_err());
    assert_eq!(f.count("owner_capacity_grants"), 0);
    let refreshed = match f.service.review_existing(&f.registry, f.factory().await.unwrap(), original.intent_id.clone()).await.unwrap() {
        OwnerGrantReviewResult::Review(review) => *review,
        _ => panic!("expected refreshed review of the same admission"),
    };
    assert_eq!(refreshed.original, original);
    assert!(matches!(f.service.confirm(&f.registry, refreshed).await.unwrap().state, OwnerGrantState::Recorded));
    assert_eq!(f.history().await.data.records[0].original_digest, before);
    assert_eq!(f.count("owner_capacity_grants"), 1);
    f.stop().await;
}

#[tokio::test]
async fn native_owner_grant_cancellation_after_withdrawal_and_restart_preserves_original() {
    let mut f = Fixture::new().await;
    f.enable_factory().await;
    let proof = f.factory().await.unwrap();
    let original = f.service.prepare(&f.registry, proof, Fixture::choices()).await.unwrap();
    let id = original.summary().intent_id;
    let saved = f.history().await.data.records[0].original.clone();
    let enrollment_path = f.enrollment_path().await;
    let enrollment_bytes = std::fs::read(&enrollment_path).unwrap();
    drop(original);
    f.admission.lock().unwrap()["admission"]["contribution"]["state"] = json!("revoked");
    let f = f.restart().await; // no provider capabilities in the new package
    let reads = f.publications.lock().unwrap().contribution_reads;
    let review = f.cancellation(&id).await;
    let before = std::fs::read(f.history().await.path).unwrap();
    drop(review);
    assert_eq!(std::fs::read(f.history().await.path).unwrap(), before, "closing a cancellation review has no effect");
    let review = f.cancellation(&id).await;
    assert_eq!(review.summary().intent_id, id);
    assert_eq!(review.server_origin(), f.origin);
    let result = f.service.confirm_cancellation(&f.registry, review).await.unwrap();
    assert!(matches!(result.state, OwnerGrantState::Declined));
    let h = f.history().await;
    assert_eq!(h.data.records[0].original, saved);
    assert_eq!(h.data.records[0].original_digest, digest(&saved).unwrap());
    assert!(h.data.records[0].receipt.is_none());
    assert_eq!(std::fs::read(enrollment_path).unwrap(), enrollment_bytes);
    assert_eq!(f.count("owner_capacity_grants"), 0);
    assert_eq!(f.publications.lock().unwrap().contribution_reads, reads, "cancellation must not depend on live resource admission");
    let f = f.restart().await;
    assert!(matches!(f.service.recover(&f.registry, id).await.unwrap().state, OwnerGrantState::Declined));
    f.stop().await;
}

#[tokio::test]
async fn native_owner_grant_cancellation_fences_cached_confirmation() {
    let f = Fixture::new().await;
    let old = f.review().await;
    let cancellation = f.cancellation(&old.summary().intent_id).await;
    f.service.confirm_cancellation(&f.registry, cancellation).await.unwrap();
    assert!(matches!(f.service.confirm(&f.registry, old).await.unwrap().state, OwnerGrantState::Declined));
    assert_eq!(f.count("owner_capacity_grants"), 0);
    f.stop().await;
}

#[tokio::test]
async fn native_owner_grant_cancellation_of_expired_period_does_not_roll_budget_forward() {
    let f = Fixture::new().await;
    let id = f.review().await.summary().intent_id;
    let mut h = f.history().await;
    h.data.records[0].original.period = Period {
        key: "2000-01".into(), start_ms: 946_684_800_000, end_ms: 949_363_200_000,
    };
    h.data.records[0].original_digest = digest(&h.data.records[0].original).unwrap();
    h.save().unwrap();
    let original = h.data.records[0].original.clone();
    let f = f.restart().await;
    assert!(matches!(f.service.recover(&f.registry, id.clone()).await.unwrap().state, OwnerGrantState::ReviewExpired));
    let review = f.cancellation(&id).await;
    let result = f.service.confirm_cancellation(&f.registry, review).await.unwrap();
    assert!(matches!(result.state, OwnerGrantState::Declined));
    assert_eq!(f.history().await.data.records[0].original, original);
    assert_eq!(f.count("owner_capacity_grants"), 0);
    f.stop().await;
}

#[tokio::test]
async fn native_owner_grant_cancellation_cannot_erase_committed_or_unknown_effect() {
    for committed in [false, true] {
        let f = Fixture::new().await;
        let original = f.review().await;
        let id = original.summary().intent_id;
        let cancellation = f.cancellation(&id).await;
        if committed {
            f.service.confirm(&f.registry, original).await.unwrap();
        } else {
            drop(original);
            let mut h = f.history().await;
            h.data.records[0].state = State::Applying;
            h.save().unwrap();
        }
        let before = f.history().await.data.records[0].clone();
        let result = f.service.confirm_cancellation(&f.registry, cancellation).await.unwrap();
        assert!(if committed { matches!(result.state, OwnerGrantState::Recorded) } else { matches!(result.state, OwnerGrantState::OutcomeUnknown) });
        let after = f.history().await.data.records[0].clone();
        assert_eq!(after.original, before.original);
        assert_eq!(after.state, before.state);
        assert_eq!(after.receipt, before.receipt);
        assert_eq!(f.count("owner_capacity_grants"), u64::from(committed));
        assert!(matches!(f.service.review_cancellation(&f.registry, id).await.unwrap(), OwnerGrantCancellationResult::Progress(_)));
        f.stop().await;
    }
}

#[tokio::test]
async fn native_owner_grant_cancellation_rejects_another_manager_and_retired_host() {
    let f = Fixture::new().await;
    let id = f.review().await.summary().intent_id;
    let review = f.cancellation(&id).await;
    let before = std::fs::read(f.history().await.path).unwrap();
    let other = OwnerGrantIntents::new(f.owner.management(package(95, true)).await.unwrap());
    assert!(other.confirm_cancellation(&f.registry, review).await.is_err());
    assert_eq!(std::fs::read(f.history().await.path).unwrap(), before);
    let review = f.cancellation(&id).await;
    f.service.manager.retire();
    assert!(f.service.confirm_cancellation(&f.registry, review).await.is_err());
    assert_eq!(f.count("owner_capacity_grants"), 0);
    f.stop().await;
}

#[tokio::test]
async fn native_owner_grant_legacy_review_keeps_digest_and_refuses_later_round() {
    for changed in [false, true] {
        let f = Fixture::new().await;
        let id = f.review().await.summary().intent_id;
        let mut h = f.history().await;
        h.data.records[0].original.admission = None;
        h.data.records[0].original_digest = digest(&h.data.records[0].original).unwrap();
        h.save().unwrap();
        let before = std::fs::read(&h.path).unwrap();
        assert!(!String::from_utf8_lossy(&before).contains("\"admission\""));
        let f = f.restart().await;
        let original = f.history().await.data.records[0].original.clone();
        assert_eq!(std::fs::read(f.history().await.path).unwrap(), before);
        let review = match f.service.review_existing(&f.registry, f.enrollment().await, id).await.unwrap() {
            OwnerGrantReviewResult::Review(r) => *r,
            _ => panic!("expected first-admission review"),
        };
        if changed {
            let mut a = f.admission.lock().unwrap();
            a["admission"]["contribution"]["revision"] = json!(5);
            a["admission"]["request"]["revision"] = json!(5);
            a["admission"]["contribution"]["requestId"] = json!("later_request");
            a["admission"]["request"]["id"] = json!("later_request");
        }
        let result = f.service.confirm(&f.registry, review).await;
        assert_eq!(result.is_ok(), !changed);
        assert_eq!(f.history().await.data.records[0].original, original);
        assert_eq!(f.count("owner_capacity_grants"), u64::from(!changed));
        if changed { assert_eq!(std::fs::read(f.history().await.path).unwrap(), before); }
        f.stop().await;
    }
}
#[tokio::test]
async fn native_owner_grant_publication_form_prepares_only_the_exact_private_draft() {
    for legitimate in [true, false] {
        let mut f = Fixture::new().await;
        {
            let mut features = f.publications.lock().unwrap();
            features.calendar = true;
            features.admission_receipts = true;
        }
        f.service.manager.retire();
        let mut package = package(8, true);
        package.permissions.extend([Permission::ReadProviders, Permission::ConnectProviders, Permission::PublishGrants, Permission::ReadIntents]);
        f.service = OwnerGrantIntents::new(f.owner.management(package).await.unwrap());
        {
            let mut a = f.admission.lock().unwrap();
            a["admission"]["contribution"]["publicationFirst"] = json!(legitimate);
            a["admission"]["contribution"]["state"] = json!("draft");
            a["admission"]["contribution"]["revision"] = json!(1);
            a["admission"]["request"]["state"] = json!("draft");
            a["admission"]["request"]["revision"] = json!(1);
        }
        let form = f.service.publication_form(&f.registry, "fixture_contribution".into()).await;
        assert_eq!(form.is_ok(), legitimate, "{:?}", form.as_ref().err().map(|e| &e.code));
        if legitimate {
            assert!(form.unwrap().awaiting_admission);
            let proof = f.factory().await.unwrap();
            let mut choices = Fixture::choices();
            choices.calendar = Some(hagency_core::recurring_quota::Calendar {
                cadence: hagency_core::recurring_quota::Cadence::Monthly,
                time_zone: "America/Los_Angeles".into(),
            });
            let review = f.service.prepare(&f.registry, proof, choices).await.unwrap();
            let id = review.summary().intent_id;
            assert!(matches!(f.service.confirm(&f.registry, review).await.unwrap().state, OwnerGrantState::Recorded));
            let transport = crate::console::native::AllocationTransport::new(f.service.manager.clone());
            let scope = transport.scope(Permission::PublishGrants).await.unwrap();
            let grant_id = f.history().await.data.records[0].original.selection.grant_id.clone();
            let grant = f.registry.recover_capacity_grant(scope.profile.clone(), scope.device.device_id().into(), grant_id).await.unwrap().unwrap();
            assert_eq!(transport.enrollment(&scope, &grant, true).await.err().unwrap().code,
                "allocation_enrollment_changed", "publication must not authorize execution");
            let publication = f.publication_review(&id).await;
            f.service.publication_apply(&f.registry, id.clone(), Some(publication)).await.unwrap();
            // Admission changes only availability; the same owner receipt
            // remains the sole capacity authorization, including after restart.
            {
                let mut a = f.admission.lock().unwrap();
                a["admission"]["contribution"]["state"] = json!("admitted");
                a["admission"]["contribution"]["revision"] = json!(3);
                a["admission"]["request"]["state"] = json!("approved");
                a["admission"]["request"]["revision"] = json!(2);
            }
            drop(scope);
            drop(transport);
            let mut f = f.restart().await;
            f.enable_publication().await;
            assert!(f.service.publication_observe(&f.registry,id).await.unwrap().is_some());
            assert_eq!(f.count("owner_capacity_grants"),1);
            f.stop().await;
        } else { f.stop().await; }
    }
}
