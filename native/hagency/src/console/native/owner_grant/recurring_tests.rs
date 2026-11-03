#[tokio::test]
async fn calendar_owner_abandoned_manual_amendment_expires_without_authorizing_changed_terms() {
    use hagency_core::recurring_quota::{Cadence, Calendar};
    let mut f = Fixture::new().await;
    f.enable_publication().await;
    {
        let mut s = f.publications.lock().unwrap();
        s.admission_receipts = true;
        s.succession = true;
        s.calendar = true;
    }
    let calendar = Calendar {
        cadence: Cadence::Daily,
        time_zone: "America/Los_Angeles".into(),
    };
    let period = calendar.period_at(now_ms().unwrap()).unwrap();
    let choices = OwnerGrantChoices { renewal_mode: None, per_request_tokens: None,
        calendar: Some(calendar.clone()),
        delegation_expires_at_ms: period.end_ms,
        ..Fixture::choices()
    };
    let review = f
        .service
        .prepare(&f.registry, f.enrollment().await, choices.clone())
        .await
        .unwrap();
    let root = review.summary().intent_id;
    f.service.confirm(&f.registry, review).await.unwrap();
    let review = f.publication_review(&root).await;
    f.service
        .publication_apply(&f.registry, root, Some(review))
        .await
        .unwrap();
    let review = f
        .service
        .prepare_successor(
            &f.registry,
            f.enrollment().await,
            "fixture_contribution".into(),
            OwnerGrantSuccessionKind::Amendment,
            OwnerGrantChoices { renewal_mode: None, per_request_tokens: None,
                allocated_tokens: 600,
                ..choices
            },
        )
        .await
        .unwrap();
    let amendment = review.summary().intent_id;
    drop(review);
    let access = OwnerGrantIntents::new(f.service.manager.clone())
        .bind_registry(f.registry.clone())
        .await
        .unwrap();
    // While its period is open, a timer must leave the owner's manual review alone.
    assert_eq!(access.renew_recurring().await.unwrap(), 0);
    let mut h = f.history().await;
    let index = h.index(&amendment).unwrap();
    assert!(matches!(h.data.records[index].state, State::Review));
    // An explicit historical unattempted journal fixture exercises cleanup
    // without rewriting any native grant, receipt or production clock.
    let previous = calendar.period_at(period.start_ms - 1).unwrap();
    h.data.records[index].original.period = Period {
        key: previous.key,
        start_ms: previous.start_ms,
        end_ms: previous.end_ms,
    };
    h.data.records[index]
        .original
        .selection
        .delegation_expires_at_ms = previous.end_ms;
    h.data.records[index].original_digest = digest(&h.data.records[index].original).unwrap();
    h.save().unwrap();
    assert_eq!(access.renew_recurring().await.unwrap(), 0);
    let h = f.history().await;
    assert!(matches!(
        h.data.records[h.index(&amendment).unwrap()].state,
        State::Declined
    ));
    assert_eq!(f.count("owner_capacity_grants"), 1);
    assert_eq!(f.publications.lock().unwrap().successor_posts, 0);
    let cookie = f.service.cookie().await.unwrap();
    let io = f
        .service
        .io(&cookie, Permission::ConsentGrants)
        .await
        .unwrap();
    assert!(
        f.service
            .published_head(&io, &f.registry, "fixture_contribution")
            .await
            .is_ok()
    );
    drop(io);
    f.stop().await;
}

#[tokio::test]
async fn calendar_owner_publication_uses_selected_zone_and_does_not_refill_same_period() {
    use hagency_core::recurring_quota::{Cadence, Calendar};
    let mut f = Fixture::new().await;
    f.enable_publication().await;
    {
        let mut s = f.publications.lock().unwrap();
        s.admission_receipts = true;
        s.succession = true;
        s.calendar = true;
    }
    let calendar = Calendar {
        cadence: Cadence::Daily,
        time_zone: "America/Los_Angeles".into(),
    };
    let period = calendar.period_at(now_ms().unwrap()).unwrap();
    let review = f
        .service
        .prepare(
            &f.registry,
            f.enrollment().await,
            OwnerGrantChoices { renewal_mode: None, per_request_tokens: None,
                calendar: Some(calendar.clone()),
                delegation_expires_at_ms: period.end_ms,
                ..Fixture::choices()
            },
        )
        .await
        .unwrap();
    assert_eq!(review.summary().calendar, Some(calendar.clone()));
    assert_eq!(review.summary().period_key, period.key);
    assert_eq!(review.summary().period_end_ms, period.end_ms);
    let id = review.summary().intent_id;
    let original = review.original.clone();
    let result = f.service.confirm(&f.registry, review).await.unwrap();
    assert!(matches!(result.state, OwnerGrantState::Recorded));
    let access = OwnerGrantIntents::new(f.service.manager.clone())
        .bind_registry(f.registry.clone())
        .await
        .unwrap();
    // Confirming a local hold does not let a background timer publish it.
    assert_eq!(access.renew_recurring().await.unwrap(), 0);
    assert_eq!(f.publications.lock().unwrap().posts, 0);
    let review = f.publication_review(&id).await;
    f.service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    for _ in 0..2 {
        assert_eq!(access.renew_recurring().await.unwrap(), 0);
    }
    assert_eq!(f.count("owner_capacity_grants"), 1);
    assert_eq!(f.publications.lock().unwrap().posts, 1);
    let record = f
        .history()
        .await
        .data
        .records
        .into_iter()
        .find(|r| r.original.intent_id == id)
        .unwrap();
    let receipt: NativeGrantReceipt =
        serde_json::from_str(record.receipt.as_ref().unwrap()).unwrap();
    assert_eq!(receipt.version, 4);
    assert_eq!(receipt.calendar, Some(calendar));
    assert_eq!(receipt.period_start_ms, period.start_ms);
    assert!(original.matches_receipt(&receipt));
    f.stop().await;
}

#[tokio::test]
async fn calendar_owner_publication_requires_advertised_server_support() {
    use hagency_core::recurring_quota::{Cadence, Calendar};
    let f = Fixture::new().await;
    f.publications.lock().unwrap().admission_receipts = true;
    let calendar = Calendar {
        cadence: Cadence::Monthly,
        time_zone: "Asia/Shanghai".into(),
    };
    let end = calendar.period_at(now_ms().unwrap()).unwrap().end_ms;
    let error = f
        .service
        .prepare(
            &f.registry,
            f.enrollment().await,
            OwnerGrantChoices { renewal_mode: None, per_request_tokens: None,
                calendar: Some(calendar),
                delegation_expires_at_ms: end,
                ..Fixture::choices()
            },
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "management_calendar_grants_required");
    assert_eq!(f.count("owner_capacity_grants"), 0);
    assert!(f.history().await.data.records.is_empty());
    f.stop().await;
}

#[tokio::test]
async fn standing_calendar_consent_cannot_change_amount_members_zone_or_legacy_opt_in() {
    use hagency_core::recurring_quota::{Cadence, Calendar};
    let f = Fixture::new().await;
    {
        let mut s = f.publications.lock().unwrap();
        s.admission_receipts = true;
        s.calendar = true;
    }
    let calendar = Calendar {
        cadence: Cadence::Monthly,
        time_zone: "America/Los_Angeles".into(),
    };
    let end = calendar.period_at(now_ms().unwrap()).unwrap().end_ms;
    let review = f
        .service
        .prepare(
            &f.registry,
            f.enrollment().await,
            OwnerGrantChoices { renewal_mode: None, per_request_tokens: None,
                calendar: Some(calendar.clone()),
                delegation_expires_at_ms: end,
                ..Fixture::choices()
            },
        )
        .await
        .unwrap();
    let previous = review.original.clone();
    let mut next = previous.clone();
    let period = calendar.period_at(end).unwrap();
    next.intent_id = "renewal_intent".into();
    next.selection.grant_id = "renewal_grant".into();
    next.selection.command_id = "renewal_command".into();
    next.selection.delegation_expires_at_ms = period.end_ms;
    next.period = Period {
        key: period.key,
        start_ms: period.start_ms,
        end_ms: period.end_ms,
    };
    next.succession = Some(OriginalSuccession {
        kind: OwnerGrantSuccessionKind::Renewal,
        predecessor_intent_id: previous.intent_id.clone(),
        predecessor_grant_id: previous.selection.grant_id.clone(),
        predecessor_receipt_digest: "a".repeat(64),
        predecessor_grant_revision: 1,
        predecessor_allocated_tokens: previous.selection.allocated_tokens,
        reviewed_children: 0,
        reviewed_floor_tokens: 0,
    });
    assert!(super::recurring::inherits(&previous, &next));
    for mutation in 0..6 {
        let mut changed = next.clone();
        match mutation {
            0 => changed.selection.allocated_tokens += 1,
            1 => changed
                .selection
                .eligible_managers
                .push("@other:test".into()),
            2 => changed.selection.calendar.as_mut().unwrap().time_zone = "UTC".into(),
            3 => changed.selection.allow_self_approval = true,
            4 => changed.selection.delegation_expires_at_ms += 1,
            _ => changed.period.start_ms = previous.period.start_ms,
        }
        assert!(!super::recurring::inherits(&previous, &changed));
    }
    let mut legacy = previous.clone();
    legacy.receipt_version = 2;
    legacy.selection.calendar = None;
    assert!(!super::recurring::inherits(&legacy, &next));
    f.stop().await;
}

#[tokio::test]
async fn pool_owner_renewal_policy_is_in_receipt_and_cannot_be_changed_by_replay() {
    use hagency_core::recurring_quota::{Cadence, Calendar, RenewalMode};
    for mode in [RenewalMode::Owner, RenewalMode::Automatic] {
        let mut f = Fixture::new().await;
        f.enable_publication().await;
        { let mut s=f.publications.lock().unwrap();s.admission_receipts=true;s.calendar=true;s.pool_renewal=true;s.succession=true; }
        let calendar=Calendar {cadence:Cadence::Daily,time_zone:"America/Los_Angeles".into()};
        let end=calendar.period_at(now_ms().unwrap()).unwrap().end_ms;
        let review=f.service.prepare(&f.registry,f.enrollment().await,OwnerGrantChoices {
            renewal_mode:Some(mode), per_request_tokens:Some(100),calendar:Some(calendar),delegation_expires_at_ms:end,..Fixture::choices()
        }).await.unwrap();
        let original=review.original.clone();let id=review.summary().intent_id;
        assert_eq!(review.summary().renewal_mode,Some(mode));
        f.service.confirm(&f.registry,review).await.unwrap();
        let review=f.publication_review(&id).await;
        f.service.publication_apply(&f.registry,id.clone(),Some(review)).await.unwrap();
        let h=f.history().await;let record=&h.data.records[h.index(&id).unwrap()];
        let receipt:NativeGrantReceipt=serde_json::from_str(record.receipt.as_ref().unwrap()).unwrap();
        assert_eq!(receipt.version,6);assert_eq!(receipt.renewal_mode,Some(mode));assert!(original.matches_receipt(&receipt));
        let mut altered=receipt.clone();altered.renewal_mode=Some(if mode==RenewalMode::Owner{RenewalMode::Automatic}else{RenewalMode::Owner});
        assert!(!original.matches_receipt(&altered));
        let access=OwnerGrantIntents::new(f.service.manager.clone()).bind_registry(f.registry.clone()).await.unwrap();
        assert_eq!(access.renew_recurring().await.unwrap(),0);
        assert_eq!(f.count("owner_capacity_grants"),1);
        f.stop().await;
    }
}
