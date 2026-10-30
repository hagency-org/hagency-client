use super::*;
#[test]
fn shared_execution_and_work_match_central_canonical_vectors() {
    let runtime: serde_json::Value =
        serde_json::from_str(include_str!("management-shared-execution-v1.json")).unwrap();
    let command: SharedRuntimeCommand = serde_json::from_value(runtime["command"].clone()).unwrap();
    command
        .validate(&command.reserve_command.manager_mxid)
        .unwrap();
    assert_eq!(
        canonical::digest(&serde_json::to_value(&command).unwrap()).unwrap(),
        runtime["commandDigest"]
    );
    let receipt: SharedProcessReceipt = serde_json::from_value(runtime["receipt"].clone()).unwrap();
    receipt.validate_for(&command).unwrap();
    assert_eq!(
        canonical::digest(&serde_json::to_value(&receipt).unwrap()).unwrap(),
        runtime["receiptDigest"]
    );
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("management-shared-work-v1.json")).unwrap();
    let work: SharedWorkItem = serde_json::from_value(fixture["work"].clone()).unwrap();
    work.validate().unwrap();
    assert_eq!(
        canonical::digest(&serde_json::to_value(&work).unwrap()).unwrap(),
        fixture["workDigest"]
    );
    let receipt: SharedWorkReceipt = serde_json::from_value(fixture["receipt"].clone()).unwrap();
    receipt
        .validate_for(&work, &receipt.process_digest)
        .unwrap();
    assert_eq!(
        canonical::digest(&serde_json::to_value(&receipt).unwrap()).unwrap(),
        fixture["receiptDigest"]
    );
}
#[test]
fn shared_wire_rejects_changed_work_and_oversized_escaped_receipt() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("management-shared-work-v1.json")).unwrap();
    let mut work: SharedWorkItem = serde_json::from_value(fixture["work"].clone()).unwrap();
    work.body.push('!');
    assert!(work.validate().is_err());
    let work: SharedWorkItem = serde_json::from_value(fixture["work"].clone()).unwrap();
    let mut receipt: SharedWorkReceipt =
        serde_json::from_value(fixture["receipt"].clone()).unwrap();
    // The current wire ceiling is 48 KiB; the raw body still fits 32 KiB.
    receipt.body = Some("\"".repeat(24 * 1024));
    use sha2::{Digest, Sha256};
    receipt.body_digest = Some(format!(
        "{:x}",
        Sha256::digest(receipt.body.as_ref().unwrap().as_bytes())
    ));
    assert!(
        receipt
            .validate_for(&work, &receipt.process_digest)
            .is_err()
    );
    let mut unknown = fixture["work"].clone();
    unknown["authority"] = json!(true);
    assert!(serde_json::from_value::<SharedWorkItem>(unknown).is_err());
}
