//! Actual NativeOwner/TCP/SQLite/live PendingApproval tests. The Matrix source
//! is an explicit host double; actual cryptographic qualification is separate.
use super::*;
use crate::console::native::{Command, LocalAction};
use hagency_agent_local::{
    Budget, Layer, Limit, Period, Policy, RequestPolicy, ToolPolicy, audit::Phase,
    remote_approval as remote,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[derive(Default)]
struct RemoteSource {
    request: std::sync::Mutex<Option<remote::Request>>,
    verdict_calls: AtomicUsize,
    verdict_hold: AtomicBool,
    verdict_unknown: AtomicBool,
    calls: AtomicUsize,
    hold: AtomicBool,
    unknown: AtomicBool,
    wrong: AtomicBool,
    hold_before_write: AtomicBool,
    release: tokio::sync::Notify,
}
impl MatrixTokenSource for RemoteSource {
    fn list_remote_requests<'a>(
        &'a self,
        _room: &'a str,
        _cursor: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<remote::review::RequestPage, NativeError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            Ok(remote::review::RequestPage {
                requests: vec![self.reviewed()],
                next_batch: None,
            })
        })
    }
    fn inspect_remote_request<'a>(
        &'a self,
        _room: &'a str,
        _event: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<remote::review::ObservedRequest, NativeError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move { Ok(self.reviewed()) })
    }
    fn publish_remote_verdict(
        &self,
        permit: remote::review::VerdictSendPermit,
        live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + '_>>
    {
        Box::pin(async move {
            assert!(live());
            assert_eq!(permit.record().reviewer_matrix_device, "SDK_REMOTE");
            self.verdict_calls.fetch_add(1, Ordering::AcqRel);
            if self.verdict_hold.load(Ordering::Acquire) {
                self.release.notified().await;
            }
            if self.verdict_unknown.load(Ordering::Acquire) {
                return Err(NativeError {
                    status: 503,
                    code: "fixture_unknown".into(),
                });
            }
            Ok("$remote-verdict".into())
        })
    }
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Source.access_token()
    }
    fn supports_remote_approvals(&self) -> bool {
        true
    }
    fn prepare_remote_approval<'a>(
        &'a self,
        _room: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + 'a>>
    {
        Box::pin(async { Ok("SDK_HOST".into()) })
    }
    fn publish_remote_approval(
        &self,
        permit: remote::SendPermit,
        live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + '_>>
    {
        Box::pin(async move {
            assert!(live());
            assert_eq!(permit.request().owner, OWNER);
            *self.request.lock().unwrap() = Some(permit.request().clone());
            assert!(
                permit
                    .transaction_id()
                    .starts_with("hagency.remote-approval.")
            );
            self.calls.fetch_add(1, Ordering::AcqRel);
            if self.hold.load(Ordering::Acquire) {
                self.release.notified().await;
            }
            if self.hold_before_write.load(Ordering::Acquire) && !live() {
                return Err(NativeError {
                    status: 503,
                    code: "fixture_pending_closed".into(),
                });
            }
            if self.unknown.load(Ordering::Acquire) {
                Err(NativeError {
                    status: 503,
                    code: "fixture_lost_acceptance".into(),
                })
            } else {
                Ok("$original-approval:test".into())
            }
        })
    }
    fn find_remote_approval<'a>(
        &'a self,
        publication: &'a remote::Publication,
        request_event: &'a str,
        live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<remote::ObservedVerdict, NativeError>>
                + Send
                + 'a,
        >,
    > {
        self.observe_remote_approval(publication, request_event, "$allow", live)
    }
    fn observe_remote_approval<'a>(
        &'a self,
        publication: &'a remote::Publication,
        request_event: &'a str,
        verdict: &'a str,
        live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<remote::ObservedVerdict, NativeError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            assert!(live());
            Ok(remote::ObservedVerdict {
                observed_at: now(),
                sender: OWNER.into(),
                sender_device: "SDK_REMOTE".into(),
                sender_ed25519: "A".repeat(43),
                room: publication.request().approval_room.clone(),
                event_id: verdict.into(),
                request_event_id: request_event.into(),
                verdict: remote::Verdict {
                    approval_id: publication.request().approval_id.clone(),
                    request_digest: if self.wrong.load(Ordering::Acquire) {
                        "0".repeat(64)
                    } else {
                        publication.request_digest().into()
                    },
                    approved: verdict != "$deny",
                },
            })
        })
    }
}
impl RemoteSource {
    fn reviewed(&self) -> remote::review::ObservedRequest {
        remote::review::ObservedRequest {
            observed_at: now(),
            event_id: "$original-approval:test".into(),
            sender_ed25519: "A".repeat(43),
            reviewer_matrix_device: "SDK_REMOTE".into(),
            request: self.request.lock().unwrap().clone().unwrap(),
        }
    }
}
struct Gate(Arc<AtomicBool>);
impl codex::ExecutionGate for Gate {
    fn authorize<'a>(
        &'a self,
        _dispatch: &'a str,
        _execution: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async { self.0.load(Ordering::Acquire) })
    }
}
struct Fixture {
    _temp: tempfile::TempDir,
    native: NativeOwner,
    source: Arc<RemoteSource>,
    server: tokio::task::JoinHandle<()>,
    ledger: Ledger,
    view: ApprovalView,
    decision: tokio::sync::oneshot::Receiver<bool>,
    gate: Arc<AtomicBool>,
}
impl Fixture {
    async fn new() -> Self {
        let (origin, state, server) = server(false, 0).await;
        state.lock().unwrap().restore_scope = true;
        let temp = tempfile::tempdir().unwrap();
        let source = Arc::new(RemoteSource::default());
        let native = NativeOwner::open_with_matrix(
            &temp.path().join("state"),
            &origin,
            OWNER,
            source.clone(),
        )
        .await
        .unwrap();
        let console = native.test_console();
        let device = console.authorized_device().await.unwrap();
        let (path, profile) = intent_location(console, &device).unwrap();
        let mut ledger =
            Ledger::open_scoped(path.with_file_name("agent-local.sqlite"), OWNER, &profile)
                .unwrap();
        let dispatch: inbox::Dispatch =
            serde_json::from_value(event("remote-original", 1)).unwrap();
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
            .receive_dispatch(OWNER, &dispatch, inbox::Limits::default(), now())
            .unwrap();
        ledger.acknowledge_dispatch(OWNER, &dispatch.id).unwrap();
        let prepared = ledger.prepare_execution(OWNER, &dispatch.id).unwrap();
        let mut running = dispatch;
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
        let proposal = hagency_agent_local::ToolProposal {
            scope,
            dispatch: "remote-original".into(),
            tool: "codex.command".into(),
            arguments: json!({"command":"echo original","itemId":"original"}),
            canonical_directory: "/workspace".into(),
            risk: "high".into(),
            policy_revision: [1, 1, 1],
            expires: now() + 120,
        };
        let (pending, decision) = codex::pending_approval(proposal);
        let (cancel, mut cancelled) = watch::channel(false);
        let (_status, status) = watch::channel(RuntimeStatus::new("agent-a", "running", None));
        console.0.owned_runtime.entries.lock().await.insert(
            identity(&device, "agent-a"),
            Entry {
                commands: mpsc::unbounded_channel().0,
                bindings: BTreeMap::from([(
                    "binding-a".into(),
                    BindingHandle {
                        cancel: watch::channel(false).0,
                        status: status.clone(),
                    },
                )]),
                cancel,
                status,
                task: tokio::spawn(async move {
                    canceled(&mut cancelled).await;
                }),
                device: DevicePin::from(&device),
                approvals: BTreeMap::new(),
            },
        );
        let gate = Arc::new(AtomicBool::new(true));
        let turn_cancel = {
            let entries = console.0.owned_runtime.entries.lock().await;
            entries
                .get(&identity(&device, "agent-a"))
                .unwrap()
                .cancel
                .subscribe()
        };
        console
            .0
            .owned_runtime
            .offer_approval(
                console,
                &DevicePin::from(&device),
                pending,
                Arc::new(Gate(gate.clone())),
                turn_cancel,
            )
            .await
            .unwrap();
        let view = console
            .0
            .owned_runtime
            .pending(console, "agent-a")
            .await
            .unwrap()
            .pop()
            .unwrap();
        Self {
            _temp: temp,
            native,
            source,
            server,
            ledger,
            view,
            decision,
            gate,
        }
    }
    async fn command(
        &self,
        action: LocalAction,
        input: Option<Value>,
    ) -> Result<Value, NativeError> {
        self.native
            .execute(Command::Local {
                agent_id: "agent-a".into(),
                binding_id: Some("binding-a".into()),
                requester: Some("@bob:test".into()),
                action,
                input,
            })
            .await
    }
    async fn publish(&self, room: &str) -> Result<Value, NativeError> {
        self.command(
            LocalAction::PublishRemoteApproval {
                approval_id: self.view.proposal_id.clone(),
            },
            Some(json!({"argsDigest":self.view.args_digest,"approvalRoom":room})),
        )
        .await
    }
    async fn observe(&self, event: &str) -> Result<Value, NativeError> {
        self.command(
            LocalAction::ObserveRemoteApproval {
                approval_id: self.view.proposal_id.clone(),
            },
            Some(json!({"argsDigest":self.view.args_digest,"verdictEventId":event})),
        )
        .await
    }
    async fn status(&self) -> Value {
        self.command(
            LocalAction::RemoteApprovalStatus {
                approval_id: self.view.proposal_id.clone(),
            },
            None,
        )
        .await
        .unwrap()
    }
    async fn settled(&self, phase: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let v = self.status().await;
                if v["remoteApproval"]["phase"] == phase {
                    return v;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
    async fn close(self) {
        self.native.shutdown().await;
        self.server.abort();
    }
}
#[tokio::test]
async fn native_remote_approval_records_original_verdict_and_signals_only_the_live_pending_once() {
    let mut f = Fixture::new().await;
    f.publish("!private:test").await.unwrap();
    f.settled("sent").await;
    assert!(matches!(
        f.decision.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    f.source.wrong.store(true, Ordering::Release);
    assert!(f.observe("$wrong").await.is_err());
    assert_eq!(
        f.ledger.approval_phase(OWNER, &f.view.proposal_id).unwrap(),
        Phase::Offered
    );
    assert!(matches!(
        f.decision.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    f.source.wrong.store(false, Ordering::Release);
    assert_eq!(
        f.command(
            LocalAction::ObserveRemoteApproval {
                approval_id: f.view.proposal_id.clone()
            },
            Some(json!({"argsDigest":f.view.args_digest}))
        )
        .await
        .unwrap()["decided"],
        true
    );
    assert!(f.decision.try_recv().unwrap());
    assert_eq!(
        f.ledger.approval_phase(OWNER, &f.view.proposal_id).unwrap(),
        Phase::OwnerAllowed
    );
    assert!(
        f.ledger.authorize_tool(&f.view.proposal, now()).is_err(),
        "remote evidence does not itself mint a reusable tool grant"
    );
    assert!(f.observe("$allow").await.is_err());
    assert!(f.publish("!private:test").await.is_err());
    assert_eq!(f.source.calls.load(Ordering::Acquire), 1);
    assert_eq!(f.status().await["remoteApproval"]["phase"], "approved");
    f.close().await;
}
#[tokio::test]
async fn native_remote_approval_late_receipt_survives_local_denial_and_joined_shutdown() {
    let mut f = Fixture::new().await;
    f.source.hold.store(true, Ordering::Release);
    f.publish("!private:test").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.source.calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.command(
        LocalAction::Decision {
            approval_id: f.view.proposal_id.clone(),
        },
        Some(json!({"argsDigest":f.view.args_digest,"approved":false})),
    )
    .await
    .unwrap();
    assert!(!f.decision.try_recv().unwrap());
    let closing = f.native.clone();
    let shutdown = tokio::spawn(async move {
        closing.shutdown().await;
    });
    tokio::task::yield_now().await;
    assert!(
        !shutdown.is_finished(),
        "original possible SDK IO must remain owned during shutdown"
    );
    f.source.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap();
    let record = f
        .ledger
        .remote_approval_record(OWNER, "agent-a", &f.view.proposal_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        record.request_event_id.as_deref(),
        Some("$original-approval:test")
    );
    assert!(record.decision.is_none());
    assert_eq!(
        f.ledger.approval_phase(OWNER, &f.view.proposal_id).unwrap(),
        Phase::OwnerDenied
    );
    assert_eq!(f.source.calls.load(Ordering::Acquire), 1);
    f.close().await;
}
#[tokio::test]
async fn native_remote_approval_unknown_never_republishes_or_changes_destination() {
    let f = Fixture::new().await;
    f.source.unknown.store(true, Ordering::Release);
    f.publish("!private:test").await.unwrap();
    f.settled("unknown").await;
    for _ in 0..3 {
        assert_eq!(
            f.publish("!private:test").await.unwrap()["remoteApproval"]["phase"],
            "unknown"
        );
    }
    assert!(f.publish("!different:test").await.is_err());
    assert!(f.observe("$allow").await.is_err());
    assert_eq!(f.source.calls.load(Ordering::Acquire), 1);
    assert_eq!(
        f.ledger.approval_phase(OWNER, &f.view.proposal_id).unwrap(),
        Phase::Offered
    );
    f.close().await;
}
#[tokio::test]
async fn native_remote_approval_current_gate_or_policy_change_cannot_decide_the_pending_request() {
    let mut f = Fixture::new().await;
    f.publish("!private:test").await.unwrap();
    f.settled("sent").await;
    f.gate.store(false, Ordering::Release);
    assert!(f.observe("$allow").await.is_err());
    f.gate.store(true, Ordering::Release);
    f.ledger
        .set_policy(
            OWNER,
            &f.view.proposal.scope,
            Layer::Room,
            1,
            &Policy {
                budget: Budget {
                    limit: Limit::Unlimited,
                    period: Period::Lifetime,
                },
                requests: RequestPolicy::Allow,
                high_risk: ToolPolicy::Deny,
            },
        )
        .unwrap();
    assert!(f.observe("$allow").await.is_err());
    assert!(matches!(
        f.decision.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(
        f.ledger.approval_phase(OWNER, &f.view.proposal_id).unwrap(),
        Phase::Offered
    );
    assert!(
        f.ledger
            .remote_approval_record(OWNER, "agent-a", &f.view.proposal_id)
            .unwrap()
            .unwrap()
            .decision
            .is_none()
    );
    f.close().await;
}

#[tokio::test]
async fn native_remote_approval_denial_closes_only_original_pending_and_gate_precedes_reservation()
{
    let mut f = Fixture::new().await;
    f.gate.store(false, Ordering::Release);
    assert!(f.publish("!private:test").await.is_err());
    assert_eq!(f.source.calls.load(Ordering::Acquire), 0);
    assert!(
        f.ledger
            .remote_approval_record(OWNER, "agent-a", &f.view.proposal_id)
            .unwrap()
            .is_none()
    );
    f.gate.store(true, Ordering::Release);
    f.publish("!private:test").await.unwrap();
    f.settled("sent").await;
    f.observe("$deny").await.unwrap();
    assert!(!f.decision.try_recv().unwrap());
    assert_eq!(
        f.ledger.approval_phase(OWNER, &f.view.proposal_id).unwrap(),
        Phase::OwnerDenied
    );
    assert_eq!(f.status().await["remoteApproval"]["phase"], "denied");
    assert!(f.observe("$allow").await.is_err());
    assert_eq!(f.source.calls.load(Ordering::Acquire), 1);
    f.close().await;
}

#[tokio::test]
async fn native_remote_approval_closed_receiver_retires_inflight_publication_before_next_write() {
    let mut f = Fixture::new().await;
    f.source.hold.store(true, Ordering::Release);
    f.source.hold_before_write.store(true, Ordering::Release);
    f.publish("!private:test").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.source.calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(std::mem::replace(
        &mut f.decision,
        tokio::sync::oneshot::channel().1,
    ));
    f.source.release.notify_one();
    f.settled("unknown").await;
    assert!(
        f.ledger
            .remote_approval_record(OWNER, "agent-a", &f.view.proposal_id)
            .unwrap()
            .unwrap()
            .request_event_id
            .is_none()
    );
    assert!(f.publish("!private:test").await.is_err());
    assert_eq!(f.source.calls.load(Ordering::Acquire), 1);
    f.close().await;
}

#[tokio::test]
async fn native_remote_reviewer_exact_request_and_durable_verdict_do_not_signal_local_pending() {
    use crate::console::native::RemoteApprovalAction as Action;
    let mut f = Fixture::new().await;
    f.publish("!private:test").await.unwrap();
    f.settled("sent").await;
    let page = f
        .native
        .execute(Command::RemoteApprovals {
            action: Action::List {
                agent: "agent-a".into(),
                room: "!private:test".into(),
                cursor: None,
            },
        })
        .await
        .unwrap();
    let row = &page["requests"][0];
    assert_eq!(row["request"]["approvalId"], f.view.proposal_id);
    let command = |approved, digest: String| Command::RemoteApprovals {
        action: Action::Decide {
            agent: "agent-a".into(),
            room: "!private:test".into(),
            event_id: "$original-approval:test".into(),
            request_digest: digest,
            approved,
        },
    };
    assert!(
        f.native
            .execute(command(true, "0".repeat(64)))
            .await
            .is_err()
    );
    assert_eq!(f.source.verdict_calls.load(Ordering::Acquire), 0);
    let digest = row["requestDigest"].as_str().unwrap().to_owned();
    f.native
        .execute(command(true, digest.clone()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = f
                .native
                .execute(Command::RemoteApprovals {
                    action: Action::Status {
                        approval_id: f.view.proposal_id.clone(),
                    },
                })
                .await
                .unwrap();
            if status["publication"]["phase"] == "sent" {
                assert_eq!(status["publication"]["eventId"], "$remote-verdict");
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(f.native.execute(command(false, digest)).await.is_err());
    assert_eq!(f.source.verdict_calls.load(Ordering::Acquire), 1);
    assert!(matches!(
        f.decision.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    f.close().await;
}

#[tokio::test]
async fn native_remote_reviewer_unknown_is_not_republished_and_late_receipt_is_joined_on_shutdown()
{
    use crate::console::native::RemoteApprovalAction as Action;
    for lost in [false, true] {
        let f = Fixture::new().await;
        f.publish("!private:test").await.unwrap();
        f.settled("sent").await;
        f.source.verdict_hold.store(true, Ordering::Release);
        f.source.verdict_unknown.store(lost, Ordering::Release);
        let digest = f.source.reviewed().request.digest().unwrap();
        f.native
            .execute(Command::RemoteApprovals {
                action: Action::Decide {
                    agent: "agent-a".into(),
                    room: "!private:test".into(),
                    event_id: "$original-approval:test".into(),
                    request_digest: digest.clone(),
                    approved: false,
                },
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while f.source.verdict_calls.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            f.native
                .execute(Command::RemoteApprovals {
                    action: Action::Decide {
                        agent: "agent-a".into(),
                        room: "!private:test".into(),
                        event_id: "$original-approval:test".into(),
                        request_digest: digest,
                        approved: true
                    }
                })
                .await
                .is_err()
        );
        let native = f.native.clone();
        let stopping = tokio::spawn(async move { native.shutdown_checked().await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while f.native.test_console().authorized_device().await.is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!stopping.is_finished());
        f.source.release.notify_one();
        stopping.await.unwrap().unwrap();
        let r = f
            .ledger
            .remote_verdict_record(OWNER, &f.view.proposal_id)
            .unwrap()
            .unwrap();
        assert!(!r.approved);
        assert_eq!(r.event_id.is_none(), lost);
        assert_eq!(f.source.verdict_calls.load(Ordering::Acquire), 1);
        f.server.abort();
        let _ = f.server.await;
    }
}
