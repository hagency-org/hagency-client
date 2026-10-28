use super::*;
use hagency_store::installation_authority::LegacyResourceLease;
pub(super) type Ready = (
    Fixture,
    OwnerResourceRegistry,
    SourceQuiescence,
    Arc<QuiescedSources>,
    NativeEmptyDeviceInitialization,
    std::path::PathBuf,
);
pub(super) async fn ready(personal: bool) -> Ready {
    let f = Fixture::new().await;
    if personal {
        let (ledger, _) = f.personal();
        drop(ledger);
    }
    let (work, registry, _live, path) = prepared(&f, true).await;
    let review = work.review_empty_device().await.unwrap();
    let source = work.close_and_drain().await.unwrap();
    let init = source
        .prepare_empty_device_initialization("original_setup".into(), &review)
        .await
        .unwrap();
    drop(review);
    (f, registry, work, source, init, path)
}
fn app_id(path: &Path) -> i32 {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    db.pragma_query_value(None, "application_id", |r| r.get(0))
        .unwrap()
}
fn phase(f: &Fixture) -> i64 {
    let db = rusqlite::Connection::open_with_flags(
        f.temp.path().join("registry/domain.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    db.query_row(
        "SELECT phase FROM owner_resource_adoptions WHERE id='original_setup'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}
pub(super) fn backup_bytes(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut rows = std::fs::read_dir(path)
        .unwrap()
        .map(|r| {
            let r = r.unwrap();
            (
                r.file_name().to_string_lossy().into(),
                std::fs::read(r.path()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    rows.sort();
    rows
}
#[tokio::test]
async fn native_optional_source_fence_keeps_original_backup_and_permanent_refusal() {
    for present in [false, true] {
        let (f, registry, work, source, init, path) = ready(present).await;
        assert_eq!(init.summary().await.unwrap().phase, "prepared");
        assert_eq!(app_id(&path.join("domain.sqlite3")), 0x48414732);
        let mut original_files = vec![("domain", path.join("domain.sqlite3"))];
        if present {
            original_files.push(("personal", path.parent().unwrap()
                .join(format!("owner_{}",source.profile)).join("agent-local.sqlite")));
        }
        let mut original_bytes=Vec::new();
        for (prefix,file) in original_files {
            original_bytes.push((format!("{prefix}.sqlite"),std::fs::read(&file).unwrap()));
            let wal=file.with_file_name(format!("{}-wal",file.file_name().unwrap().to_str().unwrap()));
            if wal.exists() {original_bytes.push((format!("{prefix}.sqlite-wal"),std::fs::read(wal).unwrap()));}
        }
        assert_eq!(init.fence_sources().await.unwrap().phase, "sources_fenced");
        assert_eq!(init.summary().await.unwrap().phase, "sources_fenced");
        assert_eq!(app_id(&path.join("domain.sqlite3")), 0x48415231);
        let backup = path.join("adoption_original_setup");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(backup.join("backup.json")).unwrap()).unwrap();
        assert_eq!(manifest["version"], 2);
        assert_eq!(manifest["personalPresent"], present);
        // Inspect backup bytes directly: opening a WAL backup in SQLite could
        // create sidecars and would change the evidence under observation.
        for (name,bytes) in original_bytes {
            assert!(std::fs::read(backup.join(&name)).unwrap()==bytes,"backup differs from original {name}");
        }
        assert_eq!(backup.join("personal.sqlite").exists(), present);
        let before = backup_bytes(&backup);
        init.fence_sources().await.unwrap();
        assert!(
            before == backup_bytes(&backup),
            "retry changed original backup bytes"
        );
        for table in [
            "managed_accounts",
            "resources",
            "seats",
            "owner_capacity_grants",
        ] {
            assert_eq!(count(f.temp.path(), table), 0);
        }
        assert!(f.owner.management(package()).await.is_err());
        drop(init);
        drop(source);
        drop(work);
        assert!(matches!(
            LegacyResourceLease::isolated_for_tests(
                &f.temp.path().join("installation"),
                Instant::now() + Duration::from_secs(10),
                || true
            ), Err(hagency_store::Error::LocalAuthority)),
            "permanent refusal must reject the legacy writer after all native locks are released"
        );
        registry.shutdown().await.unwrap();
        f.stop().await;
    }
}
#[tokio::test]
async fn physical_fence_with_lost_canonical_reply_recovers_same_original_after_restart() {
    let (f, registry, work, source, init, path) = ready(false).await;
    let state = f.owner.0.host.state_directory().to_owned();
    let descriptor = std::fs::read(state.join(DESCRIPTOR)).unwrap();
    // Complete physical custody and lose the returned proof before the
    // canonical phase write. The original plan is still at prepared phase.
    let proof = source
        .files
        .lock()
        .unwrap()
        .fence_empty_device(
            &init.transfer,
            registry.id(),
            &source.profile,
            &source.gate,
            &source.lease,
        )
        .unwrap();
    drop(proof);
    assert_eq!(phase(&f), 0);
    let backups = backup_bytes(&path.join("adoption_original_setup"));
    drop(init);
    drop(source);
    drop(work);
    let f = f
        .restart_for_adoption(registry.clone(), "original_setup".into())
        .await;
    let work = reopened_source(&f, registry.clone()).await;
    let source = work.close_and_drain().await.unwrap();
    let init = source
        .resume_empty_device_initialization("original_setup".into())
        .await
        .unwrap();
    assert_eq!(init.summary().await.unwrap().phase, "prepared");
    init.fence_sources().await.unwrap();
    assert_eq!(phase(&f), 1);
    assert!(
        backups == backup_bytes(&path.join("adoption_original_setup")),
        "restart replaced original backup"
    );
    assert_eq!(std::fs::read(state.join(DESCRIPTOR)).unwrap(), descriptor);
    drop(init);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}
#[tokio::test]
async fn invalid_backup_blocks_fencing_before_legacy_refusal_or_source_changes() {
    let (f, registry, work, source, init, path) = ready(false).await;
    let backup = path.join("adoption_original_setup");
    hagency_store::private::directory(&backup).unwrap();
    hagency_store::private::write_new(
        &backup.join("backup.json"),
        b"retained invalid original backup",
    )
    .unwrap();
    assert!(init.fence_sources().await.is_err());
    assert_eq!(phase(&f), 0);
    assert_eq!(app_id(&path.join("domain.sqlite3")), 0x48414732);
    assert_eq!(
        std::fs::read(backup.join("backup.json")).unwrap(),
        b"retained invalid original backup"
    );
    assert!(
        !f.temp
            .path()
            .join("installation/legacy-resource-refusal.json")
            .exists()
    );
    drop(init);
    drop(source);
    drop(work);
    let lease = LegacyResourceLease::isolated_for_tests(
        &f.temp.path().join("installation"),
        Instant::now() + Duration::from_secs(10),
        || true,
    )
    .unwrap();
    drop(lease);
    registry.shutdown().await.unwrap();
    f.stop().await;
}
#[tokio::test]
async fn corrupted_retained_backup_is_never_replaced_from_already_fenced_sources() {
    let (f, registry, work, source, init, path) = ready(false).await;
    init.fence_sources().await.unwrap();
    let backup = path.join("adoption_original_setup/domain.sqlite");
    let mut original = std::fs::read(&backup).unwrap();
    original.push(7);
    hagency_store::private::replace(&backup, &original).unwrap();
    assert!(init.fence_sources().await.is_err());
    assert!(
        std::fs::read(backup).unwrap() == original,
        "failed retry rewrote retained evidence"
    );
    assert_eq!(phase(&f), 1);
    drop(init);
    drop(source);
    drop(work);
    registry.shutdown().await.unwrap();
    f.stop().await;
}
