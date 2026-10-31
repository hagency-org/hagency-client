//! Real original/successor drivers, SQLite and child process; only Matrix I/O
//! uses a local test adapter. Neither model input nor answer may reach the API.
use super::*;

const PROMPT: &str = "Private encrypted request for the new Agent";

async fn setup(successor: bool) -> (Fixture, SharedExecutionTransport, SharedRuntimeCommand) {
    let (f, t, c) = if successor {
        let (f, t, c) = Box::pin(attempts::predecessor()).await;
        attempts::prepare_lost_start(&f, &t, &c).await;
        (f, t, c)
    } else {
        fixture().await
    };
    {
        let mut s = f.state.lock().unwrap();
        let w = s.shared.work.as_mut().unwrap();
        w.encrypted = true;
        w.body = w.event_id.clone();
        w.body_digest = format!("{:x}", Sha256::digest(w.body.as_bytes()));
    }
    (f, t, c)
}

async fn run(f: &Fixture, t: &SharedExecutionTransport, successor: bool) {
    let progress = if successor {
        t.poll_successor_fixture(&f.registry, launch(f, "text-usage")).await
    } else {
        t.poll_fixture(&f.registry, launch(f, "text-usage")).await
    }.unwrap();
    assert!(matches!(progress, SharedExecutionProgress::Running {..}));
    if successor { attempts::wait_stopped(f).await; } else { stopped(f).await; }
}

#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_shared_encrypted_original_and_successor_keep_plaintext_local() {
    for successor in [false, true] {
        let (f, t, c) = Box::pin(setup(successor)).await;
        let (t, sent) = t.with_crypto_fixture(Some(PROMPT.into()));
        f.state.lock().unwrap().shared.lose_completed_work_result = true;
        run(&f, &t, successor).await;
        let receipt = f.state.lock().unwrap().shared.receipts.values()
            .find(|r| r.state == "completed").expect("encrypted request completes").clone();
        let sends = sent.lock().unwrap().clone();
        assert_eq!(sends.len(), 1, "lost result acknowledgement cannot duplicate the Matrix send");
        let wire: Value = serde_json::from_str(receipt.body.as_ref().unwrap()).unwrap();
        assert_eq!(wire["algorithm"], "m.megolm.v1.aes-sha2");
        assert_eq!(wire["delivery"], "sent");
        assert!(!receipt.body.as_ref().unwrap().contains(&sends[0].1));
        let db = rusqlite::Connection::open(f._root.path().join("registry/domain.sqlite3")).unwrap();
        let (prompt, body): (String, String) = db.query_row(
            "SELECT prompt,body FROM owner_shared_decrypted_inputs", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(prompt, PROMPT);
        assert_eq!(body, PROMPT);
        let reply: String = db.query_row("SELECT payload FROM owner_shared_reply_deliveries", [], |r| r.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&reply).unwrap()["body"], sends[0].1);
        assert_eq!(rows(&f, "owner_managed_budget_deltas"), 0, "encryption needs no top-up");
        assert_eq!(f.state.lock().unwrap().shared.starts, 1);
        let methods = std::fs::read_to_string(f._root.path().join("shared-probe/methods")).unwrap();
        assert_eq!(methods.lines().filter(|l| *l == "turn/start").count(), 1);
        // Historical upload uses the same private delivery receipt, without a second turn/send.
        if successor {
            let attempt = f.state.lock().unwrap().shared.attempts.command.clone().unwrap();
            t.recover_successor(&f.registry, attempt.command_id.clone(), digest(&attempt).unwrap(), None).await.unwrap();
        } else {
            t.recover_work(&f.registry, c.command_id.clone(), digest(&c).unwrap(), receipt.work_id.clone(), Some(receipt.observation_id.clone())).await.unwrap();
        }
        assert_eq!(sent.lock().unwrap().len(), 1);
        f.stop().await;
    }
}

#[tokio::test]
#[ignore = "requires explicit offline supervised runtime probe"]
async fn native_shared_encrypted_missing_keys_never_start_model() {
    for successor in [false, true] {
        let (f, t, _) = Box::pin(setup(successor)).await;
        let (t, sent) = t.with_crypto_fixture(None);
        run(&f, &t, successor).await;
        assert_eq!(f.state.lock().unwrap().shared.starts, 0);
        assert_eq!(rows(&f, "owner_managed_calls"), 0);
        assert_eq!(rows(&f, "owner_shared_decrypted_inputs"), 0);
        assert!(sent.lock().unwrap().is_empty());
        let methods = std::fs::read_to_string(f._root.path().join("shared-probe/methods")).unwrap();
        assert!(!methods.contains("turn/start"));
        f.stop().await;
    }
}
