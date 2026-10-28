//! Historical setup SDK validation is independent of the owner executable.
//! Production has no setup command and never imports ambient Codex auth.
#![cfg(unix)]
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
fn paths(root: &Path) -> (PathBuf, PathBuf) {
    let codex = root.join("fixture-native-codex");
    std::fs::write(&codex, b"\xcf\xfa\xed\xfe fake native codex").unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o700)).unwrap();
    let home = root.join("fixture-provider-home");
    hagency_store::private::directory(&home).unwrap();
    (codex, home)
}
fn options(root: &Path, no_local_codex: bool) -> hagency::setup::Options {
    let (codex, home) = paths(root);
    hagency::setup::Options {
        state_dir: root.join("sdk-state"),
        listen: "127.0.0.1:13300".parse().unwrap(),
        codex: Some(codex),
        codex_home: Some(home),
        no_local_codex,
        force: false,
    }
}
#[test]
fn native_setup_sdk_materializes_explicit_private_profile_and_requires_explicit_replace() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let mut config = options(&root, false);
    let first = hagency::setup::run(&config).unwrap();
    assert!(first.initialized);
    assert!(!first.signed_in);
    assert!(first.local_codex);
    assert_eq!(
        std::fs::metadata(&first.runtime_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let before = std::fs::read(&first.runtime_file).unwrap();
    assert!(
        hagency::setup::run(&config)
            .unwrap_err()
            .contains("--force")
    );
    assert_eq!(std::fs::read(&first.runtime_file).unwrap(), before);
    config.force = true;
    hagency::setup::run(&config).unwrap();
    assert!(std::fs::read_dir(&config.state_dir).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("fleet-runtime.json.bak-")
    }));
}
#[test]
fn native_setup_sdk_refuses_foreign_state_without_import() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = options(&root, true);
    std::fs::create_dir(&config.state_dir).unwrap();
    std::fs::write(config.state_dir.join("foreign.txt"), b"original").unwrap();
    assert!(hagency::setup::run(&config).is_err());
    assert_eq!(
        std::fs::read(config.state_dir.join("foreign.txt")).unwrap(),
        b"original"
    );
}
#[test]
fn native_setup_sdk_can_use_dedicated_unsigned_provider_home() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = options(&root, true);
    let result = hagency::setup::run(&config).unwrap();
    assert!(!result.local_codex);
    assert!(!result.signed_in);
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(result.runtime_file).unwrap()).unwrap();
    assert!(json.get("local_codex").is_none());
    assert!(!config.state_dir.join("runtime-home/auth.json").exists());
}
