use super::*;
use crate::console::native::reconciliation_sources::tests::Fixture;
use crate::console::native::{ManagementPackage, SourceQuiescenceRequest};
use hagency_store::{
    management::{CommentOperatorTask, CreateOperatorTask, OperatorTaskDetailQuery, Permission},
    owner_resources::{OwnerResourceRegistry, VerifiedProviderAccount},
};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
fn package(instance: u64) -> ManagementPackage {
    ManagementPackage {
        digest: "a".repeat(64),
        instance,
        account_epoch: 1,
        permissions: vec![
            Permission::ReadResources,
            Permission::PrepareSourceQuiescence,
            Permission::ReadTasks,
            Permission::CreateTasks,
            Permission::EditTasks,
            Permission::DeleteTasks,
            Permission::AcceptTasks,
            Permission::TransitionTasks,
            Permission::CommentTasks,
            Permission::ReadIntents,
            Permission::AcknowledgeIntents,
        ],
    }
}
#[test]
fn native_task_writer_original_ids_restart_and_current_authority() {
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "console::native::operator_task_commands::tests::native_task_writer_process_child",
            "--exact",
            "--nocapture",
        ])
        .env("HAGENCY_TASK_WRITER_FIXTURE", "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
#[tokio::test]
async fn native_task_writer_process_child() {
    if std::env::var("HAGENCY_TASK_WRITER_FIXTURE").as_deref() != Ok("1") {
        return;
    }
    let f = Fixture::new().await;
    let (mut domain, path, resource) = f.domain();
    domain
        .put_seat(&hagency_core::project::Seat {
            id: resource.seat_id.clone(),
            declaration: None,
        })
        .unwrap();
    drop(domain);
    // Reconstruct an original schema83 source, preserving WAL custody exactly
    // like DomainRepository. The archive worker deliberately requires the
    // existing WAL sidecars and never manufactures missing recovery evidence.
    let source = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    source
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
    source.execute_batch("DROP TABLE IF EXISTS owner_operator_graph_recipients; DROP TABLE IF EXISTS owner_operator_graph_outbox; DROP TABLE IF EXISTS owner_operator_graph_commands; DROP TABLE IF EXISTS owner_operator_graphs; DROP TABLE IF EXISTS owner_operator_graph_archive_chunks; DROP TABLE IF EXISTS owner_operator_graph_archives; DROP TABLE IF EXISTS owned_dispatch_execution_policies; DROP TABLE IF EXISTS approval_binding_incarnations; DROP TRIGGER IF EXISTS owner_resource_provenance_immutable;DROP TRIGGER IF EXISTS owner_resource_provenance_retained;DROP TABLE IF EXISTS owner_resource_provenance;DROP TABLE IF EXISTS owner_namespace_enrollments;DROP TABLE IF EXISTS owner_resource_namespaces;DROP TABLE IF EXISTS management_account_profiles;DROP TABLE IF EXISTS owner_shared_budget_demand_decisions;DROP TABLE IF EXISTS owner_shared_budget_work_refusals;DROP TABLE IF EXISTS owner_shared_budget_start_intents;DROP TABLE IF EXISTS owner_managed_budget_deltas;DROP TABLE IF EXISTS owner_managed_budget_commands;DROP TABLE IF EXISTS owner_operator_task_transfer_comments;DROP TABLE IF EXISTS owner_operator_task_transfer_tasks;DROP TABLE IF EXISTS owner_shared_context_call_pins;DROP TABLE IF EXISTS owner_shared_reply_deliveries;DROP TABLE IF EXISTS owner_shared_contexts;DROP TABLE IF EXISTS owner_shared_input_modes;DROP TABLE IF EXISTS owner_shared_stop_receipts;DROP TABLE IF EXISTS owner_shared_stop_intents;DROP TABLE IF EXISTS owner_shared_demand_bindings;DROP TABLE IF EXISTS owner_shared_demand_decisions;DROP TABLE IF EXISTS owner_shared_demands;DROP TABLE IF EXISTS owner_shared_demand_controls;DROP TABLE owner_operator_task_comments;DROP TABLE owner_operator_tasks;DROP TABLE owner_operator_task_histories;PRAGMA user_version=83;").unwrap();
    drop(source);
    let (personal, _) = f.personal();
    drop(personal);
    let registry =
        OwnerResourceRegistry::isolated_for_tests(&f.temp.path().join("canonical")).unwrap();
    let mut request = SourceQuiescenceRequest::new(
        &f.owner,
        package(1),
        registry.clone(),
        Instant::now() + Duration::from_secs(50),
        || true,
    )
    .unwrap();
    request.lease_root =
        Some(hagency_platform::legacy_writer::test_support::initialize_process_root().unwrap());
    let work = f.owner.prepare_source_quiescence(request).await.unwrap();
    let sources = work.close_and_drain().await.unwrap();
    let provider = || {
        VerifiedProviderAccount::new(
            "codex",
            "https://provider.test",
            "synthetic-task-owner",
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap()
    };
    let transfer = sources
        .prepare_resource_adoption("tasks_original".into(), provider(), 700)
        .await
        .unwrap();
    assert_eq!(transfer.advance(provider()).await.unwrap().phase, "active");
    transfer.drain_journal().await.unwrap();
    drop(transfer);
    drop(sources);
    drop(work);
    let f = f
        .restart_adopted(registry.clone(), "tasks_original".into())
        .await;
    let manager = f.owner.management(package(2)).await.unwrap();
    let prepared = manager
        .prepare_operator_task(OperatorTaskChange::Create(CreateOperatorTask {
            title: "Native original".into(),
            ..Default::default()
        }))
        .await
        .unwrap();
    assert!(prepared.command_id.starts_with("taskcmd_") && prepared.task_id.starts_with("task_"));
    assert!(
        manager
            .operator_task_page(Default::default())
            .await
            .unwrap()
            .tasks
            .is_empty()
    );
    let saved = manager.saved_commands().await.unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].command_id, prepared.command_id);
    let original: OperatorTaskCommand = serde_json::from_value(saved[0].input.clone()).unwrap();
    // Real canonical commit, interrupted before the native intent reply is saved.
    let (context, session) = manager.authorized().await.unwrap();
    let committed = session
        .apply_operator_task(&context, original)
        .await
        .unwrap();
    assert!(
        manager
            .review_saved_operator_task(prepared.command_id.clone())
            .await
            .unwrap()
            .result
            .is_none()
    );
    manager.retire();
    assert!(
        manager
            .apply_saved_operator_task(prepared.command_id.clone())
            .await
            .is_err()
    );
    drop(session);
    drop(manager);
    let f = f
        .restart_adopted(registry.clone(), "tasks_original".into())
        .await;
    let manager = f.owner.management(package(3)).await.unwrap();
    assert_eq!(
        manager
            .apply_saved_operator_task(prepared.command_id.clone())
            .await
            .unwrap(),
        committed
    );
    assert_eq!(
        manager
            .operator_task_page(Default::default())
            .await
            .unwrap()
            .tasks
            .len(),
        1
    );
    let comment = manager
        .prepare_operator_task(OperatorTaskChange::Comment(CommentOperatorTask {
            id: prepared.task_id.clone(),
            expected_revision: 1,
            text: "Once only".into(),
        }))
        .await
        .unwrap();
    let first = manager
        .apply_saved_operator_task(comment.command_id.clone())
        .await
        .unwrap();
    assert_eq!(
        manager
            .apply_saved_operator_task(comment.command_id.clone())
            .await
            .unwrap(),
        first
    );
    let d = manager
        .operator_task_detail(OperatorTaskDetailQuery {
            id: prepared.task_id.clone(),
            comment_offset: 0,
        })
        .await
        .unwrap();
    assert_eq!(d.comment_count, 1);
    assert_eq!(d.task.unwrap().comments[0].author, "@owner:test");
    let mut read = package(4);
    read.permissions = vec![Permission::ReadTasks, Permission::ReadIntents];
    let readonly = f.owner.management(read).await.unwrap();
    assert!(
        readonly
            .apply_saved_operator_task(comment.command_id.clone())
            .await
            .is_err()
    );
    assert!(
        !readonly
            .operator_task_page(Default::default())
            .await
            .unwrap()
            .write_supported
    );
    f.live.store(false, Ordering::Release);
    assert!(
        manager
            .review_saved_operator_task(comment.command_id)
            .await
            .is_err()
    );
    assert!(
        manager
            .prepare_operator_task(OperatorTaskChange::Create(CreateOperatorTask {
                title: "Must not save".into(),
                ..Default::default()
            }))
            .await
            .is_err()
    );
    drop(readonly);
    drop(manager);
    f.stop().await;
    registry.shutdown().await.unwrap();
}
#[test]
fn native_task_writer_failure_classification_keeps_unknowns_pending() {
    for (status, code) in [
        (401, "management_access_required"),
        (409, "management_outcome_unknown"),
        (409, "management_revision_conflict"),
        (503, "management_unavailable"),
        (400, "unrelated_error"),
    ] {
        assert!(!known_failure(&failure(status, code)));
    }
    assert!(known_failure(&failure(
        409,
        "management_task_revision_changed"
    )));
}
