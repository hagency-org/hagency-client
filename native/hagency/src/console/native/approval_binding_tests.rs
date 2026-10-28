//! Real loopback identity and SQLite, synthetic rooms, no provider inference.
use super::*;
use hagency_store::management::{
    Intent, IntentOperation, ResourceApprovalBindingQuery, RetireResourceApprovalBinding,
};
use crate::file_service::test_common::domain as legacy;

fn package() -> ManagementPackage {
    ManagementPackage {
        digest: "e".repeat(64),
        instance: 1047,
        account_epoch: 1,
        permissions: vec![
            Permission::ReadApprovalBindings,
            Permission::RetireApprovalBindings,
            Permission::ReadIntents,
            Permission::AcknowledgeIntents,
        ],
    }
}
pub(super) async fn repository(owner: &NativeOwner) -> hagency_store::DomainRepository {
    let login = &owner.0.host.console.0.server_login;
    let device = login.authorized_device().await.unwrap();
    let profile = crate::console::owned_agents::profile_identity(
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let directory = login
        .state_directory()
        .unwrap()
        .join("owned-agent-owners")
        .join(format!("management_{profile}"));
    owner.suspend().await.unwrap();
    hagency_store::DomainRepository::open_managed(&directory, &profile).unwrap()
}
pub(super) async fn reopen(f: Fixture) -> Fixture {
    let Fixture {
        _root,
        owner,
        transport,
        registry,
        state,
        task,
        current,
    } = f;
    let origin = owner.0.expected_origin.clone();
    owner.suspend().await.unwrap();
    drop(transport);
    drop(owner);
    let owner = NativeOwner::open_with_matrix(
        &_root.path().join("host"),
        &origin,
        "@owner:test",
        Arc::new(Source(current.clone())),
    )
    .await
    .unwrap();
    let transport = AllocationTransport::new(owner.management(super::package(1)).await.unwrap());
    Fixture {
        _root,
        owner,
        transport,
        registry,
        state,
        task,
        current,
    }
}
pub(super) async fn fixture() -> (
    Fixture,
    String,
    hagency_core::approvals::ApprovalRoomObservation,
) {
    let f = Fixture::new().await;
    let mut db = repository(&f.owner).await;
    let resource = legacy::resource("binding_fixture", "binding_seat", 1000);
    db.put_seat(&hagency_core::project::Seat {
        id: resource.seat_id.clone(),
        declaration: None,
    })
    .unwrap();
    db.put_resource(&resource).unwrap();
    db.register(&legacy::registration()).unwrap();
    let proof = legacy::proof(&legacy::request(
        "binding_request",
        "binding_agent",
        &resource,
        20,
    ));
    let engagement = db.admit(&proof, 1000).unwrap();
    db.approve("binding_approve", &proof, 1000).unwrap();
    let effect = db.claim_effect().unwrap().unwrap();
    db.observe_effect(
        &effect.id,
        effect.fence,
        &hagency_store::EffectOutcome::Applied {
            receipt: "fixture provisioning".into(),
        },
    )
    .unwrap();
    let room = hagency_core::approvals::ApprovalRoomObservation {
        engagement_id: engagement.id,
        registration_generation: 1,
        generation: 1,
        room_id: "!private:example.test".into(),
        device_id: "BOT_DEVICE".into(),
        joined: std::collections::BTreeSet::from([
            "@owner:example.test".into(),
            "@approval:example.test".into(),
        ]),
        invite_only: true,
        encrypted: true,
        available: true,
    };
    db.observe_approval_room(&room, 1010).unwrap();
    drop(db);
    (reopen(f).await, resource.id(), room)
}
fn query(id: &str) -> ResourceApprovalBindingQuery {
    ResourceApprovalBindingQuery {
        resource_id: id.into(),
        after: String::new(),
        ..Default::default()
    }
}
#[tokio::test]
async fn native_approval_binding_lost_reply_restart_and_successor_are_exact() {
    let (mut f, id, room) = fixture().await;
    let m = f.owner.management(package()).await.unwrap();
    let row = m
        .resource_approval_bindings(query(&id))
        .await
        .unwrap()
        .bindings
        .remove(0);
    let input = RetireResourceApprovalBinding {
        command_id: "original_unbind".into(),
        resource_id: id.clone(),
        engagement_id: row.engagement_id,
        expected_revision: row.revision,
    };
    let (ctx, session) = m.authorized().await.unwrap();
    session
        .prepare_intent(
            &ctx,
            Intent::new(
                IntentOperation::RetireApprovalBinding,
                input.command_id.clone(),
                &input,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let original = session
        .retire_resource_approval_binding(&ctx, input.clone())
        .await
        .unwrap();
    assert!(
        m.saved_commands()
            .await
            .unwrap()
            .iter()
            .find(|r| r.command_id == input.command_id)
            .unwrap()
            .reply
            .is_none()
    );
    drop(session);
    drop(m);
    f = reopen(f).await;
    let m = f.owner.management(package()).await.unwrap();
    let reply = m
        .retry_saved_command(input.command_id.clone())
        .await
        .unwrap();
    assert_eq!(reply["operation"], "retire_approval_binding");
    assert_eq!(reply["data"], serde_json::to_value(&original).unwrap());
    drop(m);
    let mut db = repository(&f.owner).await;
    db.observe_approval_room(&room, 1020).unwrap();
    drop(db);
    f = reopen(f).await;
    let m = f.owner.management(package()).await.unwrap();
    assert_eq!(
        m.retry_saved_command(input.command_id.clone())
            .await
            .unwrap()["data"],
        serde_json::to_value(&original).unwrap()
    );
    let current = m.resource_approval_bindings(query(&id)).await.unwrap();
    assert_eq!(current.bindings.len(), 1);
    assert_eq!(current.bindings[0].incarnation, 2);
    let mut reader = package();
    reader.instance += 1;
    reader
        .permissions
        .retain(|p| *p != Permission::RetireApprovalBindings);
    let reader = f.owner.management(reader).await.unwrap();
    assert!(
        reader
            .retry_saved_command(input.command_id.clone())
            .await
            .is_err()
    );
    f.current.store(false, Ordering::Release);
    assert!(
        m.retry_saved_command(input.command_id.clone())
            .await
            .is_err()
    );
    f.current.store(true, Ordering::Release);
    assert_eq!(
        m.retire_resource_approval_binding(input).await.unwrap(),
        original
    );
    f.owner.suspend().await.unwrap();
    f.stop().await;
}
#[tokio::test]
async fn native_approval_binding_conflict_is_known_only_before_original_attempt() {
    let (f, id, _) = fixture().await;
    let m = f.owner.management(package()).await.unwrap();
    let row = m
        .resource_approval_bindings(query(&id))
        .await
        .unwrap()
        .bindings
        .remove(0);
    let mut input = RetireResourceApprovalBinding {
        command_id: "stale_unbind".into(),
        resource_id: id.clone(),
        engagement_id: row.engagement_id,
        expected_revision: "b".repeat(64),
    };
    assert_eq!(
        m.retire_resource_approval_binding(input.clone())
            .await
            .unwrap_err()
            .code,
        "management_revision_conflict"
    );
    assert!(
        m.saved_commands()
            .await
            .unwrap()
            .iter()
            .any(|r| r.command_id == input.command_id && r.reply.is_some())
    );
    input.command_id = "unknown_unbind".into();
    let (ctx, session) = m.authorized().await.unwrap();
    session
        .prepare_intent(
            &ctx,
            Intent::new(
                IntentOperation::RetireApprovalBinding,
                input.command_id.clone(),
                &input,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        m.retire_resource_approval_binding(input.clone())
            .await
            .unwrap_err()
            .code,
        "management_outcome_unknown"
    );
    assert!(
        m.saved_commands()
            .await
            .unwrap()
            .iter()
            .any(|r| r.command_id == input.command_id && r.reply.is_none())
    );
    assert_eq!(
        m.resource_approval_bindings(query(&id))
            .await
            .unwrap()
            .bindings
            .len(),
        1
    );
    f.owner.suspend().await.unwrap();
    f.stop().await;
}
