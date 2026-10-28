use super::*;
use super::super::tests::Fixture;
use rusqlite::{
    OpenFlags,
    config::DbConfig,
    hooks::{AuthAction, AuthContext, Authorization},
};
fn db(path: &Path, read: bool) -> Connection {
    let c = Connection::open_with_flags(
        path,
        if read {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        },
    )
    .unwrap();
    c.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
        .unwrap();
    c
}
pub(in super::super) fn sources(f: &Fixture, personal: bool, read: bool) -> (Connection, Option<Connection>) {
    (
        db(&f.root.path().join("source/domain.sqlite3"), read),
        personal.then(|| db(&f.root.path().join("personal.sqlite"), read)),
    )
}
pub(in super::super) async fn prepared(personal: bool) -> (Fixture, EmptyDeviceTransfer) {
    let f = Fixture::new();
    if personal {
        let path = f.root.path().join("personal.sqlite");
        crate::private::open(&path, true).unwrap();
        drop(
            hagency_agent_local::Ledger::open_scoped(&path, &f.profile.account, &f.profile.digest)
                .unwrap(),
        );
    }
    let (mut d, mut p) = sources(&f, personal, true);
    let inventory =
        EmptyDeviceInventory::inspect(&mut d, p.as_mut(), &f.profile.digest, &f.profile.account)
            .unwrap();
    drop(d);
    drop(p);
    let prepared = f
        .registry
        .empty_device_preparation(
            f.authority(),
            "original_setup".into(),
            inventory,
            "c".repeat(64),
        )
        .unwrap();
    let digest = prepared.plan_digest().unwrap();
    f.registry
        .commit_empty_device_preparation(prepared)
        .await
        .unwrap();
    let transfer = f
        .registry
        .empty_device_transfer(f.authority(), "original_setup".into(), digest)
        .await
        .unwrap();
    (f, transfer)
}
#[tokio::test]
async fn optional_original_sources_are_fenced_and_retained_without_creating_capacity() {
    for present in [false, true] {
        let (f, transfer) = prepared(present).await;
        let (mut d, mut p) = sources(&f, present, false);
        assert!(transfer.inspect_archives(&mut d, p.as_mut()).is_err());
        assert!(
            transfer
                .fence_sources(&mut d, p.as_mut(), &"d".repeat(64))
                .is_err()
        );
        transfer
            .fence_sources(&mut d, p.as_mut(), &"c".repeat(64))
            .unwrap();
        transfer
            .fence_sources(&mut d, p.as_mut(), &"c".repeat(64))
            .unwrap();
        assert_eq!(
            d.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))
                .unwrap(),
            0x48415231
        );
        drop(d);
        drop(p);
        assert_eq!(f.root.path().join("personal.sqlite").exists(), present);
        let (mut d, mut p) = sources(&f, present, true);
        assert!(
            EmptyDeviceInventory::inspect(
                &mut d,
                p.as_mut(),
                &f.profile.digest,
                &f.profile.account
            )
            .is_err(),
            "archives cannot become a fresh inventory"
        );
        let proof = transfer.inspect_archives(&mut d, p.as_mut()).unwrap();
        assert_eq!(
            f.registry
                .record_empty_device_sources(proof)
                .await
                .unwrap()
                .phase,
            "sources_fenced"
        );
        let proof = transfer.inspect_archives(&mut d, p.as_mut()).unwrap();
        assert_eq!(
            f.registry
                .record_empty_device_sources(proof)
                .await
                .unwrap()
                .phase,
            "sources_fenced"
        );
        assert!(
            f.registry
                .recover_empty_device_preparation(
                    f.authority(),
                    "original_setup".into(),
                    transfer.plan_digest().unwrap()
                )
                .await
                .is_err(),
            "fenced sources must not be reported as a new prepared operation"
        );
        for table in [
            "resources",
            "seats",
            "managed_accounts",
            "owner_resource_grants",
            "owner_capacity_grants",
        ] {
            assert_eq!(f.count(table).await, 0);
        }
        assert!(
            DomainRepository::open_managed(&f.root.path().join("source"), &f.profile.digest)
                .is_err()
        );
        // The unchanged private archive reader accepts the exact original
        // manifest. This also checks compatibility with its v1 manifest shape.
        let journal = crate::management::PrivateManagementJournal::open_existing(
            &f.root.path().canonicalize().unwrap().join("source"),
            transfer.archive_binding().unwrap(),
        )
        .unwrap();
        assert!(journal.writer_open());
        journal.shutdown().await.unwrap();
        drop(d);
        drop(p);
        drop(transfer);
        f.registry.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn original_personal_writer_transaction_can_finish_before_domain_fence() {
    let (f, transfer) = prepared(true).await;
    let (mut d, mut p) = sources(&f, true, false);
    transfer
        .inspect_source_state(&mut d, p.as_mut(), &"c".repeat(64))
        .unwrap();
    let personal = p.as_mut().unwrap();
    personal.execute_batch("BEGIN IMMEDIATE").unwrap();
    transfer.fence_locked_personal(personal).unwrap();
    assert!(personal.is_autocommit());
    assert_eq!(
        personal
            .pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))
            .unwrap(),
        0x48415231
    );
    assert_eq!(
        d.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))
            .unwrap(),
        0x48414732
    );
    transfer
        .fence_sources(&mut d, p.as_mut(), &"c".repeat(64))
        .unwrap();
    drop(d);
    drop(p);
    let (mut d, mut p) = sources(&f, true, true);
    let proof = transfer.inspect_archives(&mut d, p.as_mut()).unwrap();
    f.registry.record_empty_device_sources(proof).await.unwrap();
    drop(d);
    drop(p);
    drop(transfer);
    f.registry.shutdown().await.unwrap();
}
fn refuse_personal_marker(context: AuthContext<'_>) -> Authorization {
    if matches!(
        context.action,
        AuthAction::CreateTable {
            table_name: "personal_archive_identity"
        }
    ) {
        Authorization::Deny
    } else {
        Authorization::Allow
    }
}
#[tokio::test]
async fn interrupted_partial_fence_keeps_original_plan_and_retries_exactly() {
    let (f, transfer) = prepared(true).await;
    let (mut d, mut p) = sources(&f, true, false);
    p.as_ref()
        .unwrap()
        .authorizer(Some(refuse_personal_marker))
        .unwrap();
    assert!(
        transfer
            .fence_sources(&mut d, p.as_mut(), &"c".repeat(64))
            .is_err()
    );
    assert_eq!(
        d.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))
            .unwrap(),
        0x48415231
    );
    assert_eq!(
        p.as_ref()
            .unwrap()
            .pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))
            .unwrap(),
        0x48414c32
    );
    let (mut ro_d, mut ro_p) = sources(&f, true, true);
    assert!(transfer.inspect_archives(&mut ro_d, ro_p.as_mut()).is_err());
    drop(ro_d);
    drop(ro_p);
    p.as_ref()
        .unwrap()
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    transfer
        .fence_sources(&mut d, p.as_mut(), &"c".repeat(64))
        .unwrap();
    drop(d);
    drop(p);
    let (mut d, mut p) = sources(&f, true, true);
    let proof = transfer.inspect_archives(&mut d, p.as_mut()).unwrap();
    assert_eq!(
        f.registry
            .record_empty_device_sources(proof)
            .await
            .unwrap()
            .initialization_id,
        "original_setup"
    );
    drop(d);
    drop(p);
    drop(transfer);
    f.registry.shutdown().await.unwrap();
}
#[tokio::test]
async fn changed_history_missing_personal_or_wrong_authority_cannot_record_a_fence() {
    let (f, transfer) = prepared(true).await;
    let (mut d, mut p) = sources(&f, true, false);
    assert!(
        transfer
            .fence_sources(&mut d, None, &"c".repeat(64))
            .is_err()
    );
    d.execute("INSERT INTO managed_accounts(id,ordinal,generation,state) VALUES('retained',1,1,'preparing')",[]).unwrap();
    assert!(
        transfer
            .fence_sources(&mut d, p.as_mut(), &"c".repeat(64))
            .is_err()
    );
    assert_eq!(
        d.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))
            .unwrap(),
        0x48414732
    );
    drop(d);
    drop(p);
    drop(transfer);
    f.registry.shutdown().await.unwrap();
    let (f, transfer) = prepared(false).await;
    let (mut d, _) = sources(&f, false, false);
    transfer
        .fence_sources(&mut d, None, &"c".repeat(64))
        .unwrap();
    drop(d);
    let (mut d, _) = sources(&f, false, true);
    let proof = transfer.inspect_archives(&mut d, None).unwrap();
    let other = OwnerResourceRegistry::open_root(&f.root.path().join("other")).unwrap();
    assert!(other.record_empty_device_sources(proof).await.is_err());
    f.profile.retire();
    assert!(transfer.inspect_archives(&mut d, None).is_err());
    drop(d);
    drop(transfer);
    other.shutdown().await.unwrap();
    f.registry.shutdown().await.unwrap();
}
