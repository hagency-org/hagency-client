//! OwnerHost offline recovery exercises. No retired backup/restore/rotate CLI,
//! operator token, Fleet database, live SQLite copying or model execution.
#[path = "owner_cli/mod.rs"]
mod owner;
#[path = "release_state/mod.rs"]
mod release_state;
use std::{fs, io, path::Path};

/// Test procedure, not a production backup API: hold the existing owner lock,
/// require a fresh destination, and copy every regular private file recursively.
fn offline_snapshot(source: &Path, destination: &Path) -> io::Result<()> {
    let lock = hagency_store::private::open(&source.join(".owner-client.lock"), false)
        .map_err(io::Error::other)?;
    lock.try_lock()?;
    fn copy(source: &Path, destination: &Path) -> io::Result<()> {
        // create_new directory semantics protect an existing backup/owner state.
        hagency_store::private::create_directory_new(destination).map_err(io::Error::other)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                return Err(io::Error::other("snapshot contains symlink"));
            }
            let target = destination.join(entry.file_name());
            if meta.is_dir() {
                copy(&entry.path(), &target)?;
            } else if meta.is_file() {
                fs::copy(entry.path(), &target)?;
            } else {
                return Err(io::Error::other(
                    "snapshot contains non-regular file; stop owner host first",
                ));
            }
        }
        Ok(())
    }
    copy(source, destination)
}
fn initialize(state: &Path) {
    let out = owner::command()
        .args(["init", "--state-dir"])
        .arg(state)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn native_ops_offline_owner_snapshot_preserves_unknown_and_private_state() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("state");
    initialize(&state);
    release_state::seed_owned_state(&state);
    let marker = owner::marker(&state);
    let assets = owner::assets(root.path());
    let mut run = owner::launch(&state, &assets, false);
    let blocked = root.path().join("while-running");
    assert!(offline_snapshot(&state, &blocked).is_err());
    assert!(!blocked.exists());
    owner::stop_clean(&mut run);
    drop(run);
    let snapshot = root.path().join("snapshot");
    offline_snapshot(&state, &snapshot).unwrap();
    let restored = root.path().join("restored");
    offline_snapshot(&snapshot, &restored).unwrap();
    assert_eq!(owner::marker(&restored), marker);
    release_state::assert_owned_survives(&restored);
    let mut run = owner::launch(&restored, &assets, false);
    assert_eq!(
        owner::status(&owner::request(run.address, "GET", "/ready", "", None)),
        200
    );
    for retired in ["/api/native/v1/custody", "/api/native/v1/resources"] {
        assert_eq!(
            owner::status(&owner::request(run.address, "GET", retired, "", None)),
            404
        );
    }
    owner::stop_clean(&mut run);
    release_state::assert_owned_survives(&restored);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&restored).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(restored.join("hagency-client-owned-v1.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn native_ops_offline_snapshot_refuses_clobber() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("state");
    initialize(&state);
    let marker = owner::marker(&state);
    let snapshot = root.path().join("snapshot");
    offline_snapshot(&state, &snapshot).unwrap();
    let original = owner::marker(&snapshot);
    assert!(offline_snapshot(&state, &snapshot).is_err());
    assert!(offline_snapshot(&snapshot, &state).is_err());
    assert_eq!(owner::marker(&state), marker);
    assert_eq!(owner::marker(&snapshot), original);
}
#[test]
fn native_ops_retired_commands_cannot_mint_or_rotate_credentials() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("state");
    initialize(&state);
    let marker = owner::marker(&state);
    for verb in ["backup", "restore", "rotate"] {
        let out = owner::command()
            .arg(verb)
            .arg("--state-dir")
            .arg(&state)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("unrecognized subcommand"));
        assert_eq!(owner::marker(&state), marker);
    }
    assert!(!state.join("operator.token").exists());
    assert!(!state.join("domain.sqlite3").exists());
    let empty = root.path().join("empty");
    fs::create_dir(&empty).unwrap();
    let out = owner::command()
        .args(["rotate", "--state-dir"])
        .arg(&empty)
        .arg("operator-token")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);
}
