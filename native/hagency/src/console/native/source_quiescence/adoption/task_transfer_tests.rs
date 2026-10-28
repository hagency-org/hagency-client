use super::*;
use crate::console::native::OperatorTaskChange;
use hagency_store::management::{CommentOperatorTask, OperatorTaskDetailQuery};
use serde_json::json;

#[test]
fn native_task_transfer_v2_original_fence_restart_and_current_authority() {
    let result=std::process::Command::new(std::env::current_exe().unwrap())
        .args(["console::native::source_quiescence::adoption::tests::task_transfer_tests::native_task_transfer_process_child","--exact","--nocapture"])
        .env("HAGENCY_TASK_TRANSFER_V2_FIXTURE","1").output().unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
#[tokio::test]
async fn native_task_transfer_process_child() {
    if std::env::var("HAGENCY_TASK_TRANSFER_V2_FIXTURE").as_deref() != Ok("1") {
        return;
    }
    let f = Fixture::new().await;
    let (mut db, source, resource) = f.domain();
    db.put_seat(&Seat {
        id: resource.seat_id.clone(),
        declaration: None,
    })
    .unwrap();
    let original=db.create_operator_task(&json!({"title":"Retained task 原文","description":"Original description","assignee":"Historical agent","created_by":"Historical author","labels":["kept"]}),1000).unwrap();
    let original = db
        .comment_operator_task(
            &original.id,
            &json!({"text":"Original comment","author":"Historical commenter"}),
            2000,
        )
        .unwrap();
    drop(db);
    let (ledger, personal) = f.personal();
    drop(ledger);
    let registry =
        OwnerResourceRegistry::isolated_for_tests(&f.temp.path().join("canonical")).unwrap();
    let (work, sources) = quiesce(&f, &registry).await;
    // The old v1 path still refuses this history; it cannot silently adopt it.
    assert!(
        sources
            .prepare_resource_adoption("legacy_refusal".into(), provider(), 700)
            .await
            .is_err()
    );
    let transfer = sources
        .prepare_resource_adoption_with_operator_tasks(
            "populated_tasks_original".into(),
            provider(),
            700,
        )
        .await
        .unwrap();
    let prepared = transfer.status().await.unwrap();
    assert_eq!(prepared.phase, "prepared");
    assert_eq!(prepared.transferred_task_count, Some(1));
    assert_eq!(prepared.transferred_comment_count, Some(1));
    // Real retained source fence, interrupted before canonical acknowledgement.
    let proof = {
        let mut files = sources.files.lock().unwrap();
        files
            .fence(
                &transfer.transfer,
                registry.id(),
                &transfer.binding,
                &sources.gate,
                &sources.lease,
            )
            .unwrap()
    };
    drop(proof);
    assert_eq!(transfer.status().await.unwrap().phase, "prepared");
    drop(transfer);
    drop(sources);
    drop(work);
    assert!(hagency_store::DomainRepository::open(&source).is_err());
    assert!(hagency_agent_local::Ledger::open(&personal, "@owner:test").is_err());
    let f = f
        .restart_for_adoption(registry.clone(), "populated_tasks_original".into())
        .await;
    let (work, sources) = quiesce(&f, &registry).await;
    assert!(
        sources
            .prepare_resource_adoption_with_operator_tasks("replacement".into(), provider(), 700)
            .await
            .is_err()
    );
    let transfer = sources
        .resume_resource_adoption("populated_tasks_original".into())
        .await
        .unwrap();
    assert_eq!(transfer.advance(provider()).await.unwrap().phase, "active");
    transfer.drain_journal().await.unwrap();
    drop(transfer);
    drop(sources);
    drop(work);
    let f = f
        .restart_adopted(registry.clone(), "populated_tasks_original".into())
        .await;
    let mut p = package();
    p.instance = 2;
    p.permissions.extend([
        Permission::ReadTasks,
        Permission::ReadTaskBoard,
        Permission::CommentTasks,
    ]);
    let manager = f.owner.management(p).await.unwrap();
    let detail = manager
        .operator_task_detail(OperatorTaskDetailQuery {
            id: original.id.clone(),
            comment_offset: 0,
        })
        .await
        .unwrap();
    assert_eq!(detail.revision, Some(1));
    assert_eq!(detail.task.unwrap(), original);
    let pending = manager
        .prepare_operator_task(OperatorTaskChange::Comment(CommentOperatorTask {
            id: original.id.clone(),
            expected_revision: 1,
            text: "Current owner".into(),
        }))
        .await
        .unwrap();
    let first = manager
        .apply_saved_operator_task(pending.command_id.clone())
        .await
        .unwrap();
    assert_eq!(first.revision, 2);
    assert_eq!(
        manager
            .apply_saved_operator_task(pending.command_id)
            .await
            .unwrap(),
        first
    );
    drop(manager);
    let f = f
        .restart_adopted(registry.clone(), "populated_tasks_original".into())
        .await;
    let mut p = package();
    p.permissions.extend([
        Permission::ReadTasks,
        Permission::ReadTaskBoard,
        Permission::CommentTasks,
    ]);
    p.instance = 3;
    let manager = f.owner.management(p).await.unwrap();
    let detail = manager
        .operator_task_detail(OperatorTaskDetailQuery {
            id: original.id,
            comment_offset: 0,
        })
        .await
        .unwrap();
    assert_eq!(detail.revision, Some(2));
    let task = detail.task.unwrap();
    assert_eq!(task.comments.len(), 2);
    assert_eq!(task.comments[0].author, "Historical commenter");
    assert_eq!(task.comments[1].author, "@owner:test");
    f.live.store(false, Ordering::Release);
    assert!(
        manager
            .operator_task_page(Default::default())
            .await
            .is_err()
    );
    drop(manager);
    f.stop().await;
    registry.shutdown().await.unwrap();
}

#[test]
fn native_task_transfer_v2_prepare_crash_boundaries_and_changed_source_device_denial() {
    for case in [
        "before_commit",
        "after_commit",
        "changed_source",
        "wrong_device",
    ] {
        let result=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["console::native::source_quiescence::adoption::tests::task_transfer_tests::native_task_prepare_crash_child","--exact","--nocapture"])
            .env("HAGENCY_TASK_PREPARE_CRASH_FIXTURE",case).output().unwrap();
        assert!(
            result.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
#[tokio::test]
async fn native_task_prepare_crash_child() {
    let Ok(case) = std::env::var("HAGENCY_TASK_PREPARE_CRASH_FIXTURE") else {
        return;
    };
    let f = Fixture::new().await;
    let (mut db, path, resource) = f.domain();
    db.put_seat(&Seat {
        id: resource.seat_id.clone(),
        declaration: None,
    })
    .unwrap();
    let original = db
        .create_operator_task(
            &json!({"title":"Original saved task","created_by":"Previous author"}),
            1000,
        )
        .unwrap();
    drop(db);
    let (personal, _) = f.personal();
    drop(personal);
    let registry =
        OwnerResourceRegistry::isolated_for_tests(&f.temp.path().join("canonical")).unwrap();
    let (work, sources) = quiesce(&f, &registry).await;
    let state = f.owner.0.host.state_directory().to_owned();
    let descriptor = state.join("adopted-management-initializer-v1.json");
    {
        let staged = sources
            .stage_task_adoption("prepare_original".into(), provider(), 700, None)
            .await
            .unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&descriptor).unwrap()).unwrap();
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["migrationId"], "prepare_original");
        assert!(saved["taskPrepare"]["planDigest"].as_str().unwrap().len() == 64);
        if case == "after_commit" {
            registry
                .commit_resource_adoption_preparation(staged.prepared)
                .await
                .unwrap();
            // Drop the native return path here: no returned handle or caller
            // acknowledgement is required to retain the original descriptor.
        }
    }
    let original_descriptor = std::fs::read(&descriptor).unwrap();
    assert!(
        sources
            .prepare_resource_adoption_with_operator_tasks("replacement".into(), provider(), 700)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&descriptor).unwrap(), original_descriptor);
    assert!(
        sources
            .prepare_resource_adoption("legacy_bypass".into(), provider(), 700)
            .await
            .is_err()
    );
    if case == "changed_source" {
        let db = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
        db.execute(
            "UPDATE operator_tasks SET title='Changed after saved preparation' WHERE id=?1",
            [&original.id],
        )
        .unwrap();
    }
    if case == "wrong_device" {
        let mut wrong: serde_json::Value = serde_json::from_slice(&original_descriptor).unwrap();
        wrong["originalDevice"] = json!("former-device");
        hagency_store::private::replace(&descriptor, &serde_json::to_vec(&wrong).unwrap()).unwrap();
        assert!(
            sources
                .resume_resource_adoption_with_operator_tasks("prepare_original".into(), provider())
                .await
                .is_err()
        );
        // The current real SDK source was accepted for the saved original
        // device; changing that retained binding does not permit a new claim.
        hagency_store::private::replace(&descriptor, &original_descriptor).unwrap();
    }
    if case == "changed_source" {
        assert!(
            sources
                .resume_resource_adoption_with_operator_tasks("prepare_original".into(), provider())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&descriptor).unwrap(), original_descriptor);
        let db = rusqlite::Connection::open_with_flags(
            f.temp.path().join("canonical/domain.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM owner_resource_adoptions", [], |r| r
                .get::<_, u64>(
                0
            ))
            .unwrap(),
            0
        );
        drop(db);
        drop(sources);
        drop(work);
        f.stop().await;
        registry.shutdown().await.unwrap();
        return;
    }
    drop(sources);
    drop(work);
    let f = f
        .restart_for_adoption(registry.clone(), "prepare_original".into())
        .await;
    let (work, sources) = quiesce(&f, &registry).await;
    let recovered = sources
        .resume_resource_adoption_with_operator_tasks("prepare_original".into(), provider())
        .await;
    let transfer = recovered.unwrap();
    assert_eq!(transfer.status().await.unwrap().phase, "prepared");
    assert_eq!(std::fs::read(&descriptor).unwrap(), original_descriptor);
    assert_eq!(transfer.advance(provider()).await.unwrap().phase, "active");
    transfer.drain_journal().await.unwrap();
    drop(transfer);
    drop(sources);
    drop(work);
    let f = f
        .restart_adopted(registry.clone(), "prepare_original".into())
        .await;
    let mut p = package();
    p.permissions.push(Permission::ReadTasks);
    let manager = f.owner.management(p).await.unwrap();
    assert_eq!(
        manager
            .operator_task_detail(OperatorTaskDetailQuery {
                id: original.id.clone(),
                comment_offset: 0
            })
            .await
            .unwrap()
            .task
            .unwrap(),
        original
    );
    drop(manager);
    f.stop().await;
    registry.shutdown().await.unwrap();
}
