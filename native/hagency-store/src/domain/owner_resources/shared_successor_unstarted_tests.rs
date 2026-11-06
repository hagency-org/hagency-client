//! v2 non-issuance and the uncertainty time bound (ADR-193). An unclaimed
//! fence yields not_started; a claimed process with no stop evidence blocks
//! only for its grace period. Neither invents usage or replays old work.
use super::*;
use std::sync::{Arc, atomic::AtomicU64};

fn e(f: &Fixture) -> VerifiedManagementEnrollment {
    enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1)
}
async fn base() -> (Fixture, SharedRuntimeCommand) {
    let f = fixture().await;
    let original = command(&f).await;
    f.registry.claim_shared_runtime(proof(&f, original.clone())).await.unwrap();
    stopped(&f, &original, "stopped").await;
    call(&f, &original, "old_unknown", "unknown", 40, 7, false, false).await;
    (f, original)
}
// A successor built from a saved start input, as the server would issue it.
fn successor(previous: &SharedAttemptCommand, input: &SharedAttemptStartInput, id: &str) -> SharedAttemptCommand {
    let now = graphs::now_ms().unwrap();
    let mut c = previous.clone();
    c.command_id = id.into();
    c.process_attempt = input.predecessor.process_attempt + 1;
    c.predecessor = input.predecessor.clone();
    c.budget_observation = input.budget_observation.clone();
    c.start_intent_id = input.start_intent_id.clone();
    c.issued_at_ms = now;
    c.expires_at_ms = now + 20_000;
    c
}

#[test]
fn shared_successor_v2_unstarted_and_timeout_receipts_match_central_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "capacity/managed_agents/custody/shared_execution/management-shared-attempt-v2.json"
    ))
    .unwrap();
    let c: SharedAttemptCommand = serde_json::from_value(fixture["command"].clone()).unwrap();
    for (receipt, hash) in [("unstartedReceipt", "unstartedReceiptDigest"), ("timeoutReceipt", "timeoutReceiptDigest")] {
        let r: SharedAttemptProcessReceipt = serde_json::from_value(fixture[receipt].clone()).unwrap();
        r.validate_for(&c).unwrap();
        assert_eq!(digest(&r), fixture[hash], "{receipt}");
    }
    let mut first = c.clone();
    first.predecessor.process_digest.clear();
    assert!(first.validate(&c.original_runtime_command.reserve_command.manager_mxid).is_err(), "the original is never unstarted");
    let mut bad: SharedAttemptProcessReceipt = serde_json::from_value(fixture["timeoutReceipt"].clone()).unwrap();
    bad.state = "qualified".into();
    bad.policy_digest = Some("b".repeat(64));
    assert!(bad.validate_for(&c).is_err(), "only a stop names the time bound");
}

#[tokio::test]
async fn shared_successor_v2_unstarted_fence_continues_chain_without_replay_or_release() {
    let (mut f, original) = base().await;
    let failed = attempt(&f, &original).await;
    assert!(f.registry.recover_shared_attempt_process(cleanup(&f, &original, 1).await, failed.clone(), None).await.unwrap().is_none(), "no fence, no evidence");
    let fence = f.registry.fence_shared_successor(cleanup(&f, &original, 1).await, failed.clone()).await.unwrap();
    assert!(fence.claimed_revision.is_none());
    let r = f.registry.recover_shared_attempt_process(cleanup(&f, &original, 1).await, failed.clone(), None).await.unwrap().unwrap();
    assert_eq!(r.state, "not_started");
    assert!(r.process_digest.is_none() && r.policy_digest.is_none() && r.reason.is_none());
    assert_eq!(r.ledger_revision, fence.fence_revision);
    assert!(f.registry.claim_shared_successor(attempt_proof(&f, failed.clone())).await.is_err());
    f.registry.shutdown().await.unwrap();
    f.registry = OwnerResourceRegistry::open_root(&f._temp.path().join("owner")).unwrap();
    let again = f.registry.recover_shared_attempt_process(cleanup(&f, &original, 2).await, failed.clone(), Some(r.observation_id.clone())).await.unwrap().unwrap();
    assert_eq!(digest(&again), digest(&r), "identical bytes on every replay");
    // The next attempt follows the unstarted one; old unknown usage stays held.
    let selected = f.registry.shared_successor_selection(f.profile.clone(), original.assignment_id.clone()).await.unwrap().unwrap();
    assert_eq!(digest(selected.predecessor().unwrap()), digest(&failed));
    let saved = f.registry.prepare_shared_attempt_start(e(&f), original.clone(), Some(failed.clone()), "after_unstarted".into()).await.unwrap();
    let input = saved.input().clone();
    assert_eq!(input.predecessor.command_id, failed.command_id);
    assert_eq!(input.predecessor.process_attempt, failed.process_attempt);
    assert!(input.predecessor.process_digest.is_empty());
    assert_eq!(input.predecessor.stopped_receipt_digest, digest(&r));
    assert_eq!((input.budget_observation.known_consumed_tokens, input.budget_observation.retained_tokens), (7, 7));
    let next = successor(&failed, &input, "successor_three");
    saved.matches_command(&next).unwrap();
    assert!(matches!(f.registry.claim_shared_successor(attempt_proof(&f, next.clone())).await.unwrap(), SharedAttemptClaim::Issued(_)));
    // Claimed custody is never relabelled as never started.
    let claimed = f.registry.fence_shared_successor(cleanup(&f, &original, 2).await, next.clone()).await.unwrap();
    assert!(claimed.claimed_revision.is_some());
    assert!(f.registry.recover_shared_attempt_process(cleanup(&f, &original, 2).await, next.clone(), None).await.unwrap().is_none());
    let mut forged = failed;
    forged.start_intent_id = "changed".into();
    assert!(f.registry.recover_shared_attempt_process(cleanup(&f, &original, 2).await, forged, None).await.is_err());
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_successor_v2_uncertain_claim_blocks_only_until_grace_then_chain_continues() {
    let (f, original) = base().await;
    let c = attempt(&f, &original).await;
    assert!(matches!(f.registry.claim_shared_successor(attempt_proof(&f, c.clone())).await.unwrap(), SharedAttemptClaim::Issued(_)));
    // The claimed process left no evidence; cleanup fences it permanently.
    f.registry.fence_shared_successor(cleanup(&f, &original, 1).await, c.clone()).await.unwrap();
    // Within the grace period nothing changes and no second process can start.
    assert!(f.registry.record_shared_attempt_timeout(cleanup(&f, &original, 1).await, c.clone()).await.unwrap().is_none());
    assert!(f.registry.prepare_shared_attempt_start(e(&f), original.clone(), Some(c.clone()), "during_grace".into()).await.is_err());
    assert!(f.registry.claim_shared_successor(attempt_proof(&f, c.clone())).await.is_ok_and(|v| matches!(v, SharedAttemptClaim::History(_))), "the same attempt never claims twice");
    // After it, the claim is interrupted with explicit evidence.
    let clock = Arc::new(AtomicU64::new(c.expires_at_ms + 30_000 + UNCERTAIN_GRACE_MS));
    let worker = clock.clone();
    f.registry.0.store.call(1024, move |_| { graphs::install_test_clock(&worker); Ok(()) }).await.unwrap();
    let timeout = f.registry.record_shared_attempt_timeout(cleanup(&f, &original, 1).await, c.clone()).await.unwrap().unwrap();
    assert_eq!((timeout.state.as_str(), timeout.reason.as_deref()), ("stopped", Some(UNCERTAIN_TIMEOUT)));
    assert!(timeout.process_digest.is_some() && timeout.policy_digest.is_none());
    assert!(f.registry.record_shared_attempt_timeout(cleanup(&f, &original, 1).await, c.clone()).await.unwrap().is_none(), "recorded once");
    let recovered = f.registry.recover_shared_attempt_process(cleanup(&f, &original, 1).await, c.clone(), None).await.unwrap().unwrap();
    assert_eq!(digest(&recovered), digest(&timeout));
    drop(clock);
    // The chain continues from the interrupted attempt; known usage is unchanged.
    let saved = f.registry.prepare_shared_attempt_start(e(&f), original.clone(), Some(c.clone()), "after_timeout".into()).await.unwrap();
    let input = saved.input().clone();
    assert_eq!(input.predecessor.command_id, c.command_id);
    assert_eq!(Some(&input.predecessor.process_digest), timeout.process_digest.as_ref());
    assert_eq!(input.predecessor.stopped_receipt_digest, digest(&timeout));
    assert_eq!((input.budget_observation.known_consumed_tokens, input.budget_observation.retained_tokens), (7, 7));
    let next = successor(&c, &input, "successor_after_timeout");
    assert!(matches!(f.registry.claim_shared_successor(attempt_proof(&f, next)).await.unwrap(), SharedAttemptClaim::Issued(_)));
    f.registry.shutdown().await.unwrap();
}
