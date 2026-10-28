async fn publication_observation_snapshot(f: &Fixture) -> (Vec<u8>, serde_json::Value) {
    let history = f.history().await;
    let cookie = f.service.cookie().await.unwrap();
    let io = f
        .service
        .io(&cookie, Permission::ReadIntents)
        .await
        .unwrap();
    (
        std::fs::read(history.path).unwrap(),
        serde_json::to_value(io.session.pending_intents(&io.context).await.unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn publication_observation_never_prepares_an_unconfirmed_command() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    for prepared in [false, true] {
        if prepared {
            drop(f.publication_review(&id).await);
        }
        let before = publication_observation_snapshot(&f).await;
        assert!(
            f.service
                .publication_observe(&f.registry, id.clone())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(publication_observation_snapshot(&f).await, before);
        assert_eq!(f.publications.lock().unwrap().recover_reads, 0);
        assert_eq!(f.publications.lock().unwrap().posts, 0);
    }
    assert_eq!(f.count("owner_capacity_grants"), 1);
    f.stop().await;
}

#[tokio::test]
async fn publication_observation_confirmation_before_transport_stays_unknown() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    drop(f.publication_review(&id).await);
    let mut history = f.history().await;
    history.data.records[0]
        .publication
        .as_mut()
        .unwrap()
        .confirmed = true;
    history.save().unwrap();
    let before = publication_observation_snapshot(&f).await;
    let progress = f
        .service
        .publication_observe(&f.registry, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progress.state, OwnerGrantPublicationState::OutcomeUnknown);
    assert_eq!(publication_observation_snapshot(&f).await, before);
    assert_eq!(f.publications.lock().unwrap().posts, 0);
    assert_eq!(f.publications.lock().unwrap().recover_reads, 0);
    f.stop().await;
}

#[tokio::test]
async fn publication_observation_lost_reply_never_retries_or_records_a_reply() {
    for server_recorded in [false, true] {
        let mut f = Fixture::new().await;
        let id = f.recorded().await;
        let review = f.publication_review(&id).await;
        f.publications.lock().unwrap().lose_reply = true;
        assert_eq!(
            f.service
                .publication_apply(&f.registry, id.clone(), Some(review))
                .await
                .unwrap()
                .state,
            OwnerGrantPublicationState::OutcomeUnknown
        );
        if !server_recorded {
            f.publications.lock().unwrap().commands.clear();
        }
        let before = publication_observation_snapshot(&f).await;
        let progress = f
            .service
            .publication_observe(&f.registry, id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(progress.summary.intent_id, id);
        assert_eq!(
            progress.state,
            if server_recorded {
                OwnerGrantPublicationState::ServerObserved
            } else {
                OwnerGrantPublicationState::OutcomeUnknown
            }
        );
        assert_eq!(publication_observation_snapshot(&f).await, before);
        assert_eq!(f.publications.lock().unwrap().posts, 1);
        assert_eq!(f.publications.lock().unwrap().recover_reads, 1);
        assert_eq!(f.count("owner_capacity_grants"), 1);
        f.stop().await;
    }
}

#[tokio::test]
async fn publication_observation_restart_reads_current_state_without_republishing() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let review = f.publication_review(&id).await;
    f.service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    let mut f = f.restart().await;
    f.service.manager.retire();
    let mut p = package(9, false);
    p.permissions
        .extend([Permission::ReadIntents, Permission::PublishGrants]);
    f.service = OwnerGrantIntents::new(f.owner.management(p).await.unwrap());
    let before = publication_observation_snapshot(&f).await;
    for revoked in [false, true] {
        f.publications.lock().unwrap().revoked = revoked;
        let progress = f
            .service
            .publication_observe(&f.registry, id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(progress.state, OwnerGrantPublicationState::ServerObserved);
        assert_eq!(
            progress.server_state.as_deref(),
            Some(if revoked { "revoked" } else { "active" })
        );
        assert_eq!(publication_observation_snapshot(&f).await, before);
        assert_eq!(f.publications.lock().unwrap().posts, 1);
    }
    f.stop().await;
}

#[tokio::test]
async fn publication_observation_permission_filter_cannot_hide_an_original_as_unknown() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let review = f.publication_review(&id).await;
    f.service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    let before = publication_observation_snapshot(&f).await;
    f.service.manager.retire();
    let mut p = package(9, false);
    p.permissions.push(Permission::ReadIntents);
    f.service = OwnerGrantIntents::new(f.owner.management(p).await.unwrap());
    assert!(
        f.service
            .publication_observe(&f.registry, id)
            .await
            .is_err()
    );
    f.enable_publication().await;
    assert_eq!(publication_observation_snapshot(&f).await, before);
    assert_eq!(f.publications.lock().unwrap().posts, 1);
    assert_eq!(f.publications.lock().unwrap().recover_reads, 0);
    f.stop().await;
}

#[tokio::test]
async fn publication_observation_rejected_command_never_retries() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let review = f.publication_review(&id).await;
    f.publications.lock().unwrap().reject = true;
    f.service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    let before = publication_observation_snapshot(&f).await;
    let progress = f
        .service
        .publication_observe(&f.registry, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progress.state, OwnerGrantPublicationState::Rejected);
    assert_eq!(publication_observation_snapshot(&f).await, before);
    assert_eq!(f.publications.lock().unwrap().posts, 1);
    assert_eq!(f.publications.lock().unwrap().recover_reads, 0);
    f.stop().await;
}

#[tokio::test]
async fn publication_observation_refuses_missing_or_mismatched_original_receipt() {
    for missing in [false, true] {
        let mut f = Fixture::new().await;
        let id = f.recorded().await;
        let review = f.publication_review(&id).await;
        f.service
            .publication_apply(&f.registry, id.clone(), Some(review))
            .await
            .unwrap();
        {
            let mut server = f.publications.lock().unwrap();
            if missing {
                server.commands.clear();
            } else {
                server.commands.values_mut().next().unwrap()["receipt"]["grantId"] = json!("other");
            }
        }
        let before = publication_observation_snapshot(&f).await;
        let error = f
            .service
            .publication_observe(&f.registry, id)
            .await
            .err()
            .unwrap();
        assert_eq!(
            error.code,
            if missing {
                "grant_publication_receipt_missing"
            } else {
                "invalid_allocation_response"
            }
        );
        assert_eq!(publication_observation_snapshot(&f).await, before);
        assert_eq!(f.publications.lock().unwrap().posts, 1);
        f.stop().await;
    }
}

#[tokio::test]
async fn publication_observation_retired_host_cannot_return_or_retry_server_result() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let review = f.publication_review(&id).await;
    f.service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    let before = publication_observation_snapshot(&f).await;
    let gate = Arc::new(AtomicBool::new(true));
    {
        let mut server = f.publications.lock().unwrap();
        server.gate = Some(gate.clone());
        server.invalidate_at = Some("recover");
    }
    f.service.host_gate = Some(Arc::new(move || gate.load(Ordering::Acquire)));
    assert_eq!(
        f.service
            .publication_observe(&f.registry, id)
            .await
            .err()
            .unwrap()
            .code,
        "management_scope_changed"
    );
    f.service = OwnerGrantIntents::new(f.service.manager.clone());
    assert_eq!(publication_observation_snapshot(&f).await, before);
    assert_eq!(f.publications.lock().unwrap().posts, 1);
    f.stop().await;
}
