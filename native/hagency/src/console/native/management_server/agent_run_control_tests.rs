//! Real authenticated loopback + native private command journal.
use super::*;
use crate::native_owner::{AgentRunMode, ManagementCommand, ManagementPermission as Permission};
#[derive(Default)]
pub(super) struct ControlState {
    pub requires_local: bool,
    pub missing_feature: bool,
    pub fail_readiness: bool,
    pub deny: bool,
    pub lose: bool,
    pub malformed: bool,
    pub invalidate_after: bool,
    pub revision: u64,
    pub mode: Option<&'static str>,
    pub writes: usize,
    pub requests: Vec<Value>,
    pub receipts: BTreeMap<String, (Value, u64)>,
}
fn control(s: &ControlState) -> Value {
    let mode = s.mode.unwrap_or("manual");
    json!({"assignmentId":"assignment_control","revision":s.revision.max(1),"mode":mode,"runningSinceMs":if mode=="running"{Some(now())}else{None},"updatedAtMs":now(),"origin":"legacy_manual"})
}
pub(super) fn management(path: &str, body: &Value, s: &mut State) -> Option<Value> {
    let c = &mut s.control;
    if path.ends_with("/session") {
        return Some(
            json!({"features":if c.missing_feature{vec![]}else{vec!["shared-run-control-v1"]}}),
        );
    }
    if path.ends_with("/readiness") {
        if c.fail_readiness {
            return Some(json!({"fixtureStatus":503,"error":"readiness_unavailable"}));
        }
        return Some(
            json!({"readiness":{"control":control(c),"canRun":!c.deny,"canStop":!c.deny,"requiresLocalStop":c.requires_local,"runBlockingReason":null,"intakeEnabled":false,"automaticDeliveryEnabled":false},"observedAtMs":now()}),
        );
    }
    assert!(path.ends_with("/control"));
    c.requests.push(body.clone());
    if c.deny {
        return Some(json!({"fixtureStatus":403,"error":"shared_run_role_required"}));
    }
    let id = body["commandId"].as_str().unwrap().to_owned();
    if let Some((old, _)) = c.receipts.get(&id) {
        assert_eq!(old, body)
    } else {
        if body["expectedRevision"] != json!(c.revision.max(1)) {
            return Some(json!({"fixtureStatus":409,"error":"shared_control_revision_changed"}));
        }
        c.revision = c.revision.max(1) + 1;
        c.mode = Some(if body["mode"] == "running" {
            "running"
        } else {
            "paused"
        });
        c.writes += 1;
        c.receipts.insert(id.clone(), (body.clone(), c.revision));
    }
    if c.invalidate_after {
        s.source.as_ref().unwrap().store(false, Ordering::Release);
    }
    if std::mem::take(&mut c.lose) {
        return None;
    }
    Some(
        json!({"commandId":if c.malformed{"wrong"}else{&id},"appliedRevision":c.receipts[&id].1,"control":control(c),"observedAtMs":now()}),
    )
}
async fn manager(f: &Fixture) -> Arc<crate::native_owner::NativeManagement> {
    f.owner
        .management(ManagementPackage {
            digest: "7".repeat(64),
            instance: 991,
            account_epoch: 1,
            permissions: vec![
                Permission::ReadAgentRunControl,
                Permission::ChangeAgentRunControl,
                Permission::ReadIntents,
                Permission::AcknowledgeIntents,
            ],
        })
        .await
        .unwrap()
}
fn command(id: &str, mode: AgentRunMode) -> ManagementCommand {
    ManagementCommand::AgentRunControl {
        command_id: id.into(),
        assignment_id: "assignment_control".into(),
        expected_revision: 1,
        mode,
    }
}
#[tokio::test]
async fn native_run_control_exact_lost_reply_retry_and_latest_projection() {
    let f = Fixture::new().await;
    let m = manager(&f).await;
    f.state.lock().unwrap().control.lose = true;
    assert!(
        m.central(command("original_run", AgentRunMode::Running))
            .await
            .is_err()
    );
    assert_eq!(m.saved_commands().await.unwrap().len(), 1);
    let r = m.retry_saved_command("original_run".into()).await.unwrap();
    assert_eq!(r["data"]["appliedRevision"], 2);
    {
        let mut s = f.state.lock().unwrap();
        s.control.revision = 4;
        s.control.mode = Some("paused");
    }
    let r = m
        .central(command("original_run", AgentRunMode::Running))
        .await
        .unwrap();
    assert_eq!(r["appliedRevision"], 2);
    assert_eq!(r["control"]["revision"], 4);
    assert_eq!(f.state.lock().unwrap().control.writes, 1);
    assert!(
        m.central(command("original_run", AgentRunMode::Paused))
            .await
            .is_err()
    );
    f.state.lock().unwrap().control.deny = true;
    assert!(
        m.central(command("original_run", AgentRunMode::Running))
            .await
            .is_err()
    );
    f.owner.shutdown().await;
}
#[tokio::test]
async fn native_run_control_owner_admin_missing_local_custody_never_posts() {
    let f = Fixture::new().await;
    let m = manager(&f).await;
    f.state.lock().unwrap().control.requires_local = true;
    let e = m
        .central(command("owner_stop", AgentRunMode::Paused))
        .await
        .unwrap_err();
    assert_eq!(e.code, "owner_stop_local_custody_required");
    assert_eq!(f.state.lock().unwrap().control.requests.len(), 0);
    assert!(m.saved_commands().await.unwrap()[0].reply.is_none());
    f.owner.shutdown().await;
}
#[tokio::test]
async fn native_run_control_pair_readiness_failure_and_wrong_role_have_no_effect() {
    let f = Fixture::new().await;
    let m = manager(&f).await;
    f.state.lock().unwrap().control.missing_feature = true;
    assert!(
        m.central(ManagementCommand::AgentRunReadiness {
            assignment_id: "assignment_control".into()
        })
        .await
        .is_err()
    );
    {
        let mut s = f.state.lock().unwrap();
        s.control.missing_feature = false;
        s.control.fail_readiness = true;
    }
    assert!(
        m.central(command("unavailable", AgentRunMode::Running))
            .await
            .is_err()
    );
    assert_eq!(f.state.lock().unwrap().control.requests.len(), 0);
    {
        let mut s = f.state.lock().unwrap();
        s.control.fail_readiness = false;
        s.control.deny = true;
    }
    assert!(
        m.central(command("wrong_role", AgentRunMode::Running))
            .await
            .is_err()
    );
    assert_eq!(f.state.lock().unwrap().control.writes, 0);
    f.owner.shutdown().await;
}
#[tokio::test]
async fn native_run_control_issuance_off_manager_stop_and_malformed_reply_retains_intent() {
    let f = Fixture::new().await;
    let m = manager(&f).await;
    let r = m
        .central(ManagementCommand::AgentRunReadiness {
            assignment_id: "assignment_control".into(),
        })
        .await
        .unwrap();
    assert_eq!(r["readiness"]["automaticDeliveryEnabled"], false);
    f.state.lock().unwrap().control.malformed = true;
    assert!(
        m.central(command("manager_stop", AgentRunMode::Paused))
            .await
            .is_err()
    );
    assert!(m.saved_commands().await.unwrap()[0].reply.is_none());
    f.state.lock().unwrap().control.malformed = false;
    assert_eq!(
        m.retry_saved_command("manager_stop".into()).await.unwrap()["data"]["appliedRevision"],
        2
    );
    assert_eq!(f.state.lock().unwrap().control.writes, 1);
    f.owner.shutdown().await;
}

#[tokio::test]
async fn native_run_control_read_grant_cannot_write_and_post_sdk_loss_stays_pending() {
    let f = Fixture::new().await;
    let read = f
        .owner
        .management(ManagementPackage {
            digest: "6".repeat(64),
            instance: 992,
            account_epoch: 1,
            permissions: vec![Permission::ReadAgentRunControl],
        })
        .await
        .unwrap();
    read.central(ManagementCommand::AgentRunReadiness {
        assignment_id: "assignment_control".into(),
    })
    .await
    .unwrap();
    assert!(
        read.central(command("forbidden", AgentRunMode::Running))
            .await
            .is_err()
    );
    assert_eq!(f.state.lock().unwrap().control.requests.len(), 0);
    let m = manager(&f).await;
    f.state.lock().unwrap().control.invalidate_after = true;
    assert!(
        m.central(command("uncertain_scope", AgentRunMode::Running))
            .await
            .is_err()
    );
    f.current.store(true, Ordering::Release);
    f.state.lock().unwrap().control.invalidate_after = false;
    assert!(m.saved_commands().await.unwrap()[0].reply.is_none());
    assert_eq!(
        m.retry_saved_command("uncertain_scope".into())
            .await
            .unwrap()["data"]["appliedRevision"],
        2
    );
    assert_eq!(f.state.lock().unwrap().control.writes, 1);
    f.owner.shutdown().await;
}
