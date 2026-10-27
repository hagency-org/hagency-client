use super::*;
use ::hagency::native_owner::remote_approval::ExecutionDevice;
use matrix_sdk::deserialized_responses::{DecryptedRoomEvent, EncryptionInfo, VerificationLevel};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};
fn request() -> Request {
    let proposal=serde_json::from_value(json!({"scope":{"agent":"agent","binding":"binding","room":"!project:test","requester":"@bob:test","thread":"$thread"},"dispatch":"dispatch","tool":"codex.command","arguments":{"command":"echo original"},"canonical_directory":"/workspace","risk":"high","policy_revision":[1,1,1],"expires":500})).unwrap();
    let mut request = Request {
        version: 1,
        owner: "@alice:test".into(),
        profile: "a".repeat(64),
        approval_id: "b".repeat(64),
        approval_room: "!private:test".into(),
        execution_id: "execution".into(),
        binding_generation: 1,
        dispatch_epoch: 1,
        execution_device: ExecutionDevice {
            id: "device".into(),
            generation: 1,
        },
        publisher_matrix_device: "HOST".into(),
        proposal_digest: String::new(),
        proposal,
    };
    // The persisted proposal digest uses the same serde tuple/canonical map as
    // the ledger. This fixture's SDK metadata below is explicit test evidence.
    request.proposal_digest =
        "fb5e2d3e7d8f938385d59602ee3bea6fa814bbfbff620579c28f7cf338f1da42".into();
    request
}
fn state() -> Value {
    json!([
        {"type":"m.room.create","state_key":"","content":{}},
        {"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}},
        {"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}},
        {"type":"m.room.guest_access","state_key":"","content":{"guest_access":"forbidden"}},
        {"type":"m.room.history_visibility","state_key":"","content":{"history_visibility":"joined"}},
        {"type":"m.room.member","state_key":"@alice:test","content":{"membership":"join"}}
    ])
}
fn decrypted(
    request: &Request,
    content: Value,
    verification: VerificationState,
    device: &str,
) -> TimelineEvent {
    let event = json!({"type":"m.room.message","event_id":"$verdict","room_id":request.approval_room,"sender":request.owner,"origin_server_ts":1,"content":content});
    TimelineEvent::from_decrypted(
        DecryptedRoomEvent {
            event: Raw::from_json(serde_json::value::to_raw_value(&event).unwrap()),
            encryption_info: Arc::new(EncryptionInfo {
                sender: request.owner.as_str().try_into().unwrap(),
                sender_device: Some(device.into()),
                forwarder: None,
                algorithm_info: AlgorithmInfo::MegolmV1AesSha2 {
                    curve25519_key: "curve-key".into(),
                    sender_claimed_keys: BTreeMap::from([(
                        DeviceKeyAlgorithm::Ed25519,
                        "current-device-key".into(),
                    )]),
                    session_id: Some("session".into()),
                },
                verification_state: verification,
            }),
            unsigned_encryption_info: None,
        },
        None,
    )
}
#[test]
fn remote_approval_private_state_rejects_plain_shared_space_foreign_members_and_duplicate_state() {
    let original = state();
    private_state(&original, "@alice:test").unwrap();
    for index in [0, 1, 2, 3, 4, 5] {
        let mut changed = original.clone();
        match index {
            0 => changed[0]["content"]["type"] = json!("m.space"),
            1 => changed[1]["content"]["algorithm"] = json!("plaintext"),
            2 => changed[2]["content"]["join_rule"] = json!("public"),
            3 => changed[3]["content"]["guest_access"] = json!("can_join"),
            4 => changed[4]["content"]["history_visibility"] = json!("shared"),
            _ => changed[5]["content"]["membership"] = json!("leave"),
        };
        assert!(private_state(&changed, "@alice:test").is_err());
    }
    for membership in ["join", "invite", "knock"] {
        let mut changed = original.clone();
        changed.as_array_mut().unwrap().push(json!({"type":"m.room.member","state_key":"@mallory:test","content":{"membership":membership}}));
        assert!(private_state(&changed, "@alice:test").is_err());
    }
    let mut changed = original.clone();
    changed.as_array_mut().unwrap().push(original[1].clone());
    assert!(private_state(&changed, "@alice:test").is_err());
}
#[test]
fn remote_approval_sdk_metadata_not_wire_device_claims_is_required_for_verdict() {
    let request = request();
    let digest = request.digest().unwrap();
    let verdict = Verdict {
        approval_id: request.approval_id.clone(),
        request_digest: digest.clone(),
        approved: true,
    };
    let content = verdict.message_content("$request").unwrap();
    let verified = decrypted(
        &request,
        content.clone(),
        VerificationState::Verified,
        "REMOTE",
    );
    let result = envelope(
        &verified,
        &request,
        &digest,
        "$request",
        "$verdict",
        "current-device-key",
        32,
    )
    .unwrap();
    assert!(result.verdict.approved);
    assert_eq!(result.sender_device, "REMOTE");
    let plain=TimelineEvent::from_plaintext(Raw::from_json(serde_json::value::to_raw_value(&json!({"type":"m.room.message","event_id":"$verdict","sender":"@alice:test","origin_server_ts":1,"content":content,"unsigned":{"device_id":"REMOTE","verified":true}})).unwrap()));
    assert!(
        envelope(
            &plain,
            &request,
            &digest,
            "$request",
            "$verdict",
            "current-device-key",
            32
        )
        .is_err()
    );
    let unverified = decrypted(
        &request,
        content.clone(),
        VerificationState::Unverified(VerificationLevel::UnsignedDevice),
        "REMOTE",
    );
    assert!(
        envelope(
            &unverified,
            &request,
            &digest,
            "$request",
            "$verdict",
            "current-device-key",
            32
        )
        .is_err()
    );
    let self_echo = decrypted(
        &request,
        content.clone(),
        VerificationState::Verified,
        "HOST",
    );
    assert!(
        envelope(
            &self_echo,
            &request,
            &digest,
            "$request",
            "$verdict",
            "current-device-key",
            32
        )
        .is_err()
    );
    assert!(
        envelope(
            &verified,
            &request,
            &digest,
            "$request",
            "$verdict",
            "changed-device-key",
            32
        )
        .is_err()
    );
    assert!(
        envelope(
            &verified,
            &request,
            &digest,
            "$other-request",
            "$verdict",
            "current-device-key",
            32
        )
        .is_err()
    );
    let mut edit = content;
    edit["m.relates_to"]["rel_type"] = json!("m.replace");
    let edit = decrypted(&request, edit, VerificationState::Verified, "REMOTE");
    assert!(
        envelope(
            &edit,
            &request,
            &digest,
            "$request",
            "$verdict",
            "current-device-key",
            32
        )
        .is_err()
    );
}
