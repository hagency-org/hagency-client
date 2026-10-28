//! OwnerHost foreground supervisor contract. Historical custody/domain rows
//! are tested in SDK libraries and are never imported by this executable.
#[path = "owner_cli/mod.rs"]
mod owner;
use owner::*;
#[test]
fn native_launchd_agent_starts_and_reports_ready() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = assets(root.path());
    let run = launch(&root.path().join("state"), &bundle, true);
    assert_eq!(
        status(&request(run.address, "GET", "/ready", "", None)),
        200
    );
    assert_eq!(
        status(&request(run.address, "GET", "/console/login/", "", None)),
        200
    );
}
#[test]
fn native_launchd_agent_stops_cleanly() {
    #[cfg(unix)]
    {
        let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
        let bundle = assets(root.path());
        let mut run = launch(&root.path().join("state"), &bundle, true);
        stop_clean(&mut run);
    }
    #[cfg(not(unix))]
    {
        let out = command().args(["open", "--no-open"]).output().unwrap();
        assert!(
            !out.status.success(),
            "OS private access IPC is unavailable on this platform"
        );
    }
}
#[test]
fn native_launchd_restart_preserves_owner_state() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let bundle = assets(root.path());
    let state = root.path().join("state");
    let mut run = launch(&state, &bundle, true);
    let original = marker(&state);
    #[cfg(unix)]
    stop_clean(&mut run);
    #[cfg(not(unix))]
    {
        run.child.kill().unwrap();
        run.child.wait().unwrap();
    }
    drop(run);
    let run = launch(&state, &bundle, true);
    assert_eq!(marker(&state), original);
    assert_eq!(
        status(&request(
            run.address,
            "GET",
            "/console/api/owned-agents",
            "",
            None
        )),
        401,
        "restart cannot recover a Pasion session or authorize inference"
    );
    assert!(!state.join("domain.sqlite3").exists());
}
