use super::*;
use rusqlite::OpenFlags;

struct Fixture {
    root: tempfile::TempDir,
    source: PathBuf,
    personal: PathBuf,
    profile: String,
}
impl Fixture {
    fn new(personal: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let profile = "a".repeat(64);
        drop(DomainRepository::open_managed(&source, &profile).unwrap());
        let personal_path = root.path().join("personal.sqlite");
        if personal {
            crate::private::open(&personal_path, true).unwrap();
            drop(
                hagency_agent_local::Ledger::open_scoped(&personal_path, "@owner:a.test", &profile)
                    .unwrap(),
            );
        }
        Self {
            root,
            source,
            personal: personal_path,
            profile,
        }
    }
    fn domain(&self, readonly: bool) -> Connection {
        let db = Connection::open_with_flags(
            self.source.join("domain.sqlite3"),
            if readonly {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            } else {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            },
        )
        .unwrap();
        // Match native source custody: closing a fixture writer must not
        // checkpoint/remove the WAL while the read-only assertion observes it.
        db.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
        db
    }
    fn inspect(&self) -> Result<EmptyDeviceInventory, Error> {
        let mut d = self.domain(true);
        let mut p = self.personal.exists().then(|| {
            Connection::open_with_flags(&self.personal, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
        });
        EmptyDeviceInventory::inspect(&mut d, p.as_mut(), &self.profile, "@owner:a.test")
    }
    fn bytes(&self) -> Vec<(String, usize, String)> {
        use sha2::Digest;
        ["domain.sqlite3", "domain.sqlite3-wal"]
            .into_iter()
            .filter_map(|name| {
                std::fs::read(self.source.join(name)).ok().map(|bytes| {
                    (
                        name.into(),
                        bytes.len(),
                        format!("{:x}", sha2::Sha256::digest(bytes)),
                    )
                })
            })
            .collect()
    }
    fn seat(&self, declared: bool) {
        let value = if declared {
            json!({"id":"seat_original","declaration":{"quotaTokens":700,"period":"monthly"}})
        } else {
            json!({"id":"seat_original","declaration":null})
        };
        self.domain(false)
            .execute(
                "INSERT INTO seats VALUES(?1,?2)",
                params!["seat_original", value.to_string()],
            )
            .unwrap();
    }
}

#[test]
fn truly_empty_device_with_missing_personal_store_is_read_only_and_mints_nothing() {
    let f = Fixture::new(false);
    let before = f.bytes();
    let inventory = f.inspect().unwrap();
    assert!(!inventory.personal_present());
    assert_eq!(inventory.completed_private_records(), 0);
    assert_eq!(
        inventory.digest().unwrap(),
        f.inspect().unwrap().digest().unwrap()
    );
    assert_eq!(before, f.bytes());
    assert!(!f.personal.exists());
    assert_eq!(std::fs::read_dir(f.root.path()).unwrap().count(), 1);
    for table in [
        "resources",
        "seats",
        "managed_accounts",
        "owner_capacity_grants",
        "owner_resource_adoptions",
    ] {
        assert_eq!(
            f.domain(true)
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn existing_empty_personal_identity_and_settings_are_part_of_the_review() {
    let f = Fixture::new(true);
    let old = f.inspect().unwrap();
    assert!(old.personal_present());
    Connection::open(&f.personal)
        .unwrap()
        .execute("UPDATE inbox_limits SET max_records=max_records+1", [])
        .unwrap();
    assert_ne!(
        old.digest().unwrap(),
        f.inspect().unwrap().digest().unwrap()
    );
    Connection::open(&f.personal)
        .unwrap()
        .execute("INSERT INTO bindings VALUES('retained','agent','room')", [])
        .unwrap();
    assert!(matches!(f.inspect(), Err(Error::Quarantined)));
}

#[test]
fn undeclared_legacy_seat_is_recorded_but_an_existing_quota_is_not_empty() {
    let f = Fixture::new(false);
    let none = f.inspect().unwrap().digest().unwrap();
    f.seat(false);
    assert_ne!(none, f.inspect().unwrap().digest().unwrap());
    f.domain(false)
        .execute(
            "UPDATE seats SET config=json_set(config,'$.declaration',json(?1))",
            [json!({"quotaTokens":0,"period":"monthly"}).to_string()],
        )
        .unwrap();
    assert!(matches!(f.inspect(), Err(Error::Quarantined)));
}

#[test]
fn a_resource_or_prepared_account_requires_its_original_history_flow() {
    let f = Fixture::new(false);
    f.seat(false);
    let resource: Resource = serde_json::from_value(json!({"presetId":"original","seatId":"seat_original","framework":"codex","model":"gpt-5.6-sol","reasoning":"medium","published":false})).unwrap();
    f.domain(false)
        .execute(
            "INSERT INTO resources VALUES(?1,?2,?3)",
            params![
                resource.id(),
                resource.preset_id,
                serde_json::to_string(&resource).unwrap()
            ],
        )
        .unwrap();
    assert!(matches!(f.inspect(), Err(Error::Quarantined)));
    let f = Fixture::new(false);
    f.domain(false).execute("INSERT INTO managed_accounts(id,ordinal,generation,state) VALUES('original',1,1,'preparing')", []).unwrap();
    assert!(matches!(f.inspect(), Err(Error::Quarantined)));
}

#[test]
fn empty_unknown_tables_or_missing_required_tables_never_prove_freshness() {
    let f = Fixture::new(false);
    f.domain(false)
        .execute("CREATE TABLE unknown_history(value TEXT)", [])
        .unwrap();
    assert!(matches!(f.inspect(), Err(Error::Schema)));
    f.domain(false)
        .execute("DROP TABLE unknown_history", [])
        .unwrap();
    f.inspect().unwrap();
    f.domain(false)
        .execute("DROP TABLE workspace_resources", [])
        .unwrap();
    assert!(matches!(f.inspect(), Err(Error::Schema)));
}

#[test]
fn exact_profile_read_only_connections_and_unfenced_schema_are_required() {
    let f = Fixture::new(false);
    assert!(matches!(
        EmptyDeviceInventory::inspect(&mut f.domain(false), None, &f.profile, "@owner:a.test"),
        Err(Error::LocalAuthority)
    ));
    assert!(matches!(
        EmptyDeviceInventory::inspect(&mut f.domain(true), None, &"b".repeat(64), "@owner:a.test"),
        Err(Error::LocalAuthority)
    ));
    let mut d = f.domain(true);
    d.execute_batch("BEGIN").unwrap();
    assert!(matches!(
        EmptyDeviceInventory::inspect(&mut d, None, &f.profile, "@owner:a.test"),
        Err(Error::LocalAuthority)
    ));
    d.execute_batch("ROLLBACK").unwrap();
    f.domain(false)
        .pragma_update(None, "application_id", 0x48415231)
        .unwrap();
    assert!(matches!(f.inspect(), Err(Error::Schema)));
    let f = Fixture::new(true);
    let mut p = Connection::open_with_flags(&f.personal, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(matches!(
        EmptyDeviceInventory::inspect(
            &mut f.domain(true),
            Some(&mut p),
            &f.profile,
            "@different:a.test"
        ),
        Err(Error::LocalAuthority)
    ));
}

#[test]
fn original_terminal_custody_is_bound_and_pending_commands_are_not_discarded() {
    let f = Fixture::new(false);
    let old = f.inspect().unwrap().digest().unwrap();
    let operation =
        serde_json::to_string(&crate::management::IntentOperation::PrepareAccount).unwrap();
    f.domain(false)
        .execute(
            "INSERT INTO management_intents VALUES(?1,'original',?2,?3,NULL,NULL,'acknowledged')",
            params!["c".repeat(64), operation, "d".repeat(64)],
        )
        .unwrap();
    let observed = f.inspect().unwrap();
    assert_eq!(observed.completed_private_records(), 1);
    assert_ne!(old, observed.digest().unwrap());
    // A separate still-pending operation must survive; setup is refused.
    f.domain(false)
        .execute(
            "INSERT INTO management_intents VALUES(?1,'pending',?2,?3,'{}',NULL,'pending')",
            params!["c".repeat(64), operation, "e".repeat(64)],
        )
        .unwrap();
    let before = f.bytes();
    assert!(matches!(f.inspect(), Err(Error::Quarantined)));
    assert_eq!(before, f.bytes());
    assert_eq!(
        f.domain(true)
            .query_row("SELECT COUNT(*) FROM management_intents", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        2
    );
}
