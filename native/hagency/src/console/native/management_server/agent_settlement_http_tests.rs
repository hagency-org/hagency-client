//! Authenticated HTTP, exact original human intent and real private SQLite journal.
use super::*;
use crate::native_owner::{ManagementCommand, ManagementPermission as Permission};
#[derive(Default)]
pub(super) struct SettlementState {
    deny: bool,
    lose: bool,
    malformed: bool,
    invalidate: bool,
    writes: usize,
    requests: Vec<Value>,
    receipts: BTreeMap<String, Value>,
    read: Option<Value>,
}
fn progress() -> Value {
    json!({"requestId":"human_request","requestRevision":3,"fenceReceiptDigest":"d".repeat(64),"pendingCommandId":null,"latestReceipt":null,"settlementCurrent":false,"canRequest":true,"blockingReason":null,"knownConsumedTokens":40,"retainedTokensFloor":100})
}
pub(super) fn management(body: &Value, state: &mut State) -> Option<Value> {
    let s = &mut state.human_settlement;
    if s.deny {
        return Some(json!({"fixtureStatus":404,"error":"agent_request_not_found"}));
    }
    if body.is_null() {
        return Some(
            json!({"settlement":s.read.clone().unwrap_or_else(progress),"observedAtMs":now(),"token":"PRIVATE"}),
        );
    }
    s.requests.push(body.clone());
    let id = body["commandId"].as_str().unwrap();
    if let Some(old) = s.receipts.get(id) {
        assert_eq!(old, body)
    } else {
        s.receipts.insert(id.into(), body.clone());
        s.writes += 1;
    }
    if s.invalidate {
        state
            .source
            .as_ref()
            .unwrap()
            .store(false, Ordering::Release);
    }
    if std::mem::take(&mut s.lose) {
        return None;
    }
    Some(
        json!({"requested":{"requestId":if s.malformed{"foreign"}else{"human_request"},"commandId":"mst_generated_by_server","commandDigest":"a".repeat(64),"ticket":"PRIVATE"},"observedAtMs":now(),"token":"PRIVATE"}),
    )
}
fn package() -> ManagementPackage {
    ManagementPackage {
        digest: "5".repeat(64),
        instance: 993,
        account_epoch: 1,
        permissions: vec![
            Permission::ReadAgentSettlement,
            Permission::RequestAgentSettlement,
            Permission::ReadIntents,
            Permission::AcknowledgeIntents,
        ],
    }
}
fn command() -> ManagementCommand {
    ManagementCommand::AgentSettlementRequest {
        command_id: "human_original".into(),
        request_id: "human_request".into(),
        expected_request_revision: 3,
        expected_fence_receipt_digest: "d".repeat(64),
    }
}
#[tokio::test]
async fn native_human_settlement_lost_reply_restart_exact_replay_and_role_recheck() {
    let f = Fixture::new().await;
    let m = f.owner.management(package()).await.unwrap();
    f.state.lock().unwrap().human_settlement.lose = true;
    assert!(m.central(command()).await.is_err());
    assert!(m.saved_commands().await.unwrap()[0].reply.is_none());
    let Fixture {
        _root,
        owner,
        transport,
        registry,
        state,
        task,
        current,
    } = f;
    let path = _root.path().join("host");
    let origin = owner.0.expected_origin.clone();
    owner.suspend().await.unwrap();
    drop(m);
    drop(transport);
    drop(owner);
    let owner =
        NativeOwner::open_with_matrix(&path, &origin, "@owner:test", Arc::new(Source(current)))
            .await
            .unwrap();
    let m = owner.management(package()).await.unwrap();
    let r = m
        .retry_saved_command("human_original".into())
        .await
        .unwrap();
    assert_eq!(r["operation"], "agent_settlement_request");
    assert_eq!(
        r["data"]["requested"]["commandId"],
        "mst_generated_by_server"
    );
    assert!(!r.to_string().contains("PRIVATE"));
    assert_eq!(state.lock().unwrap().human_settlement.writes, 1);
    {
        let s = state.lock().unwrap();
        assert_eq!(
            s.human_settlement.requests[0],
            s.human_settlement.requests[1]
        );
    }
    assert!(m.central(command()).await.is_ok());
    state.lock().unwrap().human_settlement.deny = true;
    assert!(
        m.central(command()).await.is_err(),
        "cached success must not bypass current visibility"
    );
    owner.shutdown().await;
    registry.shutdown().await.unwrap();
    task.abort();
}
#[tokio::test]
async fn native_human_settlement_malformed_and_sdk_loss_keep_original_pending() {
    let f = Fixture::new().await;
    let m = f.owner.management(package()).await.unwrap();
    f.state.lock().unwrap().human_settlement.malformed = true;
    assert!(m.central(command()).await.is_err());
    assert!(m.saved_commands().await.unwrap()[0].reply.is_none());
    {
        let mut s = f.state.lock().unwrap();
        s.human_settlement.malformed = false;
        s.human_settlement.invalidate = true;
    }
    assert!(
        m.retry_saved_command("human_original".into())
            .await
            .is_err()
    );
    f.current.store(true, Ordering::Release);
    f.state.lock().unwrap().human_settlement.invalidate = false;
    assert!(m.saved_commands().await.unwrap()[0].reply.is_none());
    m.retry_saved_command("human_original".into())
        .await
        .unwrap();
    assert_eq!(f.state.lock().unwrap().human_settlement.writes, 1);
    f.owner.shutdown().await;
    f.stop().await;
}
#[tokio::test]
async fn native_human_settlement_read_only_grant_and_bounded_separate_projection() {
    let f = Fixture::new().await;
    let mut p = package();
    p.permissions = vec![Permission::ReadAgentSettlement];
    let m = f.owner.management(p).await.unwrap();
    let get = || ManagementCommand::AgentSettlement {
        id: "human_request".into(),
    };
    let r = m.central(get()).await.unwrap();
    assert_eq!(r["settlement"]["knownConsumedTokens"], 40);
    assert!(!r.to_string().contains("PRIVATE"));
    assert!(m.central(command()).await.is_err());
    assert_eq!(f.state.lock().unwrap().human_settlement.writes, 0);
    for (field, bad) in [
        ("requestId", json!("foreign")),
        ("knownConsumedTokens", json!(101)),
        ("retainedTokensFloor", json!(9007199254740992u64)),
        ("fenceReceiptDigest", Value::Null),
        ("pendingCommandId", json!("pending")),
        ("settlementCurrent", json!(true)),
    ] {
        let mut v = progress();
        v[field] = bad;
        f.state.lock().unwrap().human_settlement.read = Some(v);
        assert!(m.central(get()).await.is_err(), "{field}");
    }
    let receipt = json!({"requestId":"human_request","commandId":"mst_old","fenceReceiptDigest":"d".repeat(64),"ledgerRevision":12,"state":"settled","processState":"stopped","knownConsumedTokens":40,"retainedTokens":40,"unresolvedTokens":0,"usageComplete":true,"reserveCommand":"PRIVATE"});
    let mut v = progress();
    v["latestReceipt"] = receipt.clone();
    v["canRequest"] = false.into();
    v["settlementCurrent"] = true.into();
    v["blockingReason"] = "settlement_current".into();
    v["retainedTokensFloor"] = 40.into();
    f.state.lock().unwrap().human_settlement.read = Some(v.clone());
    let r = m.central(get()).await.unwrap();
    assert!(!r.to_string().contains("PRIVATE"));
    assert_eq!(r["settlement"]["latestReceipt"]["retainedTokens"], 40);
    v["latestReceipt"]["unresolvedTokens"] = 1.into();
    f.state.lock().unwrap().human_settlement.read = Some(v);
    assert!(m.central(get()).await.is_err());
    f.owner.shutdown().await;
    f.stop().await;
}
