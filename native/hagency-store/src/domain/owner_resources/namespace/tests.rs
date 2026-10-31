use super::*;
fn authority(which: char) -> ProfileAuthority {
    ProfileAuthority::new(
        which.to_string().repeat(64),
        format!("@owner:{which}.test"),
        Instant::now() + Duration::from_secs(120),
    )
    .unwrap()
}
fn proof(subject: &str) -> VerifiedProviderAccount {
    VerifiedProviderAccount::new(
        "codex",
        "provider.test",
        subject,
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap()
}
async fn account(r: &OwnerResourceRegistry, p: &ProfileAuthority) -> AccountModelQuery {
    let profile = p.digest.clone();
    r.0.store
        .call(1024, move |db| {
            let choice = db.reserve_account(crate::ACCOUNT_PROFILE)?;
            db.db.execute(
                "INSERT INTO management_account_profiles VALUES(?1,?2)",
                params![choice.id, profile],
            )?;
            let choice = db.materialize_account(&choice.id)?;
            Ok(AccountModelQuery {
                account_id: choice.id,
                expected_revision: choice.revision,
            })
        })
        .await
        .unwrap()
}
fn input(account: AccountModelQuery, command: &str) -> NamespaceEnrollment {
    NamespaceEnrollment {
        command_id: command.into(),
        account,
        model: "gpt-5.6-sol".into(),
        reasoning: Some("medium".into()),
        monthly_tokens: 1000,
    }
}
async fn consent(
    r: &OwnerResourceRegistry,
    p: &ProfileAuthority,
    i: NamespaceEnrollment,
    subject: &str,
) -> OwnerResourceConsent {
    let selected = r
        .select_resource_account(p.clone(), i.account.clone())
        .await
        .unwrap();
    r.prepare_namespace_enrollment(p.clone(), i, selected, proof(subject))
        .unwrap()
}
async fn counts(r: &OwnerResourceRegistry) -> Vec<u64> {
    r.0.store
        .call(1024, |db| {
            [
                "resources",
                "owner_resource_accounts",
                "owner_resource_namespaces",
                "owner_resource_grants",
                "owner_resource_receipts",
                "owner_namespace_enrollments",
            ]
            .iter()
            .map(|t| {
                Ok(db
                    .db
                    .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))?)
            })
            .collect()
        })
        .await
        .unwrap()
}
async fn grant(
    r: &OwnerResourceRegistry,
    p: &ProfileAuthority,
    resource: &str,
    id: &str,
    tokens: u64,
) -> Result<NativeGrantReceipt, Error> {
    let revision = r.resource(p, resource.into()).await?.revision;
    let enrollment = VerifiedManagementEnrollment::new(
        p.clone(),
        format!("enrollment_{id}"),
        format!("contribution_{id}"),
        resource.into(),
        revision,
        "@coordinator:a.test".into(),
        "device".into(),
        1,
        Instant::now() + Duration::from_secs(30),
    )?;
    r.prepare_capacity_grant(
        enrollment,
        OwnerGrantSelection {
            command_id: format!("grant_{id}"),
            grant_id: id.into(),
            coordinator_mxid: "@coordinator:a.test".into(),
            delegation_expires_at_ms: graphs::now_ms()? + 60_000,
            allow_self_approval: false,
            eligible_managers: vec!["@manager:a.test".into()],
            allocated_tokens: tokens,
        },
        Instant::now() + Duration::from_secs(30),
    )?
    .approve()
    .await
}
#[tokio::test]
async fn current_codex_models_enroll_and_recover_without_rewriting_old_resources_or_quota() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("owner");
    let r = OwnerResourceRegistry::open_root(&root).unwrap();
    let p = authority('a');
    let a = account(&r, &p).await;
    let old = consent(&r, &p, input(a.clone(), "old-model"), "one-quota")
        .await.approve().await.unwrap();
    let old_resource = serde_json::to_value(r.resource(&p, old.resource_id.clone()).await.unwrap()).unwrap();
    let mut originals = Vec::new();
    for (index, model) in ["gpt-6.1-sol", "gpt-6-astra"].into_iter().enumerate() {
        let mut request = input(a.clone(), &format!("current-{index}"));
        request.model = model.into();
        request.reasoning = Some("high".into());
        let receipt = consent(&r, &p, request.clone(), "one-quota").await.approve().await.unwrap();
        let row = r.resource(&p, receipt.resource_id.clone()).await.unwrap();
        assert!(!row.published);
        let resource_id = receipt.resource_id.clone();
        let stored = r.0.store.call(1024, move |db| read_resource(&db.db, &resource_id)).await.unwrap();
        assert_eq!(stored.model, model);
        assert_eq!(stored.reasoning.as_deref(), Some("high"));
        assert_eq!(stored.ceiling.unwrap().tokens.map(u64::from), Some(1000));
        let selected = r.resource_namespace(p.clone(), receipt.resource_id.clone(), row.revision).await.unwrap().unwrap();
        let mut env = std::collections::BTreeMap::new();
        selected.launch().apply_codex_environment(&mut env).unwrap();
        assert!(Path::new(env.get(std::ffi::OsStr::new("CODEX_HOME")).unwrap()).ends_with(&a.account_id));
        originals.push((request, serde_json::to_value(receipt).unwrap()));
    }
    assert_eq!(serde_json::to_value(r.resource(&p, old.resource_id.clone()).await.unwrap()).unwrap(), old_resource);
    assert_eq!(counts(&r).await, vec![3, 1, 3, 3, 3, 3]);
    r.shutdown().await.unwrap();
    drop(r);
    let r = OwnerResourceRegistry::open_root(&root).unwrap();
    for (request, original) in originals {
        let recovered = r.recover_namespace_enrollment(p.clone(), request.clone()).await.unwrap().unwrap();
        assert_eq!(serde_json::to_value(recovered).unwrap(), original);
        let replayed = consent(&r, &p, request, "one-quota").await.approve().await.unwrap();
        assert_eq!(serde_json::to_value(replayed).unwrap(), original);
    }
    assert_eq!(counts(&r).await, vec![3, 1, 3, 3, 3, 3]);
    assert_eq!(serde_json::to_value(r.resource(&p, old.resource_id).await.unwrap()).unwrap(), old_resource);
    r.shutdown().await.unwrap();
}

#[tokio::test]
async fn two_namespaces_share_login_but_use_owner_chosen_pool_limits() {
    let temp = tempfile::tempdir().unwrap();
    let r = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let p = authority('a');
    let a = account(&r, &p).await;
    let b = account(&r, &p).await;
    let first = consent(&r, &p, input(a.clone(), "first"), "same-quota")
        .await
        .approve()
        .await
        .unwrap();
    let second = consent(&r, &p, input(b.clone(), "second"), "same-quota")
        .await
        .approve()
        .await
        .unwrap();
    assert_ne!(first.resource_id, second.resource_id);
    assert_eq!(counts(&r).await, vec![2, 1, 2, 2, 2, 2]);
    let profile = p.digest.clone();
    let selected = a.account_id.clone();
    let expected = first.resource_id.clone();
    r.0.store
        .call(1024, move |db| {
            let page = db.management_account_inventory(
                Some(&profile),
                crate::management::AccountInventoryQuery {
                    id: Some(selected),
                    ..Default::default()
                },
            )?;
            assert_eq!(page.account.unwrap().resource_count, 1);
            assert_eq!(page.resources.len(), 1);
            assert_eq!(page.resources[0].id, expected);
            Ok(())
        })
        .await
        .unwrap();
    for (result, query) in [(&first, &a), (&second, &b)] {
        let resource = r.resource(&p, result.resource_id.clone()).await.unwrap();
        assert!(!resource.published);
        let selected = r
            .resource_namespace(p.clone(), result.resource_id.clone(), resource.revision)
            .await
            .unwrap()
            .unwrap();
        let mut env = std::collections::BTreeMap::new();
        selected.launch().apply_codex_environment(&mut env).unwrap();
        assert!(
            Path::new(env.get(std::ffi::OsStr::new("CODEX_HOME")).unwrap())
                .ends_with(&query.account_id)
        );
        selected.check().await.unwrap();
    }
    grant(&r, &p, &first.resource_id, "first_capacity", 600)
        .await
        .unwrap();
    grant(&r, &p, &second.resource_id, "second_capacity", 500).await.unwrap();
    assert!(grant(&r, &p, &second.resource_id, "over_pool", 501).await.is_err());
    let first_budget = r.resource(&p, first.resource_id.clone()).await.unwrap().budget;
    assert_eq!(first_budget.seat.quota, None);
    assert_eq!(first_budget.pool.remaining.map(u64::from), Some(400));
    let mut altered = input(b, "different_ceiling");
    altered.monthly_tokens = 2000;
    altered.model = "gpt-6.1-terra".into();
    let third = consent(&r, &p, altered, "same-quota").await.approve().await.unwrap();
    assert_eq!(r.resource(&p, third.resource_id).await.unwrap().budget.remaining_tokens.map(u64::from), Some(2000));
    r.shutdown().await.unwrap();
}
#[tokio::test]
async fn different_provider_quotas_remain_independent_and_recovery_is_original() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("owner");
    let r = OwnerResourceRegistry::open_root(&root).unwrap();
    let p = authority('a');
    let first = input(account(&r, &p).await, "first");
    let second = input(account(&r, &p).await, "second");
    let receipt = consent(&r, &p, first.clone(), "quota_a")
        .await
        .approve()
        .await
        .unwrap();
    consent(&r, &p, second, "quota_b")
        .await
        .approve()
        .await
        .unwrap();
    assert_eq!(counts(&r).await, vec![2, 2, 2, 2, 2, 2]);
    r.shutdown().await.unwrap();
    drop(r);
    let r = OwnerResourceRegistry::open_root(&root).unwrap();
    let original = r
        .recover_namespace_enrollment(p.clone(), first.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(original).unwrap(),
        serde_json::to_value(&receipt).unwrap()
    );
    assert!(
        r.recover_namespace_enrollment(authority('b'), first.clone())
            .await
            .is_err()
    );
    let mut changed = first;
    changed.monthly_tokens = 2000;
    assert!(r.recover_namespace_enrollment(p, changed).await.is_err());
    assert_eq!(counts(&r).await, vec![2, 2, 2, 2, 2, 2]);
    r.shutdown().await.unwrap();
}
#[tokio::test]
async fn namespace_enrollment_rolls_back_quota_resource_binding_grant_and_receipt_together() {
    let temp = tempfile::tempdir().unwrap();
    let r = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let p = authority('a');
    let input = input(account(&r, &p).await, "rollback");
    r.0.store.call(128,|db|{db.db.execute_batch("CREATE TEMP TRIGGER reject_namespace_receipt BEFORE INSERT ON owner_namespace_enrollments BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;Ok(())}).await.unwrap();
    assert!(
        consent(&r, &p, input.clone(), "quota_a")
            .await
            .approve()
            .await
            .is_err()
    );
    assert_eq!(counts(&r).await, vec![0; 6]);
    assert!(
        r.recover_namespace_enrollment(p.clone(), input.clone())
            .await
            .unwrap()
            .is_none()
    );
    r.0.store
        .call(128, |db| {
            db.db
                .execute_batch("DROP TRIGGER reject_namespace_receipt;")?;
            Ok(())
        })
        .await
        .unwrap();
    consent(&r, &p, input, "quota_a")
        .await
        .approve()
        .await
        .unwrap();
    assert_eq!(counts(&r).await, vec![1; 6]);
    r.shutdown().await.unwrap();
}
#[tokio::test]
async fn selected_namespace_rechecks_profile_revision_pending_login_and_retirement() {
    let temp = tempfile::tempdir().unwrap();
    let r = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let p = authority('a');
    let a = account(&r, &p).await;
    assert!(
        r.select_resource_account(authority('b'), a.clone())
            .await
            .is_err()
    );
    assert!(
        r.select_resource_account(
            p.clone(),
            AccountModelQuery {
                expected_revision: "f".repeat(64),
                ..a.clone()
            }
        )
        .await
        .is_err()
    );
    let pending = consent(&r, &p, input(a.clone(), "pending"), "quota").await;
    let id = a.account_id.clone();
    r.0.store
        .call(1024, move |db| {
            db.begin_account_login(&id, graphs::now_ms()?)
        })
        .await
        .unwrap();
    assert!(pending.approve().await.is_err());
    assert_eq!(counts(&r).await, vec![0; 6]);
    let b = account(&r, &p).await;
    let pending = consent(&r, &p, input(b.clone(), "retired"), "quota").await;
    r.0.store
        .call(1024, move |db| {
            db.retire_account(&b.account_id, crate::LogoutObservation::unobserved())
        })
        .await
        .unwrap();
    assert!(pending.approve().await.is_err());
    assert_eq!(counts(&r).await, vec![0; 6]);
    r.shutdown().await.unwrap();
}
#[tokio::test]
async fn bound_namespace_cannot_relogin_or_retire_and_revoked_grant_invalidates_launch() {
    let temp = tempfile::tempdir().unwrap();
    let r = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let p = authority('a');
    let a = account(&r, &p).await;
    let result = consent(&r, &p, input(a.clone(), "enroll"), "quota")
        .await
        .approve()
        .await
        .unwrap();
    let revision = r
        .resource(&p, result.resource_id.clone())
        .await
        .unwrap()
        .revision;
    let selected = r
        .resource_namespace(p.clone(), result.resource_id.clone(), revision.clone())
        .await
        .unwrap()
        .unwrap();
    let id = a.account_id.clone();
    r.0.store
        .call(1024, move |db| {
            assert!(db.begin_account_login(&id, graphs::now_ms()?).is_err());
            assert!(
                db.retire_account(&id, crate::LogoutObservation::unobserved())
                    .is_err()
            );
            Ok(())
        })
        .await
        .unwrap();
    selected.check().await.unwrap();
    let live = Arc::new(AtomicBool::new(true));
    let gate = live.clone();
    let gated = p
        .clone()
        .with_native_gate(move || gate.load(Ordering::Acquire))
        .unwrap();
    let renewed = r
        .resource_namespace(gated, result.resource_id.clone(), revision.clone())
        .await
        .unwrap()
        .unwrap();
    live.store(false, Ordering::Release);
    assert!(renewed.check().await.is_err());
    renewed.check_with(p.clone()).await.unwrap();
    assert!(renewed.check_with(authority('b')).await.is_err());
    assert!(
        r.resource_namespace(p.clone(), result.resource_id.clone(), "f".repeat(64))
            .await
            .is_err()
    );
    r.prepare_revoke(
        p.clone(),
        "revoke".into(),
        result.resource_id.clone(),
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap()
    .approve()
    .await
    .unwrap();
    assert!(selected.check().await.is_err());
    r.prepare_link(
        p.clone(),
        "regrant".into(),
        result.resource_id.clone(),
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap()
    .approve()
    .await
    .unwrap();
    assert!(
        selected.check_with(p.clone()).await.is_err(),
        "a new grant must not revive a captured process namespace"
    );
    r.resource_namespace(p, result.resource_id, revision)
        .await
        .unwrap()
        .unwrap()
        .check()
        .await
        .unwrap();
    r.shutdown().await.unwrap();
}
#[tokio::test]
async fn missing_recorded_namespace_never_falls_back_to_default_profile() {
    let temp = tempfile::tempdir().unwrap();
    let r = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let p = authority('a');
    let input = input(account(&r, &p).await, "enroll");
    let result = consent(&r, &p, input, "quota")
        .await
        .approve()
        .await
        .unwrap();
    let revision = r
        .resource(&p, result.resource_id.clone())
        .await
        .unwrap()
        .revision;
    r.0.store.call(1024,|db|{db.db.execute_batch("DROP TRIGGER owner_resource_namespaces_retained; DELETE FROM owner_resource_namespaces;")?;Ok(())}).await.unwrap();
    assert!(matches!(
        r.resource_namespace(p, result.resource_id, revision).await,
        Err(Error::Quarantined)
    ));
    r.shutdown().await.unwrap();
}
