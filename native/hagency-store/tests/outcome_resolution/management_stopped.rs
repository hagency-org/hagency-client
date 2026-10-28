use super::*;
use hagency_store::{DomainStore, management::*};
use std::time::{Duration, Instant};

fn context(account: &str) -> HostContext {
    HostContext::new(
        account.into(),
        1,
        "a".repeat(64),
        1,
        "stopped_device".into(),
        1,
    )
    .unwrap()
}
fn session(store: &DomainStore, ctx: &HostContext) -> Session {
    Session::new(
        store.clone(),
        ctx.clone(),
        [
            Permission::ReadStoppedTasks,
            Permission::InspectStoppedTasks,
            Permission::ResolveStoppedTasks,
        ],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}
fn query(f: &Fixture) -> StoppedQuery {
    StoppedQuery {
        resource_id: resource("pool", "seat", 1000).id(),
        engagement_id: f.engagement.clone(),
        dispatch_id: "dispatch".into(),
        entry_offset: 0,
    }
}
fn list(q: &StoppedQuery) -> StoppedListQuery {
    StoppedListQuery {
        resource_id: q.resource_id.clone(),
        workspace_id: String::new(),
        after: String::new(),
    }
}
fn review(row: &StoppedInspection, id: &str) -> ReviewStopped {
    ReviewStopped {
        command_id: id.into(),
        resource_id: row.attempt.resource_id.clone(),
        engagement_id: row.attempt.engagement_id.clone(),
        dispatch_id: row.attempt.dispatch_id.clone(),
        expected_revision: row.revision.clone(),
    }
}
fn decision(row: &StoppedReview, action: StoppedDecision) -> ResolveStopped {
    ResolveStopped {
        command_id: "resolve_original".into(),
        resource_id: row.inspection.attempt.resource_id.clone(),
        engagement_id: row.inspection.attempt.engagement_id.clone(),
        dispatch_id: row.inspection.attempt.dispatch_id.clone(),
        review_command_id: row.review_command_id.clone(),
        review_revision: row.inspection.revision.clone(),
        action,
        operator_note: "Reviewed workspace and external effects".into(),
        continuation_instruction: (action == StoppedDecision::Continue)
            .then(|| "Finish remaining work only".into()),
    }
}

#[tokio::test]
async fn native_management_all_three_decisions_are_atomic_and_restart_idempotent() {
    for action in [
        StoppedDecision::Continue,
        StoppedDecision::AcceptCompleted,
        StoppedDecision::KeepBlocked,
    ] {
        let (f, _, _) = stopped_fixture();
        let q = query(&f);
        let sql = f.sql();
        let Fixture { root, db, .. } = f;
        let store = DomainStore::start(db, 16).unwrap();
        let ctx = context("@owner:example.test");
        let m = session(&store, &ctx);
        let page = m.stopped_tasks(&ctx, list(&q)).await.unwrap();
        assert_eq!(page.dispatches.len(), 1);
        let row = m.stopped_inspection(&ctx, q.clone()).await.unwrap();
        assert!(row.resolvable && row.can_continue);
        let inspect = review(&row, "review_original");
        let reviewed = m.review_stopped(&ctx, inspect.clone()).await.unwrap();
        assert_eq!(reviewed, m.review_stopped(&ctx, inspect).await.unwrap());
        let command = decision(&reviewed, action);
        let result = m.resolve_stopped(&ctx, command.clone()).await.unwrap();
        assert_eq!(
            result.task_status,
            match action {
                StoppedDecision::Continue => "in_progress",
                StoppedDecision::AcceptCompleted => "done",
                StoppedDecision::KeepBlocked => "blocked",
            }
        );
        assert!(
            m.stopped_tasks(&ctx, list(&q))
                .await
                .unwrap()
                .dispatches
                .is_empty()
        );
        assert_eq!(
            result,
            m.resolve_stopped(&ctx, command.clone()).await.unwrap()
        );
        assert_eq!(
            sql.query_row("SELECT COUNT(*) FROM outcome_resolutions", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            sql.query_row("SELECT COUNT(*) FROM management_commands", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            sql.query_row(
                "SELECT COUNT(*) FROM outcome_inspections WHERE consumed_at IS NOT NULL",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            1
        );
        if let Some(id) = &result.replacement_id {
            let value: String = sql
                .query_row(
                    "SELECT input FROM runner_dispatches WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            let next: DispatchInput = serde_json::from_str(&value).unwrap();
            assert_eq!(next.session_id, "session");
            assert_eq!(next.task_id.as_deref(), Some("task"));
            assert_eq!(next.resources[0].id, "workspace");
            assert!(next.resources[0].exclusive);
            assert_eq!(next.payload["instruction"], "Finish remaining work only");
            assert!(next.payload.get("inbox").is_none());
            assert!(
                next.payload["recoveryInbox"]
                    .as_array()
                    .is_some_and(|rows| rows.len() == 1)
            );
            assert_eq!(
                sql.query_row(
                    "SELECT COUNT(*) FROM dispatch_inputs WHERE dispatch_id=?1",
                    [id],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
                1
            );
        }
        let leaked: String = sql
            .query_row(
                "SELECT group_concat(result) FROM management_commands",
                [],
                |r| r.get(0),
            )
            .unwrap();
        for forbidden in [
            "inspectionToken",
            "token_hash",
            "capability_hash",
            "root",
            "scope",
        ] {
            assert!(!leaked.contains(forbidden), "{forbidden}");
        }
        let mut changed = command.clone();
        changed.operator_note = "different decision".into();
        assert!(matches!(
            m.resolve_stopped(&ctx, changed).await,
            Err(Error::Conflict)
        ));
        let mut duplicate = command.clone();
        duplicate.command_id = "different_command".into();
        assert!(matches!(
            m.resolve_stopped(&ctx, duplicate).await,
            Err(Error::Conflict)
        ));
        m.retire();
        store.shutdown().await.unwrap();
        let store = DomainStore::start(
            DomainRepository::open(&root.path().join("state")).unwrap(),
            16,
        )
        .unwrap();
        let m = session(&store, &ctx);
        assert_eq!(result, m.resolve_stopped(&ctx, command).await.unwrap());
        store.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn native_management_review_is_bound_to_current_snapshot_and_original_actor() {
    let (f, _, _) = stopped_fixture();
    let q = query(&f);
    let sql = f.sql();
    let Fixture {
        root: _root, db, ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context("@owner:example.test");
    let m = session(&store, &ctx);
    let row = m.stopped_inspection(&ctx, q.clone()).await.unwrap();
    let reviewed = m
        .review_stopped(&ctx, review(&row, "review_original"))
        .await
        .unwrap();
    let command = decision(&reviewed, StoppedDecision::AcceptCompleted);
    let foreign = context("@other:example.test");
    let other = session(&store, &foreign);
    assert!(matches!(
        other.resolve_stopped(&foreign, command.clone()).await,
        Err(Error::StoppedNotResolvable)
    ));
    let reader = Session::new(
        store.clone(),
        ctx.clone(),
        [Permission::ReadStoppedTasks],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    assert!(matches!(
        reader.review_stopped(&ctx, review(&row, "forbidden")).await,
        Err(Error::LocalAuthority)
    ));
    assert!(matches!(
        reader.resolve_stopped(&ctx, command.clone()).await,
        Err(Error::LocalAuthority)
    ));
    let mut wrong = q.clone();
    wrong.engagement_id = "unrelated".into();
    assert!(matches!(
        m.stopped_inspection(&ctx, wrong).await,
        Err(Error::NotFound)
    ));
    let mut wrong = q.clone();
    wrong.resource_id = "unrelated".into();
    assert!(matches!(
        m.stopped_inspection(&ctx, wrong).await,
        Err(Error::NotFound)
    ));
    sql.execute(
        "UPDATE canonical_tasks SET config=json_set(config,'$.title','New task review')",
        [],
    )
    .unwrap();
    assert!(matches!(
        m.resolve_stopped(&ctx, command).await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        m.review_stopped(&ctx, review(&row, "stale_review")).await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        sql.query_row("SELECT COUNT(*) FROM outcome_resolutions", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_management_review_and_resolution_roll_back_without_a_receipt() {
    for stage in ["stopped.inspect", "stopped.resolve"] {
        let (f, _, _) = stopped_fixture();
        let q = query(&f);
        let sql = f.sql();
        let Fixture {
            root: _root, db, ..
        } = f;
        let store = DomainStore::start(db, 16).unwrap();
        let ctx = context("@owner:example.test");
        let m = session(&store, &ctx);
        let row = m.stopped_inspection(&ctx, q.clone()).await.unwrap();
        let inspected = if stage == "stopped.resolve" {
            Some(
                m.review_stopped(&ctx, review(&row, "review"))
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        sql.execute_batch(&format!("CREATE TRIGGER fail_receipt BEFORE INSERT ON management_commands WHEN NEW.operation='{stage}' BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;")).unwrap();
        if let Some(r) = inspected {
            assert!(
                m.resolve_stopped(&ctx, decision(&r, StoppedDecision::AcceptCompleted))
                    .await
                    .is_err()
            );
        } else {
            assert!(
                m.review_stopped(&ctx, review(&row, "review"))
                    .await
                    .is_err()
            );
        }
        assert_eq!(
            sql.query_row("SELECT COUNT(*) FROM outcome_resolutions", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            sql.query_row(
                "SELECT COUNT(*) FROM outcome_inspections WHERE consumed_at IS NOT NULL",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            sql.query_row("SELECT COUNT(*) FROM outcome_inspections", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            u64::from(stage == "stopped.resolve")
        );
        assert_eq!(
            sql.query_row(
                "SELECT COUNT(*) FROM dispatch_stops WHERE settled_at IS NOT NULL",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        assert!(m.stopped_inspection(&ctx, q).await.unwrap().resolvable);
        store.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn native_management_inventory_is_bounded_and_projects_no_host_roots_or_symlink_targets() {
    let mut f = Fixture::configured(true, None, "dispatch", true);
    f.db.observe_owned_failure(&f.cap, OwnedFailure::Protocol, 1007)
        .unwrap();
    let mut entries = vec![json!({"path":"shortcut","kind":"symlink","target":"/private/secret"})];
    for i in 0..20 {
        entries.push(json!({"path":format!("folder/{i:02}.txt"),"kind":"file","bytes":2,"sha256":"a".repeat(64)}));
    }
    f.db.record_owned_stop_inspection(&f.cap,&f.started,&json!({"profile":"stopped-content-inventory-v1","root":{"private":"host-secret"},"entries":entries}),1008).unwrap();
    let q = query(&f);
    let Fixture {
        root: _root, db, ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context("@owner:example.test");
    let m = session(&store, &ctx);
    let first = m.stopped_inspection(&ctx, q.clone()).await.unwrap();
    assert_eq!(first.entry_count, 21);
    assert_eq!(first.entries.len(), 8);
    assert_eq!(first.next_entry, Some(8));
    let data = serde_json::to_string(&first).unwrap();
    assert!(!data.contains("host-secret"));
    assert!(!data.contains("/private/secret"));
    let mut next = q.clone();
    next.entry_offset = 8;
    let second = m.stopped_inspection(&ctx, next.clone()).await.unwrap();
    assert_eq!(second.revision, first.revision);
    assert_eq!(second.entries[0].relative_path, "folder/07.txt");
    next.entry_offset = 16;
    let last = m.stopped_inspection(&ctx, next.clone()).await.unwrap();
    assert_eq!(last.entries.len(), 5);
    assert_eq!(last.next_entry, None);
    next.entry_offset = 22;
    assert!(matches!(
        m.stopped_inspection(&ctx, next).await,
        Err(Error::Capacity)
    ));
    let r = m
        .review_stopped(&ctx, review(&first, "review"))
        .await
        .unwrap();
    assert!(serde_json::to_vec(&r).unwrap().len() < 64 * 1024);
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_management_expired_review_cannot_settle_a_task() {
    let (f, _, _) = stopped_fixture();
    let q = query(&f);
    let sql = f.sql();
    let Fixture {
        root: _root, db, ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context("@owner:example.test");
    let m = session(&store, &ctx);
    let row = m.stopped_inspection(&ctx, q).await.unwrap();
    let r = m
        .review_stopped(&ctx, review(&row, "review"))
        .await
        .unwrap();
    sql.execute(
        "UPDATE outcome_inspections SET created_at=1,expires_at=2",
        [],
    )
    .unwrap();
    assert!(matches!(
        m.resolve_stopped(&ctx, decision(&r, StoppedDecision::KeepBlocked))
            .await,
        Err(Error::StoppedNotResolvable)
    ));
    assert_eq!(
        sql.query_row("SELECT COUNT(*) FROM outcome_resolutions", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    store.shutdown().await.unwrap();
}
