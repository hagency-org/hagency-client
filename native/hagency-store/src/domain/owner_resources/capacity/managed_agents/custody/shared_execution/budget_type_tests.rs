use super::*;
fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("management-shared-budget-execution-v3.json")).unwrap()
}
#[test]
fn budget_execution_v3_canonical_types_preserve_old_versions_and_exact_new_pins() {
    let f = fixture();
    let c: SharedBudgetAttemptCommand = serde_json::from_value(f["command"].clone()).unwrap();
    c.validate("@owner:example.test").unwrap();
    let s: SharedBudgetAttemptStartInput = serde_json::from_value(f["start"].clone()).unwrap();
    s.validate().unwrap();
    let r: SharedBudgetAttemptProcessReceipt =
        serde_json::from_value(f["receipt"].clone()).unwrap();
    r.validate_for(&c).unwrap();
    let o: SharedDemandOffer = serde_json::from_value(f["offer"].clone()).unwrap();
    let d: SharedBudgetDemandStartInput = serde_json::from_value(f["demandStart"].clone()).unwrap();
    d.validate_for(&o).unwrap();
    for (value, name) in [
        (serde_json::to_value(&c).unwrap(), "command"),
        (serde_json::to_value(&s).unwrap(), "start"),
        (serde_json::to_value(&r).unwrap(), "receipt"),
        (serde_json::to_value(&d).unwrap(), "demandStart"),
        (
            serde_json::to_value(&c.execution_budget).unwrap(),
            "executionBudget",
        ),
    ] {
        assert_eq!(
            allocation_digest(&value).unwrap(),
            f[format!("{name}Digest")]
        );
    }
    assert!(serde_json::from_value::<SharedAttemptCommand>(f["command"].clone()).is_err());
    assert!(serde_json::from_value::<SharedAttemptProcessReceipt>(f["receipt"].clone()).is_err());
    let mut forged = f["command"].clone();
    forged["executionAllowed"] = true.into();
    assert!(serde_json::from_value::<SharedBudgetAttemptCommand>(forged).is_err());
    let mut bad = c.clone();
    bad.execution_budget.effective_ceiling_tokens += 1;
    assert!(bad.validate("@owner:example.test").is_err());
    let mut bad = c.clone();
    bad.execution_budget.original_reserve_receipt_digest = "0".repeat(64);
    assert!(bad.validate("@owner:example.test").is_err());
    let mut bad = c.clone();
    bad.expires_at_ms = bad.issued_at_ms + 30_001;
    assert!(bad.validate("@owner:example.test").is_err());
}
#[test]
fn budget_execution_v3_no_start_proof_is_exact_and_does_not_authorize_execution() {
    let f = fixture();
    let c: SharedBudgetAttemptCommand = serde_json::from_value(f["command"].clone()).unwrap();
    let w: SharedWorkItem = serde_json::from_value(f["promotedWork"].clone()).unwrap();
    let r: SharedBudgetWorkRefusal = serde_json::from_value(f["refusal"].clone()).unwrap();
    r.validate_for(&c, &w, &r.input_digest).unwrap();
    assert_eq!(allocation_digest(&r).unwrap(), f["refusalDigest"]);
    let mut bad = r.clone();
    bad.current_budget_digest = bad.execution_budget.budget_digest.clone();
    assert!(bad.validate_for(&c, &w, &bad.input_digest).is_err());
    let mut bad = r.clone();
    bad.state = "stopped".into();
    assert!(bad.validate_for(&c, &w, &bad.input_digest).is_err());
    let mut bad = r.clone();
    bad.work_id = "different_work".into();
    assert!(bad.validate_for(&c, &w, &bad.input_digest).is_err());
    let mut bad = r.clone();
    bad.execution_budget.applied_receipt_digest = "0".repeat(64);
    assert!(bad.validate_for(&c, &w, &bad.input_digest).is_err());
    assert!(r.validate_for(&c, &w, &"1".repeat(64)).is_err());
}
#[test]
fn budget_execution_v3_demand_cannot_rebase_block_or_control() {
    let f = fixture();
    let o: SharedDemandOffer = serde_json::from_value(f["offer"].clone()).unwrap();
    let d: SharedBudgetDemandStartInput = serde_json::from_value(f["demandStart"].clone()).unwrap();
    let mut bad = d.clone();
    bad.expected_control_revision += 1;
    assert!(bad.validate_for(&o).is_err());
    let mut bad = d.clone();
    bad.original_block.as_mut().unwrap().reason = "usage_unrepresentable".into();
    assert!(bad.validate_for(&o).is_err());
    let mut bad = d.clone();
    bad.start.predecessor.command_digest = "1".repeat(64);
    assert!(bad.validate_for(&o).is_err());
    let mut bad = d;
    bad.start.budget_observation.remaining_tokens = 0;
    assert!(bad.validate_for(&o).is_err());
}
