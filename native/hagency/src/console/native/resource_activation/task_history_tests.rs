use super::*;
use super::tests::{package, provider};
use crate::console::native::reconciliation_sources::tests::Fixture;
use serde_json::json;
#[tokio::test]
async fn activation_task_preview_counts_and_same_count_change_refuse_before_retirement() {
    let f = Fixture::new().await;
    let (mut db, path, resource) = f.domain();
    db.put_seat(&hagency_core::project::Seat {
        id: resource.seat_id.clone(),
        declaration: None,
    })
    .unwrap();
    let original = db
        .create_operator_task(
            &json!({"title":"Original task", "created_by":"Historical author"}),
            1000,
        )
        .unwrap();
    db.comment_operator_task(
        &original.id,
        &json!({"text":"Retained comment", "author":"Historical commenter"}),
        2000,
    )
    .unwrap();
    drop(db);
    let review = f
        .owner
        .review_resource_activation(package(), None, 700)
        .await
        .unwrap();
    assert_eq!(review.context().task_count, Some(1));
    assert_eq!(review.context().comment_count, Some(1));
    // Simulate an original local edit with unchanged counts; digest must notice.
    let db = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    db.execute(
        "UPDATE operator_tasks SET title='Changed after review' WHERE id=?1",
        [&original.id],
    )
    .unwrap();
    drop(db);
    let result = f
        .owner
        .confirm_resource_activation_fixture(review, package(), provider())
        .await;
    assert_eq!(result.err().unwrap().code, "source_adoption_review_changed");
    assert!(
        !f.owner
            .0
            .configuration_stopped
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(
        !f.owner
            .0
            .host
            .state_directory()
            .join("adopted-management-initializer-v1.json")
            .exists()
    );
    f.stop().await;
}
#[tokio::test]
async fn activation_task_preview_requires_native_permission_and_keeps_source_read_only() {
    let f = Fixture::new().await;
    let (mut db, path, _) = f.domain();
    db.create_operator_task(&json!({"title":"Keep original"}), 1000)
        .unwrap();
    drop(db);
    let bytes = std::fs::read(path.join("domain.sqlite3")).unwrap();
    let mut p = package();
    p.permissions
        .retain(|p| *p != Permission::PrepareSourceQuiescence);
    assert!(f.owner.resource_activation_context(p, None).await.is_err());
    assert_eq!(std::fs::read(path.join("domain.sqlite3")).unwrap(), bytes);
    let context = f
        .owner
        .resource_activation_context(package(), None)
        .await
        .unwrap();
    assert_eq!(context.task_count, Some(1));
    assert_eq!(context.comment_count, Some(0));
    assert_eq!(std::fs::read(path.join("domain.sqlite3")).unwrap(), bytes);
    assert!(
        !f.owner
            .0
            .configuration_stopped
            .load(std::sync::atomic::Ordering::Acquire)
    );
    f.stop().await;
}

#[tokio::test]
async fn activation_alert_preview_preserves_scope_and_rejects_same_count_note_changes() {
    let f = Fixture::new().await;
    let (mut db, path, resource) = f.domain();
    db.put_seat(&hagency_core::project::Seat {
        id: resource.seat_id.clone(),
        declaration: None,
    })
    .unwrap();
    drop(db);
    let source = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    source.execute("INSERT INTO ceiling_alerts(dedupe_key,resource_id,summary,detail,runbook,impact,recovery_condition,occurrences,first_seen_ms,last_seen_ms) VALUES('original-alert',?1,'Original alert','{truncated detail','Runbook','Impact','Recovery',1,100,200)", [&resource.id()]).unwrap();
    source.execute("INSERT INTO ceiling_alert_notes VALUES('original-alert',7,'Original author','Original note',150)", []).unwrap();
    drop(source);
    let review = f
        .owner
        .review_resource_activation(package(), None, 700)
        .await
        .unwrap();
    assert_eq!(review.context().alert_count, Some(1));
    assert_eq!(review.context().alert_note_count, Some(1));
    assert_eq!(review.context().task_count, Some(0));
    let source = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    source.execute("UPDATE ceiling_alert_notes SET text='Changed note' WHERE dedupe_key='original-alert' AND seq=7", []).unwrap();
    drop(source);
    let error = f
        .owner
        .confirm_resource_activation_fixture(review, package(), provider())
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "source_adoption_review_changed");
    assert!(
        !f.owner
            .0
            .configuration_stopped
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(
        !f.owner
            .0
            .host
            .state_directory()
            .join("adopted-management-initializer-v1.json")
            .exists()
    );
    f.stop().await;
}
