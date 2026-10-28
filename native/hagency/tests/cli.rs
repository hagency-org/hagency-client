//! Release entry point coverage. Historical Fleet/domain behavior remains in
//! SDK tests; this executable exclusively constructs the new OwnerHost.
#[path = "owner_cli/mod.rs"]
mod owner;
use owner::*;
use std::fs;
#[test]
fn native_owner_oauth_return_documents_allow_external_navigation_but_not_api_reads() {
    use std::io::{Read, Write};
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("state");
    let init = command()
        .args(["init", "--state-dir"])
        .arg(&state)
        .output()
        .unwrap();
    assert!(init.status.success());
    let bundle = assets(root.path());
    let run = launch(&state, &bundle, false);
    let navigate = |path: &str, site: &str| {
        let mut socket = std::net::TcpStream::connect(run.address).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        write!(socket, "GET {path} HTTP/1.1\r\nHost: {}\r\nSec-Fetch-Site: {site}\r\nSec-Fetch-Mode: navigate\r\nSec-Fetch-Dest: document\r\nConnection: close\r\n\r\n", run.address).unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        response
    };
    for site in ["same-site", "cross-site"] {
        for path in ["/console/", "/console/login/", "/console/agents-owned/"] {
            let response = navigate(path, site);
            assert_eq!(status(&response), 200, "{site} {path}: {response}");
            assert!(response.contains("owner CLI fixture"));
        }
        let response = navigate("/console/server-login", site);
        assert_eq!(status(&response), 400);
        assert!(response.contains("invalid_console_request"));
        let api = navigate("/console/api/owned-agents", site);
        assert_eq!(status(&api), 401);
        assert!(api.contains("console_access_required"));
        assert_eq!(status(&navigate("/console/project-sides/", site)), 403);
        assert_eq!(status(&navigate("/console/?unexpected=1", site)), 400);
    }
}
#[test]
fn native_owner_init_is_private_idempotent_and_has_no_legacy_store() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("中文 owner state");
    for _ in 0..2 {
        let out = command()
            .args(["init", "--state-dir"])
            .arg(&state)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }
    assert!(
        String::from_utf8(marker(&state))
            .unwrap()
            .contains("hagency-owned-agent-client")
    );
    for name in ["operator.token", "domain.sqlite3", "console-logins.json"] {
        assert!(!state.join(name).exists());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&state).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(state.join("hagency-client-owned-v1.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn native_owner_rejects_retired_commands_and_options() {
    for name in [
        "account",
        "registration",
        "console-access",
        "agents",
        "tasks",
        "alerts",
        "custody",
    ] {
        let out = command().arg(name).output().unwrap();
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("unrecognized subcommand"),
            "{name}: {out:?}"
        );
    }
    // The executable also hosts the private supervisor entry, before CLI
    // parsing. Without its inherited IPC channel it must refuse quietly.
    let out = command().arg("guardian").output().unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    for flag in [
        "--agent-driver",
        "--palpo-transport",
        "--fleet",
        "--operator-token",
        "--agent-config",
    ] {
        let out = command().args(["serve", flag]).output().unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(flag));
    }
    let help = command().arg("--help").output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for entry in ["init", "serve", "start", "open", "service"] {
        assert!(help.contains(entry));
    }
}
#[test]
fn native_owner_rejects_legacy_data_without_import() {
    for name in [
        "domain.sqlite3",
        "operator.token",
        "fleet.json",
        "bootstrap.json",
    ] {
        let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
        let state = root.path().join("state");
        hagency_store::private::directory(&state).unwrap();
        fs::write(state.join(name), b"legacy never import").unwrap();
        let out = command()
            .args(["init", "--state-dir"])
            .arg(&state)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(fs::read(state.join(name)).unwrap(), b"legacy never import");
        assert!(!state.join("hagency-client-owned-v1.json").exists());
    }
}
#[test]
fn native_binary_survives_crash_without_node() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = assets(root.path());
    let state = root.path().join("state");
    let mut first = launch(&state, &bundle, false);
    let original = marker(&state);
    first.child.kill().unwrap();
    first.child.wait().unwrap();
    drop(first);
    let second = launch(&state, &bundle, false);
    assert_eq!(marker(&state), original);
    assert_eq!(
        status(&request(second.address, "GET", "/ready", "", None)),
        200
    );
    assert!(!state.join("domain.sqlite3").exists());
}
#[test]
fn native_owner_start_serves_only_owner_routes() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = assets(root.path());
    let run = launch(&root.path().join("state"), &bundle, true);
    for path in ["/console/", "/console/login/", "/console/agents-owned/"] {
        assert_eq!(
            status(&request(run.address, "GET", path, "", None)),
            200,
            "{path}"
        );
    }
    for path in [
        "/api/native/v1/custody",
        "/api/native/v1/accounts",
        "/console/setup/",
        "/console/resources/",
        "/console/api/resources",
        "/console/api/accounts",
        "/console/api/tasks",
    ] {
        assert_eq!(
            status(&request(run.address, "GET", path, "", None)),
            404,
            "{path}"
        );
    }
    assert_eq!(
        status(&request(
            run.address,
            "GET",
            "/console/api/owned-agents",
            "",
            None
        )),
        401
    );
}
#[test]
#[cfg(unix)]
fn native_owner_open_uses_private_ipc_single_use_ticket_without_pasion_rights() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = assets(root.path());
    let state = root.path().join("state");
    let run = launch(&state, &bundle, false);
    let out = command()
        .args(["open", "--no-open", "--state-dir"])
        .arg(&state)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let url = String::from_utf8(out.stdout).unwrap();
    assert!(url.starts_with(&format!("http://{}/console/login#access=", run.address)));
    let ticket = url.trim().split("#access=").nth(1).unwrap();
    assert_eq!(ticket.len(), 64);
    let body = serde_json::json!({"ticket":ticket}).to_string();
    let first = request(run.address, "POST", "/console/session", &body, None);
    assert_eq!(status(&first), 200);
    let cookie = first
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
                .map(|(_, value)| value.trim().split(';').next().unwrap())
        })
        .unwrap();
    assert_eq!(
        status(&request(
            run.address,
            "POST",
            "/console/session",
            &body,
            None
        )),
        401
    );
    assert_eq!(
        status(&request(
            run.address,
            "GET",
            "/console/api/owned-agents",
            "",
            Some(cookie)
        )),
        401,
        "Local access does not imply Pasion owner authorization"
    );
    assert_eq!(
        status(&request(
            run.address,
            "POST",
            "/api/native/v1/console-access",
            "",
            None
        )),
        404
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(state.join("owner-console.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
#[test]
fn native_owner_requires_console_and_valid_loopback_listener() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = assets(root.path());
    for address in ["0.0.0.0:13300", "127.0.0.1:0"] {
        let out = command()
            .args(["serve", "--state-dir"])
            .arg(root.path().join(address.replace(':', "_")))
            .args(["--listen", address, "--console-assets"])
            .arg(&bundle)
            .output()
            .unwrap();
        assert!(!out.status.success());
    }
    let rejected_state = root.path().join("invalid-service-state");
    let out = command()
        .args([
            "service",
            "install",
            "--listen",
            "127.0.0.1:0",
            "--state-dir",
        ])
        .arg(&rejected_state)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        !rejected_state.exists(),
        "invalid service listeners must not initialize state or install a supervisor"
    );
    let out = command()
        .args(["serve", "--state-dir"])
        .arg(root.path().join("missing-assets"))
        .args(["--console-assets", "/nonexistent/hagency-owner-assets"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Console"));
}
#[test]
fn native_owner_running_state_cannot_be_opened_twice() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = assets(root.path());
    let state = root.path().join("state");
    let _run = launch(&state, &bundle, false);
    let out = command()
        .args(["init", "--state-dir"])
        .arg(&state)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("AlreadyRunning"));
}
#[test]
fn native_logs_to_stderr_with_no_file_sink() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = assets(root.path());
    let state = root.path().join("state");
    let _run = launch(&state, &bundle, false);
    assert!(!state.join("logs").exists());
    assert!(!state.join("hagency.log").exists());
}
#[test]
#[cfg(unix)]
fn native_owner_service_install_uninstall_uses_only_per_user_supervisor() {
    use std::os::unix::fs::PermissionsExt;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = assets(root.path());
    let run = launch(&root.path().join("live-state"), &bundle, true);
    let home = root.path().join("home");
    fs::create_dir(&home).unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let log = root.path().join("supervisor-arguments");
    for name in ["launchctl", "systemctl", "id"] {
        let file = bin.join(name);
        fs::write(
            &file,
            if name == "id" {
                "#!/bin/sh\nprintf '1000\\n'\n"
            } else {
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$HAGENCY_TEST_SUPERVISOR_LOG\"\n"
            },
        )
        .unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let state = root.path().join("installed-state");
    let mut install = command();
    install
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("PATH", &bin)
        .env("HAGENCY_TEST_SUPERVISOR_LOG", &log)
        .args([
            "service",
            "install",
            "--no-open",
            "--listen",
            &run.address.to_string(),
            "--state-dir",
        ])
        .arg(&state);
    let out = install.output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let file = if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents/io.hagency.plist")
    } else {
        home.join("config/systemd/user/hagency.service")
    };
    let spec = fs::read_to_string(&file).unwrap();
    assert!(spec.contains("start"));
    assert!(spec.contains("--no-open"));
    assert!(spec.contains(state.to_str().unwrap()));
    for retired in [
        "--palpo-transport",
        "--agent-driver",
        "operator.token",
        "fleet",
    ] {
        assert!(!spec.contains(retired));
    }
    let calls = fs::read_to_string(&log).unwrap();
    if cfg!(target_os = "macos") {
        assert!(calls.contains("bootstrap"));
        assert!(calls.contains("gui/1000"));
    } else {
        assert!(calls.contains("--user"));
        assert!(calls.contains("enable"));
        assert!(calls.contains("restart"));
    }
    let out = command()
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("PATH", &bin)
        .env("HAGENCY_TEST_SUPERVISOR_LOG", &log)
        .args(["service", "uninstall"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(!file.exists());
    assert!(state.join("hagency-client-owned-v1.json").is_file());
}
#[test]
#[cfg(not(unix))]
fn native_owner_open_uses_private_ipc_single_use_ticket_without_pasion_rights() {
    let out = command().args(["open", "--no-open"]).output().unwrap();
    assert!(
        !out.status.success(),
        "owner-private OS IPC is unavailable on this platform"
    );
}
#[test]
#[cfg(not(unix))]
fn native_owner_service_install_uninstall_uses_only_per_user_supervisor() {
    let out = command()
        .args(["service", "install", "--listen", "127.0.0.1:0"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nonzero loopback"));
}

#[test]
fn native_owner_setup_command_cannot_import_daily_provider_credentials() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(home.join(".codex/auth.json"), b"synthetic never import").unwrap();
    let state = root.path().join("state");
    let out = owner::command()
        .env("HOME", &home)
        .args(["setup", "--state-dir"])
        .arg(&state)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unrecognized subcommand"));
    assert!(!state.exists());
    assert_eq!(
        std::fs::read(home.join(".codex/auth.json")).unwrap(),
        b"synthetic never import"
    );
}
#[test]
fn native_owner_start_invalid_console_creates_no_credentials() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("state");
    let out = owner::command()
        .args(["start", "--no-open", "--state-dir"])
        .arg(&state)
        .args(["--console-assets", "/nonexistent/owner-console-build"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains("--console-assets"));
    assert!(error.contains("validated private owner console"));
    for name in [
        "domain.sqlite3",
        "operator.token",
        "server-login.json",
        "owned-agent-owners",
    ] {
        assert!(!state.join(name).exists());
    }
}
