use super::*;
use crate::management::{OperatorTaskDetailQuery, OperatorTaskQuery};
fn source_bytes(f: &Fixture) -> Vec<Option<Vec<u8>>> {
    ["domain.sqlite3", "domain.sqlite3-wal", "domain.sqlite3-shm"]
        .iter()
        .map(|name| std::fs::read(f.source.join(name)).ok())
        .collect()
}
#[tokio::test]
async fn operator_read_adopted_history_is_explicitly_unavailable_without_source_access() {
    let f = fixture().await;
    let backing = active(&f).await;
    let c = context(&f);
    let s = Session::new_adopted(
        backing.clone(),
        c.clone(),
        [Permission::ReadTasks, Permission::ReadTaskBoard],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    let before = source_bytes(&f);
    let list = s
        .operator_task_page(&c, OperatorTaskQuery::default())
        .await
        .unwrap();
    assert!(!list.supported);
    assert_eq!(list.reason, Some("task_history_not_migrated"));
    let detail = s
        .operator_task_detail(
            &c,
            OperatorTaskDetailQuery {
                id: "task_original".into(),
                comment_offset: 0,
            },
        )
        .await
        .unwrap();
    assert!(!detail.supported);
    assert!(detail.task.is_none());
    let board = s.operator_board(&c, Default::default()).await.unwrap();
    assert!(!board.supported);
    assert_eq!(
        source_bytes(&f),
        before,
        "reads cannot reopen/migrate/checkpoint the original archive"
    );
    let other = HostContext::new(
        "@other:test".into(),
        1,
        "a".repeat(64),
        1,
        "name_device".into(),
        1,
    )
    .unwrap();
    assert!(matches!(
        s.operator_task_page(&other, Default::default()).await,
        Err(Error::LocalAuthority)
    ));
    s.retire();
    assert!(matches!(
        s.operator_board(&c, Default::default()).await,
        Err(Error::LocalAuthority)
    ));
    backing.shutdown().await.unwrap();
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn operator_history_is_not_silently_discarded_by_existing_resource_adoption() {
    let f = fixture().await;
    let mut db = DomainRepository::open_managed(&f.source, &f.profile.digest).unwrap();
    db.create_operator_task(
        &json!({"title":"Original task","assignee":"Original agent"}),
        1,
    )
    .unwrap();
    drop(db);
    assert!(matches!(inventory(&f), Err(Error::Quarantined)));
    f.registry.shutdown().await.unwrap();
}
