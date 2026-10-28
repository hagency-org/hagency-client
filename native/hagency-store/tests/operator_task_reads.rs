use hagency_store::{DomainRepository, DomainStore, Error, management::*};
use serde_json::json;
use std::time::{Duration, Instant};
fn context(owner: &str) -> HostContext {
    HostContext::new(owner.into(), 1, "a".repeat(64), 1, "device".into(), 1).unwrap()
}
fn session(store: &DomainStore, c: &HostContext, permissions: &[Permission]) -> Session {
    Session::new(
        store.clone(),
        c.clone(),
        permissions.iter().copied(),
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}
fn bytes(path: &std::path::Path) -> (Vec<u8>, Option<Vec<u8>>) {
    (
        std::fs::read(path.join("domain.sqlite3")).unwrap(),
        std::fs::read(path.join("domain.sqlite3-wal")).ok(),
    )
}
#[tokio::test]
async fn operator_read_pages_preserve_filters_detail_comments_and_do_not_write() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let path = root.path().join("domain");
    let mut db = DomainRepository::open(&path).unwrap();
    let mut ids = Vec::new();
    for i in 0..18 {
        let t=db.create_operator_task(&json!({"title":format!("Task{i}"),"assignee":"Alpha","labels":["review"],"priority":"p1"}),1000+i).unwrap();
        ids.push(t.id);
    }
    let id = ids.last().unwrap().clone();
    for i in 0..5 {
        db.comment_operator_task(
            &id,
            &json!({"text":format!("comment{i}"),"author":"original author"}),
            2000 + i,
        )
        .unwrap();
    }
    let store = DomainStore::start(db, 8).unwrap();
    let c = context("@owner:test");
    let reader = session(&store, &c, &[Permission::ReadTasks]);
    let before = bytes(&path);
    let q = OperatorTaskQuery {
        assignee: Some("Alpha".into()),
        label: Some("review".into()),
        priority: Some("p1".into()),
        ..Default::default()
    };
    let page = reader.operator_task_page(&c, q.clone()).await.unwrap();
    assert_eq!(page.tasks.len(), 16);
    assert_eq!(page.tasks[0].id, id);
    assert_eq!(page.next_offset, Some(16));
    let rest = reader
        .operator_task_page(&c, OperatorTaskQuery { offset: 16, ..q })
        .await
        .unwrap();
    assert_eq!(rest.tasks.len(), 2);
    assert!(rest.next_offset.is_none());
    assert!(
        !rest
            .tasks
            .iter()
            .any(|b| page.tasks.iter().any(|a| a.id == b.id))
    );
    let mut texts = Vec::new();
    let mut offset = 0;
    loop {
        let d = reader
            .operator_task_detail(
                &c,
                OperatorTaskDetailQuery {
                    id: id.clone(),
                    comment_offset: offset,
                },
            )
            .await
            .unwrap();
        assert_eq!(d.comment_count, 5);
        assert_eq!(d.next, ["accepted"]);
        let t = d.task.unwrap();
        assert_eq!(t.labels, ["review"]);
        texts.extend(t.comments.into_iter().map(|c| c.text));
        if let Some(next) = d.next_comment_offset {
            offset = next
        } else {
            break;
        }
    }
    assert_eq!(
        texts,
        (0..5).map(|i| format!("comment{i}")).collect::<Vec<_>>()
    );
    assert_eq!(bytes(&path), before);
    store.shutdown().await.unwrap();
}
#[tokio::test]
async fn operator_read_authority_rejects_wrong_owner_missing_grant_and_retirement() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let db = DomainRepository::open(&root.path().join("domain")).unwrap();
    let store = DomainStore::start(db, 8).unwrap();
    let c = context("@owner:test");
    let denied = session(&store, &c, &[Permission::ReadResources]);
    assert!(matches!(
        denied.operator_task_page(&c, Default::default()).await,
        Err(Error::LocalAuthority)
    ));
    let reader = session(&store, &c, &[Permission::ReadTasks]);
    assert!(matches!(
        reader
            .operator_task_page(&context("@other:test"), Default::default())
            .await,
        Err(Error::LocalAuthority)
    ));
    assert!(matches!(
        reader.operator_board(&c, Default::default()).await,
        Err(Error::LocalAuthority)
    ));
    reader.retire();
    assert!(matches!(
        reader.operator_task_page(&c, Default::default()).await,
        Err(Error::LocalAuthority)
    ));
    store.shutdown().await.unwrap();
}
#[tokio::test]
async fn operator_read_board_preserves_retained_counts_and_unavailable_columns() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let path = root.path().join("domain");
    let mut db = DomainRepository::open(&path).unwrap();
    db.create_operator_task(&json!({"title":"Original","assignee":"agent"}), 1)
        .unwrap();
    let baseline = db.operator_project_board(10, 20).unwrap();
    let store = DomainStore::start(db, 8).unwrap();
    let c = context("@owner:test");
    let reader = session(&store, &c, &[Permission::ReadTaskBoard]);
    let before = bytes(&path);
    let board = reader.operator_board(&c, Default::default()).await.unwrap();
    assert_eq!(
        serde_json::to_value(board.task_totals).unwrap(),
        baseline["totals"]["tasks"]
    );
    assert_eq!(
        board.project_count,
        baseline["totals"]["projects"].as_u64().unwrap()
    );
    assert_eq!(
        serde_json::to_value(board.unavailable).unwrap(),
        baseline["unavailable"]
    );
    assert_eq!(bytes(&path), before);
    store.shutdown().await.unwrap();
}
#[test]
fn operator_read_inputs_reject_path_raw_command_and_unbounded_pages() {
    assert!(serde_json::from_value::<OperatorTaskQuery>(json!({"path":"foreign"})).is_err());
    assert!(
        serde_json::from_value::<OperatorTaskDetailQuery>(
            json!({"id":"original","command":"delete"})
        )
        .is_err()
    );
    assert!(
        OperatorTaskQuery {
            offset: 100001,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        OperatorTaskQuery {
            status: Some("running".into()),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        OperatorBoardQuery {
            agent_after: Some("agent".into()),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

#[tokio::test]
async fn operator_read_board_pages_projects_agents_and_preserves_original_name_association() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let path = root.path().join("domain");
    let mut db = DomainRepository::open(&path).unwrap();
    let raw = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    raw.execute(
        "INSERT INTO registrations VALUES('fleet_a',1,'{}'),('fleet_b',1,'{}')",
        [],
    )
    .unwrap();
    raw.execute("INSERT INTO resources VALUES('resource','preset','{}')", [])
        .unwrap();
    for i in 0..18 {
        let id = format!("project_{i:02}");
        raw.execute(
            "INSERT INTO projects VALUES('fleet_a',?1,1,?2,'@owner:test','!owner:test')",
            rusqlite::params![id, format!("!room{i}:test")],
        )
        .unwrap();
    }
    // The original board groups by project ID and agent name, even across
    // legacy fleets. Do not invent stricter task/project identity here.
    raw.execute("INSERT INTO projects VALUES('fleet_b','project_00',1,'!other:test','@owner:test','!owner:test')", []).unwrap();
    for i in 0..19 {
        raw.execute("INSERT INTO engagements(id,fleet_id,generation,request_id,digest,context,evidence,project_id,name,resource_id,tokens,state,projection) VALUES(?1,'fleet_a',1,?1,'digest','{}','{}','project_00',?2,'resource',1,'active','{}')",rusqlite::params![format!("engagement_{i}"),format!("Agent {i:02}")]).unwrap();
    }
    raw.execute("INSERT INTO engagements(id,fleet_id,generation,request_id,digest,context,evidence,project_id,name,resource_id,tokens,state,projection) VALUES('duplicate','fleet_b',1,'duplicate','digest','{}','{}','project_00','Agent 00','resource',1,'active','{}')",[]).unwrap();
    drop(raw);
    db.create_operator_task(
        &json!({"title":"Assigned original","assignee":"Agent 00"}),
        10,
    )
    .unwrap();
    db.create_operator_task(
        &json!({"title":"Unassigned original","assignee":"Elsewhere"}),
        11,
    )
    .unwrap();
    let original = db.operator_project_board(20, 20).unwrap();
    let store = DomainStore::start(db, 8).unwrap();
    let c = context("@owner:test");
    let s = session(&store, &c, &[Permission::ReadTaskBoard]);
    let before = bytes(&path);
    let page = s.operator_board(&c, Default::default()).await.unwrap();
    assert_eq!(page.project_count, 18);
    assert_eq!(page.agent_count, 19);
    assert_eq!(page.projects.len(), 16);
    assert_eq!(
        page.projects[0].agents,
        ["Agent 00", "Agent 01", "Agent 02", "Agent 03"]
    );
    assert_eq!(page.projects[0].agent_count, 19);
    assert_eq!(
        serde_json::to_value(&page.projects[0].task_lanes).unwrap(),
        original["projects"][0]["taskLanes"]
    );
    assert_eq!(page.projects[0].task_lanes["created"], 1);
    assert_eq!(
        serde_json::to_value(&page.task_totals).unwrap(),
        original["totals"]["tasks"]
    );
    let rest = s
        .operator_board(
            &c,
            OperatorBoardQuery {
                after: page.next_after,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        rest.projects
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["project_16", "project_17"]
    );
    assert!(rest.next_after.is_none());
    let agents = s
        .operator_board(
            &c,
            OperatorBoardQuery {
                project_id: Some("project_00".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(agents.projects[0].agents.len(), 16);
    let rest = s
        .operator_board(
            &c,
            OperatorBoardQuery {
                project_id: Some("project_00".into()),
                agent_after: agents.projects[0].next_agent_after.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        rest.projects[0].agents,
        ["Agent 16", "Agent 17", "Agent 18"]
    );
    assert!(rest.projects[0].next_agent_after.is_none());
    assert_eq!(bytes(&path), before);
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn operator_read_maximum_escaped_detail_is_bounded_without_losing_original_text() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("domain")).unwrap();
    let escaped = "\u{1}".repeat(4096);
    let title = "\u{1}".repeat(255);
    let t=db.create_operator_task(&json!({"title":title,"description":escaped,"labels":(0..20).map(|i|format!("{i}{}","\u{1}".repeat(62))).collect::<Vec<_>>()}),1).unwrap();
    for i in 0..3 {
        db.comment_operator_task(
            &t.id,
            &json!({"author":"original author","text":escaped}),
            2 + i,
        )
        .unwrap();
    }
    let store = DomainStore::start(db, 8).unwrap();
    let c = context("@owner:test");
    let s = session(&store, &c, &[Permission::ReadTasks]);
    let filtered = s
        .operator_task_page(
            &c,
            OperatorTaskQuery {
                label: Some(format!("0{}", "\u{1}".repeat(62))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        filtered.tasks.len(),
        1,
        "retained labels remain queryable without normalizing their text"
    );
    let page = s
        .operator_task_detail(
            &c,
            OperatorTaskDetailQuery {
                id: t.id,
                comment_offset: 0,
            },
        )
        .await
        .unwrap();
    assert!(serde_json::to_vec(&page).unwrap().len() < 120 * 1024);
    assert_eq!(page.comment_count, 3);
    assert_eq!(page.next_comment_offset, Some(2));
    let task = page.task.unwrap();
    assert_eq!(task.description, escaped);
    assert_eq!(task.comments.len(), 2);
    assert_eq!(task.comments[0].text, escaped);
    store.shutdown().await.unwrap();
}
