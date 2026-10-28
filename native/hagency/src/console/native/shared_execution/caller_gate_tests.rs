//! Actual original process/HTTP/SQLite fixtures; no live account or provider.
use super::*;
#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn caller_gate_retired_while_start_writer_waits_creates_no_intent_or_post() {
    let (f, t, c) = Box::pin(predecessor()).await;
    let live = Arc::new(AtomicBool::new(true));
    let gate = live.clone();
    let prepared = t
        .prepare_successor(&f.registry, c.assignment_id.clone())
        .await
        .unwrap()
        .restrict_to_host(Arc::new(move || gate.load(Ordering::Acquire)))
        .unwrap();
    assert_eq!(
        prepared.review().agent_name,
        c.reserve_command.definition.name
    );
    assert_eq!(
        prepared.review().project_id,
        c.reserve_command.project_id
    );
    assert_eq!(rows(&f, "owner_shared_start_intents"), 0);
    let mut db =
        rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
    let lock = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let mut pending = Box::pin(t.start_prepared_successor(&f.registry, prepared));
    tokio::select! {biased;result=&mut pending=>panic!("unexpected completion {}",result.is_ok()),_=tokio::time::sleep(Duration::from_millis(100))=>{}}
    live.store(false, Ordering::Release);
    lock.commit().unwrap();
    assert!(pending.await.is_err());
    assert_eq!(rows(&f, "owner_shared_start_intents"), 0);
    assert!(
        f.state
            .lock()
            .unwrap()
            .shared
            .attempts
            .start_inputs
            .is_empty()
    );
    f.stop().await;
}
#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn caller_gate_loss_after_original_post_retains_original_native_recovery() {
    let (f, t, c) = Box::pin(predecessor()).await;
    let live = Arc::new(AtomicBool::new(true));
    let first = live.clone();
    let prepared = t
        .prepare_successor(&f.registry, c.assignment_id.clone())
        .await
        .unwrap()
        .restrict_to_host(Arc::new(move || first.load(Ordering::Acquire)))
        .unwrap()
        .restrict_to_host(Arc::new(|| true))
        .unwrap();
    f.state.lock().unwrap().shared.attempts.retire_start_caller = Some(live);
    assert!(
        t.start_prepared_successor(&f.registry, prepared)
            .await
            .is_err()
    );
    assert_eq!(rows(&f, "owner_shared_start_intents"), 1);
    let original = f.state.lock().unwrap().shared.attempts.start_inputs[0].clone();
    // A caller-lifetime failure must not retire the native service/executor or
    // contaminate its background recovery with the now-ended package lease.
    assert!(matches!(
        t.resume_successor(&f.registry, c.assignment_id)
            .await
            .unwrap()
            .unwrap(),
        SharedExecutionProgress::HistoricalEvidenceMissing { .. }
    ));
    assert_eq!(rows(&f, "owner_shared_start_intents"), 1);
    assert!(
        f.state
            .lock()
            .unwrap()
            .shared
            .attempts
            .start_inputs
            .iter()
            .all(|v| v == &original)
    );
    f.stop().await;
}
