use super::*;
use super::fencing::{ready, backup_bytes};
use hagency_store::management::{
    Intent, IntentOperation, PrepareAccount, OperatorTaskCommand, OperatorTaskChange,
    CreateOperatorTask,
};

fn active_package() -> ManagementPackage {
    let mut result = package();
    result.permissions.extend([
        Permission::ReadTasks,
        Permission::CreateTasks,
        Permission::ReadAccounts,
        Permission::PrepareAccounts,
        Permission::ReadIntents,
        Permission::RequestProjects,
    ]);
    result
}

#[tokio::test]
async fn empty_native_active_handoff_and_restart_keep_accounts_tasks_and_pending_commands() {
    for present in [false, true] {
        let (f, registry, work, source, init, path) = ready(present).await;
        assert!(init.finish_handoff().await.is_err());
        let descriptor = std::fs::read(f.owner.0.host.state_directory().join(DESCRIPTOR)).unwrap();
        assert_eq!(init.advance().await.unwrap().phase, "active");
        assert_eq!(init.advance().await.unwrap().phase, "active");
        let backups = backup_bytes(&path.join("adoption_original_setup"));
        let selection = init.finish_handoff().await.unwrap();
        assert!(
            init.advance().await.is_err(),
            "a drained handoff cannot start another journal"
        );
        drop(init);
        drop(source);
        drop(work);
        let f = f
            .restart_adopted(registry.clone(), "original_setup".into())
            .await;
        f.owner
            .adopted_management_selection(registry.clone())
            .await
            .unwrap();
        let manager = f.owner.management(active_package()).await.unwrap();
        assert!(manager.account_enrollment_ready());
        assert!(
            manager
                .resources(Default::default())
                .await
                .unwrap()
                .resources
                .is_empty()
        );
        for table in [
            "resources",
            "seats",
            "managed_accounts",
            "owner_resource_grants",
            "owner_capacity_grants",
        ] {
            assert_eq!(
                count(f.temp.path(), table),
                0,
                "initialization created {table}"
            );
        }
        let (context, session) = manager.authorized().await.unwrap();
        let account_request = PrepareAccount {
            command_id: "first_account".into(),
        };
        let account = session
            .prepare_account(&context, account_request.clone())
            .await
            .unwrap();
        let task = OperatorTaskCommand {
            command_id: "first_task".into(),
            task_id: "task_original".into(),
            change: OperatorTaskChange::Create(CreateOperatorTask {
                title: "First native task".into(),
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
        let pending = Intent::new(IntentOperation::ProjectRequest,"pending_original".into(),&serde_json::json!({"commandId":"pending_original","allocationId":"allocation_original","name":"Project"})).unwrap();
        session.prepare_intent(&context, pending).await.unwrap();
        drop(session);
        drop(manager);
        let f = f
            .restart_adopted(registry.clone(), "original_setup".into())
            .await;
        f.owner
            .adopted_management_selection(registry.clone())
            .await
            .unwrap();
        let manager = f.owner.management(active_package()).await.unwrap();
        let (context, session) = manager.authorized().await.unwrap();
        assert_eq!(
            session
                .prepare_account(&context, account_request)
                .await
                .unwrap()
                .account_id,
            account.account_id
        );
        assert_eq!(
            session
                .account_inventory(&context, Default::default())
                .await
                .unwrap()
                .accounts
                .len(),
            1
        );
        let tasks = session
            .operator_task_page(&context, Default::default())
            .await
            .unwrap();
        assert!(tasks.supported);
        assert_eq!(tasks.tasks.len(), 1);
        assert_eq!(
            session
                .apply_operator_task(&context, task)
                .await
                .unwrap()
                .revision,
            1
        );
        let commands = manager.saved_commands().await.unwrap();
        assert!(
            commands
                .iter()
                .any(|c| c.command_id == "pending_original" && c.reply.is_none())
        );
        assert_eq!(count(f.temp.path(), "owner_resource_adoptions"), 1);
        for table in [
            "resources",
            "owner_resource_grants",
            "owner_capacity_grants",
        ] {
            assert_eq!(count(f.temp.path(), table), 0);
        }
        // Preparing the requested account creates its empty identity seat;
        // setup itself created none, and neither action declares a quota.
        assert_eq!(count(f.temp.path(), "seats"), 1);
        let db = rusqlite::Connection::open_with_flags(
            f.temp.path().join("registry/domain.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let raw: String = db
            .query_row("SELECT config FROM seats", [], |r| r.get(0))
            .unwrap();
        assert!(
            serde_json::from_str::<hagency_core::project::Seat>(&raw)
                .unwrap()
                .declaration
                .is_none()
        );
        drop(db);
        assert_eq!(
            std::fs::read(f.owner.0.host.state_directory().join(DESCRIPTOR)).unwrap(),
            descriptor
        );
        assert!(backups == backup_bytes(&path.join("adoption_original_setup")));
        drop(session);
        drop(manager);
        drop(selection);
        f.stop().await;
        registry.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn empty_native_restart_recovers_after_registry_or_custody_commit() {
    for stopped_phase in [2, 3] {
        let (f, registry, work, source, init, path) = ready(false).await;
        let canonical = f.temp.path().join("registry/domain.sqlite3");
        let db = rusqlite::Connection::open(&canonical).unwrap();
        db.execute_batch(&format!("CREATE TRIGGER interrupt_activation BEFORE UPDATE ON owner_resource_adoptions WHEN NEW.phase={} BEGIN SELECT RAISE(ABORT,'test interrupted activation'); END;",stopped_phase+1)).unwrap();
        assert!(init.advance().await.is_err());
        let phase: i64 = db
            .query_row("SELECT phase FROM owner_resource_adoptions", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(phase, stopped_phase);
        db.execute_batch("DROP TRIGGER interrupt_activation")
            .unwrap();
        drop(db);
        let backups = backup_bytes(&path.join("adoption_original_setup"));
        init.drain_journal().await.unwrap();
        drop(init);
        drop(source);
        drop(work);
        let f = f
            .restart_for_adoption(registry.clone(), "original_setup".into())
            .await;
        let work = reopened_source(&f, registry.clone()).await;
        let source = work.close_and_drain().await.unwrap();
        let init = source
            .resume_empty_device_initialization("original_setup".into())
            .await
            .unwrap();
        assert_eq!(
            init.summary().await.unwrap().phase,
            if stopped_phase == 2 {
                "registry_ready"
            } else {
                "custody_ready"
            }
        );
        assert_eq!(init.advance().await.unwrap().phase, "active");
        let selection = init.finish_handoff().await.unwrap();
        drop(init);
        drop(source);
        drop(work);
        let f = f
            .restart_adopted(registry.clone(), "original_setup".into())
            .await;
        f.owner
            .adopted_management_selection(registry.clone())
            .await
            .unwrap();
        assert!(backups == backup_bytes(&path.join("adoption_original_setup")));
        assert_eq!(count(f.temp.path(), "owner_resource_adoptions"), 1);
        drop(selection);
        f.stop().await;
        registry.shutdown().await.unwrap();
    }
}
