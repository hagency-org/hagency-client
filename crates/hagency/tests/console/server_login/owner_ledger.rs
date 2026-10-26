//! Authenticated NativeOwner commands, real disk ledgers and local OAuth HTTP.
use hagency::native_owner::{
    Command, LedgerAction, NativeOwner, UsageAction, UsageGroup, UsageQuery,
};
use hagency_agent_local::{
    Budget, Ledger, Limit, Period, Policy, RequestPolicy, Scope, ToolPolicy, Usage,
};

pub(super) async fn roundtrip(native: &NativeOwner, root: &std::path::Path) {
    let initial = root
        .join("initial.hagency-ledger")
        .to_str()
        .unwrap()
        .to_owned();
    let exported = native
        .execute(Command::Ledger {
            action: LedgerAction::Export {
                path: initial.clone(),
            },
        })
        .await
        .unwrap();
    assert_eq!(exported["operation"], "exported");
    assert_eq!(exported["snapshot"]["calls"], 0);
    let identity = native.identity().await.unwrap();
    let profile = exported["snapshot"]["profile"].as_str().unwrap();
    let scope = Scope {
        agent: "backup-agent".into(),
        binding: "backup-binding".into(),
        room: "!backup:example.test".into(),
        requester: identity.owner.clone(),
        thread: "original-thread".into(),
    };
    let mut original =
        Ledger::open_scoped(root.join("original.sqlite"), &identity.owner, profile).unwrap();
    original.register_binding(&identity.owner, &scope).unwrap();
    original
        .set_agent_policy(
            &identity.owner,
            &scope.agent,
            0,
            &Policy {
                budget: Budget {
                    limit: Limit::Tokens(250),
                    period: Period::Lifetime,
                },
                requests: RequestPolicy::Allow,
                high_risk: ToolPolicy::AskOwner,
            },
        )
        .unwrap();
    original
        .reserve(&scope, "original-call", "original-dispatch", 100, 100)
        .unwrap();
    original
        .settle(
            &scope,
            "original-call",
            &Usage {
                input: 20,
                output: 7,
                cached_input: 3,
                reasoning_output: 2,
                accounting_version: "fixture-v1".into(),
            },
        )
        .unwrap();
    original
        .reserve(&scope, "unfinished-call", "unfinished-dispatch", 200, 101)
        .unwrap();
    let snapshot = original.snapshot(&identity.owner, profile, 123).unwrap();
    let backup = root.join("original.hagency-ledger");
    hagency_store::private::write_new(&backup, snapshot.bytes()).unwrap();
    let path = backup.to_str().unwrap().to_owned();
    let inspected = native
        .execute(Command::Ledger {
            action: LedgerAction::Inspect { path: path.clone() },
        })
        .await
        .unwrap();
    assert_eq!(inspected["snapshot"]["calls"], 2);
    assert_eq!(inspected["snapshot"]["pendingCalls"], 1);
    assert!(!inspected.to_string().contains("secret"));
    let wrong = native
        .execute(Command::Ledger {
            action: LedgerAction::Restore {
                path: path.clone(),
                digest: "0".repeat(64),
            },
        })
        .await
        .unwrap_err();
    assert_eq!(wrong.code, "ledger_backup_changed");
    let digest = inspected["digest"].as_str().unwrap().to_owned();
    let restored = native
        .execute(Command::Ledger {
            action: LedgerAction::Restore {
                path: path.clone(),
                digest: digest.clone(),
            },
        })
        .await
        .unwrap();
    assert_eq!(restored["operation"], "restored");
    assert_eq!(restored["snapshot"], inspected["snapshot"]);
    let blocked = native
        .execute(Command::Ledger {
            action: LedgerAction::Restore {
                path: path.clone(),
                digest,
            },
        })
        .await
        .unwrap_err();
    assert_eq!(blocked.code, "ledger_restore_requires_empty_ledger");
    let query = UsageQuery {
        agent: scope.agent.clone(),
        binding: None,
        requester: None,
        from: 0,
        until: 1000,
        group: UsageGroup::Day,
        alerts_before: None,
    };
    let report = native
        .execute(Command::AgentUsage {
            action: UsageAction::Report {
                query: query.clone(),
            },
        })
        .await
        .unwrap();
    assert_eq!(report["totals"]["input"], 20);
    assert_eq!(report["totals"]["output"], 7);
    assert_eq!(report["totals"]["cachedInput"], 3);
    assert_eq!(report["totals"]["reasoningOutput"], 2);
    assert_eq!(report["totals"]["pendingCalls"], 1);
    assert_eq!(report["totals"]["reserved"], 200);
    assert_eq!(report["alerts"].as_array().unwrap().len(), 1);
    let id = report["alerts"][0]["id"].as_str().unwrap().to_owned();
    let mut invalid = query.clone();
    invalid.until = invalid.from;
    assert_eq!(
        native
            .execute(Command::AgentUsage {
                action: UsageAction::Acknowledge {
                    query: invalid,
                    id: id.clone()
                }
            })
            .await
            .unwrap_err()
            .code,
        "invalid_usage_query"
    );
    let unchanged = native
        .execute(Command::AgentUsage {
            action: UsageAction::Report {
                query: query.clone(),
            },
        })
        .await
        .unwrap();
    assert!(unchanged["alerts"][0]["acknowledgedAt"].is_null());
    let acknowledged = native
        .execute(Command::AgentUsage {
            action: UsageAction::Acknowledge {
                query: query.clone(),
                id,
            },
        })
        .await
        .unwrap();
    assert_eq!(acknowledged["totals"], report["totals"]);
    assert!(
        acknowledged["alerts"][0]["acknowledgedAt"]
            .as_i64()
            .is_some()
    );
    let reread = native
        .execute(Command::AgentUsage {
            action: UsageAction::Report { query },
        })
        .await
        .unwrap();
    assert_eq!(acknowledged["alerts"], reread["alerts"]);
    let after = root.join("after.hagency-ledger");
    native
        .execute(Command::Ledger {
            action: LedgerAction::Export {
                path: after.to_str().unwrap().into(),
            },
        })
        .await
        .unwrap();
    let after = hagency_agent_local::backup::Snapshot::inspect(
        std::fs::read(after).unwrap(),
        &identity.owner,
        profile,
    )
    .unwrap();
    let mut verify =
        Ledger::open_scoped(root.join("verified.sqlite"), &identity.owner, profile).unwrap();
    verify
        .restore_snapshot(&identity.owner, profile, &after)
        .unwrap();
    assert_eq!(
        verify
            .agent_account(&identity.owner, &scope.agent, Period::Lifetime, 200)
            .unwrap(),
        (27, 200)
    );
    assert_eq!(
        verify
            .agent_policy(&identity.owner, &scope.agent)
            .unwrap()
            .revision,
        1
    );
    assert!(
        native
            .execute(Command::Ledger {
                action: LedgerAction::Export { path: initial }
            })
            .await
            .is_err()
    );
    let mut broken = snapshot.bytes().to_vec();
    *broken.last_mut().unwrap() ^= 1;
    let corrupt = root.join("corrupt.hagency-ledger");
    std::fs::write(&corrupt, broken).unwrap();
    assert_eq!(
        native
            .execute(Command::Ledger {
                action: LedgerAction::Inspect {
                    path: corrupt.to_str().unwrap().into()
                }
            })
            .await
            .unwrap_err()
            .code,
        "invalid_ledger_backup"
    );
}
