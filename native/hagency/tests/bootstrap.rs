//! Direct historical SDK config validation, plus actual OwnerHost bootstrap gates.
//! No historical executable/CLI/Fleet runner is constructed by these tests.
#[path = "bootstrap/accounts.rs"]
mod accounts;
#[path = "bootstrap/fixture.rs"]
mod fixture;
#[path = "owner_cli/mod.rs"]
mod owner;
#[path = "bootstrap/scope.rs"]
mod scope;
#[allow(dead_code)]
#[path = "support/approval_enrollment.rs"]
mod support;
use fixture::*;
use serde_json::json;
#[tokio::test]
async fn native_configured_fleet_profile() {
    use hagency_store::private;
    for kind in [
        "token",
        "appservice",
        "missing_approval",
        "missing_home",
        "account_only",
        "profile",
        "extra",
        "idle_zero",
        "idle_large",
        "foreign_approval",
        "no_intake",
    ] {
        let f = Fixture::new(false).await;
        let base = f.root.path().canonicalize().unwrap();
        let homes = base.join("fleet-homes");
        let source = base.join("fleet-source");
        private::directory(&homes).unwrap();
        private::directory(&source).unwrap();
        let mut config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(f.state_dir.join("development-driver.json")).unwrap(),
        )
        .unwrap();
        let registration = hagency_store::DomainRepository::open(&f.state_dir)
            .unwrap()
            .provisioning_registration_for_engagement(
                config["matrix"]["engagement_id"].as_str().unwrap(),
            )
            .unwrap();
        let fingerprint = hagency_core::canonical::digest(&json!(&registration)).unwrap();
        let peer = support::crypto::Peer::new().await;
        let anchors = json!([{"user_id":"@owner:example.test","master_key":peer.anchor()}]);
        config["profile"] = json!("codex_app_server_agent_v1");
        config["intake_sessions"] = json!(["session"]);
        config["factory_service"] =
            json!({"profile":"inline_factory_service_checkpoint_v1","idle_ms":10_000});
        config["matrix"]["registration_fingerprint"] = json!(fingerprint);
        config["matrix"]["token_provisioning"] = json!({"profile":"registration_token_home_rooms_enrollment_step_v1","peer_masters":anchors,
            "home":{"root":homes,"task_client":std::path::PathBuf::from(env!("CARGO_BIN_EXE_hagency")).canonicalize().unwrap(),
                "projects":[{"project_id":"project_provision","source":source,"mode":"copy"}]}});
        config["approval"] = json!({"origin":f.fake.endpoint,"server_name":registration.server_name,"registration_fingerprint":fingerprint,
            "engagement_id":config["matrix"]["engagement_id"],"registration_generation":1,"transport_generation":1,
            "sender_mxid":registration.approval_bot_mxid,"device_id":"APPROVAL_DEVICE",
            "rooms":[{"id":"!private:example.test","generation":1,"privacy":{"kind":"direct","human_mxid":"@owner:example.test"}}],"peer_masters":anchors});
        match kind {
            "appservice" => {
                config["matrix"]["token_provisioning"]["profile"] =
                    json!("appservice_login_home_rooms_enrollment_step_v1");
                config["matrix"]["token_provisioning"]["namespace_prefix"] =
                    json!(format!("{}_", registration.fleet_id));
            }
            "missing_approval" => {
                config.as_object_mut().unwrap().remove("approval");
            }
            "missing_home" => {
                config["matrix"]["token_provisioning"]
                    .as_object_mut()
                    .unwrap()
                    .remove("home");
            }
            "account_only" => {
                config["matrix"]["token_provisioning"] =
                    json!({"profile":"registration_token_account_step_v1"})
            }
            "profile" => config["factory_service"]["profile"] = json!("complete_native_fleet"),
            "extra" => config["factory_service"]["ready"] = json!(true),
            "idle_zero" => config["factory_service"]["idle_ms"] = json!(0),
            "idle_large" => config["factory_service"]["idle_ms"] = json!(1_200_001),
            "foreign_approval" => {
                config["approval"]["registration_fingerprint"] = json!("0".repeat(64))
            }
            "no_intake" => config["intake_sessions"] = json!([]),
            _ => {}
        }
        private::write_new(
            &f.state_dir.join("agent-driver.json"),
            &serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        for (name, bytes) in [
            (
                "matrix.registration_token",
                b"synthetic-registration-token".as_slice(),
            ),
            (
                "matrix.appservice_token",
                b"synthetic-fixed-side-token".as_slice(),
            ),
            (
                "matrix.representative_token",
                b"synthetic-separate-representative".as_slice(),
            ),
            ("matrix.provisioning_key", &[73; 32]),
            (
                "approval.access_token",
                b"synthetic-approval-token".as_slice(),
            ),
            ("approval.sdk_key", &[85; 32]),
        ] {
            private::write_new(&f.state_dir.join(name), bytes).unwrap();
        }
        let result = hagency::bootstrap::Bootstrap::open_with_options(
            &f.state_dir,
            f.address,
            16,
            hagency::bootstrap::Options {
                agent_driver: true,
                ..Default::default()
            },
        );
        if matches!(kind, "token" | "appservice") {
            let mut service = result.unwrap();
            service.close().await.unwrap();
            assert!(f.state_dir.join("factory-task-contexts").is_dir());
        } else {
            assert!(
                matches!(result, Err(hagency::bootstrap::Failure::Config { .. })),
                "{kind}"
            );
        }
        assert_eq!(
            f.fake.requests(),
            0,
            "configuration must not create physical/Matrix effects"
        );
        assert_eq!(f.attempts(), 0);
        assert_eq!(std::fs::read_dir(homes).unwrap().count(), 0);
        f.fake.close().await;
    }
}

#[test]
fn native_owner_bootstrap_refuses_old_driver_and_matrix_secret_files() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = owner::assets(root.path());
    for name in [
        "agent-driver.json",
        "development-driver.json",
        "matrix.access_token",
        "matrix.appservice_token",
        "approval.access_token",
    ] {
        let state = root.path().join(name.replace('.', "_"));
        let out = owner::command()
            .args(["init", "--state-dir"])
            .arg(&state)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::write(state.join(name), b"never import old settings or secrets").unwrap();
        let out = owner::command()
            .args(["serve", "--state-dir"])
            .arg(&state)
            .arg("--console-assets")
            .arg(&bundle)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(
            std::fs::read(state.join(name)).unwrap(),
            b"never import old settings or secrets"
        );
        assert!(!state.join("domain.sqlite3").exists());
    }
}
#[test]
fn native_owner_bootstrap_no_anonymous_provider_model_tool_or_approval_access() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = owner::assets(root.path());
    let run = owner::launch(&root.path().join("state"), &bundle, true);
    for path in [
        "/console/api/owner-provider",
        "/console/api/owned-agents/agt_test/runtime/start",
        "/console/api/owned-agents/agt_test/runtime/approvals/nonce/decision",
        "/console/api/matrix-creations",
    ] {
        let response = owner::request(run.address, "POST", path, "{}", None);
        assert_eq!(owner::status(&response), 401, "{path}");
    }
}
#[test]
fn native_owner_bootstrap_restart_does_not_restore_owner_session() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = owner::assets(root.path());
    let state = root.path().join("state");
    let mut first = owner::launch(&state, &bundle, true);
    first.child.kill().unwrap();
    first.child.wait().unwrap();
    drop(first);
    let second = owner::launch(&state, &bundle, true);
    assert_eq!(
        owner::status(&owner::request(
            second.address,
            "GET",
            "/console/api/owned-agents",
            "",
            None
        )),
        401
    );
    assert!(!state.join("console-logins.json").exists());
}
