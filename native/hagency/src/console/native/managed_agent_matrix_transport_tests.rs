//! Exact HTTP observation transport over real canonical owner SQLite.
use super::*;
use crate::console::native::{ManagedAgentMatrixTransport, ManagedMatrixProgress};
pub(super) fn management(path: &str, body: &Value, s: &mut State) -> Option<Value> {
    let at = now();
    let c = s.matrix_challenge.clone().unwrap();
    let id = c["commandId"].as_str().unwrap();
    let hash = digest(&c).unwrap();
    if path.ends_with("/result") {
        assert_eq!(body["commandDigest"], hash);
        assert_eq!(body["observation"]["commandId"], id);
        if let Some(old) = &s.matrix_observation {
            assert_eq!(old, &body["observation"]);
        } else {
            s.matrix_writes += 1;
            s.matrix_observation = Some(body["observation"].clone());
        }
        if s.invalidate_at_result {
            s.source.as_ref().unwrap().store(false, Ordering::Release);
        }
        if std::mem::take(&mut s.lose_result) {
            return None;
        }
        return Some(
            json!({"commandId":id,"commandDigest":hash,"observationDigest":if s.wrong_ack{"f".repeat(64)}else{digest(&body["observation"]).unwrap()},"membership":{"assignmentId":c["assignmentId"],"requestId":c["reserveCommand"]["requestId"],"agentId":c["reserveCommand"]["proposedAgentId"],"puppetMxid":c["puppetMxid"],"roomId":c["reserveCommand"]["roomId"],"state":if body["observation"]["eligible"]==true{"authorized"}else{"waiting_native"},"verifiedAtMs":null,"cleanupObservedAtMs":null,"blockingReason":null,"runtimeState":"unsupported","workAvailable":false},"observedAtMs":at}),
        );
    }
    let historical = path.ends_with("/recover");
    if historical {
        assert_eq!(body["commandDigest"], hash);
    }
    Some(
        json!({"delivery":{"command":c,"commandDigest":if s.tamper_digest{"f".repeat(64)}else{hash},"token":"e".repeat(64),"leaseUntilMs":if s.expired_ticket{at-1}else{at+20000},"executionAllowed":if historical{s.historical_executable}else{s.matrix_observation.is_none()},"recorded":s.matrix_observation},"observedAtMs":at}),
    )
}
async fn prepared_matrix() -> (
    Fixture,
    ManagedAgentTransport,
    ManagedAgentMatrixTransport,
    ManagedAgentMatrixChallenge,
) {
    let (f, t, c) = prepared().await;
    t.poll(&f.registry, None).await.unwrap();
    let r: ManagedAgentReservationReceipt =
        serde_json::from_value(f.state.lock().unwrap().agent_receipts[&c.command_id].clone())
            .unwrap();
    let q = ManagedAgentMatrixChallenge {
        version: 1,
        capability: AGENT_MATRIX_CAPABILITY.into(),
        operation: "agent.matrix.check".into(),
        command_id: "matrix_fixture".into(),
        assignment_id: "assignment_fixture".into(),
        assignment_generation: 1,
        agent_reservation_id: r.agent_reservation_id.clone().unwrap(),
        reserve_command_digest: digest(&c).unwrap(),
        reserve_receipt_digest: digest(&r).unwrap(),
        reserve_command: c,
        space_id: "!space:test".into(),
        puppet_mxid: "@_hagency_fixture:test".into(),
        issued_at_ms: now(),
        expires_at_ms: now() + 20000,
    };
    f.state.lock().unwrap().matrix_challenge = Some(serde_json::to_value(&q).unwrap());
    let m = ManagedAgentMatrixTransport::new(f.owner.management(package(1)).await.unwrap());
    (f, t, m, q)
}
fn files(f: &Fixture) -> Vec<Option<Vec<u8>>> {
    ["domain.sqlite3", "domain.sqlite3-wal"]
        .iter()
        .map(|n| std::fs::read(f._root.path().join("registry").join(n)).ok())
        .collect()
}
#[tokio::test]
async fn native_managed_matrix_reply_loss_recovers_fixed_observation_without_ledger_write() {
    let (f, _, m, q) = prepared_matrix().await;
    let before = files(&f);
    assert!(before[0].is_some());
    f.state.lock().unwrap().lose_result = true;
    assert!(m.poll(&f.registry, None).await.is_err());
    assert_eq!(f.state.lock().unwrap().matrix_writes, 1);
    assert_eq!(files(&f), before);
    assert!(matches!(
        m.recover(&f.registry, q.command_id.clone(), digest(&q).unwrap())
            .await
            .unwrap(),
        ManagedMatrixProgress::Observed { eligible: true, .. }
    ));
    assert_eq!(f.state.lock().unwrap().matrix_writes, 1);
    assert_eq!(files(&f), before);
    f.stop().await;
}
#[tokio::test]
async fn native_managed_matrix_historical_missing_or_untrusted_delivery_never_checks_or_uploads() {
    let (f, _, m, q) = prepared_matrix().await;
    assert!(matches!(
        m.recover(&f.registry, q.command_id.clone(), digest(&q).unwrap())
            .await
            .unwrap(),
        ManagedMatrixProgress::HistoricalObservationMissing { .. }
    ));
    f.state.lock().unwrap().historical_executable = true;
    assert!(
        m.recover(&f.registry, q.command_id.clone(), digest(&q).unwrap())
            .await
            .is_err()
    );
    {
        let mut s = f.state.lock().unwrap();
        s.historical_executable = false;
        s.tamper_digest = true;
    }
    assert!(m.poll(&f.registry, None).await.is_err());
    {
        let mut s = f.state.lock().unwrap();
        s.tamper_digest = false;
        s.expired_ticket = true;
    }
    assert!(m.poll(&f.registry, None).await.is_err());
    assert_eq!(f.state.lock().unwrap().matrix_writes, 0);
    f.stop().await;
}
#[tokio::test]
async fn native_managed_matrix_permanent_child_fence_reports_ineligible_without_runtime() {
    let (f, t, m, q) = prepared_matrix().await;
    let fence = fence(&q.reserve_command);
    f.state.lock().unwrap().agent = Some(serde_json::to_value(&fence).unwrap());
    t.poll(&f.registry, None).await.unwrap();
    let before = files(&f);
    assert!(matches!(
        m.poll(&f.registry, None).await.unwrap(),
        ManagedMatrixProgress::Observed {
            eligible: false,
            ..
        }
    ));
    assert_eq!(files(&f), before);
    assert_eq!(
        f.state.lock().unwrap().matrix_observation.as_ref().unwrap()["reason"],
        "native_reservation_unavailable"
    );
    f.stop().await;
}
#[tokio::test]
async fn native_managed_matrix_live_profile_checked_before_and_after_http_result() {
    let (f, _, m, _) = prepared_matrix().await;
    f.state.lock().unwrap().invalidate_at_contribution = true;
    assert!(m.poll(&f.registry, None).await.is_err());
    assert_eq!(f.state.lock().unwrap().matrix_writes, 0);
    f.stop().await;
    let (f, _, m, _) = prepared_matrix().await;
    f.state.lock().unwrap().invalidate_at_result = true;
    assert!(m.poll(&f.registry, None).await.is_err());
    assert_eq!(f.state.lock().unwrap().matrix_writes, 1);
    f.stop().await;
}
