//! Bounded live admissions, real registry claims, and actual offline children.
use super::*;

async fn large_fixture() -> (Fixture, SharedExecutionTransport, SharedRuntimeCommand) {
    // Only the synthetic original delegation/reservation gets a longer finite
    // lifetime; no production clock or committed receipt is changed.
    let f = Fixture::with_provider_budget_lifetime(
        "openai.chatgpt",
        "fixture_account",
        50000,
        40000,
        600_000,
    )
    .await;
    let (f, t, c) = prepared_with_budget_lifetime(f, 35000, 30000, 600_000).await;
    let (f, t, c) = runtime_fixture(f, t, c).await;
    f.state.lock().unwrap().shared.idle_work = true;
    t.poll_fixture(&f.registry, launch(&f, "text"))
        .await
        .unwrap();
    f.state.lock().unwrap().shared.cleanup = true;
    t.poll(&f.registry, None).await.unwrap();
    // The inherited stopped() helper also drains/closes the whole Manager.
    // This fixture needs the original process settled while admissions remain
    // open for later turns on the same owner.
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if f.state
                .lock()
                .unwrap()
                .shared
                .processes
                .values()
                .any(|r| r.state == "stopped")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("original process has a stopped receipt before successor admission");
    f.owner
        .0
        .shared_execution
        .settle_capacity_for_test()
        .await
        .unwrap();
    {
        let mut s = f.state.lock().unwrap();
        s.shared.cleanup = false;
        s.shared.demands.active = true;
        s.shared.demands.revision = 1;
    }
    (f, t, c)
}

#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_manager_capacity_129_sequential_actual_turns_preserve_history() {
    let (f, t, c) = large_fixture().await;
    let mut originals = Vec::new();
    for n in 1..=129 {
        offer(&f, n);
        let page = t
            .poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage")))
            .await
            .unwrap();
        assert!(
            matches!(
                page.outcome.unwrap(),
                SharedDemandProgress::Execution {
                    progress: SharedExecutionProgress::Running { .. },
                    ..
                }
            ),
            "turn {n} did not start"
        );
        wait_attempt(&f).await;
        f.owner
            .0
            .shared_execution
            .settle_capacity_for_test()
            .await
            .unwrap();
        {
            let s = f.state.lock().unwrap();
            originals.push((
                s.shared.attempts.command.clone().unwrap(),
                s.shared.work.clone().unwrap(),
            ));
        }
    }
    assert_eq!(count_methods(&f, "turn/start"), 129);
    assert_eq!(rows(&f, "owner_shared_runtime_lifetimes"), 1);
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 129);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 129);
    assert_eq!(rows(&f, "owner_shared_work_inbox"), 129);
    let latest = f
        .state
        .lock()
        .unwrap()
        .shared
        .attempts
        .command
        .clone()
        .unwrap();
    assert_eq!(latest.assignment_id, c.assignment_id);
    assert_eq!(latest.process_attempt, 130);
    {
        let s = f.state.lock().unwrap();
        assert_eq!(s.shared.receipts.len(), 129);
        assert_eq!(
            s.shared
                .receipts
                .values()
                .map(|r| r.known_consumed_tokens)
                .sum::<u64>(),
            25800
        );
        assert!(
            s.shared
                .receipts
                .values()
                .all(|r| r.usage_complete && r.retained_tokens == Some(200))
        );
    }
    // The first manager entry has been retired. Its canonical original receipt
    // remains recoverable even if the server has lost its stopped projection.
    for (original, work) in originals {
        {
            let mut s = f.state.lock().unwrap();
            s.shared.attempts.command = Some(original.clone());
            s.shared.attempts.processes.clear();
            s.shared.work = Some(work);
        }
        assert!(matches!(
            t.recover_successor(
                &f.registry,
                original.command_id.clone(),
                digest(&original).unwrap(),
                None
            )
            .await
            .unwrap(),
            SharedExecutionProgress::Recorded { .. }
        ));
        let mut changed = original.clone();
        changed.issued_at_ms += 1;
        f.state.lock().unwrap().shared.attempts.command = Some(changed.clone());
        assert!(
            t.recover_successor(
                &f.registry,
                changed.command_id.clone(),
                digest(&changed).unwrap(),
                None
            )
            .await
            .is_err()
        );
    }
    assert_eq!(count_methods(&f, "turn/start"), 129);
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 129);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_manager_capacity_full_v1_does_not_consume_original_claim() {
    let (f, t, _) = fixture().await;
    let holds = f.owner.0.shared_execution.hold_capacity_for_test(128);
    let rejected = t.poll_fixture(&f.registry, launch(&f, "text-usage")).await;
    assert_eq!(rejected.err().unwrap().code, "shared_execution_closed");
    assert_eq!(rows(&f, "owner_shared_runtime_lifetimes"), 0);
    assert_eq!(rows(&f, "owner_managed_calls"), 0);
    for release in holds {
        release.send(()).unwrap();
    }
    f.owner
        .0
        .shared_execution
        .settle_capacity_for_test()
        .await
        .unwrap();
    assert!(matches!(
        t.poll_fixture(&f.registry, launch(&f, "text-usage"))
            .await
            .unwrap(),
        SharedExecutionProgress::Running { .. }
    ));
    stopped(&f).await;
    assert_eq!(rows(&f, "owner_shared_runtime_lifetimes"), 1);
    assert_eq!(count_methods(&f, "turn/start"), 1);
    f.owner
        .0
        .shared_execution
        .settle_capacity_for_test()
        .await
        .unwrap();
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_manager_capacity_full_v2_does_not_consume_original_claim() {
    let (f, t, _) = large_fixture().await;
    let holds = f.owner.0.shared_execution.hold_capacity_for_test(128);
    offer(&f, 1);
    let rejected = t
        .poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage")))
        .await
        .unwrap()
        .outcome;
    assert_eq!(rejected.err().unwrap().code, "shared_execution_closed");
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 0);
    assert_eq!(rows(&f, "owner_managed_calls"), 0);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 1);
    let original = f
        .state
        .lock()
        .unwrap()
        .shared
        .attempts
        .command
        .clone()
        .unwrap();
    for release in holds {
        release.send(()).unwrap();
    }
    f.owner
        .0
        .shared_execution
        .settle_capacity_for_test()
        .await
        .unwrap();
    assert!(matches!(
        t.poll_successor_fixture(&f.registry, launch(&f, "text-usage"))
            .await
            .unwrap(),
        SharedExecutionProgress::Running { .. }
    ));
    wait_attempt(&f).await;
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 1);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 1);
    assert_eq!(count_methods(&f, "turn/start"), 1);
    assert_eq!(
        digest(&original).unwrap(),
        digest(
            &f.state
                .lock()
                .unwrap()
                .shared
                .attempts
                .command
                .clone()
                .unwrap()
        )
        .unwrap()
    );
    f.owner
        .0
        .shared_execution
        .settle_capacity_for_test()
        .await
        .unwrap();
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_manager_capacity_failed_stop_write_stays_unknown() {
    let (f, t, c) = fixture().await;
    t.poll_fixture(&f.registry, launch(&f, "text-block"))
        .await
        .unwrap();
    method_seen(&f, "turn/start").await;
    {
        let db =
            rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER fail_test_stop BEFORE INSERT ON owner_shared_process_observations WHEN json_extract(NEW.payload,'$.state')='stopped' BEGIN SELECT RAISE(ABORT,'fixture_stop_write_failure'); END;").unwrap();
    }
    f.state.lock().unwrap().shared.cleanup = true;
    assert!(matches!(
        t.poll(&f.registry, None).await.unwrap(),
        SharedExecutionProgress::Stopping { .. }
    ));
    assert!(
        f.owner
            .0
            .shared_execution
            .settle_capacity_for_test()
            .await
            .is_err()
    );
    let holds = f.owner.0.shared_execution.hold_capacity_for_test(1);
    assert!(
        f.owner
            .0
            .shared_execution
            .retains_command_for_test(&c.command_id)
    );
    for release in holds {
        release.send(()).unwrap();
    }
    assert!(f.owner.0.shared_execution.drain().await.is_err());
    assert!(f.owner.0.shared_execution.drain().await.is_err());
    let db = rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
    let stopped:u64=db.query_row("SELECT COUNT(*) FROM owner_shared_process_observations WHERE json_extract(payload,'$.state')='stopped'",[],|r|r.get(0)).unwrap();
    assert_eq!(stopped, 0);
    assert_eq!(rows(&f, "owner_shared_runtime_lifetimes"), 1);
    assert_eq!(count_methods(&f, "turn/start"), 1);
    drop(db);
    f.stop().await;
}

fn saved_work_payloads(f: &Fixture) -> Vec<String> {
    let db = rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
    db.prepare("SELECT payload FROM owner_shared_work_observations ORDER BY ledger_revision")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_manager_capacity_failed_upload_recovers_after_eviction_and_reopen() {
    let (mut f, t, _) = large_fixture().await;
    offer(&f, 1);
    f.state.lock().unwrap().shared.attempts.fail_work_reports = true;
    assert!(matches!(
        t.poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage")))
            .await
            .unwrap()
            .outcome
            .unwrap(),
        SharedDemandProgress::Execution {
            progress: SharedExecutionProgress::Running { .. },
            ..
        }
    ));
    // A lost upload intentionally keeps this process alive for reply recovery.
    // First observe the durable completed work, then exercise the authenticated
    // original cleanup path instead of assuming a network failure stops it.
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if saved_work_payloads(&f).iter().any(|raw| {
                let receipt: SharedWorkReceipt = serde_json::from_str(raw).unwrap();
                receipt.usage_complete && receipt.known_consumed_tokens == 200
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("lost upload preserves complete local work receipt");
    assert!(matches!(
        t.poll_successor(&f.registry, None).await.unwrap(),
        SharedExecutionProgress::Running { .. }
    ));
    f.state.lock().unwrap().shared.cleanup = true;
    assert!(matches!(
        t.poll_successor(&f.registry, None).await.unwrap(),
        SharedExecutionProgress::Stopping { .. }
    ));
    wait_attempt(&f).await;
    f.owner
        .0
        .shared_execution
        .settle_capacity_for_test()
        .await
        .unwrap();
    let command = f
        .state
        .lock()
        .unwrap()
        .shared
        .attempts
        .command
        .clone()
        .unwrap();
    assert!(f.state.lock().unwrap().shared.receipts.is_empty());
    let original = saved_work_payloads(&f);
    assert!(!original.is_empty());
    let receipt: SharedWorkReceipt = serde_json::from_str(original.last().unwrap()).unwrap();
    assert_eq!(receipt.known_consumed_tokens, 200);
    assert_eq!(receipt.retained_tokens, Some(200));
    assert!(receipt.usage_complete);
    let holds = f.owner.0.shared_execution.hold_capacity_for_test(1);
    assert!(
        !f.owner
            .0
            .shared_execution
            .retains_command_for_test(&command.command_id)
    );
    for release in holds {
        release.send(()).unwrap();
    }
    f.owner
        .0
        .shared_execution
        .settle_capacity_for_test()
        .await
        .unwrap();
    f.registry.shutdown().await.unwrap();
    f.registry =
        OwnerResourceRegistry::isolated_for_tests(&f._root.path().join("registry")).unwrap();
    f.state.lock().unwrap().shared.attempts.fail_work_reports = false;
    assert!(matches!(
        t.recover_successor(
            &f.registry,
            command.command_id.clone(),
            digest(&command).unwrap(),
            None
        )
        .await
        .unwrap(),
        SharedExecutionProgress::Recorded { .. }
    ));
    assert_eq!(saved_work_payloads(&f), original);
    assert_eq!(count_methods(&f, "turn/start"), 1);
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 1);
    assert!(
        f.state
            .lock()
            .unwrap()
            .shared
            .receipts
            .values()
            .any(|r| r.observation_id == receipt.observation_id)
    );
    f.stop().await;
}
