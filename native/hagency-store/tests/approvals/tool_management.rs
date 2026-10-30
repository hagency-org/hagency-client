use super::*;
use hagency_store::management::{
    HostContext, Permission, RevokeToolGrant, Session, ToolApprovalLookup, ToolApprovalQuery,
    ToolGrantMode, ToolGrantQuery,
};
use std::time::{Duration, Instant};

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
fn session(store: &DomainStore, ctx: &HostContext) -> Session {
    Session::new(
        store.clone(),
        ctx.clone(),
        [
            Permission::ReadToolApprovals,
            Permission::InspectToolGrants,
            Permission::RevokeToolGrants,
        ],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}
fn query(agent: &str) -> ToolGrantQuery {
    ToolGrantQuery {
        resource_id: resource("pool", "seat", 1000).id(),
        engagement_id: agent.into(),
        after: String::new(),
    }
}
fn command(grant: &hagency_store::management::ToolGrant, id: &str) -> RevokeToolGrant {
    RevokeToolGrant {
        command_id: id.into(),
        resource_id: grant.resource_id.clone(),
        engagement_id: grant.engagement_id.clone(),
        grant_id: grant.id.clone(),
        expected_revision: grant.revision.clone(),
    }
}

#[tokio::test]
async fn management_tool_long_unicode_descriptions_page_without_loss() {
    let mut f = Fixture::new(true);
    let mut descriptions = BTreeSet::new();
    for id in 2600..2609 {
        let mut input = f.input(0, id);
        let command = format!("echo {id} {}", "汉".repeat(7000));
        input.params["command"] = json!(command);
        let request =
            f.db.request_owner_approval(&f.caps[0], &input, 1010)
                .unwrap();
        f.choose(&request.id, ApprovalChoice::Always);
        descriptions.insert(command);
    }
    let Fixture {
        root: _root,
        db,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    let mut input = query(&agents[0]);
    let mut ids = BTreeSet::new();
    let mut pages = 0;
    loop {
        let page = manager.tool_grants(&ctx, input.clone()).await.unwrap();
        assert!(!page.grants.is_empty());
        assert!(serde_json::to_vec(&page).unwrap().len() <= 128 * 1024);
        for grant in &page.grants {
            assert!(ids.insert(grant.id.clone()));
            let text = &grant.details.as_ref().unwrap().description;
            assert!(descriptions.iter().any(|command| text.contains(command)));
        }
        pages += 1;
        if let Some(after) = page.next_after {
            assert_ne!(after, input.after);
            input.after = after;
        } else {
            break;
        }
    }
    assert_eq!(ids.len(), 9);
    assert!(pages > 1);
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_tool_status_filters_before_paging_and_preserves_summary_privacy() {
    let mut f = Fixture::new(true);
    let mut expected = BTreeSet::new();
    for agent in 0..2 {
        for id in 0..9 {
            expected.insert(f.admit(agent, 2000 + id).id);
        }
    }
    let Fixture {
        root: _root,
        db,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    let all = ToolApprovalQuery {
        resource_id: query(&agents[0]).resource_id,
        ..Default::default()
    };
    let first = manager.tool_approvals(&ctx, all.clone()).await.unwrap();
    assert_eq!(first.approvals.len(), 16);
    assert_eq!(first.subjects.len(), 2);
    assert_eq!(
        first
            .subjects
            .iter()
            .map(|s| s.agent.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["a", "b"])
    );
    assert!(first.subjects.iter().all(|subject| {
        first
            .approvals
            .iter()
            .any(|row| row.engagement_id == subject.engagement_id)
    }));
    let second = manager
        .tool_approvals(
            &ctx,
            ToolApprovalQuery {
                after: first.next_after.unwrap(),
                ..all.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(second.approvals.len(), 2);
    assert!(second.next_after.is_none());
    assert_eq!(
        first
            .approvals
            .iter()
            .chain(&second.approvals)
            .map(|r| r.id.clone())
            .collect::<BTreeSet<_>>(),
        expected
    );
    let filtered = manager
        .tool_approvals(
            &ctx,
            ToolApprovalQuery {
                engagement_id: agents[0].clone(),
                ..all.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(filtered.approvals.len(), 9);
    assert_eq!(filtered.subjects.len(), 1);
    assert_eq!(filtered.subjects[0].engagement_id, agents[0]);
    assert_eq!(filtered.subjects[0].agent, "a");
    assert!(filtered.next_after.is_none());
    assert!(
        filtered
            .approvals
            .iter()
            .all(|r| r.engagement_id == agents[0])
    );
    let item = manager
        .tool_approval(
            &ctx,
            ToolApprovalLookup {
                resource_id: all.resource_id.clone(),
                approval_id: filtered.approvals[0].id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(item.resource_id, all.resource_id);
    assert_eq!(item.subject, filtered.subjects[0]);
    assert!(item.observed_at_ms >= first.observed_at_ms);
    let value = serde_json::to_value(&item.approval).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 7);
    let encoded = serde_json::to_string(&item).unwrap();
    for private in [
        "echo approved",
        "!private:",
        "BOT_DEVICE",
        "workspace_",
        "params",
        "config",
    ] {
        assert!(!encoded.contains(private));
    }
    assert_eq!(
        item.approval.project_room_id.as_deref(),
        Some("!project:example.test")
    );
    assert!(matches!(
        manager
            .tool_approval(
                &ctx,
                ToolApprovalLookup {
                    resource_id: "foreign_resource".into(),
                    approval_id: item.approval.id
                }
            )
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        manager
            .tool_approvals(
                &ctx,
                ToolApprovalQuery {
                    engagement_id: "foreign_engagement".into(),
                    ..all
                }
            )
            .await,
        Err(Error::NotFound)
    ));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_tool_grants_recover_original_description_and_task_scope() {
    let mut f = Fixture::new(true);
    let always = f.admit(0, 2100);
    f.choose(&always.id, ApprovalChoice::Always);
    let task = f.admit(1, 2101);
    f.choose(&task.id, ApprovalChoice::Task);
    let Fixture {
        root: _root,
        db,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    for (index, mode) in [(0, ToolGrantMode::Always), (1, ToolGrantMode::Task)] {
        let page = manager
            .tool_grants(&ctx, query(&agents[index]))
            .await
            .unwrap();
        assert_eq!(page.grants.len(), 1);
        let grant = &page.grants[0];
        assert_eq!(grant.mode, mode);
        assert_eq!(grant.kind, "exact_command");
        assert_eq!(grant.task_id.is_some(), index == 1);
        assert!(grant.binding_current);
        assert!(!grant.revoked);
        assert_eq!(grant.revision.len(), 64);
        let detail = grant.details.as_ref().unwrap();
        assert!(detail.description.contains("echo approved"));
        assert_eq!(detail.owner_mxid, "@owner:example.test");
        assert!(!detail.project_id.is_empty());
        let value = serde_json::to_value(grant).unwrap();
        for absent in ["createdAt", "scopeKey", "contextKey", "config", "params"] {
            assert!(value.get(absent).is_none());
        }
    }
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_tool_grant_rules_state_the_description_as_fields() {
    use hagency_core::execution::{CLAUDE_TOOL_METHOD, OCTOS_APPROVAL_METHOD};
    let mut f = Fixture::new(true);
    let only = |input: &mut HostApprovalRequest, keep: &[&str]| {
        input
            .params
            .as_object_mut()
            .unwrap()
            .retain(|k, _| keep.contains(&k.as_str()));
    };
    let ids = ["threadId", "turnId", "itemId"];
    // Codex: an exact command that also asks for sandbox permissions.
    let mut command = f.input(0, 2700);
    command.params["command"] = json!("cargo test");
    command.params["cwd"] = json!("/work/a/crate");
    command.params["additionalPermissions"] = json!({"network":{"enabled":true},
        "fileSystem":{"read":["/data/b","/data/a"],"write":["/work/a/out"]}});
    // Codex: one network host.
    let mut network = f.input(0, 2701);
    only(&mut network, &ids);
    network.params["networkApprovalContext"] = json!({"host":"Crates.IO","protocol":"https"});
    // Codex: a permission profile.
    let mut profile = f.input(0, 2702);
    profile.method = "item/permissions/requestApproval".into();
    only(&mut profile, &["threadId", "turnId", "itemId", "cwd"]);
    profile.params["permissions"] = json!({"fileSystem":{"read":["/opt/sdk"],
        "entries":[{"access":"write","path":{"type":"path","path":"/work/a/cache"}}]}});
    // Claude Code: one exact tool call.
    let mut claude = f.input(0, 2703);
    claude.method = CLAUDE_TOOL_METHOD.into();
    only(&mut claude, &ids);
    claude.params["toolName"] = json!("Bash");
    claude.params["toolUseId"] = json!("toolu_private");
    claude.params["input"] = json!({"description":"Run tests","command":"npm test"});
    // Octos: a typed shell command.
    let mut octos = f.input(0, 2704);
    octos.method = OCTOS_APPROVAL_METHOD.into();
    only(&mut octos, &ids);
    for (key, value) in [("toolName", "shell"), ("octosTurnId", "octos_turn_private"), ("title", "Run checks"),
        ("body", "body_private"), ("command", "make check"), ("cwd", "/work/a")] {
        octos.params[key] = json!(value);
    }
    let mut approvals = Vec::new();
    for input in [command, network, profile, claude, octos] {
        let a = f.db.request_owner_approval(&f.caps[0], &input, 1010).unwrap();
        assert!(a.reusable_scope, "{}", input.method);
        f.choose(&a.id, ApprovalChoice::Always);
        approvals.push(a.id);
    }
    let sql = f.sql();
    let Fixture { root: _root, db, agents, .. } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    let rules = || async {
        manager
            .tool_grants(&ctx, query(&agents[0]))
            .await
            .unwrap()
            .grants
            .into_iter()
            .map(|g| {
                let details = g.details.unwrap();
                (g.kind, details.description, serde_json::to_value(details.rule).unwrap())
            })
            .collect::<Vec<_>>()
    };
    let rule = |runtime: &str, fields: Value| {
        let mut rule = json!({"runtime":runtime,"tool":null,"command":null,"cwd":null,"host":null,
            "protocol":null,"input":null,"permissions":null});
        for (key, value) in fields.as_object().unwrap() {
            rule[key] = value.clone();
        }
        rule
    };
    let expected = [
        ("exact_command", rule("codex", json!({"command":"cargo test","cwd":"/work/a/crate",
            "permissions":{"network":true,"read":["/data/a","/data/b"],"write":["/work/a/out"]}}))),
        ("network_host", rule("codex", json!({"host":"crates.io","protocol":"https"}))),
        ("permission_profile", rule("codex", json!({"cwd":"/work/a",
            "permissions":{"network":null,"read":["/opt/sdk"],"write":["/work/a/cache"]}}))),
        ("exact_tool_call", rule("claude", json!({"tool":"Bash",
            "input":{"command":"npm test","description":"Run tests"}}))),
        ("exact_command", rule("octos", json!({"command":"make check","cwd":"/work/a"}))),
    ];
    let found = rules().await;
    assert_eq!(found.len(), expected.len());
    for (kind, rule) in &expected {
        assert!(
            found.iter().any(|(k, _, r)| k == kind && r == rule),
            "missing {kind} {rule} in {found:?}"
        );
    }
    // The fields restate the description; nothing else from the request.
    let encoded = serde_json::to_string(&found).unwrap();
    for private in ["thread_a", "turn_a", "item_27", "toolu_private", "octos_turn_private", "body_private"] {
        assert!(!encoded.contains(private), "{private}");
    }
    // A stored description that no longer matches its request keeps the
    // English text but loses the structured fields.
    sql.execute(
        "UPDATE owner_approvals SET description=description||' (edited)' WHERE id=?1",
        [&approvals[1]],
    )
    .unwrap();
    let drifted = rules().await;
    let network = drifted.iter().find(|(k, _, _)| k == "network_host").unwrap();
    assert!(network.1.ends_with(" (edited)"));
    assert!(network.2.is_null());
    assert_eq!(drifted.iter().filter(|(_, _, r)| !r.is_null()).count(), 4);
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_tool_permissions_and_current_host_are_separate() {
    let mut f = Fixture::new(true);
    let request = f.admit(0, 2200);
    f.choose(&request.id, ApprovalChoice::Always);
    let Fixture {
        root: _root,
        db,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    let selected = manager
        .tool_grants(&ctx, query(&agents[0]))
        .await
        .unwrap()
        .grants
        .remove(0);
    let reader = Session::new(
        store.clone(),
        ctx.clone(),
        [Permission::ReadToolApprovals],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    assert!(
        reader
            .tool_approval(
                &ctx,
                ToolApprovalLookup {
                    resource_id: selected.resource_id.clone(),
                    approval_id: request.id
                }
            )
            .await
            .is_ok()
    );
    assert!(matches!(
        reader.tool_grants(&ctx, query(&agents[0])).await,
        Err(Error::LocalAuthority)
    ));
    assert!(matches!(
        reader
            .revoke_tool_grant(&ctx, command(&selected, "no_consent"))
            .await,
        Err(Error::LocalAuthority)
    ));
    let other = HostContext::new(
        "@resource_owner:example.test".into(),
        1,
        "b".repeat(64),
        1,
        "management_device".into(),
        1,
    )
    .unwrap();
    assert!(matches!(
        manager.tool_grants(&other, query(&agents[0])).await,
        Err(Error::LocalAuthority)
    ));
    let mut foreign = command(&selected, "foreign_agent");
    foreign.engagement_id = agents[1].clone();
    assert!(matches!(
        manager.revoke_tool_grant(&ctx, foreign).await,
        Err(Error::NotFound)
    ));
    manager.retire();
    assert!(matches!(
        manager.tool_grants(&ctx, query(&agents[0])).await,
        Err(Error::LocalAuthority)
    ));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_tool_revoke_replays_original_result_after_change_and_restart() {
    let mut f = Fixture::new(true);
    let a = f.admit(0, 2300);
    f.choose(&a.id, ApprovalChoice::Always);
    let b = f.admit(1, 2301);
    f.choose(&b.id, ApprovalChoice::Always);
    let sql = f.sql();
    let Fixture {
        root, db, agents, ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    let selected = manager
        .tool_grants(&ctx, query(&agents[0]))
        .await
        .unwrap()
        .grants
        .remove(0);
    let input = command(&selected, "original_revoke");
    let mut stale = input.clone();
    stale.expected_revision = "0".repeat(64);
    assert!(matches!(
        manager.revoke_tool_grant(&ctx, stale).await,
        Err(Error::Conflict)
    ));
    let original = manager
        .revoke_tool_grant(&ctx, input.clone())
        .await
        .unwrap();
    assert_eq!(original.grant_id, selected.id);
    assert_eq!(original.reviewed_revision, selected.revision);
    assert!(original.revoked);
    assert!(
        !manager
            .tool_grants(&ctx, query(&agents[1]))
            .await
            .unwrap()
            .grants[0]
            .revoked
    );
    sql.execute("UPDATE engagements SET projection=json_set(projection,'$.agentName','renamed_agent') WHERE id=?1",[&agents[0]]).unwrap();
    let latest = manager
        .tool_grants(&ctx, query(&agents[0]))
        .await
        .unwrap()
        .grants
        .remove(0);
    assert_eq!(latest.agent, "renamed_agent");
    assert!(latest.revoked);
    assert_ne!(latest.revision, selected.revision);
    assert_eq!(
        manager
            .revoke_tool_grant(&ctx, input.clone())
            .await
            .unwrap(),
        original
    );
    let mut rebound = input.clone();
    rebound.grant_id = manager
        .tool_grants(&ctx, query(&agents[1]))
        .await
        .unwrap()
        .grants[0]
        .id
        .clone();
    assert!(matches!(
        manager.revoke_tool_grant(&ctx, rebound).await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        sql.query_row(
            "SELECT count(*) FROM management_commands WHERE operation='tool_grant.revoke'",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    manager.retire();
    store.shutdown().await.unwrap();
    let store = DomainStore::start(
        DomainRepository::open(&root.path().join("state")).unwrap(),
        16,
    )
    .unwrap();
    let manager = session(&store, &ctx);
    assert_eq!(
        manager
            .revoke_tool_grant(&ctx, input.clone())
            .await
            .unwrap(),
        original
    );
    manager.retire();
    assert!(matches!(
        manager.revoke_tool_grant(&ctx, input).await,
        Err(Error::LocalAuthority)
    ));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_tool_revoke_receipt_failure_rolls_back() {
    let mut f = Fixture::new(true);
    let a = f.admit(0, 2400);
    f.choose(&a.id, ApprovalChoice::Always);
    let sql = f.sql();
    sql.execute_batch("CREATE TRIGGER fail_tool_receipt BEFORE INSERT ON management_commands WHEN NEW.operation='tool_grant.revoke' BEGIN SELECT RAISE(ABORT,'fixture receipt failure'); END;").unwrap();
    let Fixture {
        root: _root,
        db,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    let selected = manager
        .tool_grants(&ctx, query(&agents[0]))
        .await
        .unwrap()
        .grants
        .remove(0);
    let input = command(&selected, "receipt_failure");
    assert!(
        manager
            .revoke_tool_grant(&ctx, input.clone())
            .await
            .is_err()
    );
    assert_eq!(
        manager
            .tool_grants(&ctx, query(&agents[0]))
            .await
            .unwrap()
            .grants[0],
        selected
    );
    assert_eq!(
        sql.query_row("SELECT count(*) FROM management_commands", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    sql.execute_batch("DROP TRIGGER fail_tool_receipt;")
        .unwrap();
    assert!(
        manager
            .revoke_tool_grant(&ctx, input)
            .await
            .unwrap()
            .revoked
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_tool_description_rejects_unmatched_context_and_keeps_rule_without_description()
{
    let mut f = Fixture::new(true);
    let a = f.admit(0, 2500);
    f.choose(&a.id, ApprovalChoice::Always);
    let sql = f.sql();
    let Fixture {
        root: _root,
        db,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    sql.execute("UPDATE approval_contexts SET config=json_set(config,'$.workspace','/other/workspace') WHERE engagement_id=?1",[&agents[0]]).unwrap();
    assert!(matches!(
        manager.tool_grants(&ctx, query(&agents[0])).await,
        Err(Error::Schema)
    ));
    sql.execute(
        "UPDATE owner_approvals SET description=NULL WHERE id=?1",
        [&a.id],
    )
    .unwrap();
    let grants = manager
        .tool_grants(&ctx, query(&agents[0]))
        .await
        .unwrap()
        .grants;
    assert_eq!(grants.len(), 1);
    assert!(grants[0].details.is_none());
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_tool_detail_reads_latest_terminal_state_without_repeating_decision() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let mut f = Fixture::new_at(true, now.saturating_sub(20), 60_000);
    let mut input = f.input(0, 2900);
    input.expires_at = now + 30_000;
    let request = f.db.request_owner_approval(&f.caps[0], &input, now).unwrap();
    let card = f.db.private_approval(&request.id, now).unwrap();
    let verdict = OwnerVerdictObservation {
        request_id: request.id.clone(), request_digest:card.digest,
        binding_generation:card.binding_generation, server_name:"example.test".into(),
        room_id:card.room_id, sender_mxid:card.owner_mxid, event_id:"$fresh-management-detail".into(),
        encrypted:true, choice:ApprovalChoice::Deny,
    };
    let Fixture {
        root: _root,
        db,
        agents,
        ..
    } = f;
    let store = DomainStore::start(db, 16).unwrap();
    let ctx = context();
    let manager = session(&store, &ctx);
    let lookup = ToolApprovalLookup {
        resource_id: query(&agents[0]).resource_id,
        approval_id: request.id.clone(),
    };
    let before = manager.tool_approval(&ctx, lookup.clone()).await.unwrap();
    assert_eq!(before.approval.state, "pending");
    assert!(before.approval.expires_at > before.observed_at_ms);
    store.observe_owner_verdict(verdict).await.unwrap();
    let after = manager.tool_approval(&ctx, lookup.clone()).await.unwrap();
    assert_eq!(after.approval.state, "decided");
    assert_eq!(after.approval.choice, Some(ApprovalChoice::Deny));
    assert_eq!(after.subject, before.subject);
    let repeated = manager.tool_approval(&ctx, lookup).await.unwrap();
    assert_eq!(repeated.approval, after.approval);
    assert_eq!(repeated.subject, after.subject);
    assert!(
        manager
            .tool_grants(&ctx, query(&agents[0]))
            .await
            .unwrap()
            .grants
            .is_empty()
    );
    store.shutdown().await.unwrap();
}
