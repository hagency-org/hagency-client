//! Human allocation HTTP custody; no native reserve or provider execution.
use super::*;
#[derive(Default)]
pub(super) struct AllocationState {
    detail: Option<Value>,
    receipts: BTreeMap<String, Value>,
    writes: usize,
    reads: Vec<Value>,
    lose_after: bool,
    lose_before: bool,
    invalidate: bool,
    reject: Option<&'static str>,
}
fn detail() -> Value {
    json!({"allocation":{"id":"human_allocation","projectId":"project_fixture","projectRevision":2,"grantId":"grant_original","requestedTokens":100,"state":"pending_coordinator","revision":7,"requestId":"allocation_request","requesterMxid":"@manager:test","coordinatorMxid":"@owner:test","periodKey":"2026-10","expiresAtMs":now()+60000,"canDecide":true,"canRelease":false,"blockingReason":null,"result":null,"balance":null,"observedUsage":null},"request":{"id":"allocation_request","kind":"project_allocation","contributionId":"contribution_fixture","projectId":"project_fixture","allocationId":"human_allocation","requesterMxid":"@manager:test","approverMxid":"@owner:test","state":"pending","revision":3,"expiresAtMs":now()+60000,"canDecide":true},"projectName":"Project","resourceLabel":"Coding resource","grant":{"id":"grant_original","revision":1,"state":"active","ownerMxid":"@resource_owner:test","coordinatorMxid":"@owner:test","totalTokens":700,"periodKey":"2026-10","periodStartMs":1790812800000i64,"periodEndMs":1793491200000i64,"delegationExpiresAtMs":1793491200000i64,"nativeAvailableTokens":null,"receipt":"private"},"token":"private"})
}
pub(super) fn management(
    path: &str,
    body: &Value,
    state: &mut super::super::State,
) -> Option<Value> {
    let s = &mut state.admission.allocations;
    let d = s.detail.get_or_insert_with(detail);
    if path.contains("/allocation-commands/") {
        let id = path.split('/').rev().nth(1).unwrap();
        return Some(if s.receipts.contains_key(id) {
            json!({"recorded":true,"allocation":d["allocation"],"observedAtMs":now()})
        } else {
            json!({"recorded":false,"allocation":null,"observedAtMs":now()})
        });
    }
    if path.ends_with("/allocations/query")
        || path.ends_with("/allocations/human_allocation")
        || path.ends_with("/inbox")
    {
        s.reads.push(body.clone());
        if s.invalidate {
            state
                .source
                .as_ref()
                .unwrap()
                .store(false, Ordering::Release);
        }
        if path.ends_with("/inbox") {
            return Some(json!({"items":[],"nextAfter":null,"observedAtMs":now()}));
        }
        if let Some(code) = s.reject {
            return Some(json!({"code":code,"fixtureStatus":503}));
        }
        return Some(if path.ends_with("/query") {
            json!({"allocations":{"items":[d],"nextAfter":null},"observedAtMs":now()})
        } else {
            json!({"detail":d,"observedAtMs":now()})
        });
    }
    assert!(
        path.ends_with("/allocation-requests/allocation_request/decide")
            || path.ends_with("/allocations/human_allocation/release")
    );
    if std::mem::take(&mut s.lose_before) {
        return None;
    }
    if let Some(code) = s.reject {
        return Some(json!({"code":code}));
    }
    let id = body["commandId"].as_str().unwrap().to_owned();
    if let std::collections::btree_map::Entry::Vacant(entry) = s.receipts.entry(id) {
        if path.ends_with("/decide") {
            assert_eq!(
                body["expectedRevision"], 3,
                "decision must bind request revision, not allocation revision"
            );
            d["allocation"]["state"] = json!(if body["decision"] == "approve" {
                "approved_waiting_executor"
            } else {
                "rejected"
            });
        } else {
            d["allocation"]["state"] = json!("releasing");
        }
        d["allocation"]["canDecide"] = json!(false);
        entry.insert(d["allocation"].clone());
        s.writes += 1;
    }
    if s.invalidate {
        state
            .source
            .as_ref()
            .unwrap()
            .store(false, Ordering::Release);
    }
    if std::mem::take(&mut s.lose_after) {
        return None;
    }
    Some(json!({"allocation":d["allocation"],"observedAtMs":now()}))
}
fn get() -> ManagementCommand {
    ManagementCommand::Allocation {
        id: "human_allocation".into(),
    }
}
fn decide(id: &str) -> ManagementCommand {
    ManagementCommand::AllocationDecide {
        command_id: id.into(),
        request_id: "allocation_request".into(),
        expected_revision: 3,
        approve: true,
    }
}
fn grants() -> Vec<Permission> {
    vec![
        Permission::ReadAllocations,
        Permission::DecideAllocations,
        Permission::ReleaseAllocations,
        Permission::ReadIntents,
        Permission::AcknowledgeIntents,
    ]
}
#[tokio::test]
async fn human_allocation_review_is_exact_bounded_and_not_native_headroom() {
    let f = Fixture::new().await;
    let m = manager(&f, 71, &grants()).await;
    let r = m.central(get()).await.unwrap();
    assert_eq!(r["detail"]["allocation"]["revision"], 7);
    assert_eq!(r["detail"]["request"]["revision"], 3);
    assert_eq!(r["detail"]["grant"]["totalTokens"], 700);
    assert!(r["detail"]["grant"]["nativeAvailableTokens"].is_null());
    assert!(!r.to_string().contains("private"));
    let list = |project: &str, after: Option<&str>, limit| ManagementCommand::Allocations {
        project_id: project.into(),
        after: after.map(str::to_owned),
        limit,
    };
    assert_eq!(
        m.central(list("project_fixture", None, Some(1)))
            .await
            .unwrap()["allocations"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for c in [
        list("project_other", None, None),
        list("project_fixture", Some("human_allocation"), None),
        list("project_fixture", None, Some(17)),
    ] {
        assert!(m.central(c).await.is_err());
    }
    assert!(m.saved_commands().await.unwrap().is_empty());
    assert_eq!(f.state.lock().unwrap().admission.allocations.writes, 0);
    f.state
        .lock()
        .unwrap()
        .admission
        .allocations
        .detail
        .as_mut()
        .unwrap()["grant"]["nativeAvailableTokens"] = json!(0);
    assert_eq!(
        m.central(get()).await.unwrap_err().code,
        "management_allocation_reply_invalid"
    );
    drop(m);
    f.stop().await;
}
#[tokio::test]
async fn human_allocation_decision_recovers_original_latest_after_lost_reply_and_restart() {
    let f = Fixture::new().await;
    let first = manager(&f, 72, &grants()).await;
    f.state.lock().unwrap().admission.allocations.lose_after = true;
    assert!(first.central(decide("original_decision")).await.is_err());
    assert_eq!(first.saved_commands().await.unwrap().len(), 1);
    first.retire();
    drop(first);
    f.state
        .lock()
        .unwrap()
        .admission
        .allocations
        .detail
        .as_mut()
        .unwrap()["allocation"]["state"] = json!("released");
    let fresh = manager(&f, 73, &grants()).await;
    let r = fresh
        .retry_saved_command("original_decision".into())
        .await
        .unwrap();
    assert_eq!(r["operation"], "allocation_decide");
    assert_eq!(r["data"]["allocation"]["state"], "released");
    assert_eq!(
        fresh.central(decide("original_decision")).await.unwrap()["allocation"]["state"],
        "released"
    );
    assert_eq!(f.state.lock().unwrap().admission.allocations.writes, 1);
    fresh
        .acknowledge_command("original_decision".into())
        .await
        .unwrap();
    drop(fresh);
    f.stop().await;
}
#[tokio::test]
async fn human_allocation_scope_loss_after_commit_retains_original_for_fresh_authority() {
    let f = Fixture::new().await;
    let first = manager(&f, 74, &grants()).await;
    f.state.lock().unwrap().admission.allocations.invalidate = true;
    assert!(first.central(decide("scope_lost")).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.allocations.writes, 1);
    first.retire();
    f.current.store(true, Ordering::Release);
    let fresh = manager(&f, 75, &grants()).await;
    assert_eq!(
        fresh
            .retry_saved_command("scope_lost".into())
            .await
            .unwrap()["data"]["allocation"]["id"],
        "human_allocation"
    );
    assert!(first.central(decide("scope_lost")).await.is_err());
    assert_eq!(f.state.lock().unwrap().admission.allocations.writes, 1);
    drop(first);
    drop(fresh);
    f.stop().await;
}
#[tokio::test]
async fn human_allocation_permissions_and_uncertainty_never_grant_executor_authority() {
    let f = Fixture::new().await;
    let denied = manager(
        &f,
        76,
        &[
            Permission::DecideProjects,
            Permission::ReadProjects,
            Permission::ReadInbox,
        ],
    )
    .await;
    assert!(denied.central(get()).await.is_err());
    assert!(denied.central(decide("denied")).await.is_err());
    assert!(
        denied
            .central(ManagementCommand::Inbox {
                after: None,
                needs_my_action: true,
                include_projects: false,
                include_allocations: true
            })
            .await
            .is_err()
    );
    assert_eq!(f.state.lock().unwrap().admission.allocations.writes, 0);
    drop(denied);
    let m = manager(&f, 77, &grants()).await;
    f.state.lock().unwrap().admission.allocations.lose_before = true;
    assert!(m.central(decide("unknown")).await.is_err());
    f.state.lock().unwrap().admission.allocations.reject = Some("management_action_closed");
    assert_eq!(
        m.retry_saved_command("unknown".into())
            .await
            .unwrap_err()
            .code,
        "management_outcome_unknown"
    );
    assert!(m.acknowledge_command("unknown".into()).await.is_err());
    assert_eq!(
        m.central(decide("first_rejected")).await.unwrap_err().code,
        "management_action_closed"
    );
    m.acknowledge_command("first_rejected".into())
        .await
        .unwrap();
    assert_eq!(f.state.lock().unwrap().admission.allocations.writes, 0);
    drop(m);
    f.stop().await;
}
#[tokio::test]
async fn human_allocation_release_uses_own_grant_and_original_command() {
    let f = Fixture::new().await;
    let m = manager(&f, 78, &grants()).await;
    let release = || ManagementCommand::AllocationRelease {
        command_id: "release_original".into(),
        allocation_id: "human_allocation".into(),
        expected_revision: 7,
    };
    assert_eq!(
        m.central(release()).await.unwrap()["allocation"]["state"],
        "releasing"
    );
    f.state
        .lock()
        .unwrap()
        .admission
        .allocations
        .detail
        .as_mut()
        .unwrap()["allocation"]["state"] = json!("release_blocked");
    assert_eq!(
        m.central(release()).await.unwrap()["allocation"]["state"],
        "release_blocked"
    );
    assert_eq!(f.state.lock().unwrap().admission.allocations.writes, 1);
    drop(m);
    f.stop().await;
}
