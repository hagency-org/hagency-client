use super::*;
async fn selected(
    f: &Fixture,
    child: &str,
    generation: u64,
) -> Result<crate::owner_resources::shared_execution::SharedExecutionBudget, Error> {
    f.registry
        .managed_agent_execution_budget(
            enrollment_from(
                &f.profile,
                &f.resource,
                &f.grant.resource_revision,
                generation,
            ),
            child.into(),
        )
        .await
}
#[tokio::test]
async fn budget_execution_v3_head_requires_applied_chain_and_current_native_identity() {
    let f = fixture().await;
    let (c, r) = reserve_qualified(&f, "execution_head", 150).await;
    let child = r.agent_reservation_id.as_ref().unwrap();
    assert!(selected(&f, child, 1).await.is_err());
    let pending = delta(&f, &c, &r, "execution_head", 50);
    let applied = f
        .registry
        .apply_managed_budget(proof(&f, pending.clone(), false, 1))
        .await
        .unwrap();
    let pin = selected(&f, child, 1).await.unwrap();
    assert_eq!(pin.applied_command_id, pending.command_id);
    assert_eq!(pin.applied_command_digest, hash_value(&pending));
    assert_eq!(pin.applied_receipt_digest, hash_value(&applied));
    assert_eq!(pin.original_reserve_receipt_digest, hash_value(&r));
    assert_eq!(pin.effective_ceiling_tokens, 200);
    assert!(
        selected(&f, child, 2).await.is_err(),
        "cleanup generation cannot create fresh budget selection"
    );
    let mut next = delta(&f, &c, &r, "execution_head_next", 25);
    next.expected_budget_revision = pin.budget_revision;
    next.expected_budget_digest = pin.budget_digest.clone();
    next.expected_ceiling_tokens = pin.effective_ceiling_tokens;
    let applied2 = f
        .registry
        .apply_managed_budget(proof(&f, next, false, 1))
        .await
        .unwrap();
    let newest = selected(&f, child, 1).await.unwrap();
    assert_eq!(newest.budget_revision, 3);
    assert_eq!(newest.effective_ceiling_tokens, 225);
    assert_ne!(newest, pin);
    let original = f
        .registry
        .recover_managed_budget(
            enrollment_from(&f.profile, &f.resource, &f.grant.resource_revision, 1),
            pending,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hash_value(&original), hash_value(&applied));
    assert_eq!(newest.applied_receipt_digest, hash_value(&applied2));
}
#[tokio::test]
async fn budget_execution_v3_refused_delta_never_produces_an_applied_reference() {
    let f = fixture().await;
    let (c, r) = reserve_qualified(&f, "execution_refusal", 150).await;
    let child = r.agent_reservation_id.as_ref().unwrap();
    let rejected = f
        .registry
        .apply_managed_budget(proof(
            &f,
            delta(&f, &c, &r, "execution_refusal", 50),
            true,
            1,
        ))
        .await
        .unwrap();
    assert_eq!(rejected.state, "cancelled_before_apply");
    assert!(selected(&f, child, 1).await.is_err());
    let actual = f
        .registry
        .apply_managed_budget(proof(
            &f,
            delta(&f, &c, &r, "execution_after_refusal", 50),
            false,
            1,
        ))
        .await
        .unwrap();
    assert_eq!(
        selected(&f, child, 1).await.unwrap().applied_receipt_digest,
        hash_value(&actual)
    );
}
