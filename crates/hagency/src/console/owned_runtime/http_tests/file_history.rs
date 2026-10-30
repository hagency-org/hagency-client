use super::*;
use crate::console::native::{Command, HistoryAction};
use hagency_agent_local::{
    Budget, Layer, Limit, Period, Policy, RequestPolicy, ToolPolicy,
    audit::Phase,
    file_delivery::{FileLookup, FileObservation},
    sent_files::Sender,
};
#[derive(Default)]
struct HistorySource {
    denied: Arc<std::sync::atomic::AtomicBool>,
}
impl MatrixTokenSource for HistorySource {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Box::pin(async move {
            if self.denied.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(NativeError {
                    status: 401,
                    code: "fixture_owner_changed".into(),
                });
            }
            Source.access_token().await
        })
    }
}
#[tokio::test]
async fn native_owner_file_history_refreshes_ended_originals_without_restarting_or_resending() {
    let (origin, state, server) = server(false, 0).await;
    let temp = tempfile::tempdir().unwrap();
    let source = Arc::new(HistorySource::default());
    let native = NativeOwner::open_with_matrix_and_credentials(
        &temp.path().join("state"),
        &origin,
        OWNER,
        source.clone(),
        crate::console::server_login::credential_tests::vault(),
    )
    .await
    .unwrap();
    let device = native.test_console().authorized_device().await.unwrap();
    let (path, profile) = intent_location(native.test_console(), &device).unwrap();
    let ledger_path = path.with_file_name("agent-local.sqlite");
    let mut ledger = Ledger::open_scoped(&ledger_path, OWNER, &profile).unwrap();
    let dispatch: inbox::Dispatch = serde_json::from_value(event("original-files", 1)).unwrap();
    let scope = dispatch.scope();
    ledger.register_binding(OWNER, &scope).unwrap();
    for layer in [Layer::Agent, Layer::Room, Layer::Requester] {
        ledger
            .set_policy(
                OWNER,
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
        .receive_dispatch(OWNER, &dispatch, inbox::Limits::default(), 100)
        .unwrap();
    ledger.acknowledge_dispatch(OWNER, &dispatch.id).unwrap();
    let prepared = ledger.prepare_execution(OWNER, &dispatch.id).unwrap();
    let mut running = dispatch.clone();
    running.state = "running".into();
    running.execution_id = Some(prepared.execution_id.clone());
    ledger
        .confirm_execution_start(
            OWNER,
            &inbox::ServerStart {
                dispatch: running,
                newly_started: true,
            },
        )
        .unwrap()
        .unwrap();
    ledger
        .reserve(&scope, "original-files-call", &dispatch.id, 10, 101)
        .unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let sender = Sender::open(&workspace).unwrap();
    let mut originals = vec![];
    for (name, bytes) in [
        ("first.txt", &b"hello"[..]),
        ("absent.txt", &b"absent"[..]),
        ("revoked.txt", &b"revoked"[..]),
    ] {
        std::fs::write(workspace.join(name), bytes).unwrap();
        let c = sender
            .capture(
                &ledger,
                &scope,
                &dispatch.id,
                &prepared.execution_id,
                name,
                None,
                None,
                "thread",
                "turn",
                name,
                101,
            )
            .unwrap();
        let p = c.proposal();
        let audit = ledger.begin_approval_audit(OWNER, p, 101).unwrap();
        ledger
            .record_approval_event(OWNER, &audit, Phase::OwnerAllowed, 101, &json!({}))
            .unwrap();
        ledger.approve_tool(OWNER, p, 101).unwrap();
        ledger
            .record_approval_event(OWNER, &audit, Phase::Applying, 101, &json!({}))
            .unwrap();
        let custody = sender.begin(&mut ledger, c, &audit, 101).unwrap();
        originals.push(custody.record().clone());
    }
    ledger
        .finish_local_execution(
            OWNER,
            &dispatch.id,
            &prepared.execution_id,
            inbox::Finish::Unknown,
        )
        .unwrap();
    drop(ledger);
    let command = |id: &str| Command::AgentHistory {
        action: HistoryAction::RefreshFile {
            agent: "agent-a".into(),
            id: id.into(),
        },
    };
    let page = native
        .execute(Command::AgentHistory {
            action: HistoryAction::Files {
                agent: "agent-a".into(),
                cursor: None,
            },
        })
        .await
        .unwrap();
    assert_eq!(page["files"]["records"].as_array().unwrap().len(), 3);
    assert_eq!(page["runtimeActive"], false);
    assert_eq!(
        native
            .execute(command(&originals[0].id))
            .await
            .unwrap_err()
            .code,
        "file_history_upgrade_required"
    );
    state.lock().unwrap().file_history = true;
    let query = |i: usize| FileLookup {
        dispatch_id: dispatch.id.clone(),
        execution_id: prepared.execution_id.clone(),
        request_id: originals[i].request_id.clone(),
    };
    let observation = |i: usize, closed: bool| {
        serde_json::to_value(FileObservation {
            request: query(i),
            delivery: None,
            preparation_closed: closed,
        })
        .unwrap()
    };
    state.lock().unwrap().file_delivery_response = Some(observation(0, false));
    let pending = native.execute(command(&originals[0].id)).await.unwrap();
    assert!(pending["file"]["record"]["absence"].is_null());
    assert!(pending["file"]["record"]["remote"].is_null());
    let first = &originals[0];
    let mut delivery = json!({"id":"file-original","dispatchId":dispatch.id,"executionId":prepared.execution_id,"requestId":first.request_id,"createdAtMs":1,"manifest":first.manifest,"manifestDigest":first.manifest.digest().unwrap(),"receivedBytes":5,"state":"unknown","uploadAccepted":true,"matrixEventId":null});
    state.lock().unwrap().file_delivery_response =
        Some(json!({"request":query(0),"delivery":delivery,"preparationClosed":true}));
    let unknown = native.execute(command(&first.id)).await.unwrap();
    assert_eq!(unknown["file"]["record"]["remote"]["state"], "unknown");
    delivery["state"] = "sent".into();
    delivery["matrixEventId"] = "$actualHistoricalReceipt".into();
    state.lock().unwrap().file_delivery_response =
        Some(json!({"request":query(0),"delivery":delivery,"preparationClosed":true}));
    let sent = native.execute(command(&first.id)).await.unwrap();
    assert_eq!(sent["file"]["record"]["remote"]["state"], "sent");
    assert_eq!(sent["runtimeActive"], false);
    state.lock().unwrap().file_delivery_response = Some(observation(1, true));
    let absent = native.execute(command(&originals[1].id)).await.unwrap();
    assert!(absent["file"]["record"]["absence"].is_object());
    assert!(absent["file"]["record"]["remote"].is_null());
    state.lock().unwrap().file_delivery_response = Some(observation(2, true));
    state.lock().unwrap().file_lookup_deny = Some(source.denied.clone());
    assert!(native.execute(command(&originals[2].id)).await.is_err());
    let ledger = Ledger::open_scoped(&ledger_path, OWNER, &profile).unwrap();
    let summary = ledger
        .sent_file_summary(&scope, &dispatch.id, &prepared.execution_id)
        .unwrap();
    assert_eq!(
        (summary.sent, summary.not_prepared, summary.unconfirmed),
        (1, 1, 1)
    );
    assert!(
        ledger
            .sent_file_record(OWNER, "agent-a", &originals[2].id)
            .unwrap()
            .record
            .absence
            .is_none()
    );
    assert_eq!(
        ledger.approval_phase(OWNER, &first.audit).unwrap(),
        Phase::Unknown
    );
    let wire = state.lock().unwrap();
    assert_eq!(
        wire.paths.iter().filter(|p| p.contains("/files/")).count(),
        5
    );
    assert!(
        wire.paths
            .iter()
            .filter(|p| p.contains("/files/"))
            .all(|p| p.ends_with("/lookup"))
    );
    assert!(
        !wire
            .paths
            .iter()
            .any(|p| p.ends_with("/acquire") || p.ends_with("/events/start"))
    );
    server.abort();
}

#[tokio::test]
async fn native_owner_encrypted_file_history_reads_late_receipt_without_lease_or_plaintext_lookup()
{
    let (origin, state, server) = server(false, 0).await;
    let temp = tempfile::tempdir().unwrap();
    let source = Arc::new(HistorySource::default());
    let native = NativeOwner::open_with_matrix_and_credentials(
        &temp.path().join("state"),
        &origin,
        OWNER,
        source.clone(),
        crate::console::server_login::credential_tests::vault(),
    )
    .await
    .unwrap();
    let device = native.test_console().authorized_device().await.unwrap();
    let (path, profile) = intent_location(native.test_console(), &device).unwrap();
    let ledger_path = path.with_file_name("agent-local.sqlite");
    let mut ledger = Ledger::open_scoped(&ledger_path, OWNER, &profile).unwrap();
    let mut dispatch: inbox::Dispatch = serde_json::from_value(event("original-files", 1)).unwrap();
    dispatch.requester_mxid = OWNER.into();
    let scope = dispatch.scope();
    ledger.register_binding(OWNER, &scope).unwrap();
    for layer in [Layer::Agent, Layer::Room, Layer::Requester] {
        ledger
            .set_policy(
                OWNER,
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
    let ciphertext = json!({"type":"m.room.encrypted","event_id":dispatch.event_id,"room_id":dispatch.room_id,"sender":OWNER,"origin_server_ts":1,"content":{"algorithm":"m.megolm.v1.aes-sha2","ciphertext":"c2VhbGVk","session_id":"original-session"}});
    let input = hagency_agent_local::encrypted_inputs::Input {
        inbox_id: "enc_original".into(),
        source_digest: crate::console::device_execution::ciphertexts::source_digest(&ciphertext)
            .unwrap(),
        agent_generation: 1,
        owner_direct: true,
        ciphertext,
        message: json!({"type":"m.room.message","event_id":dispatch.event_id,"sender":OWNER,"content":{"msgtype":"m.text","body":dispatch.body,"m.relates_to":{"rel_type":"m.thread","event_id":dispatch.thread_root}}}),
    };
    ledger
        .receive_encrypted_dispatch(OWNER, &dispatch, &input, inbox::Limits::default(), 100)
        .unwrap();
    ledger.acknowledge_dispatch(OWNER, &dispatch.id).unwrap();
    let prepared = ledger
        .prepare_encrypted_execution(OWNER, &dispatch.id)
        .unwrap();
    let mut running = dispatch.clone();
    running.state = "running".into();
    running.execution_id = Some(prepared.execution_id.clone());
    ledger
        .confirm_encrypted_execution_start(
            OWNER,
            &inbox::ServerStart {
                dispatch: running,
                newly_started: true,
            },
        )
        .unwrap()
        .unwrap();
    ledger
        .reserve(&scope, "original-files-call", &dispatch.id, 10, 101)
        .unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let sender = Sender::open(&workspace).unwrap();
    let first = {
        let (name, bytes) = ("first.txt", &b"hello"[..]);
        std::fs::write(workspace.join(name), bytes).unwrap();
        let c = sender
            .capture(
                &ledger,
                &scope,
                &dispatch.id,
                &prepared.execution_id,
                name,
                None,
                None,
                "thread",
                "turn",
                name,
                101,
            )
            .unwrap();
        let p = c.proposal();
        let audit = ledger.begin_approval_audit(OWNER, p, 101).unwrap();
        ledger
            .record_approval_event(OWNER, &audit, Phase::OwnerAllowed, 101, &json!({}))
            .unwrap();
        ledger.approve_tool(OWNER, p, 101).unwrap();
        ledger
            .record_approval_event(OWNER, &audit, Phase::Applying, 101, &json!({}))
            .unwrap();
        let custody = sender.begin(&mut ledger, c, &audit, 101).unwrap();
        custody.record().clone()
    };
    ledger
        .finish_local_execution(
            OWNER,
            &dispatch.id,
            &prepared.execution_id,
            inbox::Finish::Unknown,
        )
        .unwrap();
    let remote = hagency_agent_local::file_delivery::Delivery {
        id: "efile_original".into(),
        dispatch_id: dispatch.id.clone(),
        execution_id: prepared.execution_id.clone(),
        request_id: first.request_id.clone(),
        created_at_ms: 1,
        manifest: first.manifest.clone(),
        manifest_digest: first.manifest.digest().unwrap(),
        received_bytes: first.manifest.bytes,
        state: "unknown".into(),
        upload_accepted: true,
        matrix_event_id: None,
    };
    ledger
        .observe_sent_file(
            &scope,
            &dispatch.id,
            &prepared.execution_id,
            &first.id,
            remote,
        )
        .unwrap();
    drop(ledger);
    let command = || Command::AgentHistory {
        action: HistoryAction::RefreshFile {
            agent: "agent-a".into(),
            id: first.id.clone(),
        },
    };
    // No live service or file-history capability: only original encrypted status is needed.
    let missing = native.execute(command()).await.unwrap();
    assert!(missing["file"]["record"]["absence"].is_null());
    assert_eq!(missing["file"]["record"]["remote"]["state"], "unknown");
    state.lock().unwrap().encrypted_publication_response = Some(
        json!({"dispatchId":dispatch.id,"executionId":prepared.execution_id,"requestId":first.request_id,"kind":"file","state":"possible","matrixEventId":null}),
    );
    let possible = native.execute(command()).await.unwrap();
    assert_eq!(possible["file"]["record"]["remote"]["state"], "unknown");
    assert!(possible["file"]["record"]["absence"].is_null());
    let publication = json!({"dispatchId":dispatch.id,"executionId":prepared.execution_id,"requestId":first.request_id,"kind":"file","state":"sent","matrixEventId":"$lateEncryptedReceipt"});
    for (field, value) in [
        ("dispatchId", json!("foreign")),
        ("executionId", json!("foreign")),
        ("requestId", json!("foreign")),
        ("kind", json!("reply")),
        ("state", json!("invalid")),
        ("matrixEventId", Value::Null),
    ] {
        let mut changed = publication.clone();
        changed[field] = value;
        state.lock().unwrap().encrypted_publication_response = Some(changed);
        assert!(native.execute(command()).await.is_err(), "{field}");
    }
    state.lock().unwrap().encrypted_publication_response = Some(publication.clone());
    // Losing current SDK authority during the read must not commit the late receipt.
    state.lock().unwrap().file_lookup_deny = Some(source.denied.clone());
    assert!(native.execute(command()).await.is_err());
    let ledger = Ledger::open_scoped(&ledger_path, OWNER, &profile).unwrap();
    let retained = ledger
        .sent_file_record(OWNER, "agent-a", &first.id)
        .unwrap();
    assert!(retained.record.absence.is_none());
    assert_eq!(retained.record.remote.unwrap().state, "unknown");
    drop(ledger);
    source
        .denied
        .store(false, std::sync::atomic::Ordering::SeqCst);
    state.lock().unwrap().file_lookup_deny = None;
    // Account loss retires the original owner. Reopen explicitly; never revive it.
    drop(native);
    let native = NativeOwner::open_with_matrix_and_credentials(
        &temp.path().join("state"),
        &origin,
        OWNER,
        source.clone(),
        crate::console::server_login::credential_tests::vault(),
    )
    .await
    .unwrap();
    let sent = native.execute(command()).await.unwrap();
    assert_eq!(sent["file"]["record"]["remote"]["state"], "sent");
    assert_eq!(
        sent["file"]["record"]["remote"]["matrixEventId"],
        "$lateEncryptedReceipt"
    );
    assert_eq!(sent["runtimeActive"], false);
    let ledger = Ledger::open_scoped(&ledger_path, OWNER, &profile).unwrap();
    let original = ledger
        .sent_file_record(OWNER, "agent-a", &first.id)
        .unwrap();
    assert!(original.record.absence.is_none());
    assert_eq!(
        original.record.remote.unwrap().matrix_event_id.as_deref(),
        Some("$lateEncryptedReceipt")
    );
    assert_eq!(
        ledger.approval_phase(OWNER, &first.audit).unwrap(),
        Phase::Unknown
    );
    let wire = state.lock().unwrap();
    let input = wire.file_delivery_input.as_ref().unwrap();
    assert_eq!(
        input.as_object().unwrap().len(),
        6,
        "status carries identities only"
    );
    assert_eq!(input["lease"]["epoch"], 1);
    assert_eq!(input["dispatchId"], dispatch.id);
    assert_eq!(input["executionId"], prepared.execution_id);
    assert_eq!(input["requestId"], first.request_id);
    assert!(
        !wire.paths.iter().any(|p| p.contains("/files/")
            || p.ends_with("/acquire")
            || p.ends_with("/events/start"))
    );
    server.abort();
}
