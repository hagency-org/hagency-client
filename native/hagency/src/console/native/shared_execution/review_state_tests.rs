//! Real loopback/native ledger and offline child; read projection creates no Start.
use super::*;
use crate::console::native::{SharedSuccessorAssessment, SharedSuccessorBlockReason};

#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_successor_review_exhaustion_and_recorded_state_preserve_originals() {
    let (f, t, c) = fixture().await;
    t.poll_fixture(&f.registry, launch(&f, "text-usage"))
        .await
        .unwrap();
    stopped(&f).await;
    let description = t
        .successor_description(&f.registry, c.assignment_id.clone())
        .await
        .unwrap();
    assert_eq!(description.agent_name, c.reserve_command.definition.name);
    assert_eq!(description.project_id, c.reserve_command.project_id);
    assert_eq!(description.assignment_id, c.assignment_id);
    let recovered = t.poll(&f.registry, None).await.unwrap();
    let saved = f
        .state
        .lock()
        .unwrap()
        .shared
        .processes
        .values()
        .filter(|r| r.state == "stopped")
        .max_by_key(|r| r.ledger_revision)
        .unwrap()
        .clone();
    match recovered {
        SharedExecutionProgress::Recorded {
            command_id,
            observation_id,
            process_state,
        } => {
            assert_eq!(command_id, c.command_id);
            assert_eq!(observation_id, saved.observation_id);
            assert_eq!(process_state.as_deref(), Some("stopped"));
        }
        _ => panic!("expected original stopped evidence"),
    }
    let SharedSuccessorAssessment::Blocked(review) = t
        .assess_successor(&f.registry, c.assignment_id.clone())
        .await
        .unwrap()
    else {
        panic!("overspent child must not yield Start handle")
    };
    assert_eq!(review.reason, SharedSuccessorBlockReason::Exhausted);
    assert_eq!(review.allocated_tokens, 100);
    assert_eq!(review.known_consumed_tokens, 200);
    assert_eq!(review.retained_tokens, 200);
    assert_eq!(review.known_overrun_tokens, 100);
    assert_eq!(review.remaining_tokens, Some(0));
    assert!(review.usage_complete);
    assert_eq!(review.description.agent_name, description.agent_name);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 0);
    assert!(
        f.state
            .lock()
            .unwrap()
            .shared
            .attempts
            .command
            .is_none()
    );
    f.current.store(false, Ordering::Release);
    assert!(
        t.assess_successor(&f.registry, c.assignment_id)
            .await
            .is_err()
    );
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_successor_review_incomplete_and_unrepresentable_usage_are_not_spent() {
    let (f, t, c) = fixture().await;
    // Offline child deliberately supplies no usage report.
    t.poll_fixture(&f.registry, launch(&f, "text"))
        .await
        .unwrap();
    stopped(&f).await;
    let SharedSuccessorAssessment::Blocked(review) = t
        .assess_successor(&f.registry, c.assignment_id.clone())
        .await
        .unwrap()
    else {
        panic!("unresolved full hold cannot yield Start handle")
    };
    assert_eq!(review.reason, SharedSuccessorBlockReason::UsageUnresolved);
    assert_eq!(review.allocated_tokens, 100);
    assert_eq!(review.known_consumed_tokens, 0);
    assert_eq!(review.retained_tokens, 100);
    assert_eq!(review.remaining_tokens, Some(0));
    assert!(!review.usage_complete);
    // Explicitly seed the separate retained-overflow flag; this is a fixture
    // state, not a claim that the offline provider produced unbounded tokens.
    let db = rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
    assert_eq!(
        db.execute("UPDATE owner_managed_calls SET unrepresentable_usage=1", [])
            .unwrap(),
        1
    );
    drop(db);
    let SharedSuccessorAssessment::Blocked(review) = t
        .assess_successor(&f.registry, c.assignment_id.clone())
        .await
        .unwrap()
    else {
        panic!("unrepresentable cannot yield Start handle")
    };
    assert_eq!(review.reason, SharedSuccessorBlockReason::UsageUnresolved);
    assert_eq!(review.remaining_tokens, None);
    assert_eq!(rows(&f, "owner_shared_start_intents"), 0);
    assert!(
        f.state
            .lock()
            .unwrap()
            .shared
            .attempts
            .command
            .is_none()
    );
    f.stop().await;
}
