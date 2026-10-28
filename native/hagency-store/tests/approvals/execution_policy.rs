use super::*;
use hagency_store::management::{
    AgentExecutionPolicy, EditExecutionPolicy, ExecutionPolicyQuery, ExecutionPolicyValue,
    HostContext, Permission, Session,
};
use std::time::{Duration, Instant};

fn context() -> HostContext {
    HostContext::new(
        "@owner:example.test".into(),
        1,
        "a".repeat(64),
        1,
        "policy_device".into(),
        1,
    )
    .unwrap()
}
fn session(store: &DomainStore, context: &HostContext) -> Session {
    Session::new(
        store.clone(),
        context.clone(),
        [
            Permission::ReadExecutionPolicy,
            Permission::EditExecutionPolicy,
        ],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}
fn query(agent: &str) -> ExecutionPolicyQuery {
    ExecutionPolicyQuery {
        resource_id: resource("pool", "seat", 1000).id(),
        engagement_id: agent.into(),
    }
}
fn edit(current: &AgentExecutionPolicy, id: &str, yolo: bool) -> EditExecutionPolicy {
    EditExecutionPolicy {
        command_id: id.into(),
        resource_id: current.resource_id.clone(),
        engagement_id: current.engagement_id.clone(),
        expected_revision: current.revision.clone(),
        execution_policy: ExecutionPolicyValue { yolo },
    }
}

#[tokio::test]
async fn management_execution_policy_receipt_survives_later_edit_and_restart() {
    let Fixture {
        root, db, agents, ..
    } = Fixture::new(true);
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let m = session(&store, &ctx);
    let original = m
        .agent_execution_policy(&ctx, query(&agents[0]))
        .await
        .unwrap();
    assert!(!original.execution_policy.yolo);
    assert_eq!(
        serde_json::to_value(&original).unwrap()["appliesTo"],
        "next_dispatch"
    );
    let input = edit(&original, "enable", true);
    let result = m.edit_execution_policy(&ctx, input.clone()).await.unwrap();
    assert!(result.execution_policy.yolo);
    assert_ne!(result.revision, original.revision);
    assert!(
        !m.agent_execution_policy(&ctx, query(&agents[1]))
            .await
            .unwrap()
            .execution_policy
            .yolo
    );
    assert!(matches!(
        m.edit_execution_policy(&ctx, edit(&original, "stale", false))
            .await,
        Err(Error::Conflict)
    ));
    let disabled = m
        .edit_execution_policy(&ctx, edit(&result, "disable", false))
        .await
        .unwrap();
    assert!(!disabled.execution_policy.yolo);
    assert_eq!(
        m.edit_execution_policy(&ctx, input.clone()).await.unwrap(),
        result
    );
    assert!(
        !m.agent_execution_policy(&ctx, query(&agents[0]))
            .await
            .unwrap()
            .execution_policy
            .yolo
    );
    let mut changed = input.clone();
    changed.execution_policy.yolo = false;
    assert!(matches!(
        m.edit_execution_policy(&ctx, changed).await,
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
    assert_eq!(m.edit_execution_policy(&ctx, input).await.unwrap(), result);
    assert_eq!(
        m.agent_execution_policy(&ctx, query(&agents[0]))
            .await
            .unwrap(),
        disabled
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_execution_policy_checks_permission_resource_framework_and_host() {
    let f = Fixture::new(true);
    let sql = f.sql();
    let Fixture {
        root: _root,
        db,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let m = session(&store, &ctx);
    let original = m
        .agent_execution_policy(&ctx, query(&agents[0]))
        .await
        .unwrap();
    let reader = Session::new(
        store.clone(),
        ctx.clone(),
        [Permission::ReadExecutionPolicy],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    assert!(
        reader
            .agent_execution_policy(&ctx, query(&agents[0]))
            .await
            .is_ok()
    );
    assert!(matches!(
        reader
            .edit_execution_policy(&ctx, edit(&original, "forbidden", true))
            .await,
        Err(Error::LocalAuthority)
    ));
    let other = HostContext::new(
        "@owner:example.test".into(),
        1,
        "b".repeat(64),
        1,
        "policy_device".into(),
        1,
    )
    .unwrap();
    assert!(matches!(
        m.agent_execution_policy(&other, query(&agents[0])).await,
        Err(Error::LocalAuthority)
    ));
    let mut wrong = query(&agents[0]);
    wrong.resource_id = "other_resource".into();
    assert!(matches!(
        m.agent_execution_policy(&ctx, wrong).await,
        Err(Error::NotFound)
    ));
    let mut wrong = edit(&original, "foreign", true);
    wrong.resource_id = "other_resource".into();
    assert!(matches!(
        m.edit_execution_policy(&ctx, wrong).await,
        Err(Error::NotFound)
    ));
    sql.execute(
        "UPDATE resources SET config=json_set(config,'$.framework','claude')",
        [],
    )
    .unwrap();
    let claude = m
        .agent_execution_policy(&ctx, query(&agents[0]))
        .await
        .unwrap();
    assert!(matches!(
        m.edit_execution_policy(&ctx, edit(&claude, "unsupported", true))
            .await,
        Err(Error::Invalid(_))
    ));
    assert!(
        !m.agent_execution_policy(&ctx, query(&agents[0]))
            .await
            .unwrap()
            .execution_policy
            .yolo
    );
    m.retire();
    assert!(matches!(
        m.agent_execution_policy(&ctx, query(&agents[0])).await,
        Err(Error::LocalAuthority)
    ));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_execution_policy_receipt_failure_rolls_back_policy() {
    let f = Fixture::new(true);
    let sql = f.sql();
    sql.execute_batch("CREATE TRIGGER fail_policy_receipt BEFORE INSERT ON management_commands WHEN NEW.operation='execution_policy.edit' BEGIN SELECT RAISE(ABORT,'fixture receipt failure'); END;").unwrap();
    let Fixture {
        root: _root,
        db,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let m = session(&store, &ctx);
    let original = m
        .agent_execution_policy(&ctx, query(&agents[0]))
        .await
        .unwrap();
    let input = edit(&original, "atomic_policy", true);
    assert!(m.edit_execution_policy(&ctx, input.clone()).await.is_err());
    assert_eq!(
        m.agent_execution_policy(&ctx, query(&agents[0]))
            .await
            .unwrap(),
        original
    );
    assert_eq!(
        sql.query_row("SELECT count(*) FROM agent_execution_policies", [], |r| r
            .get::<_, u64>(
            0
        ))
        .unwrap(),
        0
    );
    sql.execute_batch("DROP TRIGGER fail_policy_receipt")
        .unwrap();
    assert!(
        m.edit_execution_policy(&ctx, input)
            .await
            .unwrap()
            .execution_policy
            .yolo
    );
    store.shutdown().await.unwrap();
}

#[test]
fn management_execution_policy_input_rejects_extra_authority_and_nonboolean_mode() {
    for value in [
        json!({"yolo":1}),
        json!({"yolo":true,"owner":"@other:example.test"}),
    ] {
        assert!(serde_json::from_value::<ExecutionPolicyValue>(value).is_err());
    }
    assert!(
        serde_json::from_value::<ExecutionPolicyQuery>(
            json!({"resourceId":"r","engagementId":"e","path":"/tmp/other"})
        )
        .is_err()
    );
}

#[tokio::test]
async fn management_execution_policy_list_filters_and_pages_agents_without_approval_rooms() {
    let mut f = Fixture::configured(true, 1000, 60_000, false);
    let pool = resource("pool", "seat", 1000);
    let foreign = resource("foreign", "other_seat", 1000);
    f.db.put_resource(&foreign).unwrap();
    let mut expected = f.agents.clone();
    for i in 0..20 {
        let name = format!("policy_agent_{i}");
        let value =
            f.db.admit(&proof(&request(&name, &name, &pool, 20)), 1000)
                .unwrap();
        expected.push(value.id);
    }
    f.db.admit(
        &proof(&request("foreign_agent", "foreign_agent", &foreign, 20)),
        1000,
    )
    .unwrap();
    expected.sort();
    let Fixture {
        root: _root, db, ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let m = session(&store, &ctx);
    let mut after = String::new();
    let mut actual = Vec::new();
    let mut pages = 0;
    loop {
        let page = m
            .agent_execution_policies(
                &ctx,
                hagency_store::management::ExecutionPolicyListQuery {
                    resource_id: pool.id(),
                    after: after.clone(),
                },
            )
            .await
            .unwrap();
        pages += 1;
        assert!(page.agents.len() <= 16);
        for a in page.agents {
            assert_eq!(a.resource_id, pool.id());
            assert!(a.engagement_id > after);
            actual.push(a.engagement_id);
        }
        if let Some(next) = page.next_after {
            assert_ne!(next, after);
            after = next;
        } else {
            break;
        }
    }
    assert_eq!(pages, 2);
    assert_eq!(actual, expected);
    store.shutdown().await.unwrap();
}
