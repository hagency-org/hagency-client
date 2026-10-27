use super::*;
const OWNER: &str = "@owner:test";
fn fixture() -> (Ledger, Dispatch, Input) {
    let mut l = Ledger::open_scoped(":memory:", OWNER, &"a".repeat(64)).unwrap();
    let d = Dispatch {
        id: "evt_task".into(),
        binding_id: "bnd_binding".into(),
        agent_id: "agt_agent".into(),
        event_id: "$event".into(),
        room_id: "!room:test".into(),
        requester_mxid: OWNER.into(),
        thread_root: "!room:test".into(),
        body: "original secret".into(),
        state: "offered".into(),
        binding_generation: 1,
        dispatch_epoch: Some(1),
        dispatch_device_id: Some("dev_device".into()),
        execution_id: None,
        outcome: None,
    };
    l.register_binding(OWNER, &d.scope()).unwrap();
    let c = json!({"type":"m.room.encrypted","event_id":d.event_id,"room_id":d.room_id,"sender":OWNER,"origin_server_ts":1,"content":{"algorithm":"m.megolm.v1.aes-sha2","ciphertext":"c2VhbGVk","session_id":"session"}});
    let digest = format!(
        "{:x}",
        Sha256::digest(
            canonical(&json!(["agent-encrypted-inbox-v1", c]))
                .unwrap()
                .as_bytes()
        )
    );
    let i = Input {
        inbox_id: "enc_original".into(),
        source_digest: digest,
        agent_generation: 1,
        owner_direct: true,
        ciphertext: c,
        message: json!({"type":"m.room.message","event_id":d.event_id,"sender":OWNER,"content":{"msgtype":"m.text","body":d.body}}),
    };
    (l, d, i)
}
#[test]
fn encrypted_input_commit_binds_plaintext_locally_and_keeps_history_ciphertext_only() {
    let (mut l, d, i) = fixture();
    let r = l
        .receive_encrypted_dispatch(OWNER, &d, &i, Limits::default(), 10)
        .unwrap();
    assert!(!r.already_durable);
    let transport = l
        .encrypted_input_transport_digest(OWNER, &d.id)
        .unwrap()
        .unwrap();
    assert_ne!(transport, d.digest().unwrap());
    let mut opaque = d.clone();
    opaque.body = canonical(&i.ciphertext).unwrap();
    assert_eq!(transport, opaque.digest().unwrap());
    assert!(
        l.receive_encrypted_dispatch(OWNER, &d, &i, Limits::default(), 11)
            .unwrap()
            .already_durable
    );
    assert!(
        l.receive_dispatch(OWNER, &d, Limits::default(), 11)
            .is_err()
    );
    l.acknowledge_dispatch(OWNER, &d.id).unwrap();
    assert!(l.prepare_execution(OWNER, &d.id).is_err());
    let history = crate::inbox::HistoryExecution {
        dispatch_id: d.id.clone(),
        execution_id: "exec_old".into(),
        binding_id: d.binding_id.clone(),
        agent_id: d.agent_id.clone(),
        event_id: d.event_id.clone(),
        room_id: d.room_id.clone(),
        requester_mxid: d.requester_mxid.clone(),
        thread_root: d.thread_root.clone(),
        binding_generation: 1,
        dispatch_epoch: 1,
        dispatch_device_id: "dev_device".into(),
        immutable_digest: transport,
    };
    // Historical metadata fixture only; no server start/provider invocation is minted.
    l.db.execute(
        "UPDATE local_inbox SET state='unknown',execution_id='exec_old' WHERE id=?",
        [&d.id],
    )
    .unwrap();
    assert!(
        l.covers_execution_history(OWNER, &d.agent_id, std::slice::from_ref(&history))
            .unwrap()
    );
    let mut wrong = history;
    wrong.immutable_digest = d.digest().unwrap();
    assert!(
        !l.covers_execution_history(OWNER, &d.agent_id, &[wrong])
            .unwrap()
    );
}
#[test]
fn encrypted_input_invalid_body_or_capacity_rolls_back_both_records() {
    let (mut l, mut d, i) = fixture();
    d.body = "changed".into();
    assert!(
        l.receive_encrypted_dispatch(OWNER, &d, &i, Limits::default(), 10)
            .is_err()
    );
    assert_eq!(
        l.db.query_row("SELECT count(*) FROM encrypted_inputs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        l.db.query_row("SELECT count(*) FROM local_inbox", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    d.body = "original secret".into();
    assert!(
        l.receive_encrypted_dispatch(
            OWNER,
            &d,
            &i,
            Limits {
                max_records: 1,
                max_bytes: 1
            },
            10
        )
        .is_err()
    );
    assert_eq!(
        l.db.query_row("SELECT count(*) FROM encrypted_inputs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn encrypted_input_snapshot_retains_both_digests_without_minting_execution() {
    let (mut l, d, i) = fixture();
    l.receive_encrypted_dispatch(OWNER, &d, &i, Limits::default(), 10)
        .unwrap();
    l.acknowledge_dispatch(OWNER, &d.id).unwrap();
    let profile = "a".repeat(64);
    let snapshot = l.snapshot(OWNER, &profile, 20).unwrap();
    let inspected =
        crate::backup::Snapshot::inspect(snapshot.bytes().to_vec(), OWNER, &profile).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &profile).unwrap();
    restored
        .restore_snapshot(OWNER, &profile, &inspected)
        .unwrap();
    assert_eq!(
        restored
            .encrypted_input_transport_digest(OWNER, &d.id)
            .unwrap(),
        l.encrypted_input_transport_digest(OWNER, &d.id).unwrap()
    );
    assert!(restored.prepare_execution(OWNER, &d.id).is_err());
    assert!(
        restored
            .db
            .execute("UPDATE encrypted_inputs SET source_digest='changed'", [])
            .is_err()
    );
    assert!(
        restored
            .db
            .execute("DELETE FROM encrypted_inputs", [])
            .is_err()
    );
    let mut changed = i;
    changed.message["content"]["extra"] = json!("substituted descriptor");
    assert!(
        restored
            .receive_encrypted_dispatch(OWNER, &d, &changed, Limits::default(), 20)
            .is_err()
    );
}
#[test]
fn encrypted_input_legacy_snapshot_adds_no_crypto_claim_and_partial_schema_refuses() {
    let (mut l, d, _) = fixture();
    l.receive_dispatch(OWNER, &d, Limits::default(), 10)
        .unwrap();
    l.db.execute_batch("DROP TABLE encrypted_outputs;DROP TABLE encrypted_inputs")
        .unwrap();
    let profile = "a".repeat(64);
    let snapshot = l.snapshot(OWNER, &profile, 20).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &profile).unwrap();
    restored
        .restore_snapshot(OWNER, &profile, &snapshot)
        .unwrap();
    assert!(
        restored
            .encrypted_input_transport_digest(OWNER, &d.id)
            .unwrap()
            .is_none()
    );
    restored
        .db
        .execute_batch("DROP TRIGGER retain_encrypted_input")
        .unwrap();
    assert!(restored.snapshot(OWNER, &profile, 20).is_err());
}

#[test]
fn encrypted_input_same_sealed_message_under_a_new_event_id_never_becomes_new_work() {
    let (mut l, d, i) = fixture();
    l.receive_encrypted_dispatch(OWNER, &d, &i, Limits::default(), 10)
        .unwrap();
    let mut replay = Input {
        inbox_id: "enc_replay".into(),
        source_digest: String::new(),
        agent_generation: 1,
        owner_direct: true,
        ciphertext: i.ciphertext.clone(),
        message: i.message.clone(),
    };
    let mut next = d.clone();
    next.id = "evt_replay".into();
    next.event_id = "$different".into();
    replay.ciphertext["event_id"] = json!(next.event_id);
    replay.ciphertext["origin_server_ts"] = json!(2);
    replay.message["event_id"] = json!(next.event_id);
    // Alternate legal padding cannot create a different replay witness.
    replay.ciphertext["content"]["ciphertext"] = json!("c2VhbGVk=");
    replay.source_digest = format!(
        "{:x}",
        Sha256::digest(
            canonical(&json!(["agent-encrypted-inbox-v1", replay.ciphertext]))
                .unwrap()
                .as_bytes()
        )
    );
    assert!(
        l.receive_encrypted_dispatch(OWNER, &next, &replay, Limits::default(), 20)
            .is_err()
    );
    assert!(l.inbox_record(OWNER, &next.id).is_err());
    assert_eq!(
        l.db.query_row("SELECT count(*) FROM encrypted_inputs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn encrypted_execution_requires_original_durable_input_and_new_server_start_once() {
    let (mut l, d, input) = fixture();
    assert!(l.prepare_encrypted_execution(OWNER, &d.id).is_err());
    l.receive_encrypted_dispatch(OWNER, &d, &input, Limits::default(), 10)
        .unwrap();
    l.acknowledge_dispatch(OWNER, &d.id).unwrap();
    let prepared = l.prepare_encrypted_execution(OWNER, &d.id).unwrap();
    let mut running = d.clone();
    running.state = "running".into();
    running.execution_id = Some(prepared.execution_id);
    let response = crate::inbox::ServerStart {
        dispatch: running,
        newly_started: true,
    };
    assert!(l.confirm_execution_start(OWNER, &response).is_err());
    assert!(
        l.confirm_encrypted_execution_start(OWNER, &response)
            .unwrap()
            .is_some()
    );
    assert!(
        l.confirm_encrypted_execution_start(OWNER, &response)
            .unwrap()
            .is_none()
    );
    assert!(l.prepare_encrypted_execution(OWNER, &d.id).is_err());
}

#[test]
fn encrypted_original_output_survives_snapshot_without_resealing_or_substitution() {
    let (mut l, d, input) = fixture();
    l.receive_encrypted_dispatch(OWNER, &d, &input, Limits::default(), 10)
        .unwrap();
    let payload = json!({"content":{"ciphertext":"original"},"shares":[]});
    l.retain_encrypted_output(OWNER, &d.id, "reply", "reply", &payload)
        .unwrap();
    l.retain_encrypted_output(OWNER, &d.id, "reply", "reply", &payload)
        .unwrap();
    assert!(
        l.retain_encrypted_output(OWNER, &d.id, "reply", "reply", &json!({"changed":true}))
            .is_err()
    );
    let profile = "a".repeat(64);
    let snapshot = l.snapshot(OWNER, &profile, 20).unwrap();
    let mut restored = Ledger::open_scoped(":memory:", OWNER, &profile).unwrap();
    restored
        .restore_snapshot(OWNER, &profile, &snapshot)
        .unwrap();
    assert_eq!(
        restored
            .encrypted_output(OWNER, &d.id, "reply", "reply")
            .unwrap(),
        Some(payload)
    );
    assert!(
        restored
            .db
            .execute("DELETE FROM encrypted_outputs", [])
            .is_err()
    );
}
