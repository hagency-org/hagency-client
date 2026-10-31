//! Loopback B routes, canonical SQLite, and an actual offline supervised child.
use super::*;
use crate::console::native::SharedDemandProgress;
#[derive(Default)]
pub(super) struct State {
    active: bool,
    offer: Option<SharedDemandOffer>,
    status: String,
    binding: Option<SharedDemandBinding>,
    inputs: BTreeMap<String, Value>,
    commands: BTreeMap<String, SharedAttemptCommand>,
    revision: u64,
    paused: bool,
    lose_start: bool,
    lose_stop: bool,
    invalid_binding: bool,
    lose_binding: bool,
    stop_inputs: Vec<Value>,
    stop_results: BTreeMap<String, u64>,
    generic_starts: usize,
    control_reads: usize,
}
fn status(s: &super::super::State) -> Value {
    let d = &s.shared.demands;
    json!({"offer":d.offer,"demandDigest":d.offer.as_ref().unwrap().digest().unwrap(),"state":d.status,"runtimeCommandId":d.binding.as_ref().map(|b|b.runtime_command_id.clone()).or_else(||s.shared.attempts.command.as_ref().map(|c|c.command_id.clone())),"binding":d.binding})
}
fn control(s: &super::super::State) -> Value {
    json!({"assignmentId":s.shared.command.as_ref().unwrap().assignment_id,"revision":s.shared.demands.revision,"mode":if s.shared.demands.paused{"paused"}else{"running"},"runningSinceMs":null,"updatedAtMs":now(),"origin":"manager"})
}
pub(super) fn management(
    path: &str,
    body: &Value,
    s: &mut super::super::State,
) -> Option<Option<Value>> {
    if !s.shared.demands.active {
        return None;
    }
    let at = now();
    if path.ends_with("/attempts/start") {
        s.shared.demands.generic_starts += 1;
    }
    if path.ends_with("/control") {
        if body.is_null() {
            s.shared.demands.control_reads += 1;
            return Some(Some(json!({"control":control(s),"observedAtMs":at})));
        }
        assert_eq!(body["mode"], "paused");
        s.shared.demands.stop_inputs.push(body.clone());
        let id = body["commandId"].as_str().unwrap().to_string();
        let applied = if let Some(v) = s.shared.demands.stop_results.get(&id) {
            *v
        } else {
            if body["expectedRevision"] != s.shared.demands.revision {
                return Some(Some(
                    json!({"fixtureStatus":409,"error":"shared_control_revision_changed"}),
                ));
            }
            if !s.shared.demands.paused {
                s.shared.demands.revision += 1;
                s.shared.demands.paused = true;
            }
            s.shared.demands.status = "cancelled".into();
            let v = s.shared.demands.revision;
            s.shared.demands.stop_results.insert(id.clone(), v);
            v
        };
        if std::mem::take(&mut s.shared.demands.lose_stop) {
            return Some(None);
        }
        return Some(Some(
            json!({"commandId":id,"appliedRevision":applied,"control":control(s),"observedAtMs":at}),
        ));
    }
    if path.ends_with("/demands/poll") {
        return Some(Some(
            json!({"demand":if matches!(s.shared.demands.status.as_str(),"offered"|"start_committed"){status(s)}else{Value::Null},"nextAfter":s.shared.command.as_ref().unwrap().assignment_id,"observedAtMs":at}),
        ));
    }
    if path.contains("/demands/") {
        let o = s.shared.demands.offer.clone().unwrap();
        if path.ends_with("/recover") {
            assert_eq!(body["demandDigest"], o.digest().unwrap());
            return Some(Some(json!({"demand":status(s),"observedAtMs":at})));
        }
        if path.ends_with("/block") {
            assert_eq!(body["expectedControlRevision"], o.control_revision);
            s.shared.demands.status = body["reason"].as_str().unwrap().into();
            return Some(Some(json!({"demand":status(s),"observedAtMs":at})));
        }
        if path.ends_with("/start") {
            let input: SharedDemandStartInput = serde_json::from_value(body.clone()).unwrap();
            input.validate_for(&o).unwrap();
            let id = input.start.start_intent_id.clone();
            let replay = s.shared.demands.inputs.contains_key(&id);
            if let Some(old) = s.shared.demands.inputs.get(&id) {
                assert_eq!(old, body)
            } else {
                if s.shared.demands.paused || s.shared.demands.revision != o.control_revision {
                    return Some(Some(
                        json!({"fixtureStatus":409,"error":"shared_demand_control_changed"}),
                    ));
                }
                let original = s.shared.command.clone().unwrap();
                let c = SharedAttemptCommand {
        web_search: false, tool_policy: None,
                    version: 2,
                    capability: SHARED_ATTEMPT_CAPABILITY.into(),
                    operation: "agent.execution.provision".into(),
                    command_id: format!(
                        "demand_attempt_{}",
                        input.start.predecessor.process_attempt + 1
                    ),
                    assignment_id: original.assignment_id.clone(),
                    assignment_generation: 1,
                    agent_reservation_id: original.agent_reservation_id.clone(),
                    process_attempt: input.start.predecessor.process_attempt + 1,
                    original_runtime_digest: digest(&original).unwrap(),
                    original_runtime_command: original,
                    predecessor: input.start.predecessor,
                    budget_observation: input.start.budget_observation,
                    start_intent_id: id.clone(),
                    issued_at_ms: at,
                    expires_at_ms: at + 30000,
                };
                s.shared.demands.inputs.insert(id.clone(), body.clone());
                s.shared.demands.commands.insert(id.clone(), c.clone());
                s.shared.attempts.command = Some(c.clone());
                s.shared.attempts.processes.clear();
                let w = s.shared.work.as_mut().unwrap();
                w.runtime_command_digest = digest(&c).unwrap();
                s.shared.demands.status = "start_committed".into();
            }
            let c = s.shared.demands.commands[&id].clone();
            assert_eq!(
                s.shared.attempts.command.as_ref().unwrap().command_id,
                c.command_id
            );
            if std::mem::take(&mut s.shared.demands.lose_start) {
                return Some(None);
            }
            return Some(if replay {
                attempts::management(
                    "/attempts/recover",
                    &json!({"commandDigest":digest(&c).unwrap(),"observationId":null}),
                    s,
                )
            } else {
                attempts::management("/attempts/poll", &json!({"after":null}), s)
            });
        }
    }
    if path.ends_with("/demand") {
        let o = s.shared.demands.offer.clone().unwrap();
        let w = s.shared.work.clone().unwrap();
        assert_eq!(body["workDigest"], digest(&w).unwrap());
        assert_eq!(body["runtimeCommandDigest"], w.runtime_command_digest);
        if s.shared.demands.paused || s.shared.demands.revision != o.control_revision {
            return Some(Some(
                json!({"fixtureStatus":409,"error":"shared_demand_control_changed"}),
            ));
        }
        let runtime = s
            .shared
            .attempts
            .command
            .as_ref()
            .map(|c| c.command_id.clone())
            .unwrap_or_else(|| s.shared.command.as_ref().unwrap().command_id.clone());
        let mut binding = SharedDemandBinding {
            version: 1,
            demand_id: o.demand_id.clone(),
            demand_digest: o.digest().unwrap(),
            control_revision: o.control_revision,
            assignment_id: o.assignment_id.clone(),
            event_id: o.event_id.clone(),
            received_at_ms: o.received_at_ms,
            runtime_command_id: runtime,
            runtime_command_digest: w.runtime_command_digest.clone(),
            work_id: w.id.clone(),
            work_digest: digest(&w).unwrap(),
        };
        if s.shared.demands.invalid_binding {
            binding.event_id = "$foreign:test".into();
        }
        s.shared.demands.binding = Some(binding.clone());
        s.shared.demands.status = "bound".into();
        if std::mem::take(&mut s.shared.demands.lose_binding) {
            return Some(Some(json!({"fixtureStatus":409,"error":"fixture_binding_reply_lost"})));
        }
        return Some(Some(
            json!({"demand":{"offer":o,"demandDigest":o.digest().unwrap(),"binding":binding},"observedAtMs":at}),
        ));
    }
    if path.ends_with("/work/start")
        && (s.shared.demands.paused
            || s.shared.demands.offer.as_ref().unwrap().control_revision
                != s.shared.demands.revision)
    {
        return Some(Some(
            json!({"fixtureStatus":409,"error":"shared_demand_control_changed"}),
        ));
    }
    None
}
pub(super) async fn fixture_budget(tokens: u64) -> (Fixture, SharedExecutionTransport, SharedRuntimeCommand) {
    let f = Fixture::with_provider_budget("openai.chatgpt", "fixture_account", 10000, 7000).await;
    let (f, t, c) = prepared_with_budget(f, 6000, tokens).await;
    let (f, t, c) = runtime_fixture(f, t, c).await;
    f.state.lock().unwrap().shared.idle_work = true;
    t.poll_fixture(&f.registry, launch(&f, "text"))
        .await
        .unwrap();
    f.state.lock().unwrap().shared.cleanup = true;
    t.poll(&f.registry, None).await.unwrap();
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
    .unwrap();
    {
        let mut s = f.state.lock().unwrap();
        s.shared.cleanup = false;
        s.shared.demands.active = true;
        s.shared.demands.revision = 1;
    }
    (f, t, c)
}
pub(super) fn offer(f: &Fixture, number: u64) {
    let mut s = f.state.lock().unwrap();
    let c = s.shared.command.clone().unwrap();
    let (id, digest) = s
        .shared
        .attempts
        .command
        .as_ref()
        .map(|c| (c.command_id.clone(), digest(c).unwrap()))
        .unwrap_or_else(|| (c.command_id.clone(), digest(&c).unwrap()));
    let o = SharedDemandOffer {
        version: 1,
        demand_id: format!("demand_{number:03}"),
        assignment_id: c.assignment_id,
        event_id: format!("$event{number}:test"),
        event_digest: "d".repeat(64),
        control_revision: s.shared.demands.revision,
        received_at_ms: now(),
        predecessor_command_id: id,
        predecessor_command_digest: digest,
    };
    let w = s.shared.work.as_mut().unwrap();
    w.id = format!("demand_work_{number}");
    w.execution_id = format!("demand_execution_{number}");
    w.event_id = o.event_id.clone();
    w.thread_root = o.event_id.clone();
    w.created_at_ms = o.received_at_ms;
    s.shared.demands.offer = Some(o);
    s.shared.demands.status = "offered".into();
    s.shared.demands.binding = None;
    s.shared.idle_work = false;
    s.shared.starts = 0;
}
pub(super) async fn wait_attempt(f: &Fixture) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if f.state
                .lock()
                .unwrap()
                .shared
                .attempts
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
    .unwrap();
}
pub(super) fn count_methods(f: &Fixture, method: &str) -> usize {
    std::fs::read_to_string(f._root.path().join("shared-probe/methods"))
        .unwrap()
        .lines()
        .filter(|v| *v == method)
        .count()
}

#[cfg(feature = "shared-context")]
#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_recovered_promotion_unblocks_next_encrypted_message() {
    let (f, t, _) = Box::pin(fixture_budget(1000)).await;
    let (t, sent) = t.with_crypto_fixture(Some("Private request after interrupted delivery".into()));
    offer(&f, 1);
    {
        let mut s = f.state.lock().unwrap();
        s.shared.demands.lose_binding = true;
        let w = s.shared.work.as_mut().unwrap();
        w.encrypted = true;
        w.body = w.event_id.clone();
        w.body_digest = format!("{:x}", Sha256::digest(w.body.as_bytes()));
    }
    t.poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage"))).await.unwrap().outcome.unwrap();
    wait_attempt(&f).await;
    assert_eq!(rows(&f, "owner_shared_demand_bindings"), 0);
    assert_eq!(rows(&f, "owner_managed_calls"), 0);
    assert_eq!(count_methods(&f, "turn/start"), 0);
    let binding = f.state.lock().unwrap().shared.demands.binding.clone().unwrap();
    // A forged event cannot release queue position or authorize another turn.
    f.state.lock().unwrap().shared.demands.binding.as_mut().unwrap().event_id = "$foreign:test".into();
    assert!(t.resume_next_demand(&f.registry, None).await.unwrap().unwrap().outcome.is_err());
    assert_eq!(rows(&f, "owner_shared_demand_bindings"), 0);
    f.state.lock().unwrap().shared.demands.binding = Some(binding);
    for _ in 0..2 {
        assert!(matches!(t.resume_next_demand(&f.registry, None).await.unwrap().unwrap().outcome.unwrap(), SharedDemandProgress::Historical { status, .. } if status == "bound"));
    }
    assert_eq!(rows(&f, "owner_shared_demand_bindings"), 1);
    assert_eq!(rows(&f, "owner_shared_work_inbox"), 0, "history must not admit old work");
    assert_eq!(rows(&f, "owner_managed_calls"), 0);
    assert!(sent.lock().unwrap().is_empty());
    offer(&f, 2);
    {
        let mut s = f.state.lock().unwrap();
        let w = s.shared.work.as_mut().unwrap();
        w.body = w.event_id.clone();
        w.body_digest = format!("{:x}", Sha256::digest(w.body.as_bytes()));
    }
    let progress = t.poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage"))).await.unwrap().outcome.unwrap();
    assert!(matches!(progress, SharedDemandProgress::Execution { progress: SharedExecutionProgress::Running { .. }, .. }));
    wait_attempt(&f).await;
    assert_eq!(count_methods(&f, "turn/start"), 1);
    assert_eq!(rows(&f, "owner_managed_calls"), 1);
    assert_eq!(rows(&f, "owner_shared_demand_bindings"), 2);
    assert_eq!(rows(&f, "owner_managed_budget_deltas"), 0);
    assert_eq!(sent.lock().unwrap().len(), 1);
    assert_eq!(f.state.lock().unwrap().shared.demands.generic_starts, 0);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_two_events_keep_original_assignment_and_charges() {
    let (f, t, c) = fixture_budget(1000).await;
    for n in 1..=2 {
        offer(&f, n);
        let p = t
            .poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage")))
            .await
            .unwrap();
        assert_eq!(p.next_after.as_deref(), Some(c.assignment_id.as_str()));
        assert!(matches!(
            p.outcome.unwrap(),
            SharedDemandProgress::Execution {
                progress: SharedExecutionProgress::Running { .. },
                ..
            }
        ));
        wait_attempt(&f).await;
    }
    assert_eq!(count_methods(&f, "turn/start"), 2);
    assert_eq!(rows(&f, "owner_shared_demands"), 2);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 2);
    assert_eq!(rows(&f, "owner_shared_work_inbox"), 2);
    assert_eq!(rows(&f, "owner_shared_demand_bindings"), 2);
    {
        let s = f.state.lock().unwrap();
        assert_eq!(s.shared.demands.generic_starts, 0);
        assert_eq!(s.shared.demands.inputs.len(), 2);
        assert_eq!(s.shared.receipts.len(), 2);
        assert!(
            s.shared
                .receipts
                .values()
                .all(|r| r.known_consumed_tokens == 200 && r.usage_complete)
        );
    }
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_lost_start_reopens_original_and_never_generic_start() {
    let (mut f, t, _) = fixture_budget(1000).await;
    offer(&f, 1);
    f.state.lock().unwrap().shared.demands.lose_start = true;
    assert!(
        t.poll_demand(&f.registry, None)
            .await
            .unwrap()
            .outcome
            .is_err()
    );
    let original = f.state.lock().unwrap().shared.demands.inputs.clone();
    f.registry.shutdown().await.unwrap();
    f.registry =
        OwnerResourceRegistry::isolated_for_tests(&f._root.path().join("registry")).unwrap();
    let resumed = t
        .resume_next_demand(&f.registry, None)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        resumed.outcome.unwrap(),
        SharedDemandProgress::Execution {
            progress: SharedExecutionProgress::HistoricalEvidenceMissing { .. },
            ..
        }
    ));
    assert_eq!(original, f.state.lock().unwrap().shared.demands.inputs);
    let scope = f
        .transport
        .scope(Permission::ExecuteManagedAgents)
        .await
        .unwrap();
    assert!(
        f.registry
            .next_shared_attempt_start(scope.profile, None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        t.poll_successor_fixture(&f.registry, launch(&f, "text-usage"))
            .await
            .unwrap(),
        SharedExecutionProgress::Running { .. }
    ));
    wait_attempt(&f).await;
    assert_eq!(count_methods(&f, "turn/start"), 1);
    assert_eq!(f.state.lock().unwrap().shared.demands.generic_starts, 0);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_exact_717_overrun_blocks_next_offer_without_id_or_process() {
    let (f, t, _) = fixture_budget(4096).await;
    offer(&f, 1);
    t.poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-overrun")))
        .await
        .unwrap()
        .outcome
        .unwrap();
    wait_attempt(&f).await;
    let receipt = f
        .state
        .lock()
        .unwrap()
        .shared
        .receipts
        .values()
        .next()
        .unwrap()
        .clone();
    assert_eq!(receipt.known_consumed_tokens, 4813);
    assert_eq!(receipt.retained_tokens, Some(4813));
    assert!(receipt.usage_complete);
    offer(&f, 2);
    let p = t
        .poll_demand(&f.registry, None)
        .await
        .unwrap()
        .outcome
        .unwrap();
    assert!(matches!(p,SharedDemandProgress::Blocked{reason,..} if reason=="budget_blocked"));
    let scope = f
        .transport
        .scope(Permission::ExecuteManagedAgents)
        .await
        .unwrap();
    let grant = f.state.lock().unwrap().grant.clone().unwrap();
    let e = f.transport.enrollment(&scope, &grant, true).await.unwrap();
    let head = f
        .state
        .lock()
        .unwrap()
        .shared
        .attempts
        .command
        .clone()
        .unwrap();
    let budget = f
        .registry
        .shared_attempt_successor_budget(e, head)
        .await
        .unwrap();
    assert_eq!(budget.allocated_tokens, 4096);
    assert_eq!(budget.retained_tokens, 4813);
    assert_eq!(budget.remaining_tokens, 0);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 1);
    assert_eq!(count_methods(&f, "turn/start"), 1);
    assert_eq!(count_methods(&f, "initialize"), 2);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_owner_stop_loss_restart_and_later_run_never_replays_stop() {
    let (mut f, t, c) = fixture_budget(1000).await;
    offer(&f, 1);
    f.state.lock().unwrap().shared.idle_work = true;
    t.poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage")))
        .await
        .unwrap()
        .outcome
        .unwrap();
    f.state.lock().unwrap().shared.demands.lose_stop = true;
    assert!(
        t.stop_demand_assignment(&f.registry, c.assignment_id.clone())
            .await
            .is_err()
    );
    wait_attempt(&f).await;
    assert_eq!(rows(&f, "owner_shared_stop_intents"), 1);
    assert_eq!(rows(&f, "owner_shared_stop_receipts"), 0);
    f.registry.shutdown().await.unwrap();
    f.registry =
        OwnerResourceRegistry::isolated_for_tests(&f._root.path().join("registry")).unwrap();
    // Manager Run is a separate central action; old Stop receipt may now show it.
    {
        let mut s = f.state.lock().unwrap();
        s.shared.demands.revision = 3;
        s.shared.demands.paused = false;
    }
    t.resume_next_demand_stop(&f.registry, None)
        .await
        .unwrap()
        .unwrap()
        .outcome
        .unwrap();
    {
        let s = f.state.lock().unwrap();
        assert_eq!(
            s.shared.demands.stop_inputs[0],
            s.shared.demands.stop_inputs[1]
        );
        assert!(!s.shared.demands.paused);
    }
    t.stop_demand_assignment(&f.registry, c.assignment_id.clone())
        .await
        .unwrap();
    {
        let s = f.state.lock().unwrap();
        assert_ne!(
            s.shared.demands.stop_inputs[0]["commandId"],
            s.shared.demands.stop_inputs[2]["commandId"]
        );
    }
    assert_eq!(rows(&f, "owner_shared_stop_receipts"), 2);
    assert_eq!(count_methods(&f, "turn/start"), 0);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_cancelled_waiter_and_closed_miniapp_preserve_accepted_custody() {
    let (f, t, _) = fixture_budget(1000).await;
    offer(&f, 1);
    let ui = f.owner.management(package(77)).await.unwrap();
    let previous = count_methods(&f, "account/rateLimits/read");
    let registry = f.registry.clone();
    let prepared = launch(&f, "text-held-qualify");
    let t = Arc::new(t);
    let waiting = t.clone();
    let task = tokio::spawn(async move {
        waiting
            .poll_demand_fixture(&registry, None, Some(prepared))
            .await
    });
    method_count_seen(&f, "account/rateLimits/read", previous + 1).await;
    task.abort();
    let _ = task.await;
    ui.retire();
    drop(ui);
    std::fs::write(
        f._root.path().join("shared-probe/release-qualification"),
        b"release",
    )
    .unwrap();
    wait_attempt(&f).await;
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 1);
    assert_eq!(count_methods(&f, "turn/start"), 1);
    assert_eq!(rows(&f, "owner_shared_work_inbox"), 1);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_late_stop_and_logout_preserve_unknown_charge_and_deny_stale_offer() {
    for logout in [false, true] {
        let (f, t, c) = fixture_budget(1000).await;
        offer(&f, 1);
        t.poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-block")))
            .await
            .unwrap()
            .outcome
            .unwrap();
        method_seen(&f, "turn/start").await;
        if logout {
            f.current.store(false, Ordering::Release);
            f.owner.retire();
            f.owner.0.shared_execution.drain().await.unwrap();
        } else {
            t.stop_demand_assignment(&f.registry, c.assignment_id.clone())
                .await
                .unwrap();
            wait_attempt(&f).await;
        }
        let db =
            rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
        let payload:String=db.query_row("SELECT payload FROM owner_shared_work_observations ORDER BY ledger_revision DESC LIMIT 1",[],|r|r.get(0)).unwrap();
        let receipt: SharedWorkReceipt = serde_json::from_str(&payload).unwrap();
        assert_eq!(receipt.state, "unknown");
        assert_eq!(receipt.retained_tokens, Some(1000));
        assert!(!receipt.usage_complete);
        if logout {
            assert!(t.poll_demand(&f.registry, None).await.is_err());
        } else {
            assert!(
                matches!(t.resume_next_demand(&f.registry,None).await.unwrap().unwrap().outcome.unwrap(),SharedDemandProgress::Historical{status,..} if status=="cancelled")
            );
        }
        assert_eq!(count_methods(&f, "turn/start"), 1);
        assert_eq!(rows(&f, "owner_shared_start_intents"), 1);
        f.stop().await;
    }
}
#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_mismatched_promotion_cannot_create_inbox_or_call() {
    let (f, t, _) = fixture_budget(1000).await;
    offer(&f, 1);
    f.state.lock().unwrap().shared.demands.invalid_binding = true;
    t.poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage")))
        .await
        .unwrap()
        .outcome
        .unwrap();
    wait_attempt(&f).await;
    assert_eq!(rows(&f, "owner_shared_work_inbox"), 0);
    assert_eq!(rows(&f, "owner_managed_calls"), 0);
    assert_eq!(count_methods(&f, "turn/start"), 0);
    f.stop().await;
}

async fn method_count_seen(f: &Fixture, method: &str, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if count_methods(f, method) >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_older_server_allows_only_manual_without_local_history() {
    let (f, t, _) = fixture().await;
    f.state.lock().unwrap().missing_demand_capability = true;
    t.poll_fixture(&f.registry, launch(&f, "text-usage"))
        .await
        .unwrap();
    stopped(&f).await;
    assert_eq!(count_methods(&f, "turn/start"), 1);
    assert_eq!(rows(&f, "owner_shared_demands"), 0);
    assert!(t.poll_demand(&f.registry, None).await.is_err());
    f.stop().await;
    let (f, t, _) = fixture_budget(1000).await;
    offer(&f, 1);
    f.state.lock().unwrap().shared.idle_work = true;
    t.poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage")))
        .await
        .unwrap()
        .outcome
        .unwrap();
    {
        let mut s = f.state.lock().unwrap();
        s.missing_demand_capability = true;
        s.shared.idle_work = false;
    }
    wait_attempt(&f).await;
    assert_eq!(rows(&f, "owner_shared_demands"), 1);
    assert_eq!(rows(&f, "owner_shared_work_inbox"), 0);
    assert_eq!(count_methods(&f, "turn/start"), 0);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_stop_waiter_cancellation_retains_queued_original_command() {
    let (f, t, c) = fixture_budget(1000).await;
    let mut db =
        rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
    let lock = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let registry = f.registry.clone();
    let t = Arc::new(t);
    let waiting = t.clone();
    let task = tokio::spawn(async move {
        waiting
            .stop_demand_assignment(&registry, c.assignment_id)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.owner.0.shared_execution.retained_stop_for_test() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // The SQLite write is still blocked, but actual Stop ownership has
    // transferred to the Manager. Cancel only the original UI waiter.
    assert_eq!(rows(&f, "owner_shared_stop_intents"), 0);
    task.abort();
    let _ = task.await;
    lock.commit().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if rows(&f, "owner_shared_stop_receipts") == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(rows(&f, "owner_shared_stop_intents"), 1);
    assert_eq!(f.state.lock().unwrap().shared.demands.stop_inputs.len(), 1);
    assert_eq!(count_methods(&f, "turn/start"), 0);
    let stopped_through: u64 = db.query_row(
        "SELECT stopped_through FROM owner_shared_demand_controls",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(stopped_through, 1);
    assert_eq!(f.state.lock().unwrap().shared.demands.stop_inputs[0]["expectedRevision"], 1);
    f.owner.0.shared_execution.settle_capacity_for_test().await.unwrap();
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_existing_idle_original_binds_event_without_start_intent() {
    let (f, t, _) = fixture().await;
    f.state.lock().unwrap().shared.idle_work = true;
    t.poll_fixture(&f.registry, launch(&f, "text-usage"))
        .await
        .unwrap();
    {
        let mut s = f.state.lock().unwrap();
        s.shared.demands.active = true;
        s.shared.demands.revision = 1;
    }
    offer(&f, 1);
    t.poll_demand(&f.registry, None)
        .await
        .unwrap()
        .outcome
        .unwrap();
    stopped(&f).await;
    assert_eq!(rows(&f, "owner_shared_demands"), 1);
    assert_eq!(rows(&f, "owner_shared_demand_bindings"), 1);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 0);
    assert_eq!(count_methods(&f, "initialize"), 1);
    assert_eq!(count_methods(&f, "turn/start"), 1);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_demand_new_run_revision_after_owner_stop_funds_only_new_event() {
    let (f, t, c) = fixture_budget(1000).await;
    offer(&f, 1);
    f.state.lock().unwrap().shared.idle_work = true;
    t.poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage")))
        .await
        .unwrap()
        .outcome
        .unwrap();
    f.state.lock().unwrap().shared.demands.lose_stop = true;
    assert!(
        t.stop_demand_assignment(&f.registry, c.assignment_id.clone())
            .await
            .is_err()
    );
    wait_attempt(&f).await;
    {
        let mut s = f.state.lock().unwrap();
        s.shared.demands.revision = 3;
        s.shared.demands.paused = false;
    }
    offer(&f, 2);
    f.state.lock().unwrap().shared.idle_work = true;
    t.poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage")))
        .await
        .unwrap()
        .outcome
        .unwrap();
    // Retrying original Stop1 observes its original receipt while newer Run3's
    // actual child remains alive. It must not reapply the old Stop locally.
    t.resume_next_demand_stop(&f.registry, None)
        .await
        .unwrap()
        .unwrap()
        .outcome
        .unwrap();
    assert!(
        !f.state
            .lock()
            .unwrap()
            .shared
            .attempts
            .processes
            .values()
            .any(|r| r.state == "stopped")
    );
    f.state.lock().unwrap().shared.idle_work = false;
    wait_attempt(&f).await;
    assert_eq!(count_methods(&f, "turn/start"), 1);
    assert_eq!(rows(&f, "owner_shared_stop_intents"), 1);
    assert_eq!(rows(&f, "owner_shared_demands"), 2);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 2);
    assert_eq!(rows(&f, "owner_shared_work_inbox"), 1);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_run_control_reviewed_owner_stop_retains_exact_input_across_restart() {
    let (mut f, executor, c) = fixture_budget(1000).await;
    let manager = f
        .owner
        .management(ManagementPackage {
            digest: "b".repeat(64),
            instance: 997,
            account_epoch: 1,
            permissions: vec![Permission::ChangeAgentRunControl],
        })
        .await
        .unwrap();
    let t = SharedExecutionTransport::owner_stop_only(manager, Arc::new(|| true));
    let input = |id: &str, revision| SharedDemandStopInput {
        command_id: id.into(),
        expected_revision: revision,
        mode: "paused".into(),
    };
    f.state.lock().unwrap().shared.demands.lose_stop = true;
    assert!(
        t.stop_reviewed_demand_assignment(
            &f.registry,
            c.assignment_id.clone(),
            input("reviewed_owner_stop", 1)
        )
        .await
        .is_err()
    );
    assert_eq!(rows(&f, "owner_shared_stop_intents"), 1);
    f.registry.shutdown().await.unwrap();
    f.registry =
        OwnerResourceRegistry::isolated_for_tests(&f._root.path().join("registry")).unwrap();
    {
        let mut s = f.state.lock().unwrap();
        s.shared.demands.revision = 3;
        s.shared.demands.paused = false;
    }
    let r = t
        .stop_reviewed_demand_assignment(
            &f.registry,
            c.assignment_id.clone(),
            input("reviewed_owner_stop", 1),
        )
        .await
        .unwrap();
    assert_eq!(r.command_id, "reviewed_owner_stop");
    assert_eq!(r.applied_revision, 2);
    assert!(!f.state.lock().unwrap().shared.demands.paused);
    assert!(
        t.stop_reviewed_demand_assignment(
            &f.registry,
            c.assignment_id.clone(),
            input("reviewed_owner_stop", 3)
        )
        .await
        .is_err()
    );
    assert_eq!(f.state.lock().unwrap().shared.demands.stop_inputs.len(), 2);
    assert!(
        t.poll(&f.registry, None).await.is_err(),
        "Stop capability cannot execute a runtime poll"
    );
    offer(&f, 2);
    executor
        .poll_demand_fixture(&f.registry, None, Some(launch(&f, "text-usage")))
        .await
        .unwrap()
        .outcome
        .unwrap();
    wait_attempt(&f).await;
    assert_eq!(
        count_methods(&f, "turn/start"),
        1,
        "conflicting retry must not cancel the later Run revision"
    );
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicitly built offline supervised probe"]
async fn native_run_control_local_stop_precedes_network_and_restricts_live_child() {
    let (f, t, c) = fixture().await;
    f.state.lock().unwrap().shared.idle_work = true;
    t.poll_fixture(&f.registry, launch(&f, "text-usage"))
        .await
        .unwrap();
    let manager = f
        .owner
        .management(ManagementPackage {
            digest: "b".repeat(64),
            instance: 998,
            account_epoch: 1,
            permissions: vec![Permission::ChangeAgentRunControl],
        })
        .await
        .unwrap();
    let stop = SharedExecutionTransport::owner_stop_only(manager.clone(), Arc::new(|| true));
    let input = SharedDemandStopInput {
        command_id: "offline_exact_stop".into(),
        expected_revision: 1,
        mode: "paused".into(),
    };
    // Contribution/cleanup proof is deliberately unavailable. Preparation uses
    // only current local SDK scope and the original canonical lifetime.
    f.state.lock().unwrap().unavailable_contribution = true;
    let intent = stop
        .prepare_reviewed_owner_stop(&f.registry, c.assignment_id.clone(), input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows(&f, "owner_shared_stop_intents"), 1);
    assert_eq!(rows(&f, "owner_shared_stop_receipts"), 0);
    assert_eq!(f.state.lock().unwrap().shared.demands.control_reads, 0);
    assert!(
        f.state
            .lock()
            .unwrap()
            .shared
            .demands
            .stop_inputs
            .is_empty()
    );
    f.state.lock().unwrap().control.fail_readiness = true;
    let error =
        crate::console::native::management_server::agent_run_control::test_saved_local_stop(
            &manager,
            (stop, f.registry.clone(), intent),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "management_agent_control_local_stop_pending");
    tokio::time::timeout(Duration::from_secs(15),async {
        loop {
            let db=rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
            let stopped:u64=db.query_row("SELECT count(*) FROM owner_shared_process_observations WHERE json_extract(payload,'$.state')='stopped'",[],|r|r.get(0)).unwrap();
            if stopped>0 {break;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(count_methods(&f, "turn/start"), 0);
    assert_eq!(rows(&f, "owner_shared_stop_receipts"), 0);
    f.stop().await;
}

#[path = "manager_capacity_tests.rs"]
mod capacity;

#[cfg(feature="shared-context")]
pub(super) fn context_binding(s:&super::super::State)->Option<SharedDemandBinding>{s.shared.demands.binding.clone()}
