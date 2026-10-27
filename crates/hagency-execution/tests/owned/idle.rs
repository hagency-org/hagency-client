use super::*;

#[tokio::test]
async fn native_owned_turn_long_lifetime() {
    let f = Fixture::new();
    let began = std::time::Instant::now();
    let mut operation = Operation::start(
        f.domain.clone(),
        f.cap.clone(),
        f.host("quiet-long-turn", "work", false),
        Limits {
            operation_ms: 45_000,
            response_ms: 1500,
        },
    )
    .unwrap();
    let report = operation.wait().await.unwrap();
    assert!(
        began.elapsed() >= Duration::from_secs(31),
        "early exit after {:?}: protocol={:?}, failure={:?}, startup={:?}, observation={:?}, cleanup={:?}, domain_last_unknown_dequeued={:?}",
        began.elapsed(),
        report.protocol,
        report.failure,
        report.startup_error(),
        report.runtime_observation(),
        // Names the guardian's own stop cause: a quiet turn that ends early was
        // stopped by something, and the transport error alone never says what.
        report.cleanup,
        // Read-only attribution for the most recent domain reply timeout;
        // never a retry, settlement or authority observation.
        f.domain.last_unknown_dequeued()
    );
    assert_eq!(
        report.protocol,
        Protocol::Completed,
        "{:?} {:?}",
        report.failure,
        report.runtime_observation()
    );
    assert_eq!(report.text.as_deref(), Some("离线管道验证完成"));
    if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        assert_eq!(report.failure, None);
        assert_eq!(report.settlement, Settlement::Completed);
        assert_eq!(f.state(), "completed");
        assert_eq!(f.count("SELECT COUNT(*) FROM resource_leases"), 0);
    } else {
        assert_eq!(report.failure, Some(Failure::CleanupUnknown));
        f.quarantined();
    }
    drop(report);
    drop(operation);
    f.domain.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_owned_turn_long_cancel() {
    let f = Fixture::new();
    let mut operation = Operation::start(
        f.domain.clone(),
        f.cap.clone(),
        f.host("quiet-open", "work", false),
        Limits {
            operation_ms: hagency_core::tasks::MAX_OWNED_OPERATION_MS,
            response_ms: 1500,
        },
    )
    .unwrap();
    let until = tokio::time::Instant::now() + Duration::from_secs(4);
    while !f.work.join("owned-dispatch.quiet").is_file() {
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let began = std::time::Instant::now();
    operation.cancel();
    let report = operation.wait().await.unwrap();
    assert!(
        began.elapsed() < Duration::from_secs(8),
        "cancellation must not wait for the long operation budget"
    );
    assert_eq!(report.failure, Some(Failure::Cancelled));
    f.quarantined();
    assert!(!report.retains_process_custody());
    drop(report);
    drop(operation);
    f.domain.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_owned_turn_quiet_notification_wait() {
    let f = Fixture::new();
    let at = std::time::Instant::now();
    let mut operation = f.operation("quiet-turn");
    let report = operation.wait().await.unwrap();
    assert!(f.work.join("owned-dispatch.quiet").is_file());
    assert_eq!(report.protocol, Protocol::Completed);
    assert_eq!(report.text.as_deref(), Some("离线管道验证完成"));
    assert!(at.elapsed() >= Duration::from_millis(2200));
    assert_ne!(report.failure, Some(Failure::Protocol));
    if !cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        // Preserve this generic fixture's existing unqualified descendant scope.
        assert_eq!(report.failure, Some(Failure::CleanupUnknown));
        f.quarantined();
    } else {
        assert_eq!(report.failure, None);
        assert_eq!(report.settlement, Settlement::Completed);
        assert_eq!(f.state(), "completed");
    }
    drop(report);
    f.domain.shutdown().await.unwrap();
}

/// A quiet open turn is not cut at the budget (ADR-183 decision D): both
/// arms end by the human's stop (cancellation). The second lets the budget
/// elapse first and proves the child outlived it, then stops it.
#[tokio::test]
async fn native_owned_turn_quiet_limits() {
    for cancel_early in [true, false] {
        let f = Fixture::new();
        let started = std::time::Instant::now();
        let mut operation = Operation::start(
            f.domain.clone(),
            f.cap.clone(),
            f.host("quiet-open", "work", false),
            Limits {
                operation_ms: 5000,
                response_ms: 1500,
            },
        )
        .unwrap();
        let mut budget = operation.budget_watch();
        let until = tokio::time::Instant::now() + Duration::from_secs(4);
        while !f.work.join("owned-dispatch.quiet").is_file() {
            assert!(
                tokio::time::Instant::now() < until,
                "original turn did not acknowledge start"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let marker = f.work.join("owned-dispatch.pulse");
        if cancel_early {
            operation.cancel();
        } else {
            let over = tokio::time::timeout(Duration::from_secs(8), budget.exceeded())
                .await
                .expect("the budget elapses under the open turn")
                .expect("the operation is still running when it does");
            assert_eq!(over.budget_ms, 5000);
            assert!(over.elapsed_ms >= 5000);
            assert!(
                started.elapsed() >= Duration::from_secs(5),
                "the budget signal fires at the operation bound, not the RPC interval"
            );
            assert!(
                !operation.is_finished(),
                "the budget elapsing killed nothing"
            );
            // Observe actual child progress after the budget signal, within
            // the existing response budget. A fixed 150 ms sampling gap
            // assumed the child was scheduled during that particular slice.
            let bytes = fs::metadata(&marker).unwrap().len();
            tokio::time::timeout(Duration::from_millis(limits().response_ms), async {
                loop {
                    assert!(
                        !operation.is_finished(),
                        "the quiet turn ended before the human's stop"
                    );
                    if fs::metadata(&marker).unwrap().len() > bytes {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the quiet turn's child must make real progress past the budget");
            // The human's stop.
            operation.cancel();
        }
        let report = operation.wait().await.unwrap();
        assert_eq!(report.protocol, Protocol::Unknown);
        assert!(report.text.is_none());
        assert_eq!(report.failure, Some(Failure::Cancelled));
        assert_eq!(report.over_budget.is_some(), !cancel_early);
        f.quarantined();
        drop(report);
        let bytes = fs::metadata(&marker).map_or(0, |m| m.len());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(fs::metadata(&marker).map_or(0, |m| m.len()), bytes);
        f.domain.shutdown().await.unwrap();
    }
}

/// Scenario "The execution budget notifies and the turn continues" (ADR-183
/// decision D). The probe is told a 60 s budget and keeps its turn quiet for
/// a tenth of it; the operation is granted 2 s. Nothing is killed at the
/// budget: the live signal fires, the `over_budget` event is recorded once
/// with the fixed labels, the report carries the status word, and the turn
/// completes when the probe — Codex — ends it.
#[tokio::test]
async fn native_budget_expiry_notifies_and_the_turn_continues() {
    let f = Fixture::new();
    let started = std::time::Instant::now();
    let mut operation = Operation::start(
        f.domain.clone(),
        f.cap.clone(),
        f.host_budget("quiet-turn", 60_000),
        Limits {
            operation_ms: 2000,
            response_ms: 1500,
        },
    )
    .unwrap();
    let mut budget = operation.budget_watch();
    assert_eq!(budget.current(), None);
    let over = tokio::time::timeout(Duration::from_secs(4), budget.exceeded())
        .await
        .expect("the budget elapses while the turn is still open")
        .expect("the operation is still running when it does");
    assert_eq!(over.budget_ms, 2000);
    assert!(over.elapsed_ms >= 2000, "{over:?}");
    assert!(
        !operation.is_finished(),
        "nothing is killed when the budget elapses"
    );
    let report = operation.wait().await.unwrap();
    assert!(
        started.elapsed() >= Duration::from_secs(6),
        "the turn ran its whole quiet stretch past the budget: {:?}",
        started.elapsed()
    );
    assert_eq!(
        report.protocol,
        Protocol::Completed,
        "{:?} {:?}",
        report.failure,
        report.runtime_observation()
    );
    assert_eq!(report.text.as_deref(), Some("离线管道验证完成"));
    assert_ne!(report.failure, Some(Failure::Deadline));
    let status = report
        .over_budget
        .expect("over_budget is the report's status word");
    assert_eq!(status.budget_ms, 2000);
    assert!(status.elapsed_ms >= 6000, "{status:?}");
    assert_eq!(budget.current().map(|v| v.budget_ms), Some(2000));
    if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        assert_eq!(report.failure, None);
        assert_eq!(report.settlement, Settlement::Completed);
        assert_eq!(f.state(), "completed");
    } else {
        assert_eq!(report.failure, Some(Failure::CleanupUnknown));
        f.quarantined();
    }
    // The attempt's evidence (ADR-181): exactly one `over_budget`, visited
    // after the turn started and before the stop was requested.
    let events: Vec<(String, String)> = f
        .sql()
        .prepare("SELECT phase,detail FROM runner_attempt_events WHERE dispatch_id='dispatch' ORDER BY seq")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let over_events: Vec<_> = events
        .iter()
        .filter(|(phase, _)| phase == "over_budget")
        .collect();
    assert_eq!(over_events.len(), 1, "{events:?}");
    let detail: serde_json::Value = serde_json::from_str(&over_events[0].1).unwrap();
    assert_eq!(detail["budget_ms"], 2000);
    assert!(detail["elapsed_ms"].as_u64().unwrap() >= 2000);
    // This offline dispatch answers no verified thread, so the notice has
    // nowhere to go and the event says so; the queued notice itself is
    // proven on the store (hagency-store/tests/over_budget_notice.rs).
    assert_eq!(detail["notice"], "no_thread");
    assert_eq!(f.count("SELECT COUNT(*) FROM task_notices"), 0);
    let index = |phase: &str| {
        events
            .iter()
            .position(|(p, _)| p == phase)
            .unwrap_or_else(|| panic!("{phase} missing from {events:?}"))
    };
    assert!(index("turn_started") < index("over_budget"));
    assert!(index("over_budget") < index("stop_requested"));
    drop(report);
    drop(operation);
    f.domain.shutdown().await.unwrap();
}
