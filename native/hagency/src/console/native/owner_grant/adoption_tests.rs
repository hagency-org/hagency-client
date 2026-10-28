#[cfg(feature="shared-intake")]
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn native_run_control_adopted_owner_selection_retains_original_device(){run_original_handoff("original").await;}
fn handoff_resource() -> hagency_core::project::Resource {
    serde_json::from_value(json!({"presetId":"handoff-original","seatId":"handoff-seat","framework":"codex","model":"gpt-5.6-sol","reasoning":"medium","published":false,"ceiling":{"tokens":1000,"period":"monthly"}})).unwrap()
}
fn handoff_package(instance: u64) -> ManagementPackage {
    let mut p = package(instance, false);
    p.permissions.extend([
        Permission::PrepareSourceQuiescence,
        Permission::ContributeResources,
        Permission::ReadIntents,
        Permission::ReadConnections,
        Permission::ResumeConnections,
        Permission::VerifyConnections,
        Permission::ReadAgentBudget,
    ]);
    p
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_resource_grant_handoff_survives_two_processes_and_lost_publication_reply() {
    run_original_handoff("original").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_confirmation_moves_original_resources_then_recovers_grant_after_restart() {
    run_original_handoff("native_confirmation").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_confirmation_recovers_an_original_prepared_plan_without_new_ids() {
    run_original_handoff("native_resume").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_confirmation_recovers_persisted_descriptor_after_scope_loss() {
    run_original_handoff("native_error").await;
}
async fn run_original_handoff(case: &str) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("fixture");
    hagency_store::private::directory(&root).unwrap();
    hagency_store::private::directory(&root.join("authority")).unwrap();
    let (origin, server, admission, publications) = server().await;
    let resource = handoff_resource();
    let revision = hagency_store::resource_publication_revision(&resource).unwrap();
    let at = now_ms().unwrap();
    *admission.lock().unwrap() = json!({"admission":{"contribution":{"id":"fixture_contribution","ownerMxid":"@owner:test","executorDeviceId":"device","executorGeneration":1,"resourceId":resource.id(),"resourceRevision":revision,"label":"Original resource","runtime":"codex","capabilities":["coding"],"coordinatorMxid":"@owner:test","state":"admitted","revision":2,"requestId":"original_admission_request","createdAtMs":at,"updatedAtMs":at,"verification":null},"request":{"id":"original_admission_request","kind":"resource_admission","contributionId":"fixture_contribution","projectId":null,"allocationId":null,"projectName":null,"resourceLabel":null,"requesterMxid":"@owner:test","approverMxid":"@admin:test","state":"approved","revision":2,"createdAtMs":at,"expiresAtMs":at+60000,"decidedAtMs":at,"canDecide":false}},"observedAtMs":at});
    publications.lock().unwrap().lose_reply = true;
    if case == "provenance" {
        publications.lock().unwrap().connections.provenance_path = Some(root.join("canonical/domain.sqlite3"));
    }
    let phases: &[&str] = if case=="connection" {
        publications.lock().unwrap().device_generation=Some(2);
        publications.lock().unwrap().connections.lose_resume_reply=true;
        &["adopt"]
    } else if matches!(case,"namespace"|"provenance") { &["adopt"] } else { &["adopt", "recover", "new_device"] };
    for phase in phases {
        if *phase == "new_device" {
            publications.lock().unwrap().device_override = Some("replacement-device".into());
        }
        let result = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "console::native::owner_grant::tests::adoption_grant_handoff_process_child",
                "--exact",
                "--nocapture",
            ])
            .env("HAGENCY_HANDOFF_FIXTURE_PHASE", phase)
            .env("HAGENCY_HANDOFF_FIXTURE_CASE", case)
            .env("HAGENCY_HANDOFF_FIXTURE_ROOT", &root)
            .env("HAGENCY_HANDOFF_FIXTURE_ORIGIN", &origin)
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "phase {phase}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let published = publications.lock().unwrap();
    assert_eq!(published.device_posts, if matches!(case,"connection"|"namespace"|"provenance") {1} else {2}, "ordinary source/canonical/process restarts reuse original native device; only explicit fresh login registers another");
    if case == "provenance" {
        assert_eq!(published.connections.contribution_posts, 4);
        assert_eq!(published.posts, 0, "offering does not allocate capacity");
        drop(published);server.abort();return;
    }
    if case=="namespace" {
        assert_eq!(published.posts,0,"namespace probe does not publish capacity");
        drop(published);server.abort();return;
    }
    if case=="connection" {
        assert_eq!(published.connections.resume_posts,1);
        assert_eq!(published.connections.verify_posts,1);
        assert_eq!(published.posts,0,"reconnection never publishes a capacity grant");
        drop(published);
        server.abort();
        return;
    }
    assert_eq!(
        published.posts, 1,
        "restart recovers the original command without another POST"
    );
    assert_eq!(published.commands.len(), 1);
    let receipt = &published.commands.values().next().unwrap()["receipt"];
    assert_eq!(receipt["resourceId"], resource.id());
    assert_eq!(receipt["allocatedTokens"], 300);
    assert!(published.recover_reads > 0);
    drop(published);
    server.abort();
}
#[tokio::test]
async fn adoption_grant_handoff_process_child() {
    let Ok(phase) = std::env::var("HAGENCY_HANDOFF_FIXTURE_PHASE") else {
        return;
    };
    assert!(matches!(phase.as_str(), "adopt" | "recover" | "new_device"));
    let root = PathBuf::from(std::env::var_os("HAGENCY_HANDOFF_FIXTURE_ROOT").unwrap());
    let origin = std::env::var("HAGENCY_HANDOFF_FIXTURE_ORIGIN").unwrap();
    // Test-only selection is fixed before the first normal opener in EACH process.
    // There is no production environment-variable root override.
    super::super::initialize_resource_activation_fixture(&root).unwrap();
    let case = std::env::var("HAGENCY_HANDOFF_FIXTURE_CASE").unwrap_or_default();
    let native_confirmation = case == "native_confirmation";
    let native_error = case == "native_error";
    let native_resume = case == "native_resume" || native_error;
    struct RestartSource { source: Source, explicit_login: AtomicBool }
    impl MatrixTokenSource for RestartSource {
        fn permits_explicit_reenrollment(&self) -> bool { self.explicit_login.load(Ordering::Acquire) }
        fn consume_explicit_reenrollment(&self) -> bool { self.explicit_login.swap(false, Ordering::AcqRel) }
        fn access_token(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<MatrixAccessToken,NativeError>> + Send + '_>> {
            self.source.access_token()
        }
    }
    let source = Arc::new(RestartSource { source: Source(Arc::new(AtomicBool::new(true))), explicit_login: AtomicBool::new(phase == "new_device") });
    let state = root.join("host");
    let registry = OwnerResourceRegistry::isolated_for_tests(&root.join("canonical")).unwrap();
    assert!(super::super::initialize_resource_activation_fixture(&root).is_err());
    let resource = handoff_resource();
    let mut second = resource.clone();
    second.preset_id = "handoff-second-original".into();
    let mut expected = vec![resource.id(), second.id()];
    expected.sort();
    let profile = crate::console::owned_agents::profile_identity(
        &origin,
        &format!("{origin}auth/"),
        "sub",
        "@owner:test",
    )
    .unwrap();
    let domain_path = state
        .join("owned-agent-owners")
        .join(format!("management_{profile}"));
    let owner = if phase == "adopt" {
        let owner = NativeOwner::open_with_matrix(&state, &origin, "@owner:test", source.clone())
            .await
            .unwrap();
        // Real original definitions, no synthetic replacement in the canonical ledger.
        hagency_store::private::directory(domain_path.parent().unwrap()).unwrap();
        let mut old =
            hagency_store::DomainRepository::open_managed(&domain_path, &profile).unwrap();
        old.put_seat(&hagency_core::project::Seat {
            id: resource.seat_id.clone(),
            declaration: None,
        })
        .unwrap();
        old.put_resource(&resource).unwrap();
        old.put_resource(&second).unwrap();
        drop(old);
        let personal = domain_path
            .parent()
            .unwrap()
            .join(format!("owner_{profile}"));
        hagency_store::private::directory(&personal).unwrap();
        hagency_store::private::replace(&personal.join("profile-identity.json"), &serde_json::to_vec(&json!({"version":1,"origin":origin,"issuer":format!("{origin}auth/"),"subject":"sub","owner":"@owner:test"})).unwrap()).unwrap();
        let personal_db = personal.join("agent-local.sqlite");
        if native_confirmation || native_resume {
            assert!(
                owner
                    .seed_empty_personal_activation_fixture()
                    .await
                    .unwrap()
            );
            let original = std::fs::read(&personal_db).unwrap();
            assert!(
                !owner
                    .seed_empty_personal_activation_fixture()
                    .await
                    .unwrap()
            );
            assert_eq!(std::fs::read(&personal_db).unwrap(), original);
        } else {
            hagency_store::private::open(&personal_db, true).unwrap();
            drop(
                hagency_agent_local::Ledger::open_scoped(&personal_db, "@owner:test", &profile)
                    .unwrap(),
            );
        }
        let manager = owner.management(handoff_package(1)).await.unwrap();
        let (context, session) = manager.authorized().await.unwrap();
        let intent = hagency_store::management::Intent::new(
            hagency_store::management::IntentOperation::Contribute,
            "original_admission".into(),
            &json!({"commandId":"original_admission","resourceId":resource.id()}),
        )
        .unwrap();
        session
            .prepare_intent(&context, intent.clone())
            .await
            .unwrap();
        session
            .record_intent_reply(
                &context,
                intent,
                hagency_store::management::IntentReply::Success(
                    json!({"admission":{"contribution":{"id":"fixture_contribution"}}}),
                ),
            )
            .await
            .unwrap();
        drop(session);
        drop(manager);
        let (selection, owner) = if native_resume {
            if native_error {
                let review = owner
                    .review_resource_activation(handoff_package(2), None, 700)
                    .await
                    .unwrap();
                let descriptor = state.join("adopted-management-initializer-v1.json");
                let gate_path = descriptor.clone();
                // Lose current native authority only after the durable plan and
                // original-device descriptor exist, before source fencing/import.
                assert!(
                    owner
                        .confirm_resource_activation_fixture_with_gate(
                            review,
                            handoff_package(2),
                            provider_fixture("synthetic-account"),
                            move || !gate_path.exists(),
                        )
                        .await
                        .is_err()
                );
                assert!(descriptor.is_file());
            } else {
                let mut request = super::super::SourceQuiescenceRequest::new(
                    &owner,
                    handoff_package(2),
                    registry.clone(),
                    Instant::now() + Duration::from_secs(50),
                    || true,
                )
                .unwrap();
                request.lease_root = Some(root.join("authority"));
                let work = owner.prepare_source_quiescence(request).await.unwrap();
                let sources = work.close_and_drain().await.unwrap();
                let transfer = sources
                    .prepare_resource_adoption(
                        "original_pending_migration".into(),
                        provider_fixture("synthetic-account"),
                        700,
                    )
                    .await
                    .unwrap();
                assert_eq!(transfer.status().await.unwrap().phase, "prepared");
                drop(transfer);
                drop(sources);
                drop(work);
            }
            owner.suspend().await.unwrap();
            drop(owner);
            let selected = super::super::AdoptedManagementSelection::load_for_host(
                &state,
                &origin,
                "@owner:test",
            )
            .await
            .unwrap()
            .unwrap();
            let recovered = selected
                .open_for_onboarding_with_matrix_guarded(
                    &state,
                    &origin,
                    "@owner:test",
                    source.clone(),
                    |_| Ok(()),
                )
                .await
                .unwrap();
            assert_eq!(
                recovered
                    .management(handoff_package(2))
                    .await
                    .err()
                    .unwrap()
                    .code,
                "source_adoption_recovery_only"
            );
            let context = recovered
                .resource_activation_context(handoff_package(2), Some(&selected))
                .await
                .unwrap();
            assert!(context.original_migration.is_some());
            if !native_error {
                assert_eq!(
                    context.original_migration.as_deref(),
                    Some("original_pending_migration")
                );
            }
            let original_migration = context.original_migration.clone().unwrap();
            assert_eq!(context.original_monthly_tokens, Some(700));
            assert!(
                recovered
                    .review_resource_activation(handoff_package(2), Some(selected.clone()), 701)
                    .await
                    .is_err()
            );
            let review = recovered
                .review_resource_activation(handoff_package(2), Some(selected), 700)
                .await
                .unwrap();
            assert!(review.recovering());
            let selected = recovered
                .confirm_resource_activation_fixture(
                    review,
                    handoff_package(2),
                    provider_fixture("synthetic-account"),
                )
                .await
                .unwrap();
            let descriptor: serde_json::Value = serde_json::from_slice(
                &std::fs::read(state.join("adopted-management-initializer-v1.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(descriptor["migrationId"], original_migration);
            (selected, recovered)
        } else if native_confirmation {
            assert!(
                owner
                    .review_resource_activation(handoff_package(2), None, 0)
                    .await
                    .is_err()
            );
            let review = owner
                .review_resource_activation(handoff_package(2), None, 700)
                .await
                .unwrap();
            assert_eq!(
                review
                    .context()
                    .resources
                    .iter()
                    .map(|r| r.id.clone())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(review.monthly_tokens(), 700);
            assert!(!review.recovering());
            let selected = owner
                .confirm_resource_activation_fixture(
                    review,
                    handoff_package(2),
                    provider_fixture("synthetic-account"),
                )
                .await
                .unwrap();
            (selected, owner)
        } else {
            let mut request = super::super::SourceQuiescenceRequest::new(
                &owner,
                handoff_package(2),
                registry.clone(),
                Instant::now() + Duration::from_secs(50),
                || true,
            )
            .unwrap();
            request.lease_root = Some(root.join("authority"));
            let work = owner.prepare_source_quiescence(request).await.unwrap();
            let sources = work.close_and_drain().await.unwrap();
            let transfer = sources
                .prepare_resource_adoption(
                    "handoff_original".into(),
                    provider_fixture("synthetic-account"),
                    700,
                )
                .await
                .unwrap();
            assert!(
                transfer.finish_handoff().await.is_err(),
                "prepared is not active"
            );
            assert_eq!(
                transfer
                    .advance(provider_fixture("synthetic-account"))
                    .await
                    .unwrap()
                    .phase,
                "active"
            );
            let descriptor = state.join("adopted-management-initializer-v1.json");
            let saved_descriptor = descriptor.with_extension("saved");
            std::fs::rename(&descriptor, &saved_descriptor).unwrap();
            assert!(
                transfer.finish_handoff().await.is_err(),
                "completed migration cannot recreate missing original identity"
            );
            assert!(!descriptor.exists());
            std::fs::rename(saved_descriptor, descriptor).unwrap();
            let selection = transfer.finish_handoff().await.unwrap();
            assert!(selection.matches_host(&state, &origin, "@owner:test"));
            assert!(!selection.matches_host(&state, &origin, "@other:test"));
            drop(transfer);
            drop(sources);
            drop(work);
            (selection, owner)
        };
        owner.suspend().await.unwrap();
        drop(owner);
        assert!(hagency_store::DomainRepository::open_managed(&domain_path, &profile).is_err());
        assert!(
            hagency_agent_local::Ledger::open_scoped(&personal_db, "@owner:test", &profile)
                .is_err()
        );
        selection
            .open_with_matrix_guarded(&state, &origin, "@owner:test", source.clone(), |_| Ok(()))
            .await
            .unwrap()
    } else {
        assert!(hagency_store::DomainRepository::open_managed(&domain_path, &profile).is_err());
        let descriptor = state.join("adopted-management-initializer-v1.json");
        let bytes = std::fs::read(&descriptor).unwrap();
        let mut bad: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        bad["registryPath"] = json!("/forbidden/script/path");
        hagency_store::private::replace(&descriptor, &serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(
            super::super::AdoptedManagementSelection::load_for_isolated_tests(
                &state,
                &origin,
                "@owner:test",
                registry.clone()
            )
            .is_err()
        );
        hagency_store::private::replace(&descriptor, &bytes).unwrap();
        assert!(
            super::super::AdoptedManagementSelection::load_for_isolated_tests(
                &state,
                &origin,
                "@other:test",
                registry.clone()
            )
            .is_err()
        );
        if serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"] == 2 {
            // Active v2 may restore inventory on a replacement device, but only
            // with the exact same-profile canonical plan and original descriptor.
            for field in ["profile", "registryId", "planDigest"] {
                let mut invalid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                if field == "planDigest" {
                    invalid["taskPrepare"][field] = json!("c".repeat(64));
                } else {
                    invalid[field] = json!("c".repeat(64));
                }
                hagency_store::private::replace(&descriptor, &serde_json::to_vec(&invalid).unwrap()).unwrap();
                let attempt = async {
                    let selected = super::super::AdoptedManagementSelection::load_for_host(&state, &origin, "@owner:test").await?.unwrap();
                    selected.open_for_onboarding_with_matrix_guarded(&state, &origin, "@owner:test", source.clone(), |_| Ok(())).await
                }.await;
                assert!(attempt.is_err(), "invalid active v2 {field} must not authorize inventory");
                drop(attempt);
                assert_eq!(std::fs::read(&descriptor).unwrap(), serde_json::to_vec(&invalid).unwrap());
                hagency_store::private::replace(&descriptor, &bytes).unwrap();
            }
        }
        super::super::AdoptedManagementSelection::load_for_host(&state, &origin, "@owner:test")
            .await
            .unwrap()
            .unwrap()
            .open_for_onboarding_with_matrix_guarded(
                &state,
                &origin,
                "@owner:test",
                source.clone(),
                |_| Ok(()),
            )
            .await
            .unwrap()
    };
    // Provider namespace discovery must remain usable after the real personal
    // ledger fence. It must not reopen, migrate or rewrite that database.
    let personal = state
        .join("owned-agent-owners")
        .join(format!("owner_{profile}"));
    let personal_db = personal.join("agent-local.sqlite");
    let personal_before = std::fs::read(&personal_db).unwrap();
    let marker_before = std::fs::read(personal.join("profile-identity.json")).unwrap();
    crate::console::owner_provider::paths(
        &state,
        &origin,
        &format!("{origin}auth/"),
        "sub",
        "@owner:test",
    )
    .unwrap();
    assert_eq!(std::fs::read(&personal_db).unwrap(), personal_before);
    assert_eq!(
        std::fs::read(personal.join("profile-identity.json")).unwrap(),
        marker_before
    );
    assert!(
        hagency_agent_local::Ledger::open_scoped(&personal_db, "@owner:test", &profile).is_err()
    );
    let manager = owner.management(handoff_package(3)).await.unwrap();
    // Budget discovery is a management read after adoption, including a
    // recovered source. It must not enter the fenced legacy configuration API.
    assert_eq!(
        manager.agent_budget("fixture_agent".into()).await.unwrap()["supported"],
        false
    );
    let page = manager
        .resources(hagency_store::management::ResourceQuery::default())
        .await
        .unwrap();
    assert_eq!(
        page.resources
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<_>>(),
        expected
    );
    for r in &page.resources {
        assert_eq!(
            r.budget.seat.remaining.map(u64::from),
            Some(if phase == "adopt" { 700 } else { 400 })
        );
    }
    let saved = manager.saved_commands().await.unwrap();
    if phase == "new_device" {
        assert!(
            saved.is_empty(),
            "original private commands stay bound to their device"
        );
    } else {
        assert!(
            saved
                .iter()
                .any(|c| c.command_id == "original_admission" && c.reply.is_some())
        );
    }
    if phase == "new_device" {
        let path = state.join("adopted-management-initializer-v1.json");
        let saved = path.with_extension("saved");
        std::fs::rename(&path, &saved).unwrap();
        assert!(
            owner
                .adopted_management_selection(registry.clone())
                .await
                .is_err(),
            "lost descriptor cannot be recreated from replacement device"
        );
        std::fs::rename(saved, path).unwrap();
    }
    let selection = owner
        .adopted_management_selection(registry.clone())
        .await
        .unwrap();
    let executor = selection
        .allocation_executor(&owner, package(91, false), Arc::new(|| true))
        .await;
    if phase == "new_device" {
        assert_eq!(
            executor.err().unwrap().code,
            "adopted_executor_device_reauthorization_required"
        );
    } else {
        let executor = executor.unwrap();
        executor.retire();
        assert!(
            executor.poll_allocation(None).await.is_err(),
            "a retired native worker cannot deliver a reservation"
        );
        drop(executor);
    }
    assert_eq!(
        selection
            .allocation_executor(&owner, package(92, false), Arc::new(|| false))
            .await
            .err()
            .unwrap()
            .code,
        "management_scope_changed"
    );
    #[cfg(feature = "shared-execution")]
    {
        let executor = selection
            .shared_executor(&owner, package(94, false), Arc::new(|| true))
            .await;
        if phase == "new_device" {
            assert_eq!(
                executor.err().unwrap().code,
                "adopted_executor_device_reauthorization_required"
            );
        } else {
            let executor = executor.unwrap();
            executor.retire();
            assert!(
                executor.poll(None).await.is_err(),
                "retired shared worker has no execution authority"
            );
            assert!(executor.poll_agent(None).await.is_err());
            assert!(executor.poll_matrix(None).await.is_err());
            drop(executor);
        }
        assert_eq!(
            selection
                .shared_executor(&owner, package(95, false), Arc::new(|| false))
                .await
                .err()
                .unwrap()
                .code,
            "management_scope_changed"
        );
    }
    #[cfg(feature="shared-intake")]
    {
        let mut p=handoff_package(998);p.permissions=vec![Permission::ChangeAgentRunControl];
        let control=owner.management(p).await.unwrap();
        let binding=control.owner_stop_transport().await;
        if phase=="new_device" {assert_eq!(binding.err().unwrap().code,"adopted_executor_device_reauthorization_required");}
        else {
            let (transport,_registry)=binding.unwrap();
            assert!(transport.poll(&registry,None).await.is_err(),"human Stop scope grants no execution permission");
            control.retire();
            assert!(transport.stop_reviewed_demand_assignment(&registry,"unknown_assignment".into(),hagency_store::owner_resources::shared_execution::SharedDemandStopInput{command_id:"original_stop".into(),expected_revision:1,mode:"paused".into()}).await.is_err());
        }
    }
    if phase == "new_device" {
        assert_eq!(
            selection
                .grant_access(&owner, package(4, false))
                .await
                .err()
                .unwrap()
                .code,
            "adopted_grant_device_reauthorization_required"
        );
        manager.retire();
        drop(manager);
        drop(selection);
        owner.suspend().await.unwrap();
        drop(owner);
        registry.shutdown().await.unwrap();
        return;
    }
    let access = selection
        .grant_access(&owner, package(4, false))
        .await
        .unwrap();
    #[cfg(unix)]
    if case=="namespace" {
        // Run the management call as an executor task, as the Desktop host does.
        // Keeping every adoption phase and recovery poll on this test's one
        // synchronous stack can overflow a debug libtest thread despite bounded
        // retained futures. Keep the original stack limit and all assertions.
        let probe_owner=owner.clone();let probe_registry=registry.clone();
        let probe_profile=profile.clone();let probe_state=state.clone();
        tokio::spawn(async move {
            verify_namespace_execution_contract(&probe_owner,&probe_registry,&probe_profile,&probe_state).await;
        }).await.unwrap();
        drop(access);manager.retire();drop(manager);drop(selection);
        owner.suspend().await.unwrap();drop(owner);registry.shutdown().await.unwrap();return;
    }
    if matches!(case.as_str(),"connection"|"provenance") {
        if case == "connection" {
            // Match the Desktop host's separate executor task, as the namespace
            // and provenance cases do below. Do not stack connection/provider
            // observation inside every synchronous adoption phase on libtest's
            // small debug stack. The ordinary stack limit and assertions stay.
            let probe_manager=manager.clone();let probe_owner=owner.clone();
            let probe_registry=registry.clone();let probe_access=access.clone();
            let probe_resource=resource.clone();
            tokio::spawn(async move {
                verify_adopted_connection_contract(&probe_manager,&probe_owner,&probe_registry,&probe_access,&probe_resource).await;
            }).await.unwrap();
        } else {
            // As in the Desktop host, run management on its own executor task,
            // rather than stacking it inside every synchronous adoption phase.
            let probe_manager=manager.clone();let probe_owner=owner.clone();
            let probe_path=root.join("canonical/domain.sqlite3");let probe_resource=resource.clone();
            let probe_access=access.clone();
            tokio::spawn(async move {
                verify_adopted_provenance_contract(&probe_manager,&probe_owner,&probe_access,&probe_path,&probe_resource).await;
            }).await.unwrap();
        }
        drop(access);
        manager.retire();
        drop(manager);
        drop(selection);
        owner.suspend().await.unwrap();
        drop(owner);
        registry.shutdown().await.unwrap();
        return;
    }
    if phase == "adopt" {
        // Only the external provider observation is synthetic. Enrollment still
        // checks live HTTP admission, canonical identity and original private journal.
        let service = access.service();
        let enrollment = service
            .verified_enrollment_with(&registry, "fixture_contribution".into(), |_| async {
                Ok(provider_fixture("synthetic-account"))
            })
            .await
            .unwrap();
        let review = service
            .prepare(
                &registry,
                enrollment,
                OwnerGrantChoices {
                    allocated_tokens: 300,
                    ..Fixture::choices()
                },
            )
            .await
            .unwrap();
        assert_eq!(review.summary().resource_id, resource.id());
        assert_eq!(review.context().shared_account_total_tokens, Some(700));
        assert_eq!(review.context().remaining_tokens, Some(700));
        let id = review.summary().intent_id;
        assert!(matches!(
            access.confirm(review).await.unwrap().state,
            OwnerGrantState::Recorded
        ));
        let review = match access.review_publication(id).await.unwrap() {
            OwnerGrantPublicationResult::Review(r) => *r,
            _ => panic!("expected native publication review"),
        };
        assert_eq!(
            access.confirm_publication(review).await.unwrap().state,
            OwnerGrantPublicationState::OutcomeUnknown
        );
    } else {
        let items = access.list_existing().await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].resource_id, resource.id());
        let progress = match access
            .recover_publication(items[0].intent_id.clone())
            .await
            .unwrap()
        {
            OwnerGrantPublicationResult::Progress(p) => *p,
            _ => panic!("must recover original publication"),
        };
        assert_eq!(progress.state, OwnerGrantPublicationState::ServerObserved);
    }
    let final_page = manager
        .resources(hagency_store::management::ResourceQuery::default())
        .await
        .unwrap();
    for r in final_page.resources {
        assert_eq!(r.budget.seat.remaining.map(u64::from), Some(400));
    }
    let count: u64 = rusqlite::Connection::open_with_flags(
        root.join("canonical/domain.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row("SELECT count(*) FROM owner_capacity_grants", [], |r| {
        r.get(0)
    })
    .unwrap();
    assert_eq!(count, 1);
    access.retire();
    drop(access);
    manager.retire();
    drop(manager);
    drop(selection);
    if phase == "recover" {
        owner.shutdown_checked().await.unwrap();
    } else {
        owner.suspend().await.unwrap();
    }
    drop(owner);
    registry.shutdown().await.unwrap();
}
