use super::*;

#[test]
fn shared_lifetime_custody_blocks_exclusive_until_last_connection_guard_drops() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("authority");
    let a = LegacyWriterGuard::isolated_for_tests(&root).unwrap();
    let b = LegacyWriterGuard::isolated_for_tests(&root).unwrap();
    assert!(matches!(
        ExclusiveWriterGuard::isolated_for_tests(&root),
        Err(Error::Locked)
    ));
    drop(a);
    assert!(matches!(
        ExclusiveWriterGuard::isolated_for_tests(&root),
        Err(Error::Locked)
    ));
    drop(b);
    let exclusive = ExclusiveWriterGuard::isolated_for_tests(&root).unwrap();
    assert!(matches!(
        LegacyWriterGuard::isolated_for_tests(&root),
        Err(Error::Locked)
    ));
    exclusive.check().unwrap();
    assert!(!root.join(REFUSAL).exists());
}
#[test]
fn guarded_process_child() {
    let Some(root) = std::env::var_os("HAGENCY_GUARD_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    assert!(matches!(
        ExclusiveWriterGuard::isolated_for_tests(&root),
        Err(Error::Locked)
    ));
    LegacyWriterGuard::isolated_for_tests(&root).unwrap();
}
#[test]
fn lifetime_shared_guard_excludes_an_actual_second_process() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("authority");
    let _guard = LegacyWriterGuard::isolated_for_tests(&root).unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["legacy_writer::tests::guarded_process_child", "--exact"])
        .env("HAGENCY_GUARD_TEST_ROOT", &root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn exact_or_malformed_refusal_never_becomes_legacy_authority() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("authority");
    drop(ExclusiveWriterGuard::isolated_for_tests(&root).unwrap());
    private::write_new(&root.join(REFUSAL), b"{}").unwrap();
    assert!(matches!(
        LegacyWriterGuard::isolated_for_tests(&root),
        Err(Error::Quarantined)
    ));
    let data=serde_json::to_vec(&serde_json::json!({"version":1,"registryId":"a".repeat(64),"migrationId":"original","inventoryDigest":"b".repeat(64)})).unwrap();
    private::replace(&root.join(REFUSAL), &data).unwrap();
    assert!(matches!(
        LegacyWriterGuard::isolated_for_tests(&root),
        Err(Error::Refused)
    ));
    let exclusive = ExclusiveWriterGuard::isolated_for_tests(&root).unwrap();
    assert_eq!(exclusive.refusal_registry(), Some("a".repeat(64).as_str()));
    assert_eq!(std::fs::read(root.join(REFUSAL)).unwrap(), data);
}
#[test]
fn retained_exclusive_custody_rejects_replaced_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("authority");
    let guard = ExclusiveWriterGuard::isolated_for_tests(&root).unwrap();
    std::fs::rename(root.join(LOCK), root.join("old.lock")).unwrap();
    private::write_new(&root.join(LOCK), b"").unwrap();
    assert!(matches!(guard.check(), Err(Error::Private)));
}

#[cfg(feature = "resource-adoption")]
#[test]
fn permanent_refusal_publishes_once_recovers_pending_and_denies_legacy() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("authority");
    let exclusive = ExclusiveWriterGuard::isolated_for_tests(&root).unwrap();
    let data = serde_json::to_vec(&Refusal {
        version: 1,
        registry_id: "a".repeat(64),
        migration_id: "original".into(),
        inventory_digest: "b".repeat(64),
    })
    .unwrap();
    private::write_new(&root.join("legacy-resource-refusal.pending"), &data).unwrap();
    exclusive
        .publish_refusal(&"a".repeat(64), "original", &"b".repeat(64))
        .unwrap();
    exclusive
        .publish_refusal(&"a".repeat(64), "original", &"b".repeat(64))
        .unwrap();
    assert!(
        exclusive
            .publish_refusal(&"a".repeat(64), "replacement", &"b".repeat(64))
            .is_err()
    );
    private::check_handle(&File::open(root.join(REFUSAL)).unwrap()).unwrap();
    assert!(!root.join("legacy-resource-refusal.pending").exists());
    assert_eq!(std::fs::read(root.join(REFUSAL)).unwrap(), data);
    drop(exclusive);
    assert!(matches!(
        LegacyWriterGuard::isolated_for_tests(&root),
        Err(Error::Refused)
    ));
    let replay = ExclusiveWriterGuard::isolated_for_tests(&root).unwrap();
    replay
        .publish_refusal(&"a".repeat(64), "original", &"b".repeat(64))
        .unwrap();
}
