use super::*;
use rusqlite::OpenFlags;

pub(super) struct Fixture {
    pub(super) root: tempfile::TempDir,
    pub(super) registry: OwnerResourceRegistry,
    pub(super) profile: ProfileAuthority,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let profile = ProfileAuthority::new(
            "a".repeat(64),
            "@owner:a.test".into(),
            Instant::now() + Duration::from_secs(120),
        )
        .unwrap();
        drop(DomainRepository::open_managed(&root.path().join("source"), &profile.digest).unwrap());
        let registry = OwnerResourceRegistry::open_root(&root.path().join("canonical")).unwrap();
        Self {
            root,
            registry,
            profile,
        }
    }
    fn inventory(&self) -> EmptyDeviceInventory {
        let mut d = Connection::open_with_flags(
            self.root.path().join("source/domain.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        EmptyDeviceInventory::inspect(&mut d, None, &self.profile.digest, &self.profile.account)
            .unwrap()
    }
    pub(super) fn authority(&self) -> AdoptionAuthority {
        AdoptionAuthority::new(
            self.registry.clone(),
            self.profile.clone(),
            Instant::now() + Duration::from_secs(30),
            || true,
        )
        .unwrap()
    }
    fn prepared(&self, id: &str) -> PreparedEmptyDeviceInitialization {
        self.registry
            .empty_device_preparation(
                self.authority(),
                id.into(),
                self.inventory(),
                "c".repeat(64),
            )
            .unwrap()
    }
    pub(super) async fn count(&self, table: &'static str) -> i64 {
        self.registry
            .0
            .store
            .call(1, move |db| {
                Ok(db
                    .db
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
            })
            .await
            .unwrap()
    }
    fn populated_inventory(&self) -> AdoptionInventory {
        let dir = self.root.path().join("populated");
        let mut db = DomainRepository::open_managed(&dir, &self.profile.digest).unwrap();
        db.put_seat(&Seat {
            id: "original_seat".into(),
            declaration: None,
        })
        .unwrap();
        let resource:Resource=serde_json::from_value(json!({"presetId":"original","seatId":"original_seat","framework":"codex","model":"gpt-5.6-sol","reasoning":"medium","published":false})).unwrap();
        db.put_resource(&resource).unwrap();
        drop(db);
        let personal = self.root.path().join("personal.sqlite");
        crate::private::open(&personal, true).unwrap();
        drop(
            hagency_agent_local::Ledger::open_scoped(
                &personal,
                &self.profile.account,
                &self.profile.digest,
            )
            .unwrap(),
        );
        let mut d = Connection::open_with_flags(
            dir.join("domain.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut p =
            Connection::open_with_flags(personal, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        AdoptionInventory::inspect(&mut d, &mut p, &self.profile.digest, &self.profile.account)
            .unwrap()
    }
    fn populated_plan(&self) -> PreparedResourceAdoption {
        let provider = VerifiedProviderAccount::new(
            "codex",
            "https://provider.test",
            "test_subject",
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        self.registry
            .resource_adoption_preparation(
                self.authority(),
                "original_transfer".into(),
                provider,
                self.populated_inventory(),
                "b".repeat(64),
                700,
            )
            .unwrap()
    }
}

#[tokio::test]
async fn empty_initialization_preparation_has_no_provider_quota_or_source_writes() {
    let f = Fixture::new();
    let source = f.root.path().join("source/domain.sqlite3-wal");
    let before = std::fs::read(&source).unwrap();
    let prepared = f.prepared("original_setup");
    let digest = prepared.plan_digest().unwrap();
    f.registry
        .validate_empty_device_preparation(&prepared)
        .await
        .unwrap();
    assert_eq!(
        f.count("owner_resource_adoptions").await,
        0,
        "native cancellation before commit leaves no reservation"
    );
    let summary = f
        .registry
        .commit_empty_device_preparation(prepared)
        .await
        .unwrap();
    assert_eq!(summary.phase, "prepared");
    assert!(!summary.personal_database_present);
    let encoded = serde_json::to_value(summary).unwrap();
    assert!(encoded.get("monthlyAccountTokens").is_none());
    assert!(encoded.get("providerDigest").is_none());
    let recovered = f
        .registry
        .recover_empty_device_preparation(f.authority(), "original_setup".into(), digest.clone())
        .await
        .unwrap();
    assert_eq!(recovered.plan_digest().unwrap(), digest);
    f.registry
        .commit_empty_device_preparation(recovered)
        .await
        .unwrap();
    assert_eq!(f.count("owner_resource_adoptions").await, 1);
    for table in [
        "resources",
        "seats",
        "managed_accounts",
        "owner_resource_accounts",
        "owner_resource_grants",
        "owner_capacity_grants",
    ] {
        assert_eq!(
            f.count(table).await,
            0,
            "unexpected canonical rows in {table}"
        );
    }
    assert!(
        before == std::fs::read(source).unwrap(),
        "original source WAL changed"
    );
    assert!(!f.root.path().join("personal.sqlite").exists());
    // Existing v1/v2/v3 access cannot interpret version four as a provider plan.
    assert!(
        f.registry
            .resource_adoption(f.authority(), "original_setup".into())
            .await
            .is_err()
    );
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn saved_empty_preparation_recovers_exactly_after_worker_restart() {
    let mut f = Fixture::new();
    let prepared = f.prepared("original_setup");
    let digest = prepared.plan_digest().unwrap();
    f.registry
        .commit_empty_device_preparation(prepared)
        .await
        .unwrap();
    let original_registry = f.registry.id().to_owned();
    f.registry.shutdown().await.unwrap();
    f.registry = OwnerResourceRegistry::open_root(&f.root.path().join("canonical")).unwrap();
    assert_eq!(f.registry.id(), original_registry);
    let recovered = f
        .registry
        .recover_empty_device_preparation(f.authority(), "original_setup".into(), digest.clone())
        .await
        .unwrap();
    assert_eq!(recovered.plan_digest().unwrap(), digest);
    assert_eq!(
        recovered.review_summary().initialization_id,
        "original_setup"
    );
    assert!(
        f.registry
            .recover_empty_device_preparation(f.authority(), "replacement_setup".into(), digest)
            .await
            .is_err()
    );
    assert!(
        f.registry
            .recover_empty_device_preparation(
                f.authority(),
                "original_setup".into(),
                "d".repeat(64)
            )
            .await
            .is_err()
    );
    assert!(
        f.registry
            .commit_empty_device_preparation(f.prepared("replacement_setup"))
            .await
            .is_err()
    );
    assert_eq!(f.count("owner_resource_adoptions").await, 1);
    f.registry.shutdown().await.unwrap();
}

#[cfg(feature = "resource-adoption")]
#[tokio::test]
async fn empty_preparation_refuses_existing_canonical_graph_and_recipient_history() {
    use crate::owner_resources::graph_journal::GraphJournalCommand;
    for reviewed in [false,true] {
        let f=Fixture::new();
        let prepared=f.prepared("empty_setup");
        let command=|id:&str,revision|GraphJournalCommand {
            command_id:id.into(),scope_id:"scope".into(),graph_id:"graph".into(),
            expected_revision:revision,input_digest:hagency_core::project::hash(id.as_bytes()),
        };
        f.registry.transact_operator_graph(&f.profile,command("create_graph",0),|_,_|Ok(b"graph".to_vec())).await.unwrap();
        if reviewed {
            f.registry.transact_operator_graph(&f.profile,command("review_node",1),|old,w| {
                w.bind_recipient("node",br#"{"nativeReview":"retained"}"#.to_vec())?;
                Ok(old.unwrap().document)
            }).await.unwrap();
        }
        assert!(matches!(f.registry.validate_empty_device_preparation(&prepared).await,Err(Error::Conflict)));
        assert!(matches!(f.registry.commit_empty_device_preparation(prepared).await,Err(Error::Conflict)));
        assert_eq!(f.count("owner_resource_adoptions").await,0);
        assert_eq!(f.count("owner_operator_graphs").await,1);
        assert_eq!(f.count("owner_operator_graph_recipients").await,if reviewed {1}else{0});
        f.registry.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn populated_and_empty_setup_cannot_take_each_others_profile_slot() {
    for old_first in [true, false] {
        let f = Fixture::new();
        let empty = f.prepared("original_setup");
        let populated = f.populated_plan();
        if old_first {
            f.registry
                .commit_resource_adoption_preparation(populated)
                .await
                .unwrap();
            assert!(
                f.registry
                    .validate_empty_device_preparation(&empty)
                    .await
                    .is_err()
            );
            assert!(
                f.registry
                    .commit_empty_device_preparation(empty)
                    .await
                    .is_err()
            );
            assert_eq!(
                f.registry
                    .resource_adoption(f.authority(), "original_transfer".into())
                    .await
                    .unwrap()
                    .monthly_account_tokens,
                700
            );
        } else {
            f.registry
                .commit_empty_device_preparation(empty)
                .await
                .unwrap();
            assert!(
                f.registry
                    .validate_resource_adoption_preparation(&populated)
                    .await
                    .is_err()
            );
            assert!(
                f.registry
                    .commit_resource_adoption_preparation(populated)
                    .await
                    .is_err()
            );
        }
        assert_eq!(f.count("owner_resource_adoptions").await, 1);
        f.registry.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn current_native_scope_and_registry_are_required_for_each_write_or_recovery() {
    let f = Fixture::new();
    let prepared = f.prepared("original_setup");
    let digest = prepared.plan_digest().unwrap();
    let other = OwnerResourceRegistry::open_root(&f.root.path().join("other_registry")).unwrap();
    assert!(
        other
            .validate_empty_device_preparation(&prepared)
            .await
            .is_err()
    );
    assert!(
        other
            .commit_empty_device_preparation(prepared)
            .await
            .is_err()
    );
    assert_eq!(f.count("owner_resource_adoptions").await, 0);
    f.registry
        .commit_empty_device_preparation(f.prepared("original_setup"))
        .await
        .unwrap();
    let wrong_profile = ProfileAuthority::new(
        "d".repeat(64),
        "@other:a.test".into(),
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap();
    let authority = AdoptionAuthority::new(
        f.registry.clone(),
        wrong_profile,
        Instant::now() + Duration::from_secs(20),
        || true,
    )
    .unwrap();
    assert!(
        f.registry
            .recover_empty_device_preparation(authority, "original_setup".into(), digest.clone())
            .await
            .is_err()
    );
    let stale = f.authority();
    let attempted = f
        .registry
        .empty_device_preparation(
            stale.clone(),
            "another_setup".into(),
            f.inventory(),
            "c".repeat(64),
        )
        .unwrap();
    f.profile.retire();
    assert!(
        f.registry
            .commit_empty_device_preparation(attempted)
            .await
            .is_err()
    );
    assert!(
        f.registry
            .recover_empty_device_preparation(stale, "original_setup".into(), digest)
            .await
            .is_err()
    );
    assert_eq!(f.count("owner_resource_adoptions").await, 1);
    other.shutdown().await.unwrap();
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_commit_retains_no_partial_preparation_and_original_can_retry() {
    let f = Fixture::new();
    f.registry.0.store.call(1,|db| {
        db.db.execute_batch("CREATE TRIGGER fail_setup BEFORE INSERT ON owner_resource_adoptions BEGIN SELECT RAISE(ABORT,'fixture interruption'); END")?; Ok(())
    }).await.unwrap();
    let digest = f.prepared("original_setup").plan_digest().unwrap();
    assert!(
        f.registry
            .commit_empty_device_preparation(f.prepared("original_setup"))
            .await
            .is_err()
    );
    assert_eq!(f.count("owner_resource_adoptions").await, 0);
    assert!(
        f.registry
            .recover_empty_device_preparation(
                f.authority(),
                "original_setup".into(),
                digest.clone()
            )
            .await
            .is_err()
    );
    f.registry
        .0
        .store
        .call(1, |db| {
            db.db.execute_batch("DROP TRIGGER fail_setup")?;
            Ok(())
        })
        .await
        .unwrap();
    f.registry
        .commit_empty_device_preparation(f.prepared("original_setup"))
        .await
        .unwrap();
    assert_eq!(
        f.registry
            .recover_empty_device_preparation(
                f.authority(),
                "original_setup".into(),
                digest.clone()
            )
            .await
            .unwrap()
            .plan_digest()
            .unwrap(),
        digest
    );
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn prepared_record_is_retained_and_fabricated_fencing_is_rejected() {
    let f = Fixture::new();
    let prepared = f.prepared("original_setup");
    let digest = prepared.plan_digest().unwrap();
    f.registry
        .commit_empty_device_preparation(prepared)
        .await
        .unwrap();
    f.registry
        .0
        .store
        .call(1, |db| {
            assert!(
                db.db
                    .execute("DELETE FROM owner_resource_adoptions", [])
                    .is_err()
            );
            assert!(
                db.db
                    .execute("UPDATE owner_resource_adoptions SET plan='{}'", [])
                    .is_err()
            );
            db.db.execute(
                "UPDATE owner_resource_adoptions SET phase=1,source_evidence=?1",
                ["e".repeat(64)],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        f.registry
            .recover_empty_device_preparation(f.authority(), "original_setup".into(), digest)
            .await
            .is_err()
    );
    assert_eq!(f.count("owner_resource_adoptions").await, 1);
    f.registry.shutdown().await.unwrap();
}
