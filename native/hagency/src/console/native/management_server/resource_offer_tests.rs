use super::*;
use hagency_store::management::IntentOperation;

fn resource() -> ResourceSummary {
    let budget=hagency_core::allocation::resource_budget(&serde_json::from_value(json!({
        "preset":{"id":"canonical_preset"},"seatId":"canonical_seat","commitments":[]
    })).unwrap()).unwrap();
    ResourceSummary{
        id:"resource_one".into(),revision:"a".repeat(64),framework:"codex".into(),model:"model_from_ledger".into(),
        reasoning:None,capabilities:vec!["coding".into()],published:true,budget,local_name:None,
        usage:hagency_store::CeilingReport{period:hagency_store::UsagePeriodKind::Monthly,reserved:0,spent:None,consumed:None,ceiling_tokens:None,preset_name:"canonical_preset".into(),spend_period_key:None,drawn:0,evidence:hagency_store::UsageEvidence::HostAttributedUntrustedUsage}
    }
}
fn input()->Value {
    json!({"operation":"reoffer","command_id":"reoffer_once","contribution_id":"contribution_one","expected_contribution_revision":3,
        "resource_id":"resource_one","expected_revision":"a".repeat(64),"label":"Owner label","coordinator_mxid":"@coordinator:test"})
}
#[test]
fn resource_reoffer_native_wire_uses_canonical_definition_and_exact_local_and_remote_revisions() {
    let command:ManagementCommand=serde_json::from_value(input()).unwrap();
    assert_eq!(command.permission(),Permission::ContributeResources);
    assert!(command.mutation());assert_eq!(command.intent().unwrap().unwrap().operation(),IntentOperation::Reoffer);
    let actual=resource();let (method,path,body)=ServerOperation::prepare(&command,Some(&actual)).unwrap().request().unwrap();
    assert_eq!(method,reqwest::Method::POST);assert_eq!(path,"/hagency/api/v1/management/contributions/contribution_one/reoffer");
    assert_eq!(body.unwrap(),json!({"expectedRevision":3,"offer":{"commandId":"reoffer_once","resourceId":"resource_one","resourceRevision":"a".repeat(64),"label":"Owner label","runtime":"codex","capabilities":["coding"],"coordinatorMxid":"@coordinator:test"}}));
    assert!(ServerOperation::prepare(&command,None).is_err());
    let mut stale=resource();stale.revision="b".repeat(64);
    assert!(ServerOperation::prepare(&command,Some(&stale)).is_err());
    for value in [0,9_007_199_254_740_991_i64] {
        let mut bad=input();bad["expected_contribution_revision"]=json!(value);
        assert!(ServerOperation::prepare(&serde_json::from_value(bad).unwrap(),Some(&actual)).is_err());
    }
}
#[test]
fn resource_reoffer_native_recovery_keeps_original_permission_and_command() {
    let command=ManagementCommand::Recover{command_id:"reoffer_once".into(),original_operation:IntentOperation::Reoffer};
    assert_eq!(command.permission(),Permission::ContributeResources);
    let (_,path,body)=ServerOperation::prepare(&command,None).unwrap().request().unwrap();
    assert_eq!(path,"/hagency/api/v1/management/commands/reoffer_once/recover");
    assert_eq!(body.unwrap(),json!({"operation":"contribution.reoffer"}));
}
#[test]
fn resource_reoffer_native_contract_rejects_script_authority_and_arbitrary_routes() {
    for key in ["runtime","capabilities","owner_mxid","allocated_tokens","native_fence","url","token"] {
        let mut forged=input();forged[key]=json!("forged");
        assert!(serde_json::from_value::<ManagementCommand>(forged).is_err());
    }
    for path in ["../secrets","a/b","id?token=other","https://elsewhere"] {
        let mut forged=input();forged["contribution_id"]=json!(path);
        assert!(ServerOperation::prepare(&serde_json::from_value(forged).unwrap(),Some(&resource())).is_err());
        assert!(ServerOperation::prepare(&ManagementCommand::ConnectionHistory{id:path.into(),after:None},None).is_err());
    }
    let command=ManagementCommand::ConnectionHistory{id:"contribution_one".into(),after:Some("request_one".into())};
    assert_eq!(command.permission(),Permission::ReadConnections);assert!(!command.mutation());assert!(command.intent().unwrap().is_none());
    let (_,path,body)=ServerOperation::prepare(&command,None).unwrap().request().unwrap();
    assert_eq!(path,"/hagency/api/v1/management/contributions/contribution_one/history");
    assert_eq!(body.unwrap(),json!({"after":"request_one"}));
}
