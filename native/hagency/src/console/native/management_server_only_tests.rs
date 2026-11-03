use super::*;
use crate::console::native::{ManagementCommand, reconciliation_sources::tests::Fixture};
use serde_json::json;

#[test]
fn server_management_after_another_account_initializes_resources() {
    for case in ["fresh", "existing", "existing_empty"] {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "console::native::management::server_only_tests::server_management_process_child",
                "--exact",
                "--nocapture",
            ])
            .env("HAGENCY_SERVER_JOURNAL_CASE", case)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[tokio::test]
async fn server_management_process_child() {
    let case = match std::env::var("HAGENCY_SERVER_JOURNAL_CASE") {
        Ok(case) => case,
        Err(_) => return,
    };
    let installation =
        hagency_platform::legacy_writer::test_support::initialize_process_root().unwrap();
    let f = Fixture::with_configured_installation().await;
    let old = if case == "existing" {
        let (db, path, _) = f.domain();
        drop(db);
        Some(path)
    } else {
        None
    };
    let empty = if case == "existing_empty" {
        let device = f.owner.0.host.console.authorized_device().await.unwrap();
        let profile = profile_identity(
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .unwrap();
        let owners = f.owner.0.host.state_directory().join("owned-agent-owners");
        hagency_store::private::directory(&owners).unwrap();
        let directory = owners.join(format!("management_{profile}"));
        drop(DomainRepository::open_managed(&directory, &profile).unwrap());
        Some((directory, profile))
    } else {
        None
    };
    let exclusive = hagency_platform::legacy_writer::ExclusiveWriterGuard::acquire().unwrap();
    exclusive
        .publish_refusal(&"a".repeat(64), "other_owner_setup", &"b".repeat(64))
        .unwrap();
    drop(exclusive);
    let marker = installation.join("legacy-resource-refusal.json");
    let original = std::fs::read(&marker).unwrap();
    f.recipient_routes.lock().unwrap().extend([
        ("/hagency/api/v1/management/session".into(), json!({"protocol":"hagency-management-v1","account":"@owner:test","device":"fixture-device","deviceGeneration":1,"canAdmitResources":true})),
        ("/hagency/api/v1/management/inbox".into(), json!({"protocol":"hagency-management-v1","items":[],"nextAfter":null})),
        ("/hagency/api/v1/management/requests/req_1/decide".into(), json!({"admission":{"contribution":{"id":"con_1","state":"rejected"}}})),
    ]);
    let package = ManagementPackage {
        digest: "c".repeat(64),
        instance: 1,
        account_epoch: 1,
        permissions: vec![
            service::Permission::ReadConnections,
            service::Permission::ReadResources,
            service::Permission::ReadInbox,
            service::Permission::DecideInbox,
            service::Permission::ReadIntents,
        ],
    };
    let result = f.owner.management(package).await;
    if let Some(path) = old {
        assert_eq!(result.err().unwrap().code, "device_setup_required");
        assert!(
            !path.join("server-management.sqlite3").exists(),
            "never split original command custody"
        );
    } else {
        let manager = result.unwrap();
        assert!(matches!(manager.store, ManagementStore::Server(_)));
        let session = manager
            .central(ManagementCommand::Session {})
            .await
            .unwrap();
        assert_eq!(session["account"], "@owner:test");
        assert_eq!(session["canAdmitResources"], true);
        manager
            .central(ManagementCommand::Inbox {
                after: None,
                needs_my_action: false,
                include_projects: false,
                include_allocations: false,
            })
            .await
            .unwrap();
        assert_eq!(
            manager
                .resources(Default::default())
                .await
                .unwrap_err()
                .code,
            "device_setup_required"
        );
        manager
            .central(ManagementCommand::Decide {
                command_id: "admin_decision".into(),
                request_id: "req_1".into(),
                expected_revision: 1,
                approve: false,
            })
            .await
            .unwrap();
        let (context, access) = manager.authorized().await.unwrap();
        let saved = access.pending_intents(&context).await.unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].command_id, "admin_decision");
        assert!(saved[0].reply.is_some());
        if let Some((directory, profile)) = &empty {
            assert!(!directory.join("server-management.sqlite3").exists());
            let db = rusqlite::Connection::open_with_flags(
                directory.join("domain.sqlite3"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            assert_eq!(
                db.query_row("SELECT profile_digest FROM management_identity", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
                *profile
            );
            assert_eq!(
                db.query_row("SELECT command_id FROM management_intents", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "admin_decision"
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM resources", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                0
            );
            // SDK revocation still fences an existing server-only manager.
            f.live.store(false, Ordering::Release);
            assert!(
                manager
                    .central(ManagementCommand::Session {})
                    .await
                    .is_err()
            );
            f.live.store(true, Ordering::Release);
        }
        manager.retire();
        assert_eq!(
            manager
                .central(ManagementCommand::Session {})
                .await
                .unwrap_err()
                .code,
            "management_access_required"
        );
    }
    assert_eq!(std::fs::read(marker).unwrap(), original);
    assert!(matches!(
        hagency_platform::legacy_writer::LegacyWriterGuard::acquire(),
        Err(hagency_platform::legacy_writer::Error::Refused)
    ));
    f.stop().await;
}
