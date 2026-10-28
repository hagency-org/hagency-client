//! Removing a binding must not revive the original review on re-observation.
use super::*;
use hagency_store::management::{
    HostContext, Permission, ResourceApprovalBindingQuery, RetireResourceApprovalBinding, Session,
};
use std::time::{Duration, Instant};

#[test]
fn management_binding_filter_input_rejects_paths_and_non_room_values() {
    for project in ["project_one", "!project:example.test", ""] {
        let query = ResourceApprovalBindingQuery {
            resource_id: "resource".into(),
            agent: "agent_one".into(),
            project: project.into(),
            ..Default::default()
        };
        query.validate().unwrap();
    }
    for project in [
        "../private",
        "https://example.test",
        "!room",
        "!room:\0host",
    ] {
        let query = ResourceApprovalBindingQuery {
            resource_id: "resource".into(),
            project: project.into(),
            ..Default::default()
        };
        assert!(query.validate().is_err(), "{project}");
    }
    assert!(
        serde_json::from_value::<ResourceApprovalBindingQuery>(json!({
            "resourceId": "resource", "agent": "a", "project": "project_one", "owner": "forged"
        }))
        .is_err()
    );
}

#[tokio::test]
async fn management_binding_filters_keep_project_identity_and_paginate_matching_rows() {
    let mut f = Fixture::new(true);
    let pool = resource("pool", "seat", 1000);
    let foreign = resource("filter_other_resource", "other_seat", 1000);
    f.db.put_resource(&foreign).unwrap();
    // A project room is not its dedicated private approval room. Native
    // observation deliberately rejects using one approval room for two projects.
    let mut second_room = None;
    for index in 0..18 {
        let name = format!("filter_agent_{index}");
        let mut requested = request(&format!("filter_request_{index}"), &name, &pool, 20);
        if index == 0 {
            requested.target_project_id = "project_two".into();
            requested.target_room_id = "!second:example.test".into();
            requested.owner_dm_room_id = "!second_private:example.test".into();
        }
        let reviewed = proof(&requested);
        let engagement = f.db.admit(&reviewed, 1000).unwrap();
        f.db.approve(&format!("filter_approve_{index}"), &reviewed, 1000)
            .unwrap();
        let effect = f.db.claim_effect().unwrap().unwrap();
        f.db.observe_effect(
            &effect.id,
            effect.fence,
            &EffectOutcome::Applied {
                receipt: format!("filter provisioning {index}"),
            },
        )
        .unwrap();
        let mut room = f.rooms[0].clone();
        room.engagement_id = engagement.id;
        room.room_id = requested.owner_dm_room_id.clone();
        f.db.observe_approval_room(&room, 1020 + index).unwrap();
        if index == 0 { second_room = Some(room); }
    }
    let mut second_room = second_room.unwrap();
    second_room.available = false;
    f.db.observe_approval_room(&second_room, 1100).unwrap();
    let Fixture {
        root: _root, db, ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    let mut query = ResourceApprovalBindingQuery {
        resource_id: pool.id(),
        project: "!project:example.test".into(),
        ..Default::default()
    };
    let first = manager
        .resource_approval_bindings(&ctx, query.clone())
        .await
        .unwrap();
    assert_eq!(first.bindings.len(), 16);
    assert_eq!(first.project, query.project);
    assert_eq!(first.agent, "");
    assert!(first.bindings.iter().all(|r| r.project_id == "project_one"));
    query.after = first.next_after.unwrap();
    let rest = manager
        .resource_approval_bindings(&ctx, query.clone())
        .await
        .unwrap();
    assert_eq!(rest.bindings.len(), 3);
    assert!(rest.next_after.is_none());
    let ids: BTreeSet<_> = first
        .bindings
        .iter()
        .chain(rest.bindings.iter())
        .map(|r| r.engagement_id.clone())
        .collect();
    assert_eq!(ids.len(), 19);
    assert!(rest.bindings.iter().all(|r| r.project_id == "project_one"));

    query.after.clear();
    query.project = "project_two".into();
    let second = manager
        .resource_approval_bindings(&ctx, query.clone())
        .await
        .unwrap();
    assert_eq!(second.bindings.len(), 1);
    assert_eq!(second.bindings[0].agent, "filter_agent_0");
    assert_eq!(second.bindings[0].project_id, "project_two");
    assert!(!second.bindings[0].active);
    query.project = "!second:example.test".into();
    assert_eq!(
        manager
            .resource_approval_bindings(&ctx, query.clone())
            .await
            .unwrap()
            .bindings,
        second.bindings
    );

    query.project = "!private:example.test".into();
    assert!(
        manager
            .resource_approval_bindings(&ctx, query.clone())
            .await
            .unwrap()
            .bindings
            .is_empty()
    );
    query.project = "project_two".into();
    query.agent = "a".into();
    assert!(
        manager
            .resource_approval_bindings(&ctx, query.clone())
            .await
            .unwrap()
            .bindings
            .is_empty()
    );
    query.project = "project_one".into();
    assert_eq!(
        manager
            .resource_approval_bindings(&ctx, query.clone())
            .await
            .unwrap()
            .bindings[0]
            .agent,
        "a"
    );
    query.resource_id = foreign.id();
    assert!(
        manager
            .resource_approval_bindings(&ctx, query)
            .await
            .unwrap()
            .bindings
            .is_empty()
    );
    store.shutdown().await.unwrap();
}

#[test]
fn approval_binding_recreation_does_not_reuse_original_incarnation() {
    let mut f = Fixture::new(true);
    let original = f.admit(0, 900);
    let old_verdict = f.verdict(&original.id, ApprovalChoice::Once, "old_review");
    let removed =
        f.db.retire_approval_binding("a", "!private:example.test")
            .unwrap();
    f.db.observe_approval_room(&f.rooms[0], 1020).unwrap();
    let current =
        f.db.approval_bindings("a", "!private:example.test", 16)
            .unwrap();
    assert_eq!(current.len(), 1);
    assert!(
        current[0].incarnation > removed.incarnation,
        "recreated binding must not match an old context or review"
    );
    assert!(f.db.observe_owner_verdict(&old_verdict, 1021).is_err());
    assert!(
        f.db.request_owner_approval(&f.caps[0], &f.input(0, 901), 1022)
            .is_err()
    );
    // The other agent's independently observed binding remains unchanged.
    let other =
        f.db.approval_bindings("b", "!private:example.test", 16)
            .unwrap();
    assert_eq!(other[0].incarnation, 1);
}

#[test]
fn approval_binding_counter_survives_restart_and_ignores_duplicate_observations() {
    let mut f = Fixture::new(true);
    f.db.observe_approval_room(&f.rooms[0], 1020).unwrap();
    assert_eq!(
        f.db.approval_bindings("a", "", 16).unwrap()[0].incarnation,
        1
    );
    f.db.retire_approval_binding("a", "!private:example.test")
        .unwrap();
    let Fixture {
        root, db, rooms, ..
    } = f;
    drop(db);
    let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
    db.observe_approval_room(&rooms[0], 1021).unwrap();
    assert_eq!(db.approval_bindings("a", "", 16).unwrap()[0].incarnation, 2);
    db.retire_approval_binding("a", "!private:example.test")
        .unwrap();
    db.observe_approval_room(&rooms[0], 1022).unwrap();
    db.observe_approval_room(&rooms[0], 1023).unwrap();
    assert_eq!(db.approval_bindings("a", "", 16).unwrap()[0].incarnation, 3);
}

#[test]
fn approval_binding_schema93_recovers_removed_binding_generation_from_context() {
    let mut f = Fixture::new(true);
    f.rooms[0].generation = 2;
    f.db.observe_approval_room(&f.rooms[0], 1020).unwrap();
    f.contexts[0].id = "new_context_a".into();
    f.db.bind_approval_context(&f.caps[0], &f.contexts[0], 1021)
        .unwrap();
    let request = f.input(0, 902);
    let approval =
        f.db.request_owner_approval(&f.caps[0], &request, 1022)
            .unwrap();
    let verdict = f.verdict(&approval.id, ApprovalChoice::Once, "before_upgrade");
    f.db.retire_approval_binding("a", "!private:example.test")
        .unwrap();
    let sql = f.sql();
    let before: String = sql
        .query_row(
            "SELECT config FROM approval_contexts WHERE id='new_context_a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Reconstruct schema 93: neither later additive table existed then.
    sql.execute_batch("DROP TABLE IF EXISTS owner_operator_graph_recipients; DROP TABLE IF EXISTS owner_operator_graph_outbox; DROP TABLE IF EXISTS owner_operator_graph_commands; DROP TABLE IF EXISTS owner_operator_graphs; DROP TABLE IF EXISTS owner_operator_graph_archive_chunks; DROP TABLE IF EXISTS owner_operator_graph_archives; DROP TABLE owned_dispatch_execution_policies; DROP TABLE approval_binding_incarnations; DROP TABLE IF EXISTS owner_shared_tool_policies;PRAGMA user_version=93;")
        .unwrap();
    drop(sql);
    let Fixture {
        root, db, rooms, ..
    } = f;
    drop(db);
    let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
    db.observe_approval_room(&rooms[0], 1023).unwrap();
    assert_eq!(db.approval_bindings("a", "", 16).unwrap()[0].incarnation, 3);
    assert!(db.observe_owner_verdict(&verdict, 1024).is_err());
    let sql = rusqlite::Connection::open(root.path().join("state/domain.sqlite3")).unwrap();
    let after: String = sql
        .query_row(
            "SELECT config FROM approval_contexts WHERE id='new_context_a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        sql.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        hagency_store::DOMAIN_SCHEMA_VERSION
    );
}

#[test]
fn approval_binding_counter_exhaustion_rolls_back_room_observation() {
    let mut f = Fixture::new(true);
    let sql = f.sql();
    sql.execute(
        "UPDATE approval_binding_incarnations SET incarnation=?1 WHERE engagement_id=?2",
        rusqlite::params![hagency_core::JSON_SAFE_MAX, f.agents[0]],
    )
    .unwrap();
    let before: String = sql
        .query_row(
            "SELECT config FROM approval_rooms WHERE room_id='!private:example.test'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    f.rooms[0].generation = 2;
    assert!(matches!(
        f.db.observe_approval_room(&f.rooms[0], 1020),
        Err(Error::Capacity)
    ));
    let after: String = sql
        .query_row(
            "SELECT config FROM approval_rooms WHERE room_id='!private:example.test'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        f.db.approval_bindings("a", "", 16).unwrap()[0].incarnation,
        1
    );
}

fn context() -> HostContext {
    HostContext::new(
        "@resource_owner:example.test".into(),
        1,
        "a".repeat(64),
        1,
        "management_device".into(),
        1,
    )
    .unwrap()
}
fn session(store: &DomainStore, context: &HostContext) -> Session {
    Session::new(
        store.clone(),
        context.clone(),
        [
            Permission::ReadApprovalBindings,
            Permission::RetireApprovalBindings,
        ],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}

#[tokio::test]
async fn management_binding_removal_receipt_never_removes_a_successor() {
    let mut f = Fixture::new(true);
    let original = f.admit(0, 903);
    f.choose(&original.id, ApprovalChoice::Always);
    let resource_id = resource("pool", "seat", 1000).id();
    let Fixture {
        root,
        db,
        rooms,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    let query = ResourceApprovalBindingQuery {
        resource_id: resource_id.clone(),
        after: String::new(),
        ..Default::default()
    };
    let list = manager
        .resource_approval_bindings(&ctx, query.clone())
        .await
        .unwrap();
    assert_eq!(list.bindings.len(), 2);
    let selected = list
        .bindings
        .into_iter()
        .find(|r| r.engagement_id == agents[0])
        .unwrap();
    let input = RetireResourceApprovalBinding {
        command_id: "original_unbind".into(),
        resource_id,
        engagement_id: selected.engagement_id.clone(),
        expected_revision: selected.revision.clone(),
    };
    let reader = Session::new(
        store.clone(),
        ctx.clone(),
        [Permission::ReadApprovalBindings],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    assert!(matches!(
        reader
            .retire_resource_approval_binding(&ctx, input.clone())
            .await,
        Err(Error::LocalAuthority)
    ));
    let foreign = HostContext::new(
        "@other:example.test".into(),
        1,
        "a".repeat(64),
        1,
        "management_device".into(),
        1,
    )
    .unwrap();
    assert!(
        manager
            .resource_approval_bindings(&foreign, query.clone())
            .await
            .is_err()
    );
    let mut wrong_resource = input.clone();
    wrong_resource.resource_id = "unknown_resource".into();
    assert!(
        manager
            .retire_resource_approval_binding(&ctx, wrong_resource)
            .await
            .is_err()
    );
    let removed = manager
        .retire_resource_approval_binding(&ctx, input.clone())
        .await
        .unwrap();
    assert!(removed.unbound);
    assert_eq!(removed.revoked_grants, 1);
    store.observe_approval_room(rooms[0].clone()).await.unwrap();
    let current = manager
        .resource_approval_bindings(&ctx, query.clone())
        .await
        .unwrap();
    let successor = current
        .bindings
        .iter()
        .find(|r| r.engagement_id == agents[0])
        .unwrap();
    assert!(successor.incarnation > removed.binding.incarnation);
    assert_ne!(successor.revision, selected.revision);
    assert_eq!(
        manager
            .retire_resource_approval_binding(&ctx, input.clone())
            .await
            .unwrap(),
        removed
    );
    let mut stale = input.clone();
    stale.command_id = "new_stale_command".into();
    assert!(matches!(
        manager.retire_resource_approval_binding(&ctx, stale).await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        manager
            .resource_approval_bindings(&ctx, query.clone())
            .await
            .unwrap()
            .bindings
            .len(),
        2
    );
    let mut forged = input.clone();
    forged.engagement_id = agents[1].clone();
    assert!(matches!(
        manager.retire_resource_approval_binding(&ctx, forged).await,
        Err(Error::Conflict)
    ));
    manager.retire();
    reader.retire();
    store.shutdown().await.unwrap();
    let store = DomainStore::start(
        DomainRepository::open(&root.path().join("state")).unwrap(),
        16,
    )
    .unwrap();
    let manager = session(&store, &ctx);
    assert_eq!(
        manager
            .retire_resource_approval_binding(&ctx, input)
            .await
            .unwrap(),
        removed
    );
    assert_eq!(
        manager
            .resource_approval_bindings(&ctx, query)
            .await
            .unwrap()
            .bindings
            .len(),
        2
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_binding_receipt_failure_rolls_back_unbind_and_grant_revocation() {
    let mut f = Fixture::new(true);
    let original = f.admit(0, 904);
    f.choose(&original.id, ApprovalChoice::Always);
    let sql = f.sql();
    sql.execute_batch("CREATE TRIGGER fail_binding_receipt BEFORE INSERT ON management_commands WHEN NEW.operation='approval_binding.retire' BEGIN SELECT RAISE(ABORT,'fixture receipt failure'); END;").unwrap();
    let Fixture {
        root: _root,
        db,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    let query = ResourceApprovalBindingQuery {
        resource_id: resource("pool", "seat", 1000).id(),
        after: String::new(),
        ..Default::default()
    };
    let selected = manager
        .resource_approval_bindings(&ctx, query.clone())
        .await
        .unwrap()
        .bindings
        .into_iter()
        .find(|r| r.engagement_id == agents[0])
        .unwrap();
    let input = RetireResourceApprovalBinding {
        command_id: "receipt_failure".into(),
        resource_id: query.resource_id.clone(),
        engagement_id: selected.engagement_id,
        expected_revision: selected.revision,
    };
    assert!(
        manager
            .retire_resource_approval_binding(&ctx, input.clone())
            .await
            .is_err()
    );
    assert_eq!(
        manager
            .resource_approval_bindings(&ctx, query)
            .await
            .unwrap()
            .bindings
            .len(),
        2
    );
    assert_eq!(
        sql.query_row(
            "SELECT count(*) FROM approval_grants WHERE revoked=0",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        sql.query_row("SELECT count(*) FROM management_commands", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    sql.execute_batch("DROP TRIGGER fail_binding_receipt;")
        .unwrap();
    assert_eq!(
        manager
            .retire_resource_approval_binding(&ctx, input)
            .await
            .unwrap()
            .revoked_grants,
        1
    );
    store.shutdown().await.unwrap();
}
