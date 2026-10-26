use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct Host {
    live: Arc<AtomicBool>,
    downloads: Arc<AtomicUsize>,
    revoke_in_download: bool,
    execution: String,
}
impl AttachmentHost for Host {
    fn authorize<'a>(
        &'a self,
        d: &'a str,
        e: &'a str,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            d == "dispatch" && e == self.execution && self.live.load(Ordering::SeqCst)
        })
    }
    fn download<'a>(
        &'a self,
        _: &'a Attachment,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Vec<u8>, ()>> + Send + 'a>> {
        Box::pin(async move {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            if self.revoke_in_download {
                self.live.store(false, Ordering::SeqCst);
            }
            Ok(b"hello".to_vec())
        })
    }
}
const OWNER: &str = "@alice:test";
fn ledger(path: &std::path::Path) -> (Ledger, Scope, String) {
    use crate::{Budget, Layer, Limit, Period, Policy, RequestPolicy, inbox};
    let mut l = Ledger::open(path, OWNER).unwrap();
    let mut event = inbox::Dispatch {
        id: "dispatch".into(),
        binding_id: "binding".into(),
        agent_id: "agent".into(),
        event_id: "$source:test".into(),
        room_id: "!room:test".into(),
        requester_mxid: "@bob:test".into(),
        thread_root: "$root:test".into(),
        body: "Analyze original file".into(),
        state: "offered".into(),
        binding_generation: 1,
        dispatch_epoch: Some(1),
        dispatch_device_id: Some("device".into()),
        execution_id: None,
        outcome: None,
    };
    let scope = event.scope();
    l.register_binding(OWNER, &scope).unwrap();
    for layer in [Layer::Agent, Layer::Room, Layer::Requester] {
        l.set_policy(
            OWNER,
            &scope,
            layer,
            0,
            &Policy {
                budget: Budget {
                    limit: Limit::Tokens(100),
                    period: Period::Lifetime,
                },
                requests: RequestPolicy::Allow,
                high_risk: ToolPolicy::AskOwner,
            },
        )
        .unwrap();
    }
    l.receive_dispatch(OWNER, &event, inbox::Limits::default(), 10)
        .unwrap();
    l.acknowledge_dispatch(OWNER, "dispatch").unwrap();
    let p = l.prepare_execution(OWNER, "dispatch").unwrap();
    event.state = "running".into();
    event.execution_id = Some(p.execution_id.clone());
    l.confirm_execution_start(
        OWNER,
        &inbox::ServerStart {
            dispatch: event,
            newly_started: true,
        },
    )
    .unwrap()
    .unwrap();
    l.reserve(&scope, "model-call", "dispatch", 10, now())
        .unwrap();
    (l, scope, p.execution_id)
}
#[tokio::test]
async fn exact_owner_receive_publishes_real_bytes_and_duplicate_never_downloads_again() {
    for (approve, revoke) in [(true, false), (false, false), (true, true)] {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        let cwd = cwd.canonicalize().unwrap();
        let (mut ledger, scope, execution) = ledger(&temp.path().join("ledger"));
        let downloads = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicBool::new(true));
        let host = Arc::new(Host {
            downloads: downloads.clone(),
            live,
            revoke_in_download: revoke,
            execution: execution.clone(),
        });
        let (client, peer) = tokio::io::duplex(65536);
        let (r, w) = tokio::io::split(client);
        let mut session = Session::new(r, w);
        let mut peer = BufReader::new(peer);
        let source=Attachment::from_content(&value!({"msgtype":"m.file","body":"source.txt","url":"mxc://media.test/secret","info":{"size":5}})).unwrap();
        session
            .enable_received_files(
                Receiver::open(&cwd).unwrap(),
                host,
                "dispatch".into(),
                execution.clone(),
                "$source:test".into(),
                Some(source),
            )
            .unwrap();
        let (queue, mut receiver) = approval_queue(1).unwrap();
        let decision = tokio::spawn(async move {
            let proposed = receiver.recv().await.unwrap();
            assert_eq!(proposed.proposal.tool, "matrix.receive_file");
            assert_eq!(proposed.proposal.arguments["eventId"], "$source:test");
            assert!(
                !serde_json::to_string(&proposed.proposal)
                    .unwrap()
                    .contains("mxc://")
            );
            proposed.decide(approve);
        });
        let request = |id| value!({"id":id,"params":{"threadId":"thread","turnId":"turn","callId":"receive-call","tool":"receive_file","arguments":{"event_id":"$source:test"}}});
        let outcome = session
            .received_file_call(
                &mut ledger,
                OWNER,
                &scope,
                "dispatch",
                &execution,
                &request(1),
                "thread",
                "turn",
                &queue,
            )
            .await;
        decision.await.unwrap();
        if revoke {
            assert!(matches!(outcome, Err(Error::Unknown)));
            assert!(!cwd.join(".hagency-received").exists());
            assert_eq!(downloads.load(Ordering::SeqCst), 1);
            continue;
        }
        outcome.unwrap();
        let mut line = String::new();
        peer.read_line(&mut line).await.unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["result"]["success"], approve && !revoke);
        if approve && !revoke {
            let result: Value = serde_json::from_str(
                response["result"]["contentItems"][0]["text"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(cwd.join(result["path"].as_str().unwrap())).unwrap(),
                b"hello"
            );
            session
                .received_file_call(
                    &mut ledger,
                    OWNER,
                    &scope,
                    "dispatch",
                    &execution,
                    &request(2),
                    "thread",
                    "turn",
                    &queue,
                )
                .await
                .unwrap();
            assert_eq!(downloads.load(Ordering::SeqCst), 1);
            let audit = ledger.approval_history(OWNER, "agent", None).unwrap();
            assert_eq!(audit.records[0].phase, crate::audit::Phase::Completed);
        } else {
            assert!(!cwd.join(".hagency-received").exists());
            assert_eq!(downloads.load(Ordering::SeqCst), usize::from(approve));
        }
    }
}

#[tokio::test]
async fn original_metadata_cache_is_source_free_and_current_request_denial_blocks_it() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().canonicalize().unwrap();
    let (mut ledger, scope, execution) = ledger(&cwd.join("ledger"));
    let host = Arc::new(Host {
        live: Arc::new(AtomicBool::new(true)),
        downloads: Arc::new(AtomicUsize::new(0)),
        revoke_in_download: false,
        execution: execution.clone(),
    });
    let source=Attachment::from_content(&value!({"msgtype":"m.file","body":"untrusted.bin","url":"mxc://media.test/private-source"})).unwrap();
    let (client, peer) = tokio::io::duplex(65536);
    let (r, w) = tokio::io::split(client);
    let mut session = Session::new(r, w);
    let mut peer = BufReader::new(peer);
    session
        .enable_received_files(
            Receiver::open(&cwd).unwrap(),
            host,
            "dispatch".into(),
            execution.clone(),
            "$source:test".into(),
            Some(source),
        )
        .unwrap();
    let (queue, _) = approval_queue(1).unwrap();
    let request = |id| value!({"id":id,"params":{"threadId":"thread","turnId":"turn","callId":"list-call","tool":"list_received_files","arguments":{}}});
    session
        .received_file_call(
            &mut ledger,
            OWNER,
            &scope,
            "dispatch",
            &execution,
            &request(1),
            "thread",
            "turn",
            &queue,
        )
        .await
        .unwrap();
    let mut line = String::new();
    peer.read_line(&mut line).await.unwrap();
    assert!(!line.contains("mxc://"));
    assert!(line.contains("untrusted.bin"));
    let prior = ledger.policy_snapshot(&scope).unwrap()[0].clone();
    let mut policy = prior.policy;
    policy.requests = crate::RequestPolicy::Deny;
    ledger
        .set_policy(OWNER, &scope, crate::Layer::Agent, prior.revision, &policy)
        .unwrap();
    session
        .received_file_call(
            &mut ledger,
            OWNER,
            &scope,
            "dispatch",
            &execution,
            &request(2),
            "thread",
            "turn",
            &queue,
        )
        .await
        .unwrap();
    line.clear();
    peer.read_line(&mut line).await.unwrap();
    assert!(line.contains("local_request_denied"));
    assert!(!line.contains("untrusted.bin"));
}
