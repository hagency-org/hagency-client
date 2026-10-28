use super::*;
use crate::console::native::AdoptedManagementSelection;
use serde_json::json;
#[test]
fn native_task_wizard_saved_versions_and_prepare_loss_keep_exact_original() {
    for case in [
        "v1",
        "v2_before",
        "v2_after",
        "v2_without_preview",
        "v2_old_changed",
        "v3_after",
        "v3_fenced",
    ] {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["console::native::source_quiescence::adoption::tests::task_wizard_tests::native_task_wizard_process_child", "--exact", "--nocapture"])
            .env("HAGENCY_TASK_WIZARD_FIXTURE", case).output().unwrap();
        assert!(
            result.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
#[tokio::test]
async fn native_task_wizard_process_child() {
    let Ok(case) = std::env::var("HAGENCY_TASK_WIZARD_FIXTURE") else {
        return;
    };
    let f = Fixture::new().await;
    let (mut db, source_path, resource) = f.domain();
    db.put_seat(&Seat {
        id: resource.seat_id.clone(),
        declaration: None,
    })
    .unwrap();
    if case != "v1" {
        let task = db
            .create_operator_task(
                &json!({"title":"Original task", "created_by":"Original author"}),
                1000,
            )
            .unwrap();
        db.comment_operator_task(
            &task.id,
            &json!({"text":"Original comment", "author":"Original commenter"}),
            2000,
        )
        .unwrap();
    }
    drop(db);
    let alert_history = case.starts_with("v3_");
    let reviewed_history = if alert_history {
        let db = rusqlite::Connection::open(source_path.join("domain.sqlite3")).unwrap();
        db.execute("INSERT INTO ceiling_alerts(dedupe_key,resource_id,summary,detail,runbook,impact,recovery_condition,occurrences,first_seen_ms,last_seen_ms) VALUES('original-alert',?1,'Original alert','{truncated detail','Runbook','Impact','Recovery',1,100,200)", [&resource.id()]).unwrap();
        db.execute("INSERT INTO ceiling_alert_notes VALUES('original-alert',7,'Original author','Original note',150)", []).unwrap();
        drop(db);
        let manager = f.owner.management(package()).await.unwrap();
        let page = manager.resources(Default::default()).await.unwrap();
        let revisions = page
            .resources
            .iter()
            .map(|r| (r.id.clone(), r.revision.clone()))
            .collect::<Vec<_>>();
        let (context, session) = manager.authorized().await.unwrap();
        let preview = session
            .resource_adoption_history_preview(&context)
            .await
            .unwrap();
        assert_eq!(preview.alert_count(), Some(1));
        assert_eq!(preview.alert_note_count(), Some(1));
        manager.retire();
        Some((revisions, preview))
    } else {
        None
    };
    drop(f.personal().0);
    let registry =
        OwnerResourceRegistry::isolated_for_tests(&f.temp.path().join("canonical")).unwrap();
    let state = f.owner.0.host.state_directory().to_owned();
    let (work, sources) = quiesce(&f, &registry).await;
    if case == "v1" {
        drop(
            sources
                .prepare_resource_adoption("original_wizard".into(), provider(), 700)
                .await
                .unwrap(),
        );
    } else if let Some((revisions, preview)) = reviewed_history {
        let transfer = sources
            .prepare_reviewed_task_adoption(
                "original_wizard".into(),
                provider(),
                700,
                &revisions,
                &preview,
            )
            .await
            .unwrap();
        assert_eq!(
            transfer.status().await.unwrap().transferred_alert_count,
            Some(1)
        );
        if case == "v3_fenced" {
            // Physical fence completed; canonical acknowledgement was interrupted.
            let proof = sources
                .files
                .lock()
                .unwrap()
                .fence(
                    &transfer.transfer,
                    registry.id(),
                    &transfer.binding,
                    &sources.gate,
                    &sources.lease,
                )
                .unwrap();
            drop(proof);
        }
        drop(transfer);
    } else {
        let staged = sources
            .stage_task_adoption("original_wizard".into(), provider(), 700, None)
            .await
            .unwrap();
        if case == "v2_after" {
            registry
                .commit_resource_adoption_preparation(staged.prepared)
                .await
                .unwrap();
        }
    }
    drop(sources);
    drop(work);
    let descriptor = state.join("adopted-management-initializer-v1.json");
    if case == "v2_without_preview" || case == "v2_old_changed" {
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&descriptor).unwrap()).unwrap();
        value["taskPrepare"]
            .as_object_mut()
            .unwrap()
            .remove("review");
        hagency_store::private::replace(&descriptor, &serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let original = std::fs::read(&descriptor).unwrap();
    let mut f = f
        .restart_for_adoption(registry.clone(), "original_wizard".into())
        .await;
    if case == "v2_old_changed" {
        let db = rusqlite::Connection::open(source_path.join("domain.sqlite3")).unwrap();
        db.execute(
            "UPDATE operator_tasks SET title='Changed original history'",
            [],
        )
        .unwrap();
    }
    let selected = AdoptedManagementSelection::load_for_isolated_tests(
        &state,
        &f.owner.0.expected_origin,
        "@owner:test",
        registry.clone(),
    )
    .unwrap()
    .unwrap();
    let result = f
        .owner
        .resource_activation_context(package(), Some(&selected))
        .await;
    if case == "v2_old_changed" {
        // The existing source-only SDK recovery gate revalidates the saved
        // inventory before this descriptive preview can be returned.
        assert_eq!(result.err().unwrap().code, "source_recovery_required");
    } else {
        let context = result.unwrap();
        assert_eq!(
            context.original_migration.as_deref(),
            Some("original_wizard")
        );
        assert_eq!(context.original_monthly_tokens, Some(700));
        assert_eq!(
            context.task_count,
            if case == "v1" { None } else { Some(1) }
        );
        assert_eq!(
            context.comment_count,
            if case == "v1" { None } else { Some(1) }
        );
        assert_eq!(
            context.alert_count,
            if alert_history { Some(1) } else { None }
        );
        assert_eq!(
            context.alert_note_count,
            if alert_history { Some(1) } else { None }
        );
        assert!(
            f.owner
                .review_resource_activation(package(), Some(selected.clone()), 701)
                .await
                .is_err()
        );
        let review = f
            .owner
            .review_resource_activation(package(), Some(selected), 700)
            .await
            .unwrap();
        assert!(review.recovering());
        // Closing review has no effect; continue through the same native opaque
        // review to exercise saved version dispatch and exact migration identity.
        let selected = f
            .owner
            .confirm_resource_activation_fixture(review, package(), provider())
            .await
            .unwrap();
        assert!(selected.matches_host(&state, &f.owner.0.expected_origin, "@owner:test"));
        drop(selected);
        f = f
            .restart_adopted(registry.clone(), "original_wizard".into())
            .await;
        if case != "v1" {
            let mut p = package();
            p.permissions.push(Permission::ReadTasks);
            let manager = f.owner.management(p).await.unwrap();
            let page = manager
                .operator_task_page(Default::default())
                .await
                .unwrap();
            assert!(page.supported);
            assert_eq!(page.tasks.len(), 1);
            assert_eq!(page.tasks[0].title, "Original task");
            drop(manager);
        }
        if alert_history {
            let mut p = package();
            p.permissions.push(Permission::ReadAlerts);
            let manager = f.owner.management(p).await.unwrap();
            let value = manager
                .resource_alert(hagency_store::management::ResourceAlertDetailQuery {
                    resource_id: resource.id(),
                    id: "original-alert".into(),
                    after_note: 0,
                })
                .await
                .unwrap();
            assert_eq!(value.alert.summary, "Original alert");
            assert_eq!(value.notes.len(), 1);
            assert_eq!(value.notes[0].text, "Original note");
            drop(manager);
        }
    }
    assert_eq!(std::fs::read(&descriptor).unwrap(), original);
    f.stop().await;
    registry.shutdown().await.unwrap();
}
