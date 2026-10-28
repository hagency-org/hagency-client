use super::*;
use super::super::sources::tests::{prepared, sources};
use super::super::tests::Fixture;

#[tokio::test]
async fn empty_registry_activation_preserves_conflicting_task_history() {
    let (f, transfer, journal) = fenced(false).await;
    f.registry.0.store.call(1, |db| {
        db.db.execute("INSERT INTO owner_operator_task_histories(profile,migration_id,receipt_digest,receipt) SELECT profile,id,?1,'{}' FROM owner_resource_adoptions", ["f".repeat(64)])?;
        Ok(())
    }).await.unwrap();
    assert!(
        f.registry
            .prepare_empty_device_registry(&transfer)
            .await
            .is_err()
    );
    assert_eq!(transfer.summary().await.unwrap().phase, "sources_fenced");
    assert_eq!(f.count("owner_operator_task_histories").await, 1);
    journal.shutdown().await.unwrap();
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn empty_active_result_must_match_original_plan_on_recovery() {
    let (f, transfer, journal) = fenced(false).await;
    activate(&f, &transfer, &journal).await;
    f.registry.0.store.call(1, |db| {
        assert!(db.db.execute("UPDATE owner_resource_adoptions SET result='{}'", []).is_err());
        // Simulate damaged storage beyond the immutable-writer boundary.
        db.db.execute_batch("DROP TRIGGER owner_resource_adoptions_identity; UPDATE owner_resource_adoptions SET result='{}'")?;
        Ok(())
    }).await.unwrap();
    assert!(transfer.summary().await.is_err());
    assert!(
        f.registry
            .empty_device_transfer(
                f.authority(),
                transfer.initialization_id().into(),
                transfer.plan_digest().unwrap()
            )
            .await
            .is_err()
    );
    #[cfg(feature = "resource-adoption")]
    assert!(transfer.adopted_management(journal.clone()).await.is_err());
    journal.shutdown().await.unwrap();
    f.registry.shutdown().await.unwrap();
}

async fn fenced(present: bool) -> (Fixture, EmptyDeviceTransfer, PrivateManagementJournal) {
    let (f, transfer) = prepared(present).await;
    assert!(
        f.registry
            .prepare_empty_device_registry(&transfer)
            .await
            .is_err()
    );
    let (mut d, mut p) = sources(&f, present, false);
    transfer
        .fence_sources(&mut d, p.as_mut(), &"c".repeat(64))
        .unwrap();
    drop(d);
    drop(p);
    let (mut d, mut p) = sources(&f, present, true);
    let proof = transfer.inspect_archives(&mut d, p.as_mut()).unwrap();
    f.registry.record_empty_device_sources(proof).await.unwrap();
    drop(d);
    drop(p);
    let journal = PrivateManagementJournal::open_existing(
        &f.root.path().canonicalize().unwrap().join("source"),
        transfer.archive_binding().unwrap(),
    )
    .unwrap();
    (f, transfer, journal)
}
async fn activate(f: &Fixture, transfer: &EmptyDeviceTransfer, journal: &PrivateManagementJournal) {
    assert_eq!(
        f.registry
            .prepare_empty_device_registry(transfer)
            .await
            .unwrap()
            .phase,
        "registry_ready"
    );
    assert_eq!(
        f.registry
            .prepare_empty_device_registry(transfer)
            .await
            .unwrap()
            .phase,
        "registry_ready"
    );
    f.registry
        .record_empty_device_custody(transfer.verify_private_journal(journal).await.unwrap())
        .await
        .unwrap();
    assert_eq!(transfer.summary().await.unwrap().phase, "custody_ready");
    f.registry
        .activate_empty_device(transfer.verify_private_journal(journal).await.unwrap())
        .await
        .unwrap();
    assert_eq!(transfer.summary().await.unwrap().phase, "active");
}
#[tokio::test]
async fn empty_activation_requires_ordered_phases_and_real_journal_without_quota() {
    for present in [false, true] {
        let (f, transfer, journal) = fenced(present).await;
        assert!(
            f.registry
                .record_empty_device_custody(
                    transfer.verify_private_journal(&journal).await.unwrap()
                )
                .await
                .is_err()
        );
        assert!(
            f.registry
                .activate_empty_device(transfer.verify_private_journal(&journal).await.unwrap())
                .await
                .is_err()
        );
        #[cfg(feature = "resource-adoption")]
        assert!(transfer.adopted_management(journal.clone()).await.is_err());
        activate(&f, &transfer, &journal).await;
        assert_eq!(
            f.registry
                .activate_empty_device(transfer.verify_private_journal(&journal).await.unwrap())
                .await
                .unwrap()
                .phase,
            "active"
        );
        assert_eq!(
            f.registry
                .record_empty_device_custody(
                    transfer.verify_private_journal(&journal).await.unwrap()
                )
                .await
                .unwrap()
                .phase,
            "active"
        );
        assert_eq!(
            f.registry
                .prepare_empty_device_registry(&transfer)
                .await
                .unwrap()
                .phase,
            "active"
        );
        for table in [
            "resources",
            "seats",
            "managed_accounts",
            "owner_resource_grants",
            "owner_capacity_grants",
        ] {
            assert_eq!(f.count(table).await, 0);
        }
        assert_eq!(f.root.path().join("personal.sqlite").exists(), present);
        journal.shutdown().await.unwrap();
        f.registry.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn empty_activation_rejects_foreign_or_closed_journal_and_changed_custody() {
    let (f, transfer, journal) = fenced(false).await;
    let (foreign, other, wrong_journal) = fenced(false).await;
    assert!(
        transfer
            .verify_private_journal(&wrong_journal)
            .await
            .is_err()
    );
    assert!(
        foreign
            .registry
            .prepare_empty_device_registry(&transfer)
            .await
            .is_err()
    );
    f.registry
        .prepare_empty_device_registry(&transfer)
        .await
        .unwrap();
    let proof = transfer.verify_private_journal(&journal).await.unwrap();
    journal.shutdown().await.unwrap();
    assert!(f.registry.record_empty_device_custody(proof).await.is_err());
    assert_eq!(transfer.summary().await.unwrap().phase, "registry_ready");
    // Closed terminal custody can exist originally, but a later added record
    // cannot be silently absorbed into the exact reviewed inventory.
    let (d, _) = sources(&f, false, false);
    d.execute("INSERT INTO management_intents(actor_scope,command_id,operation,digest,input,reply,state) VALUES(?1,'unexpected',?2,?3,NULL,NULL,'acknowledged')", params!["e".repeat(64),serde_json::to_string(&crate::management::IntentOperation::ProjectRequest).unwrap(),"b".repeat(64)]).unwrap();
    drop(d);
    let reopened = PrivateManagementJournal::open_existing(
        &f.root.path().canonicalize().unwrap().join("source"),
        transfer.archive_binding().unwrap(),
    )
    .unwrap();
    assert!(transfer.verify_private_journal(&reopened).await.is_err());
    assert!(
        f.registry
            .record_empty_device_custody(
                other.verify_private_journal(&wrong_journal).await.unwrap()
            )
            .await
            .is_err()
    );
    reopened.shutdown().await.unwrap();
    wrong_journal.shutdown().await.unwrap();
    f.registry.shutdown().await.unwrap();
    foreign.registry.shutdown().await.unwrap();
}

#[cfg(feature = "resource-adoption")]
#[tokio::test]
async fn empty_active_manager_keeps_tasks_and_pending_commands_across_reopen() {
    use crate::management::{
        HostContext, Session, Permission, Intent, IntentOperation, OperatorTaskCommand,
        OperatorTaskChange, CreateOperatorTask,
    };
    for present in [false, true] {
        let (f, transfer, journal) = fenced(present).await;
        activate(&f, &transfer, &journal).await;
        let backing = transfer.adopted_management(journal.clone()).await.unwrap();
        let context = HostContext::new(
            f.profile.account.clone(),
            1,
            "a".repeat(64),
            1,
            "empty_device".into(),
            1,
        )
        .unwrap();
        let permissions = [
            Permission::ReadTasks,
            Permission::CreateTasks,
            Permission::ReadIntents,
            Permission::RequestProjects,
            Permission::ReadResources,
        ];
        let session = Session::new_adopted(
            backing.clone(),
            context.clone(),
            permissions,
            Instant::now() + Duration::from_secs(60),
        )
        .unwrap();
        let task = OperatorTaskCommand {
            command_id: "create_original".into(),
            task_id: "original_task".into(),
            change: OperatorTaskChange::Create(CreateOperatorTask {
                title: "First task".into(),
                ..Default::default()
            }),
        };
        assert_eq!(
            session
                .apply_operator_task(&context, task.clone())
                .await
                .unwrap()
                .revision,
            1
        );
        let intent = Intent::new(IntentOperation::ProjectRequest,"pending_original".into(), &json!({"commandId":"pending_original","allocationId":"allocation_original","name":"Project"})).unwrap();
        session.prepare_intent(&context, intent).await.unwrap();
        assert!(
            transfer.verify_private_journal(&journal).await.is_err(),
            "active journal legitimately diverges from empty source snapshot"
        );
        session.retire();
        drop(session);
        backing.shutdown().await.unwrap();
        drop(backing);
        drop(journal);
        let (mut d, mut p) = sources(&f, present, true);
        assert!(transfer.inspect_archives(&mut d, p.as_mut()).is_err());
        transfer
            .inspect_active_archives(&mut d, p.as_mut(), &"c".repeat(64))
            .unwrap();
        assert!(
            transfer
                .inspect_active_archives(&mut d, p.as_mut(), &"d".repeat(64))
                .is_err()
        );
        drop(d);
        drop(p);
        let recovered = f
            .registry
            .empty_device_transfer(
                f.authority(),
                transfer.initialization_id().into(),
                transfer.plan_digest().unwrap(),
            )
            .await
            .unwrap();
        let journal = PrivateManagementJournal::open_existing(
            &f.root.path().canonicalize().unwrap().join("source"),
            recovered.archive_binding().unwrap(),
        )
        .unwrap();
        let backing = recovered.adopted_management(journal).await.unwrap();
        let session = Session::new_adopted(
            backing.clone(),
            context.clone(),
            permissions,
            Instant::now() + Duration::from_secs(60),
        )
        .unwrap();
        let page = session
            .operator_task_page(&context, Default::default())
            .await
            .unwrap();
        assert!(page.supported);
        assert_eq!(page.tasks.len(), 1);
        assert_eq!(
            session
                .apply_operator_task(&context, task)
                .await
                .unwrap()
                .revision,
            1,
            "lost reply recovers original task"
        );
        assert_eq!(f.count("owner_operator_task_histories").await, 1);
        assert_eq!(f.count("owner_operator_tasks").await, 1);
        session.retire();
        drop(session);
        backing.shutdown().await.unwrap();
        f.registry.shutdown().await.unwrap();
    }
}
