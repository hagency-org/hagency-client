use super::*;
use hagency_store::management::{
    ClearWorkspaceDirty, HostContext, Permission, Session, WorkspaceListQuery, WorkspaceQuery,
    WorkspaceReleaseBlock, WorkspaceStatus,
};
use std::time::{Duration, Instant};

fn context() -> HostContext {
    HostContext::new(
        "@owner:example.test".into(),
        1,
        "a".repeat(64),
        1,
        "workspace_device".into(),
        1,
    )
    .unwrap()
}
fn session(store: &DomainStore, ctx: &HostContext) -> Session {
    Session::new(
        store.clone(),
        ctx.clone(),
        [Permission::ReadWorkspaces, Permission::ClearWorkspaceDirty],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}
fn query() -> WorkspaceQuery {
    WorkspaceQuery {
        resource_id: resource("pool", "seat", 1000).id(),
        workspace_id: "workspace_a".into(),
    }
}
fn clear(row: &WorkspaceStatus, command: &str) -> ClearWorkspaceDirty {
    ClearWorkspaceDirty {
        command_id: command.into(),
        resource_id: row.resource_id.clone(),
        workspace_id: row.workspace_id.clone(),
        expected_revision: row.revision.clone(),
    }
}
fn idle() -> Fixture {
    let mut f = Fixture::new(true);
    for cap in &f.caps {
        f.db.complete_dispatch(cap, &json!({"result":"done"}), 1020)
            .unwrap();
    }
    f.sql()
        .execute("UPDATE workspace_resources SET dirty=1", [])
        .unwrap();
    f
}
fn dirty(sql: &rusqlite::Connection) -> bool {
    sql.query_row(
        "SELECT dirty FROM workspace_resources WHERE id='workspace_a'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn workspace_release_receipt_survives_restart_and_never_clears_a_later_failure() {
    let f = idle();
    let sql = f.sql();
    let Fixture { root, db, .. } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let m = session(&store, &ctx);
    let old = m.resource_workspace(&ctx, query()).await.unwrap();
    assert!(old.dirty);
    assert_eq!(old.release_block, None);
    let input = clear(&old, "release_original");
    let result = m.clear_workspace_dirty(&ctx, input.clone()).await.unwrap();
    assert!(result.cleared);
    assert!(!result.workspace.dirty);
    assert!(result.workspace.last_released_at.is_some());
    assert_ne!(old.revision, result.workspace.revision);
    assert!(
        sql.query_row(
            "SELECT dirty FROM workspace_resources WHERE id='workspace_b'",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    sql.execute_batch("UPDATE workspace_resources SET dirty=1 WHERE id='workspace_a'; UPDATE runner_dispatches SET state='outcome_unknown',fence=fence+1 WHERE id='dispatch_a';").unwrap();
    assert_eq!(
        m.clear_workspace_dirty(&ctx, input.clone()).await.unwrap(),
        result
    );
    assert!(dirty(&sql));
    let current = m.resource_workspace(&ctx, query()).await.unwrap();
    assert_eq!(
        current.release_block,
        Some(WorkspaceReleaseBlock::InspectionRequired)
    );
    assert!(matches!(
        m.clear_workspace_dirty(&ctx, clear(&current, "new_review"))
            .await,
        Err(Error::WorkspaceInspectionRequired)
    ));
    let mut changed = input.clone();
    changed.expected_revision = current.revision;
    assert!(matches!(
        m.clear_workspace_dirty(&ctx, changed).await,
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
    assert_eq!(m.clear_workspace_dirty(&ctx, input).await.unwrap(), result);
    assert!(m.resource_workspace(&ctx, query()).await.unwrap().dirty);
    assert_eq!(
        sql.query_row(
            "SELECT count(*) FROM management_commands WHERE operation='workspace.clear_dirty'",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn workspace_release_refuses_active_lease_and_stale_attempt_review() {
    let f = Fixture::new(true);
    let sql = f.sql();
    let Fixture {
        root: _root, db, ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let m = session(&store, &ctx);
    let row = m.resource_workspace(&ctx, query()).await.unwrap();
    assert_eq!(row.active_leases, 1);
    assert_eq!(
        row.release_block,
        Some(WorkspaceReleaseBlock::ActiveExecution)
    );
    assert!(matches!(
        m.clear_workspace_dirty(&ctx, clear(&row, "busy")).await,
        Err(Error::WorkspaceActive)
    ));
    sql.execute_batch("DELETE FROM resource_leases; UPDATE runner_dispatches SET state='completed'; UPDATE workspace_resources SET dirty=1;").unwrap();
    let before = m.resource_workspace(&ctx, query()).await.unwrap();
    // Same counts and flags, but a newer execution attempt: the review is stale.
    sql.execute(
        "UPDATE runner_dispatches SET fence=fence+1 WHERE id='dispatch_a'",
        [],
    )
    .unwrap();
    let after = m.resource_workspace(&ctx, query()).await.unwrap();
    assert_ne!(after.revision, before.revision);
    assert!(matches!(
        m.clear_workspace_dirty(&ctx, clear(&before, "stale")).await,
        Err(Error::Conflict)
    ));
    assert!(dirty(&sql));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn workspace_release_requires_permission_context_and_resource_association() {
    let mut f = idle();
    f.db.register_workspace("unrelated").unwrap();
    let Fixture {
        root: _root, db, ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let m = session(&store, &ctx);
    let row = m.resource_workspace(&ctx, query()).await.unwrap();
    let reader = Session::new(
        store.clone(),
        ctx.clone(),
        [Permission::ReadWorkspaces],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    assert!(reader.resource_workspace(&ctx, query()).await.is_ok());
    assert!(matches!(
        reader
            .clear_workspace_dirty(&ctx, clear(&row, "reader"))
            .await,
        Err(Error::LocalAuthority)
    ));
    let other = HostContext::new(
        "@other:example.test".into(),
        1,
        "a".repeat(64),
        1,
        "workspace_device".into(),
        1,
    )
    .unwrap();
    assert!(matches!(
        m.resource_workspace(&other, query()).await,
        Err(Error::LocalAuthority)
    ));
    let mut foreign = query();
    foreign.workspace_id = "unrelated".into();
    assert!(matches!(
        m.resource_workspace(&ctx, foreign).await,
        Err(Error::NotFound)
    ));
    let mut wrong = clear(&row, "foreign");
    wrong.resource_id = "another_resource".into();
    assert!(matches!(
        m.clear_workspace_dirty(&ctx, wrong).await,
        Err(Error::NotFound)
    ));
    m.retire();
    assert!(matches!(
        m.clear_workspace_dirty(&ctx, clear(&row, "retired")).await,
        Err(Error::LocalAuthority)
    ));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn workspace_receipt_failure_rolls_back_dirty_release_and_timestamp() {
    let f = idle();
    let sql = f.sql();
    sql.execute_batch("CREATE TRIGGER reject_workspace_receipt BEFORE INSERT ON management_commands WHEN NEW.operation='workspace.clear_dirty' BEGIN SELECT RAISE(ABORT,'fixture receipt failure'); END;").unwrap();
    let Fixture {
        root: _root, db, ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let m = session(&store, &ctx);
    let row = m.resource_workspace(&ctx, query()).await.unwrap();
    assert!(matches!(
        m.clear_workspace_dirty(&ctx, clear(&row, "rollback")).await,
        Err(Error::Sqlite(_))
    ));
    assert_eq!(m.resource_workspace(&ctx, query()).await.unwrap(), row);
    assert!(dirty(&sql));
    assert_eq!(
        sql.query_row("SELECT count(*) FROM workspace_dirty_releases", [], |r| r
            .get::<_, u64>(
            0
        ))
        .unwrap(),
        0
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn workspace_list_is_bounded_and_excludes_unassociated_workspaces() {
    let mut f = idle();
    // Extra retained workspaces on one real dispatch; no additional executions.
    for index in 0..20 {
        let id = format!("extra_{index:02}");
        f.db.register_workspace(&id).unwrap();
        f.sql().execute("INSERT INTO dispatch_resources(dispatch_id,resource_id,exclusive) VALUES('dispatch_a',?1,0)",[id]).unwrap();
    }
    f.db.register_workspace("unassociated").unwrap();
    let Fixture {
        root: _root, db, ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let m = session(&store, &ctx);
    let page = m
        .resource_workspaces(
            &ctx,
            WorkspaceListQuery {
                resource_id: query().resource_id,
                after: String::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(page.workspaces.len(), 16);
    let rest = m
        .resource_workspaces(
            &ctx,
            WorkspaceListQuery {
                resource_id: query().resource_id,
                after: page.next_after.unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(rest.workspaces.len(), 6);
    assert!(rest.next_after.is_none());
    let ids = page
        .workspaces
        .iter()
        .chain(&rest.workspaces)
        .map(|r| r.workspace_id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 22);
    assert!(!ids.contains("unassociated"));
    store.shutdown().await.unwrap();
}

#[test]
fn workspace_inputs_reject_paths_and_unknown_fields() {
    assert!(
        serde_json::from_value::<WorkspaceQuery>(
            json!({"resourceId":"r","workspaceId":"w","owner":"@other:test"})
        )
        .is_err()
    );
    let mut q = query();
    q.workspace_id = "/tmp/owner".into();
    assert!(q.validate().is_err());
    let input = ClearWorkspaceDirty {
        command_id: "command".into(),
        resource_id: "r".into(),
        workspace_id: "w".into(),
        expected_revision: "A".repeat(64),
    };
    assert!(input.validate().is_err());
}
