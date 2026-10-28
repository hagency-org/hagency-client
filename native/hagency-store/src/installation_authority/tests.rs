use super::*;

fn until() -> Instant {
    Instant::now() + Duration::from_secs(60)
}
fn shared(root: &Path) -> Result<LegacyResourceLease, Error> {
    LegacyResourceLease::at(root, until(), Arc::new(|| true))
}
fn exclusive(root: &Path) -> Result<AdoptionExclusiveLease, Error> {
    AdoptionExclusiveLease::at(root, "a".repeat(64), until(), Arc::new(|| true))
}

#[test]
fn retired_queued_lease_retains_exclusion_until_last_copy_is_dropped() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registry");
    let first = shared(&root).unwrap();
    let other_profile = shared(&root).unwrap();
    let queued = first.clone();
    assert!(matches!(exclusive(&root), Err(Error::Locked)));
    first.retire();
    assert!(matches!(queued.check(), Err(Error::LocalAuthority)));
    drop(first);
    drop(other_profile);
    assert!(matches!(exclusive(&root), Err(Error::Locked)));
    drop(queued);
    let migration = exclusive(&root).unwrap();
    migration.check().unwrap();
    assert_eq!(migration.registry_id(), "a".repeat(64));
    assert!(matches!(shared(&root), Err(Error::Locked)));
    drop(migration);
    shared(&root).unwrap().check().unwrap();
    assert!(
        !root.join("domain.sqlite3").exists(),
        "No resource ledger initialized"
    );
    assert!(
        !root.join(REFUSAL).exists(),
        "Proof-only lease must not fence legacy sources"
    );
}

#[test]
fn lease_subprocess() {
    let Some(root) = std::env::var_os("HAGENCY_RESOURCE_LEASE_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    assert!(matches!(shared(&root), Err(Error::Locked)));
    assert!(matches!(exclusive(&root), Err(Error::Locked)));
}

#[test]
fn exclusive_lock_denies_a_second_actual_process_and_reopens_after_drop() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registry");
    let migration = exclusive(&root).unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "installation_authority::tests::lease_subprocess",
            "--exact",
            "--nocapture",
        ])
        .env("HAGENCY_RESOURCE_LEASE_FIXTURE", &root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    drop(migration);
    shared(&root).unwrap().check().unwrap();
}

#[test]
fn refusal_is_permanent_legacy_denial_and_never_a_new_registry_mapping() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registry");
    drop(exclusive(&root).unwrap());
    let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"registryId":"a".repeat(64),"migrationId":"migration_original","inventoryDigest":"b".repeat(64)})).unwrap();
    private::write_new(&root.join(REFUSAL), &bytes).unwrap();
    assert!(matches!(shared(&root), Err(Error::LocalAuthority)));
    let recovery = exclusive(&root).unwrap();
    recovery.check().unwrap();
    drop(recovery);
    assert!(matches!(
        AdoptionExclusiveLease::at(&root, "c".repeat(64), until(), Arc::new(|| true)),
        Err(Error::Conflict)
    ));
    assert_eq!(std::fs::read(root.join(REFUSAL)).unwrap(), bytes);
    assert!(!root.join("domain.sqlite3").exists());
}

#[test]
fn corrupt_refusal_never_means_fresh_legacy_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registry");
    drop(exclusive(&root).unwrap());
    private::write_new(&root.join(REFUSAL), b"{\"version\":").unwrap();
    assert!(matches!(shared(&root), Err(Error::Quarantined)));
    assert!(matches!(exclusive(&root), Err(Error::Quarantined)));
    assert_eq!(std::fs::read(root.join(REFUSAL)).unwrap(), b"{\"version\":");
}

#[test]
fn finite_native_gates_retire_without_releasing_a_held_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registry");
    let live = Arc::new(AtomicBool::new(true));
    let gate = live.clone();
    let migration = AdoptionExclusiveLease::at(
        &root,
        "a".repeat(64),
        until(),
        Arc::new(move || gate.load(Ordering::Acquire)),
    )
    .unwrap();
    live.store(false, Ordering::Release);
    assert!(matches!(migration.check(), Err(Error::LocalAuthority)));
    assert!(matches!(shared(&root), Err(Error::Locked)));
    drop(migration);
    assert!(matches!(
        LegacyResourceLease::at(&root, Instant::now(), Arc::new(|| true)),
        Err(Error::LocalAuthority)
    ));
    assert!(matches!(
        LegacyResourceLease::at(&root, until(), Arc::new(|| false)),
        Err(Error::LocalAuthority)
    ));
    shared(&root).unwrap().check().unwrap();
}

#[test]
fn replaced_lock_is_not_accepted_as_retained_exclusion() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registry");
    let migration = exclusive(&root).unwrap();
    std::fs::rename(root.join(LOCK), root.join("original.lock")).unwrap();
    private::write_new(&root.join(LOCK), b"").unwrap();
    assert!(matches!(migration.check(), Err(Error::Private)));
}
