//! Local HTTP homeserver double with real SDK identities, cross-signatures,
//! Olm/Megolm encryption and decryption. No synthetic VerificationState metadata.
//! This is separate from real Synapse, native approver UI and provider acceptance.
use super::super::*;
use super::state;
use hagency_agent_local::{
    Ledger, Budget, Layer, Limit, Period, Policy, RequestPolicy, ToolPolicy, ToolProposal, inbox,
    audit::Phase,
};
use matrix_sdk::{
    test_utils::mocks::MatrixMockServer,
    ruma::{device_id, event_id, room_id, user_id, TransactionId},
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path_regex},
};
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
fn journal() -> (Ledger, Request) {
    let owner = "@alice:test";
    let profile = "a".repeat(64);
    let mut ledger = Ledger::open_scoped(":memory:", owner, &profile).unwrap();
    let dispatch:inbox::Dispatch=serde_json::from_value(json!({"id":"original","bindingId":"binding","agentId":"agent","eventId":"$original","roomId":"!task:test","requesterMxid":"@bob:test","threadRoot":"$thread","body":"original task","state":"offered","bindingGeneration":1,"dispatchEpoch":1,"dispatchDeviceId":"device","executionId":null,"outcome":null})).unwrap();
    let scope = dispatch.scope();
    ledger.register_binding(owner, &scope).unwrap();
    for layer in [Layer::Agent, Layer::Room, Layer::Requester] {
        ledger
            .set_policy(
                owner,
                &scope,
                layer,
                0,
                &Policy {
                    budget: Budget {
                        limit: Limit::Unlimited,
                        period: Period::Lifetime,
                    },
                    requests: RequestPolicy::Allow,
                    high_risk: ToolPolicy::AskOwner,
                },
            )
            .unwrap();
    }
    ledger
        .receive_dispatch(owner, &dispatch, inbox::Limits::default(), now())
        .unwrap();
    ledger.acknowledge_dispatch(owner, &dispatch.id).unwrap();
    let prepared = ledger.prepare_execution(owner, &dispatch.id).unwrap();
    let mut running = dispatch;
    running.state = "running".into();
    running.execution_id = Some(prepared.execution_id.clone());
    ledger
        .confirm_execution_start(
            owner,
            &inbox::ServerStart {
                dispatch: running,
                newly_started: true,
            },
        )
        .unwrap()
        .unwrap();
    let proposal = ToolProposal {
        scope,
        dispatch: "original".into(),
        tool: "codex.command".into(),
        arguments: json!({"command":"private original command","itemId":"original"}),
        canonical_directory: "/workspace".into(),
        risk: "high".into(),
        policy_revision: [1, 1, 1],
        expires: now() + 120,
    };
    let approval_id = ledger
        .begin_approval_audit(owner, &proposal, now())
        .unwrap();
    let proposal_digest = ledger
        .approval_record(owner, "agent", &approval_id)
        .unwrap()
        .summary
        .proposal_digest;
    let request = Request {
        version: 1,
        owner: owner.into(),
        profile,
        approval_id,
        approval_room: "!private:test".into(),
        execution_id: prepared.execution_id,
        binding_generation: 1,
        dispatch_epoch: 1,
        execution_device: hagency_agent_local::remote_approval::ExecutionDevice {
            id: "device".into(),
            generation: 1,
        },
        publisher_matrix_device: "HOST".into(),
        proposal_digest,
        proposal,
    };
    (ledger, request)
}
#[tokio::test]
async fn actual_sdk_private_publication_and_cross_signed_remote_verdict_reach_original_journal() {
    let server = MatrixMockServer::new().await;
    server.mock_crypto_endpoints_preset().await;
    let owner = user_id!("@alice:test");
    let room_id = room_id!("!private:test");
    let host = server
        .client_builder_for_crypto_end_to_end(owner, device_id!("HOST"))
        .build()
        .await;
    host.encryption()
        .bootstrap_cross_signing(None)
        .await
        .unwrap();
    server.mock_sync().ok_and_run(&host, |_| {}).await;
    let remote = server
        .set_up_new_device_for_encryption(&host, device_id!("REMOTE"), vec![])
        .await;
    host.encryption()
        .request_user_identity(owner)
        .await
        .unwrap();
    host.encryption()
        .get_device(owner, remote.device_id().unwrap())
        .await
        .unwrap()
        .unwrap()
        .verify()
        .await
        .unwrap();
    host.encryption()
        .request_user_identity(owner)
        .await
        .unwrap();
    remote
        .encryption()
        .request_user_identity(owner)
        .await
        .unwrap()
        .unwrap()
        .verify()
        .await
        .unwrap();
    remote
        .encryption()
        .request_user_identity(owner)
        .await
        .unwrap();
    let unsigned = server
        .set_up_new_device_for_encryption(&host, device_id!("UNSIGNED"), vec![&remote])
        .await;
    let unsigned_room = server.sync_joined_room(&unsigned, room_id).await;
    let host_room = server.sync_joined_room(&host, room_id).await;
    let remote_room = server.sync_joined_room(&remote, room_id).await;
    server
        .mock_room_state_encryption()
        .expect_any_access_token()
        .encrypted()
        .mount()
        .await;
    // Keep one state responder. wiremock0.6.5 sorts mounted mocks by
    // priority but indexes scoped guards by their original vector position;
    // mixing priorities and scoped guards can deactivate a different mock.
    let room_state = Arc::new(Mutex::new(state()));
    let served_state = room_state.clone();
    Mock::given(method("GET"))
        .and(path_regex(r"/rooms/[^/]+/state$"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(served_state.lock().unwrap().clone())
        })
        .mount(server.server())
        .await;
    let event_response = Arc::new(Mutex::new(Value::Null));
    let served_event = event_response.clone();
    Mock::given(method("GET"))
        .and(path_regex(r"/rooms/[^/]+/event/[^/]+$"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(served_event.lock().unwrap().clone())
        })
        .mount(server.server())
        .await;
    let related = Arc::new(Mutex::new(json!({"chunk":[],"next_batch":null})));
    let served_relations = related.clone();
    Mock::given(method("GET"))
        .and(path_regex(
            r"/relations/[^/]+/m.reference/m.room.encrypted$",
        ))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(served_relations.lock().unwrap().clone())
        })
        .mount(server.server())
        .await;
    let origin = host.homeserver().to_string();
    assert_eq!(
        prepare(&host, &origin, owner.as_str(), room_id.as_str(), || Ok(()))
            .await
            .unwrap(),
        "HOST"
    );
    let (mut ledger, request) = journal();
    let mut publication = ledger
        .reserve_remote_approval(owner.as_str(), &request, now)
        .unwrap();
    let queue = Arc::new(Mutex::new(Default::default()));
    let capture_host = server
        .capture_put_to_device_traffic(owner, queue.clone())
        .await;
    let (request_rx, request_mock) = server
        .mock_room_send()
        .ok_with_capture(event_id!("$request"), owner);
    request_mock.mock_once().mount().await;
    let event = publish(
        &host,
        &origin,
        publication.begin_publication().unwrap(),
        || Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(event, "$request");
    ledger
        .record_remote_approval_receipt(owner.as_str(), &publication, &event, now())
        .unwrap();
    assert_eq!(
        find(&host, &origin, &publication, &event, || Ok(()))
            .await
            .err()
            .unwrap()
            .code,
        "remote_approval_waiting"
    );
    let encrypted_request = request_rx.await.unwrap();
    assert!(
        !encrypted_request
            .json()
            .get()
            .contains("private original command")
    );
    {
        let recipients = queue.lock().unwrap();
        let devices = recipients.get(owner).unwrap();
        assert_eq!(devices.len(), 1);
        assert!(devices.contains_key(device_id!("REMOTE")));
        assert!(!devices.contains_key(device_id!("UNSIGNED")));
    }
    server
        .sync_back_pending_to_device_messages(queue.clone(), &remote)
        .await;
    let plain = remote_room
        .decrypt_event(&Raw::from_json(encrypted_request.json().to_owned()), None)
        .await
        .unwrap();
    let clear: Value = serde_json::from_str(plain.raw().json().get()).unwrap();
    assert_eq!(
        clear["content"]["im.hagency.owner.approval"]["proposal"]["arguments"]["command"],
        "private original command"
    );
    let before = server.server().received_requests().await.unwrap().len();
    let mut shared = state();
    shared.as_array_mut().unwrap().push(
        json!({"type":"m.room.member","state_key":"@foreign:test","content":{"membership":"join"}}),
    );
    *room_state.lock().unwrap() = shared;
    assert!(
        prepare(&host, &origin, owner.as_str(), room_id.as_str(), || Ok(()))
            .await
            .is_err()
    );
    assert!(
        server.server().received_requests().await.unwrap()[before..]
            .iter()
            .all(|r| r.method != "PUT")
    );
    *room_state.lock().unwrap() = state();
    drop(capture_host);
    queue.lock().unwrap().clear();
    let capture_unsigned = server
        .capture_put_to_device_traffic(owner, queue.clone())
        .await;
    let member=Raw::from_json(serde_json::value::to_raw_value(&json!({"type":"m.room.member","state_key":owner,"sender":owner,"event_id":"$member","origin_server_ts":1,"content":{"membership":"join"}})).unwrap());
    server
        .mock_get_members()
        .expect_any_access_token()
        .ok(vec![member])
        .mount()
        .await;
    let (untrusted_rx, untrusted_mock) = server
        .mock_room_send()
        .ok_with_capture(event_id!("$untrusted"), owner);
    untrusted_mock.mock_once().mount().await;
    let claimed = Verdict {
        approval_id: request.approval_id.clone(),
        request_digest: publication.request_digest().into(),
        approved: true,
    };
    unsigned_room
        .send_raw("m.room.message", claimed.message_content(&event).unwrap())
        .await
        .unwrap();
    let raw_untrusted = untrusted_rx.await.unwrap();
    server
        .sync_back_pending_to_device_messages(queue.clone(), &host)
        .await;
    let mut forged: Value = serde_json::from_str(raw_untrusted.json().get()).unwrap();
    forged["content"]["device_id"] = json!("REMOTE");
    forged["unsigned"] = json!({"verified":true});
    *event_response.lock().unwrap() = forged;
    assert!(
        observe(
            &host,
            &origin,
            &publication,
            &event,
            "$untrusted",
            || Ok(())
        )
        .await
        .is_err()
    );
    assert_eq!(
        ledger
            .approval_phase(owner.as_str(), &request.approval_id)
            .unwrap(),
        Phase::Offered
    );
    drop(capture_unsigned);
    queue.lock().unwrap().clear();
    let _capture_remote = server
        .capture_put_to_device_traffic(owner, queue.clone())
        .await;
    let verdict = Verdict {
        approval_id: request.approval_id.clone(),
        request_digest: publication.request_digest().into(),
        approved: true,
    };
    let (verdict_rx, verdict_mock) = server
        .mock_room_send()
        .ok_with_capture(event_id!("$verdict"), owner);
    verdict_mock.mock_once().mount().await;
    remote_room
        .seal_for_own_devices("m.room.message", verdict.message_content(&event).unwrap())
        .await
        .unwrap()
        .send(&TransactionId::new(), || true)
        .await
        .unwrap();
    let encrypted_verdict = verdict_rx.await.unwrap();
    server
        .sync_back_pending_to_device_messages(queue, &host)
        .await;
    *event_response.lock().unwrap() =
        serde_json::from_str::<Value>(encrypted_verdict.json().get()).unwrap();
    let fresh_state = bounded_get(
        &host,
        &origin,
        &[
            "_matrix",
            "client",
            "v3",
            "rooms",
            room_id.as_str(),
            "state",
        ],
    )
    .await
    .unwrap();
    private_state(&fresh_state, owner.as_str()).expect("actual current room remains private");
    assert_eq!(
        host_room.state(),
        RoomState::Joined,
        "host room remains joined"
    );
    let own_identity = host
        .encryption()
        .request_user_identity(owner)
        .await
        .unwrap()
        .unwrap();
    assert!(own_identity.is_verified(), "own identity remains verified");
    let own_device = host.encryption().get_own_device().await.unwrap().unwrap();
    assert!(!own_device.is_deleted(), "own device remains current");
    assert!(
        own_device.is_verified_with_cross_signing(),
        "own publisher remains cross-signed"
    );
    let peer = host
        .encryption()
        .get_device(owner, device_id!("REMOTE"))
        .await
        .unwrap()
        .unwrap();
    assert!(
        peer.is_verified_with_cross_signing(),
        "real remote signer remains cross-signed"
    );
    assert_eq!(
        prepare(&host, &origin, owner.as_str(), room_id.as_str(), || Ok(()))
            .await
            .expect("current private state and identity after rejected forgery"),
        "HOST"
    );
    let current_event = host_room
        .decrypt_event(&Raw::from_json(encrypted_verdict.json().to_owned()), None)
        .await
        .unwrap();
    let info = current_event
        .encryption_info()
        .expect("actual SDK decrypted the original signed verdict");
    assert!(
        matches!(info.verification_state, VerificationState::Verified),
        "actual SDK verification: {:?}",
        info.verification_state
    );
    assert_eq!(info.sender_device.as_deref(), Some(device_id!("REMOTE")));
    assert!(info.forwarder.is_none());
    let fetched = bounded_get(
        &host,
        &origin,
        &[
            "_matrix",
            "client",
            "v3",
            "rooms",
            room_id.as_str(),
            "event",
            "$verdict",
        ],
    )
    .await
    .unwrap();
    assert_eq!(fetched["event_id"], "$verdict");
    let encrypted_value: Value = serde_json::from_str(encrypted_verdict.json().get()).unwrap();
    *related.lock().unwrap() = json!({"chunk":[encrypted_value.clone()],"next_batch":"more"});
    assert_eq!(
        find(&host, &origin, &publication, &event, || Ok(()))
            .await
            .err()
            .unwrap()
            .code,
        "remote_approval_incomplete"
    );
    *related.lock().unwrap() =
        json!({"chunk":[encrypted_value.clone(),encrypted_value.clone()],"next_batch":null});
    assert!(
        find(&host, &origin, &publication, &event, || Ok(()))
            .await
            .is_err()
    );
    assert_eq!(
        ledger
            .approval_phase(owner.as_str(), &request.approval_id)
            .unwrap(),
        Phase::Offered
    );
    *related.lock().unwrap() = json!({"chunk":[encrypted_value],"next_batch":null});
    let observed = find(&host, &origin, &publication, &event, || Ok(()))
        .await
        .unwrap();
    assert_eq!(observed.sender_device, "REMOTE");
    let current = host
        .encryption()
        .get_device(owner, device_id!("REMOTE"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        observed.sender_ed25519,
        current.ed25519_key().unwrap().to_base64()
    );
    assert!(current.is_verified_with_cross_signing());
    let decision = ledger
        .record_remote_approval_decision(
            owner.as_str(),
            &publication,
            &request.execution_device,
            &observed,
            now,
        )
        .unwrap();
    assert!(decision.approved());
    assert_eq!(
        ledger
            .approval_phase(owner.as_str(), &request.approval_id)
            .unwrap(),
        Phase::OwnerAllowed
    );
    assert!(ledger.authorize_tool(&request.proposal, now()).is_err());
    assert!(
        ledger
            .record_remote_approval_decision(
                owner.as_str(),
                &publication,
                &request.execution_device,
                &observed,
                now
            )
            .is_err()
    );
    assert_eq!(host_room.state(), RoomState::Joined);
}
