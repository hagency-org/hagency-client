//! Actual child plus fixed HTTP routes: start-reply loss never regenerates intent,
//! an original stopped process is retained, and new work binds only to v2.
use super::*;
#[derive(Default)]
pub(super) struct State {
    pub(super) command: Option<SharedAttemptCommand>,
    pub(super) processes: BTreeMap<String, SharedAttemptProcessReceipt>,
    start_inputs: Vec<Value>,
    ticket_sequence: u64,
    ticket_command: Option<String>,
    tickets: BTreeMap<String, (bool, u64)>,
    lose_start_reply: bool,
    retire_start_caller: Option<Arc<AtomicBool>>,
    pub(super) fail_work_reports: bool,
}
pub(super) fn management(path: &str, body: &Value, s: &mut super::super::State) -> Option<Value> {
    let at = now();
    let start = path.ends_with("/attempts/start");
    let replay = s.shared.attempts.command.is_some();
    if start {
        s.shared.attempts.start_inputs.push(body.clone());
        if let Some(first) = s.shared.attempts.start_inputs.first() {
            assert_eq!(first, body);
        }
        if s.shared.attempts.command.is_none() {
            let input: SharedAttemptStartInput = serde_json::from_value(body.clone()).unwrap();
            let original = s.shared.command.clone().unwrap();
            assert!(s.shared.processes.values().any(|r| r.state == "stopped"));
            s.shared.attempts.command = Some(SharedAttemptCommand {
        web_search: false, tool_policy: None,
                version: 2,
                capability: SHARED_ATTEMPT_CAPABILITY.into(),
                operation: "agent.execution.provision".into(),
                command_id: "successor_fixture".into(),
                assignment_id: original.assignment_id.clone(),
                assignment_generation: 1,
                agent_reservation_id: original.agent_reservation_id.clone(),
                process_attempt: 2,
                original_runtime_digest: digest(&original).unwrap(),
                original_runtime_command: original,
                predecessor: input.predecessor,
                budget_observation: input.budget_observation,
                start_intent_id: input.start_intent_id,
                issued_at_ms: at,
                expires_at_ms: at + 30_000,
            });
        }
        if let Some(gate)=&s.shared.attempts.retire_start_caller {gate.store(false,Ordering::Release);}
        if std::mem::take(&mut s.shared.attempts.lose_start_reply) {
            return None;
        }
    }
    let c = s.shared.attempts.command.clone().unwrap();
    let h = digest(&c).unwrap();
    if path.ends_with("/result") {
        let ticket = body["token"].as_str()
            .and_then(|token| s.shared.attempts.tickets.get(token))
            .filter(|(_, until)| *until > at).copied();
        let Some(ticket) = ticket else {
            return Some(json!({"fixtureStatus":409,"error":"shared_result_ticket_changed"}));
        };
        let r: SharedAttemptProcessReceipt =
            serde_json::from_value(body["receipt"].clone()).unwrap();
        r.validate_for(&c).unwrap();
        assert_eq!(body["commandDigest"], h);
        if let Some(old) = s.shared.attempts.processes.get(&r.observation_id) {
            assert_eq!(digest(old).unwrap(), digest(&r).unwrap());
        }
        if r.state == "stopped" && s.shared.reply_pending {
            s.shared.stopped_before_reply = true;
        }
        s.shared
            .attempts
            .processes
            .insert(r.observation_id.clone(), r.clone());
        let activity = if r.state == "qualified" && ticket.0 {
            s.shared.activity_epoch = 1;
            Some(
                json!({"authority":{"runtimeCommandId":c.command_id,"epoch":1,"token":"c".repeat(64)},"commandDigest":h,"processDigest":r.process_digest,"expiresAtMs":at+25000}),
            )
        } else {
            None
        };
        return Some(
            json!({"observationId":r.observation_id,"receiptDigest":digest(&r).unwrap(),"activity":activity,"state":r.state,"observedAtMs":at}),
        );
    }
    let history = path.ends_with("/recover") || start && replay;
    if path.ends_with("/recover") {
        assert_eq!(body["commandDigest"], h);
    }
    let selected = body["observationId"].as_str();
    let recorded = s
        .shared
        .attempts
        .processes
        .values()
        .filter(|r| selected.is_none_or(|id| id == r.observation_id))
        .max_by_key(|r| r.ledger_revision)
        .cloned();
    let executable = if history { s.historical_executable } else { !s.shared.cleanup && recorded.is_none() };
    let until = if s.expired_ticket { at - 1 } else if executable { (at + 20000).min(c.expires_at_ms) } else { at + 20000 };
    // This fixture serves one selected command. Execute tickets are bounded per
    // original command, as on the server, rather than across unrelated turns.
    if s.shared.attempts.ticket_command.as_deref() != Some(c.command_id.as_str()) {
        s.shared.attempts.tickets.clear();
        s.shared.attempts.ticket_command = Some(c.command_id.clone());
    }
    s.shared.attempts.ticket_sequence += 1;
    let token = format!("{:064x}", s.shared.attempts.ticket_sequence);
    // Paired schema15 contract: history/poll tickets never replace the
    // original finite execute ticket. The presented ticket controls activity.
    s.shared.attempts.tickets.retain(|_, (_, until)| *until > at);
    assert!(s.shared.attempts.tickets.len() < 64);
    s.shared.attempts.tickets.insert(token.clone(), (executable, until));
    Some(
        json!({"delivery":{"command":c,"commandDigest":h,"token":token,"leaseUntilMs":until,"executionAllowed":executable,"cleanupAllowed":!history&&s.shared.cleanup,"recorded":recorded},"observedAtMs":at}),
    )
}
pub(super) async fn predecessor() -> (Fixture, SharedExecutionTransport, SharedRuntimeCommand) {
    let (f, t, c) = fixture().await;
    f.state.lock().unwrap().shared.idle_work = true;
    assert!(matches!(
        t.poll_fixture(&f.registry, launch(&f, "text"))
            .await
            .unwrap(),
        SharedExecutionProgress::Running { .. }
    ));
    f.state.lock().unwrap().shared.cleanup = true;
    assert!(matches!(
        t.poll(&f.registry, None).await.unwrap(),
        SharedExecutionProgress::Stopping { .. }
    ));
    f.state.lock().unwrap().shared.cleanup = false;
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
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .unwrap();
    (f, t, c)
}
async fn prepare_lost_start(f: &Fixture, t: &SharedExecutionTransport, c: &SharedRuntimeCommand) {
    f.state.lock().unwrap().shared.attempts.lose_start_reply = true;
    assert!(
        t.start_successor(&f.registry, c.clone(), None)
            .await
            .is_err()
    );
    assert_eq!(rows(f, "owner_shared_start_intents"), 1);
    assert_eq!(rows(f, "owner_shared_successor_attempts"), 0);
    assert!(matches!(
        t.start_successor(&f.registry, c.clone(), None)
            .await
            .unwrap(),
        SharedExecutionProgress::HistoricalEvidenceMissing { .. }
    ));
    let mut s = f.state.lock().unwrap();
    let attempt = s.shared.attempts.command.clone().unwrap();
    assert_eq!(s.shared.attempts.start_inputs.len(), 2);
    assert_eq!(
        s.shared.attempts.start_inputs[0],
        s.shared.attempts.start_inputs[1]
    );
    s.shared.idle_work = false;
    let work = s.shared.work.as_mut().unwrap();
    work.id = "successor_work".into();
    work.execution_id = "successor_execution".into();
    work.runtime_command_digest = digest(&attempt).unwrap();
}
async fn wait_stopped(f: &Fixture) {
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
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .expect("actual successor stop");
}
#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_shared_successor_lost_start_replays_journal_and_one_exact_model_call() {
    let (f, t, c) = Box::pin(predecessor()).await;
    prepare_lost_start(&f, &t, &c).await;
    f.state.lock().unwrap().shared.lose_work_result = true;
    assert!(matches!(
        t.poll_successor_fixture(&f.registry, launch(&f, "text-usage"))
            .await
            .unwrap(),
        SharedExecutionProgress::Running { .. }
    ));
    wait_stopped(&f).await;
    let (attempt, receipt) = {
        let s = f.state.lock().unwrap();
        (
            s.shared.attempts.command.clone().unwrap(),
            s.shared.receipts.values().next().unwrap().clone(),
        )
    };
    assert_eq!(receipt.runtime_command_digest, digest(&attempt).unwrap());
    assert_eq!(receipt.known_consumed_tokens, 200);
    assert_eq!(receipt.retained_tokens, Some(200));
    assert!(receipt.usage_complete);
    t.recover_successor(
        &f.registry,
        attempt.command_id.clone(),
        digest(&attempt).unwrap(),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        t.poll_successor_fixture(&f.registry, launch(&f, "text-usage"))
            .await
            .unwrap(),
        SharedExecutionProgress::Recorded { .. }
    ));
    let methods = std::fs::read_to_string(f._root.path().join("shared-probe/methods")).unwrap();
    assert_eq!(methods.lines().filter(|m| *m == "turn/start").count(), 1);
    assert!(!f.state.lock().unwrap().shared.stopped_before_reply);
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 1);
    assert_eq!(rows(&f, "owner_shared_successor_call_pins"), 1);
    assert_eq!(rows(&f, "owner_shared_runtime_lifetimes"), 1);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_shared_successor_lost_model_start_keeps_hold_and_never_replays() {
    let (f, t, c) = Box::pin(predecessor()).await;
    prepare_lost_start(&f, &t, &c).await;
    f.state.lock().unwrap().shared.lose_start = true;
    t.poll_successor_fixture(&f.registry, launch(&f, "text-usage"))
        .await
        .unwrap();
    wait_stopped(&f).await;
    t.poll_successor_fixture(&f.registry, launch(&f, "text-usage"))
        .await
        .unwrap();
    let methods = std::fs::read_to_string(f._root.path().join("shared-probe/methods")).unwrap();
    assert!(!methods.contains("turn/start"));
    assert_eq!(rows(&f, "owner_managed_calls"), 1);
    assert_eq!(rows(&f, "owner_shared_work_observations"), 0);
    assert_eq!(f.state.lock().unwrap().shared.starts, 1);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_shared_successor_cleanup_before_claim_persists_fence_without_fake_stop() {
    let (f, t, c) = Box::pin(predecessor()).await;
    prepare_lost_start(&f, &t, &c).await;
    f.state.lock().unwrap().shared.cleanup = true;
    assert!(matches!(
        t.poll_successor(&f.registry, None).await.unwrap(),
        SharedExecutionProgress::HistoricalEvidenceMissing { .. }
    ));
    assert_eq!(rows(&f, "owner_shared_successor_fences"), 1);
    assert_eq!(rows(&f, "owner_shared_successor_observations"), 0);
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 0);
    f.state.lock().unwrap().shared.cleanup = false;
    assert!(
        t.poll_successor_fixture(&f.registry, launch(&f, "text-usage"))
            .await
            .is_err()
    );
    let methods = std::fs::read_to_string(f._root.path().join("shared-probe/methods")).unwrap();
    assert!(!methods.contains("turn/start"));
    assert_eq!(methods.lines().filter(|m| *m == "initialize").count(), 1);
    f.stop().await;
}
async fn method_count_seen(f: &Fixture, method: &str, count: usize) {
    let path = f._root.path().join("shared-probe/methods");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(&path)
                .is_ok_and(|s| s.lines().filter(|v| *v == method).count() >= count)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_shared_successor_cancelled_waiter_retains_one_process_and_usage() {
    let (f, t, c) = Box::pin(predecessor()).await;
    prepare_lost_start(&f, &t, &c).await;
    let registry = f.registry.clone();
    let previous_reads = std::fs::read_to_string(f._root.path().join("shared-probe/methods"))
        .unwrap()
        .lines()
        .filter(|m| *m == "account/rateLimits/read")
        .count();
    let prepared = launch(&f, "text-held-qualify");
    let t = Arc::new(t);
    let waiting = t.clone();
    let task =
        tokio::spawn(async move { waiting.poll_successor_fixture(&registry, prepared).await });
    method_count_seen(&f, "account/rateLimits/read", previous_reads + 1).await;
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 1);
    task.abort();
    let _ = task.await;
    std::fs::write(
        f._root.path().join("shared-probe/release-qualification"),
        b"release",
    )
    .unwrap();
    wait_stopped(&f).await;
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 1);
    assert_eq!(f.state.lock().unwrap().shared.starts, 1);
    assert_eq!(f.state.lock().unwrap().shared.work_writes, 1);
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_shared_successor_retirement_during_turn_drains_exact_child_and_keeps_unknown_hold()
{
    let (f, t, c) = Box::pin(predecessor()).await;
    prepare_lost_start(&f, &t, &c).await;
    t.poll_successor_fixture(&f.registry, launch(&f, "text-block"))
        .await
        .unwrap();
    method_seen(&f, "turn/start").await;
    f.current.store(false, Ordering::Release);
    f.owner.retire();
    f.owner.0.shared_execution.drain().await.unwrap();
    let db = rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
    let payload:String=db.query_row("SELECT payload FROM owner_shared_work_observations ORDER BY ledger_revision DESC LIMIT 1",[],|r|r.get(0)).unwrap();
    let receipt: SharedWorkReceipt = serde_json::from_str(&payload).unwrap();
    assert_eq!(receipt.state, "unknown");
    assert_eq!(receipt.retained_tokens, Some(100));
    assert!(!receipt.usage_complete);
    assert_eq!(db.query_row("SELECT COUNT(*) FROM owner_shared_successor_observations WHERE json_extract(payload,'$.state')='stopped'",[],|r|r.get::<_,u64>(0)).unwrap(),1);
    assert_eq!(f.state.lock().unwrap().shared.work_writes, 0);
    drop(db);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_shared_successor_actual_stop_upload_is_independent_of_old_work_failure() {
    let (f, t, c) = Box::pin(predecessor()).await;
    prepare_lost_start(&f, &t, &c).await;
    t.poll_successor_fixture(&f.registry, launch(&f, "text-usage"))
        .await
        .unwrap();
    wait_stopped(&f).await;
    {
        let mut s = f.state.lock().unwrap();
        // Simulate replies lost after both exact local observations persisted.
        s.shared
            .attempts
            .processes
            .retain(|_, r| r.state != "stopped");
        s.shared.receipts.clear();
        s.shared.attempts.fail_work_reports = true;
    }
    assert!(t.poll_successor(&f.registry, None).await.is_err());
    assert!(
        f.state
            .lock()
            .unwrap()
            .shared
            .attempts
            .processes
            .values()
            .any(|r| r.state == "stopped")
    );
    assert!(f.state.lock().unwrap().shared.receipts.is_empty());
    let methods = std::fs::read_to_string(f._root.path().join("shared-probe/methods")).unwrap();
    assert_eq!(methods.lines().filter(|m| *m == "turn/start").count(), 1);
    assert!(rows(&f, "owner_shared_work_observations") > 0);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_successor_handoff_reads_then_replays_only_original_start_intent() {
    let (f, t, c) = Box::pin(predecessor()).await;
    assert!(t.resume_successor(&f.registry, c.assignment_id.clone()).await.unwrap().is_none());
    assert!(t.resume_next_successor(&f.registry, None).await.unwrap().is_none());
    assert_eq!(rows(&f, "owner_shared_start_intents"), 0);
    let prepared = t.prepare_successor(&f.registry, c.assignment_id.clone()).await.unwrap();
    assert_eq!(prepared.review().assignment_id, c.assignment_id);
    assert_eq!(prepared.review().predecessor_command_id, c.command_id);
    assert_eq!(prepared.review().process_attempt, 2);
    assert!(prepared.review().remaining_tokens > 0);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 0);
    f.state.lock().unwrap().shared.attempts.lose_start_reply = true;
    assert!(t.start_prepared_successor(&f.registry, prepared).await.is_err());
    assert_eq!(rows(&f, "owner_shared_start_intents"), 1);
    let original = f.state.lock().unwrap().shared.attempts.start_inputs[0].clone();
    let recovered = t.resume_next_successor(&f.registry, None).await.unwrap().unwrap();
    assert_eq!(recovered.intent_id, original["startIntentId"]);
    assert!(matches!(recovered.outcome.unwrap(),SharedExecutionProgress::HistoricalEvidenceMissing{..}));
    assert!(t.resume_next_successor(&f.registry, Some(recovered.intent_id)).await.unwrap().is_none());
    assert!(matches!(t.resume_successor(&f.registry, c.assignment_id).await.unwrap().unwrap(),SharedExecutionProgress::HistoricalEvidenceMissing{..}));
    {
        let state=f.state.lock().unwrap();
        assert!(state.shared.attempts.start_inputs.iter().all(|v|v==&original));
    }
    assert_eq!(rows(&f,"owner_shared_start_intents"),1);
    assert_eq!(rows(&f,"owner_shared_successor_attempts"),0);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_successor_handoff_preparation_is_bound_to_manager_and_current_authority() {
    let (f,t,c)=Box::pin(predecessor()).await;
    let prepared=t.prepare_successor(&f.registry,c.assignment_id.clone()).await.unwrap();
    let other=SharedExecutionTransport::new(f.owner.management(package(81)).await.unwrap());
    assert_eq!(other.start_prepared_successor(&f.registry,prepared).await.err().unwrap().code,"shared_successor_preparation_changed");
    let prepared=t.prepare_successor(&f.registry,c.assignment_id).await.unwrap();
    f.current.store(false,Ordering::Release);
    assert!(t.start_prepared_successor(&f.registry,prepared).await.is_err());
    assert_eq!(rows(&f,"owner_shared_start_intents"),0);
    assert!(f.state.lock().unwrap().shared.attempts.start_inputs.is_empty());
    f.stop().await;
}

#[path="caller_gate_tests.rs"]
mod caller_gate_tests;

#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_shared_successor_initialization_survives_background_replay_and_poll() {
    let (f, t, c) = Box::pin(predecessor()).await;
    prepare_lost_start(&f, &t, &c).await;
    let before = std::fs::read_to_string(f._root.path().join("shared-probe/methods"))
        .unwrap().lines().filter(|m| *m == "initialize").count();
    let prepared = launch(&f, "text-held-initialize");
    let registry = f.registry.clone();
    let t = Arc::new(t);
    let first = t.clone();
    let waiting = tokio::spawn(async move {
        first.poll_successor_fixture(&registry, prepared).await
    });
    method_count_seen(&f, "initialize", before + 1).await;
    assert!(!waiting.is_finished());
    assert_eq!(rows(&f, "owner_shared_successor_attempts"), 1);
    assert_eq!(rows(&f, "owner_shared_successor_observations"), 0);
    // This is the same original durable intent, followed by normal discovery,
    // while the original actual child has not completed initialization.
    assert!(matches!(
        t.resume_successor(&f.registry, c.assignment_id).await.unwrap().unwrap(),
        SharedExecutionProgress::Running { .. }
    ));
    assert!(matches!(
        t.poll_successor(&f.registry, None).await.unwrap(),
        SharedExecutionProgress::Running { .. }
    ));
    assert!(!waiting.is_finished());
    assert!(f.state.lock().unwrap().shared.attempts.processes.is_empty());
    std::fs::write(f._root.path().join("shared-probe/release-initialization"), b"release").unwrap();
    let result = waiting.await.unwrap();
    assert!(matches!(result, Ok(SharedExecutionProgress::Running { .. })),
        "original finite execution ticket survives replay/poll: {:?}", result.err().map(|e| e.code));
    wait_stopped(&f).await;
    {
        let state = f.state.lock().unwrap();
        assert_eq!(state.shared.attempts.processes.values().filter(|r|r.state=="qualified").count(),1);
        assert_eq!(state.shared.attempts.processes.values().filter(|r|r.state=="stopped").count(),1);
        assert_eq!(state.shared.starts,1);
        assert_eq!(state.shared.work_writes,1);
        assert!(state.shared.attempts.start_inputs.iter().all(|input| input==&state.shared.attempts.start_inputs[0]));
    }
    assert_eq!(rows(&f,"owner_shared_start_intents"),1);
    assert_eq!(rows(&f,"owner_shared_successor_attempts"),1);
    let methods=std::fs::read_to_string(f._root.path().join("shared-probe/methods")).unwrap();
    assert_eq!(methods.lines().filter(|m|*m=="initialize").count(),before+1);
    assert_eq!(methods.lines().filter(|m|*m=="turn/start").count(),1);
    f.stop().await;
}


#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe; streams for 36 seconds"]
async fn native_shared_streaming_successor_renews_without_an_idle_gap() {
    let (f, t, c) = Box::pin(predecessor()).await;
    prepare_lost_start(&f, &t, &c).await;
    t.poll_successor_fixture(&f.registry, launch(&f, "text-stream")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(50), async {
        loop {
            if f.state.lock().unwrap().shared.attempts.processes.values().any(|r| r.state == "stopped") {break;}
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await.expect("streamed turn reaches a terminal state");
    let (receipt, epoch, starts) = {
        let s = f.state.lock().unwrap();
        (s.shared.receipts.values().find(|r| r.usage_complete).cloned(), s.shared.activity_epoch, s.shared.starts)
    };
    let methods = std::fs::read_to_string(f._root.path().join("shared-probe/methods")).unwrap();
    f.stop().await;
    let receipt = receipt.expect("continuous output must complete instead of exhausting its activity lease");
    assert_eq!(receipt.state, "completed");
    assert!(receipt.body.as_deref().is_some_and(|s| s.ends_with("Offline answer.")));
    assert!(epoch >= 4, "the same process must renew while events keep arriving");
    assert_eq!(starts, 1);
    assert_eq!(methods.lines().filter(|m| *m == "turn/start").count(), 1);
}
