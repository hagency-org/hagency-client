use super::*;
use crate::sent_files::tests::{LOCK, OWNER, ledger};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
struct Host {
    live: Arc<AtomicBool>,
    prepares: AtomicUsize,
    chunks: AtomicUsize,
    queues: AtomicUsize,
    receipt: Mutex<Option<Delivery>>,
    bytes: Mutex<Vec<u8>>,
    lost_queue: bool,
    execution: String,
    deny_in_status: Mutex<Option<(std::path::PathBuf, Scope)>>,
}
impl ExecutionGate for Host {
    fn authorize<'a>(
        &'a self,
        d: &'a str,
        e: &'a str,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            d == "dispatch" && e == self.execution && self.live.load(Ordering::SeqCst)
        })
    }
}
impl FileDeliveryHost for Host {
    fn prepare<'a>(&'a self, request: &'a str, file: &'a Manifest) -> FileFuture<'a, Delivery> {
        Box::pin(async move {
            self.prepares.fetch_add(1, Ordering::SeqCst);
            let r = Delivery {
                id: "fil_original".into(),
                dispatch_id: "dispatch".into(),
                execution_id: self.execution.clone(),
                request_id: request.into(),
                created_at_ms: 1,
                manifest: file.clone(),
                manifest_digest: file.digest().unwrap(),
                received_bytes: 0,
                state: "staging".into(),
                upload_accepted: false,
                matrix_event_id: None,
            };
            *self.receipt.lock().unwrap() = Some(r.clone());
            Ok(r)
        })
    }
    fn chunk<'a>(
        &'a self,
        _: &'a Record,
        offset: u64,
        bytes: &'a [u8],
    ) -> FileFuture<'a, Delivery> {
        Box::pin(async move {
            self.chunks.fetch_add(1, Ordering::SeqCst);
            let mut data = self.bytes.lock().unwrap();
            assert_eq!(offset, data.len() as u64);
            data.extend_from_slice(bytes);
            let mut r = self.receipt.lock().unwrap();
            let r = r.as_mut().unwrap();
            r.received_bytes = data.len() as u64;
            Ok(r.clone())
        })
    }
    fn queue<'a>(&'a self, _: &'a Record) -> FileFuture<'a, Delivery> {
        Box::pin(async move {
            self.queues.fetch_add(1, Ordering::SeqCst);
            let mut r = self.receipt.lock().unwrap();
            let r = r.as_mut().unwrap();
            r.state = if self.lost_queue { "unknown" } else { "sent" }.into();
            r.upload_accepted = true;
            if !self.lost_queue {
                r.matrix_event_id = Some("$actualOpaqueEvent".into());
                Ok(r.clone())
            } else {
                Err(())
            }
        })
    }
    fn status<'a>(&'a self, _: &'a Record) -> FileFuture<'a, Delivery> {
        Box::pin(async move {
            if let Some((path, scope)) = self.deny_in_status.lock().unwrap().take() {
                let mut l = Ledger::open_scoped(path, OWNER, &"a".repeat(64)).unwrap();
                let revision = l.policy_snapshot(&scope).unwrap()[0].revision;
                l.set_policy(
                    OWNER,
                    &scope,
                    crate::Layer::Agent,
                    revision,
                    &crate::Policy {
                        budget: crate::Budget {
                            limit: crate::Limit::Tokens(100),
                            period: crate::Period::Lifetime,
                        },
                        requests: crate::RequestPolicy::Deny,
                        high_risk: ToolPolicy::AskOwner,
                    },
                )
                .unwrap();
            }
            Ok(self.receipt.lock().unwrap().clone().unwrap())
        })
    }
}
async fn response(peer: &mut BufReader<tokio::io::DuplexStream>) -> Value {
    let mut line = String::new();
    peer.read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}
#[tokio::test]
async fn original_frozen_send_approval_denial_source_change_and_lost_ack_never_rearm() {
    let _lock = LOCK.lock().await;
    for (approve, mutate, revoke, lost) in [
        (true, false, false, false),
        (false, false, false, false),
        (true, true, false, false),
        (true, false, true, false),
        (true, false, false, true),
    ] {
        let t = tempfile::tempdir().unwrap();
        let cwd = t.path().canonicalize().unwrap();
        std::fs::write(cwd.join("result.txt"), b"hello").unwrap();
        let (mut l, scope, execution) = ledger(&cwd.join("ledger"));
        let host = Arc::new(Host {
            live: Arc::new(AtomicBool::new(true)),
            prepares: AtomicUsize::new(0),
            chunks: AtomicUsize::new(0),
            queues: AtomicUsize::new(0),
            receipt: Mutex::new(None),
            bytes: Mutex::new(Vec::new()),
            lost_queue: lost,
            execution: execution.clone(),
            deny_in_status: Mutex::new(None),
        });
        let (client, peer) = tokio::io::duplex(65536);
        let (r, w) = tokio::io::split(client);
        let mut session = Session::new(r, w);
        let mut peer = BufReader::new(peer);
        session
            .enable_sent_files(
                Sender::open(&cwd).unwrap(),
                host.clone(),
                "dispatch".into(),
                execution.clone(),
            )
            .unwrap();
        let (queue, mut reviews) = approval_queue(1).unwrap();
        let path = cwd.join("result.txt");
        let live = host.live.clone();
        let decision = tokio::spawn(async move {
            let review = reviews.recv().await.unwrap();
            assert_eq!(review.proposal.tool, "matrix.send_file");
            assert_eq!(review.proposal.arguments["file"]["bytes"], 5);
            assert_eq!(
                review.proposal.arguments["file"]["sha256"],
                "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
            );
            assert_eq!(review.proposal.arguments["path"], "result.txt");
            if mutate {
                std::fs::write(path, b"changed").unwrap();
            }
            if revoke {
                live.store(false, Ordering::SeqCst);
            }
            review.decide(approve);
        });
        let frame = |id, call: &str, tool: &str, args: Value| value!({"id":id,"params":{"threadId":"thread","turnId":"turn","callId":call,"tool":tool,"arguments":args}});
        session.sent_file_call(&mut l,OWNER,&scope,"dispatch",&execution,&frame(1,"send-call","send_file",value!({"path":"result.txt","filename":"report.txt","caption":"Exact original"})),"thread","turn",&queue).await.unwrap();
        decision.await.unwrap();
        let result = response(&mut peer).await;
        let sent = approve && !mutate && !revoke;
        assert_eq!(host.prepares.load(Ordering::SeqCst), usize::from(sent));
        assert_eq!(result["result"]["success"], sent);
        if !sent {
            session.sent_file_call(&mut l,OWNER,&scope,"dispatch",&execution,&frame(5,"send-call","send_file",value!({"path":"result.txt","filename":"report.txt","caption":"Exact original"})),"thread","turn",&queue).await.unwrap();
            assert_eq!(
                response(&mut peer).await["result"]["success"],
                false,
                "same denied call cannot prompt/apply again"
            );
            assert_eq!(host.prepares.load(Ordering::SeqCst), 0);
            continue;
        }
        assert_eq!(&*host.bytes.lock().unwrap(), b"hello");
        assert_eq!(host.queues.load(Ordering::SeqCst), 1);
        let payload: Value = serde_json::from_str(
            result["result"]["contentItems"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(payload["state"], if lost { "unknown" } else { "sent" });
        let id = payload["delivery_id"].as_str().unwrap();
        let record = l
            .sent_file(&scope, "dispatch", &execution, id)
            .unwrap()
            .unwrap();
        assert_eq!(
            l.approval_phase(OWNER, &record.audit).unwrap(),
            if lost {
                Phase::Unknown
            } else {
                Phase::Completed
            }
        );
        // New framework call ID, same original file: read-only observation, no review or new preparation.
        session.sent_file_call(&mut l,OWNER,&scope,"dispatch",&execution,&frame(2,"different-call","send_file",value!({"path":"result.txt","filename":"report.txt","caption":"Exact original"})),"thread","turn",&queue).await.unwrap();
        let _ = response(&mut peer).await;
        if lost {
            let mut r = host.receipt.lock().unwrap();
            let r = r.as_mut().unwrap();
            r.state = "sent".into();
            r.matrix_event_id = Some("$lateActualEvent".into());
        }
        session
            .sent_file_call(
                &mut l,
                OWNER,
                &scope,
                "dispatch",
                &execution,
                &frame(
                    3,
                    "status-call",
                    "get_file_delivery",
                    value!({"delivery_id":id}),
                ),
                "thread",
                "turn",
                &queue,
            )
            .await
            .unwrap();
        let last = response(&mut peer).await;
        let last: Value =
            serde_json::from_str(last["result"]["contentItems"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(last["state"], "sent");
        assert_eq!(host.prepares.load(Ordering::SeqCst), 1);
        assert_eq!(host.queues.load(Ordering::SeqCst), 1);
        *host.deny_in_status.lock().unwrap() = Some((cwd.join("ledger"), scope.clone()));
        let denied = session
            .sent_file_call(
                &mut l,
                OWNER,
                &scope,
                "dispatch",
                &execution,
                &frame(
                    4,
                    "revoked-status",
                    "get_file_delivery",
                    value!({"delivery_id":id}),
                ),
                "thread",
                "turn",
                &queue,
            )
            .await;
        assert!(
            matches!(denied, Err(Error::Unknown)),
            "policy revoked during read must suppress model receipt"
        );
        assert_eq!(host.prepares.load(Ordering::SeqCst), 1);
        assert_eq!(host.queues.load(Ordering::SeqCst), 1);
        if lost {
            assert_eq!(
                l.approval_phase(OWNER, &record.audit).unwrap(),
                Phase::Unknown,
                "late proof does not rewrite original uncertain audit"
            );
        }
    }
}
