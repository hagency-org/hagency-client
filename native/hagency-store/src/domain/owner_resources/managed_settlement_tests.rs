//! Synthetic ledger and authority tests; no real provider or production data.
use super::*;
use super::super::fences;
fn digest<T: Serialize>(v: &T) -> String {
    canonical::digest(&serde_json::to_value(v).unwrap()).unwrap()
}
pub(in super::super) fn command(
    fence: &ManagedAgentFenceCommand,
    receipt: &ManagedAgentFenceReceipt,
    id: &str,
) -> ManagedAgentSettlementCommand {
    ManagedAgentSettlementCommand {
        version: 1,
        capability: AGENT_SETTLEMENT_CAPABILITY.into(),
        operation: "agent.settle".into(),
        command_id: id.into(),
        fence_command: fence.clone(),
        fence_receipt_digest: digest(receipt),
        issued_at_ms: graphs::now_ms().unwrap(),
    }
}
fn proof(f: &Fixture, c: ManagedAgentSettlementCommand) -> VerifiedManagedAgentSettlement {
    VerifiedManagedAgentSettlement::new(
        enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1),
        c.clone(),
        digest(&c),
        f.project.clone(),
        true,
        Instant::now() + Duration::from_secs(20),
    )
    .unwrap()
}
async fn fence(
    f: &Fixture,
    c: ManagedAgentReserveCommand,
) -> (ManagedAgentFenceCommand, ManagedAgentFenceReceipt) {
    let c = fences::command(f, c, "fence_settlement");
    let r = fences::apply_fence(f, c.clone()).await;
    (c, r)
}
pub(in super::super) async fn settle(
    f: &Fixture,
    c: ManagedAgentSettlementCommand,
) -> ManagedAgentSettlementReceipt {
    f.registry.settle_managed_agent(proof(f, c)).await.unwrap()
}
#[tokio::test]
async fn managed_settlement_absent_and_unused_are_exactly_zero_without_reserving() {
    for exists in [false, true] {
        let f = fixture().await;
        let original = agent(&f, "settlement", 150);
        if exists {
            reserve(&f, original.clone()).await;
        }
        let (fc, fr) = fence(&f, original.clone()).await;
        let c = command(&fc, &fr, "settle_zero");
        let r = settle(&f, c.clone()).await;
        r.validate_for(&c).unwrap();
        assert_eq!(
            (
                r.known_consumed_tokens,
                r.retained_tokens,
                r.unresolved_tokens
            ),
            (0, 0, 0)
        );
        assert_eq!(r.process_state, "not_started");
        assert!(r.usage_complete);
        assert_eq!(r.agent_reservation_id.is_some(), exists);
        assert_eq!(count(&f).await, u64::from(exists));
        assert_eq!(digest(&fences::apply_fence(&f, fc).await), digest(&fr));
        assert_eq!(
            reserve(&f, original).await.state,
            if exists { "reserved" } else { "refused" }
        );
    }
}
#[tokio::test]
async fn managed_settlement_late_usage_creates_new_proof_and_preserves_all_previous_receipts() {
    let mut f = fixture().await;
    let original = agent(&f, "settlement", 150);
    let reserve_receipt = reserve(&f, original.clone()).await;
    let child = reserve_receipt.agent_reservation_id.as_ref().unwrap();
    qualify(&f, child).await;
    let call = issue(&f, child, "call", 120).await;
    f.registry
        .record_managed_usage(&call, reported("partial", 40, false, false))
        .await
        .unwrap();
    let (fc, fr) = fence(&f, original.clone()).await;
    let one = command(&fc, &fr, "settle_pending");
    let pending = settle(&f, one.clone()).await;
    assert!(!pending.usage_complete);
    assert_eq!(pending.retained_tokens, 150);
    assert_eq!(pending.known_consumed_tokens, 40);
    f.registry
        .record_late_managed_usage(
            clean(&f, child, 1),
            call.call_id().to_owned(),
            reported("done", 100, true, true),
        )
        .await
        .unwrap();
    let two = command(&fc, &fr, "settle_complete");
    let complete = settle(&f, two.clone()).await;
    assert!(complete.usage_complete);
    assert_eq!(
        (
            complete.known_consumed_tokens,
            complete.retained_tokens,
            complete.unresolved_tokens
        ),
        (100, 100, 0)
    );
    assert!(complete.ledger_revision > pending.ledger_revision);
    assert_eq!(digest(&settle(&f, one.clone()).await), digest(&pending));
    assert_eq!(
        digest(&fences::apply_fence(&f, fc.clone()).await),
        digest(&fr)
    );
    assert_eq!(
        digest(&reserve(&f, original).await),
        digest(&reserve_receipt)
    );
    f.registry.shutdown().await.unwrap();
    f.registry = OwnerResourceRegistry::open_root(&f._temp.path().join("owner")).unwrap();
    assert_eq!(digest(&settle(&f, two.clone()).await), digest(&complete));
    let recovered = f
        .registry
        .recover_managed_agent_settlement(
            f.profile.clone(),
            f.grant.executor_device_id.clone(),
            one,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(digest(&recovered), digest(&pending));
    assert!(
        f.registry
            .issue_managed_call(super::call(&f, child, "new", 1))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn managed_settlement_overrun_is_preserved_not_clamped_to_allocation() {
    let f = fixture().await;
    let original = agent(&f, "overrun", 150);
    let r = reserve(&f, original.clone()).await;
    let child = r.agent_reservation_id.unwrap();
    qualify(&f, &child).await;
    let call = issue(&f, &child, "overrun", 100).await;
    f.registry
        .record_managed_usage(&call, reported("overrun_usage", 181, true, true))
        .await
        .unwrap();
    let (fc, fr) = fence(&f, original).await;
    let r = settle(&f, command(&fc, &fr, "settle_overrun")).await;
    assert!(r.usage_complete);
    assert_eq!(r.effective_ceiling_tokens, Some(150));
    assert_eq!(r.retained_tokens, 181);
    assert_eq!(r.known_consumed_tokens, 181);
}
#[tokio::test]
async fn managed_settlement_missing_or_forged_fence_cannot_create_cleanup_authority() {
    let f = fixture().await;
    let original = agent(&f, "unfenced", 150);
    reserve(&f, original.clone()).await;
    let fc = fences::command(&f, original.clone(), "missing");
    let c = ManagedAgentSettlementCommand {
        version: 1,
        capability: AGENT_SETTLEMENT_CAPABILITY.into(),
        operation: "agent.settle".into(),
        command_id: "no_fence".into(),
        fence_command: fc,
        fence_receipt_digest: "f".repeat(64),
        issued_at_ms: graphs::now_ms().unwrap(),
    };
    assert!(f.registry.settle_managed_agent(proof(&f, c)).await.is_err());
    let (fc, fr) = fence(&f, original).await;
    let mut c = command(&fc, &fr, "bad_receipt");
    c.fence_receipt_digest = "f".repeat(64);
    assert!(
        f.registry
            .settle_managed_agent(proof(&f, c.clone()))
            .await
            .is_err()
    );
    assert!(
        f.registry
            .recover_managed_agent_settlement(
                f.profile.clone(),
                f.grant.executor_device_id.clone(),
                c
            )
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn managed_settlement_concurrent_replay_is_once_and_changed_command_conflicts() {
    let f = fixture().await;
    let (fc, fr) = fence(&f, agent(&f, "absent", 150)).await;
    let c = command(&fc, &fr, "concurrent");
    let (a, b) = tokio::join!(
        f.registry.settle_managed_agent(proof(&f, c.clone())),
        f.registry.settle_managed_agent(proof(&f, c.clone()))
    );
    let a = a.unwrap();
    assert_eq!(digest(&a), digest(&b.unwrap()));
    let mut changed = c.clone();
    changed.issued_at_ms += 1;
    assert!(matches!(
        f.registry.settle_managed_agent(proof(&f, changed)).await,
        Err(Error::Conflict)
    ));
    assert!(
        f.registry
            .recover_managed_agent_settlement(
                authority('b'),
                f.grant.executor_device_id.clone(),
                c.clone()
            )
            .await
            .is_err()
    );
    assert!(
        f.registry
            .recover_managed_agent_settlement(f.profile.clone(), "other-device".into(), c)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn managed_settlement_failure_rolls_back_revision_and_retains_original_fence() {
    let f = fixture().await;
    let (fc, fr) = fence(&f, agent(&f, "rollback", 150)).await;
    let c = command(&fc, &fr, "atomic_settle");
    let before=f.registry.0.store.call(1024,|db|{db.db.execute_batch("CREATE TRIGGER reject_settle BEFORE INSERT ON owner_capacity_receipts WHEN NEW.command_id='atomic_settle' BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;Ok(db.db.query_row("SELECT revision FROM owner_resource_identity WHERE singleton=1",[],|r|r.get::<_,u64>(0))?)}).await.unwrap();
    assert!(
        f.registry
            .settle_managed_agent(proof(&f, c.clone()))
            .await
            .is_err()
    );
    f.registry
        .0
        .store
        .call(1024, move |db| {
            assert_eq!(
                db.db.query_row(
                    "SELECT revision FROM owner_resource_identity WHERE singleton=1",
                    [],
                    |r| r.get::<_, u64>(0)
                )?,
                before
            );
            db.db.execute_batch("DROP TRIGGER reject_settle")?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(digest(&fences::apply_fence(&f, fc).await), digest(&fr));
    assert_eq!(settle(&f, c).await.ledger_revision, before + 1);
}
#[tokio::test]
async fn managed_settlement_expired_delivery_and_retired_profile_never_write() {
    let f = fixture().await;
    let (fc, fr) = fence(&f, agent(&f, "expired", 150)).await;
    let c = command(&fc, &fr, "expired_settle");
    for (execute, expires) in [
        (false, Instant::now() + Duration::from_secs(20)),
        (true, Instant::now() - Duration::from_secs(1)),
        (true, Instant::now() + Duration::from_secs(70)),
    ] {
        assert!(
            VerifiedManagedAgentSettlement::new(
                enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1),
                c.clone(),
                digest(&c),
                f.project.clone(),
                execute,
                expires
            )
            .is_err()
        );
    }
    let p = proof(&f, c);
    f.profile.retire();
    assert!(f.registry.settle_managed_agent(p).await.is_err());
}
#[tokio::test]
async fn managed_settlement_receipt_and_command_reject_open_fields_and_fabricated_credit() {
    let f = fixture().await;
    let original = agent(&f, "wire", 150);
    reserve(&f, original.clone()).await;
    let (fc, fr) = fence(&f, original).await;
    let c = command(&fc, &fr, "wire_settle");
    let r = settle(&f, c.clone()).await;
    let mut v = serde_json::to_value(&c).unwrap();
    v["ownerApproved"] = true.into();
    assert!(serde_json::from_value::<ManagedAgentSettlementCommand>(v).is_err());
    let mut v = serde_json::to_value(&r).unwrap();
    v["remainingTokens"] = 150.into();
    assert!(serde_json::from_value::<ManagedAgentSettlementReceipt>(v).is_err());
    let mut bad = r.clone();
    bad.retained_tokens = 1;
    assert!(bad.validate_for(&c).is_err());
    let mut bad = r.clone();
    bad.process_state = "unknown".into();
    assert!(bad.validate_for(&c).is_err());
    let mut bad = r;
    bad.ledger_revision = bad.fence_revision;
    assert!(bad.validate_for(&c).is_err());
}
