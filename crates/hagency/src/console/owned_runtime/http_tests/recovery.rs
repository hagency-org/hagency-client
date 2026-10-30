use super::*;
use crate::console::native::{
    Command, ContinuationMessage, HistoryAction, RecoveryDisposition, RecoveryInput,
};
use std::sync::atomic::Ordering;
#[derive(Default)]
struct Capture {
    messages: StdMutex<Vec<ContinuationMessage>>,
    unknown: std::sync::atomic::AtomicBool,
}
impl MatrixTokenSource for Capture {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Source.access_token()
    }
    fn send_continuation(
        &self,
        message: ContinuationMessage,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + '_>>
    {
        Box::pin(async move {
            self.messages.lock().unwrap().push(message);
            if self.unknown.load(Ordering::SeqCst) {
                Err(NativeError {
                    status: 503,
                    code: "fixture_lost_acceptance".into(),
                })
            } else {
                Ok("$new-owner-request:test".into())
            }
        })
    }
}
#[tokio::test]
async fn native_owner_stopped_recovery_preserves_scope_and_never_rearms_lost_continuation() {
    let (origin, state, server) = server(false, 0).await;
    state.lock().unwrap().restore_scope = true;
    let temp = tempfile::tempdir().unwrap();
    let source = Arc::new(Capture::default());
    source.unknown.store(true, Ordering::SeqCst);
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
    let mut ledger =
        Ledger::open_scoped(path.with_file_name("agent-local.sqlite"), OWNER, &profile).unwrap();
    let dispatch: inbox::Dispatch = serde_json::from_value(event("original-recovery", 1)).unwrap();
    let scope = dispatch.scope();
    ledger.register_binding(OWNER, &scope).unwrap();
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
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("answer.txt"), b"original partial effect").unwrap();
    ledger
        .record_execution_workspace(
            OWNER,
            &dispatch.id,
            &prepared.execution_id,
            &workspace,
            false,
            101,
        )
        .unwrap();
    ledger
        .record_execution_stop(OWNER, &dispatch.id, &prepared.execution_id, "leader", 102)
        .unwrap();
    ledger
        .finish_local_execution(
            OWNER,
            &dispatch.id,
            &prepared.execution_id,
            inbox::Finish::Unknown,
        )
        .unwrap();
    let stopped = native
        .execute(Command::AgentHistory {
            action: HistoryAction::Stopped {
                agent: "agent-a".into(),
                cursor: None,
            },
        })
        .await
        .unwrap();
    assert_eq!(stopped["stopped"]["records"][0]["id"], dispatch.id);
    let review = native
        .execute(Command::AgentHistory {
            action: HistoryAction::InspectStopped {
                agent: "agent-a".into(),
                dispatch: dispatch.id.clone(),
                paths: vec!["answer.txt".into()],
            },
        })
        .await
        .unwrap();
    assert_eq!(
        review["review"]["evidence"]["input"],
        "isolated fixture request"
    );
    assert_eq!(review["review"]["evidence"]["files"][0]["status"], "hashed");
    let input = RecoveryInput {
        review_id: review["review"]["id"].as_str().unwrap().into(),
        review_digest: review["review"]["digest"].as_str().unwrap().into(),
        disposition: RecoveryDisposition::Continue {
            note: "Verified partial file; original outcome unknown".into(),
            instruction: "Inspect existing effects and finish only the remaining work.".into(),
        },
    };
    let command = Command::AgentHistory {
        action: HistoryAction::DecideStopped {
            agent: "agent-a".into(),
            id: "original-action".into(),
            input: input.clone(),
        },
    };
    let decision = native.execute(command.clone()).await.unwrap();
    assert_eq!(decision["decision"]["phase"], "unknown");
    assert_eq!(
        native.execute(command).await.unwrap()["decision"],
        decision["decision"]
    );
    assert_eq!(source.messages.lock().unwrap().len(), 1);
    assert!(
        native
            .execute(Command::AgentHistory {
                action: HistoryAction::DecideStopped {
                    agent: "agent-a".into(),
                    id: "new-id-same-review".into(),
                    input
                }
            })
            .await
            .is_err()
    );
    assert_eq!(source.messages.lock().unwrap().len(), 1);
    {
        let messages = source.messages.lock().unwrap();
        let m = &messages[0];
        assert_eq!(m.owner, OWNER);
        assert_eq!(m.room, "!room-a:test");
        assert_eq!(m.thread, "$root:test");
        assert_eq!(m.agent_mxid, "@agent:test");
        assert!(
            m.body
                .starts_with("@agent:test\n\nExplicit owner continuation")
        );
        assert!(!m.body.contains("isolated fixture request"));
        assert!(m.require_encrypted);
        assert!(m.transaction_id.starts_with("hagency-continue-"));
    }
    let fresh = native
        .execute(Command::AgentHistory {
            action: HistoryAction::InspectStopped {
                agent: "agent-a".into(),
                dispatch: dispatch.id.clone(),
                paths: vec![],
            },
        })
        .await
        .unwrap();
    assert_eq!(fresh["review"]["actions"][0]["phase"], "unknown");
    let confirm = RecoveryInput {
        review_id: fresh["review"]["id"].as_str().unwrap().into(),
        review_digest: fresh["review"]["digest"].as_str().unwrap().into(),
        disposition: RecoveryDisposition::OwnerConfirmed {
            note: "Manually checked the original Room".into(),
        },
    };
    let confirmed = native
        .execute(Command::AgentHistory {
            action: HistoryAction::DecideStopped {
                agent: "agent-a".into(),
                id: "owner-confirmed".into(),
                input: confirm,
            },
        })
        .await
        .unwrap();
    assert_eq!(confirmed["decision"]["phase"], "recorded");
    assert_eq!(source.messages.lock().unwrap().len(), 1);
    assert_eq!(
        ledger.inbox_record(OWNER, &dispatch.id).unwrap().state,
        inbox::State::Unknown
    );
    assert!(
        native
            .execute(Command::AgentHistory {
                action: HistoryAction::InspectStopped {
                    agent: "other-agent".into(),
                    dispatch: dispatch.id.clone(),
                    paths: vec![]
                }
            })
            .await
            .is_err()
    );
    native.shutdown().await;
    server.abort();
}
