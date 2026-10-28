//! The backup's own contract against a REAL store layout.
//!
//! `native/hagency-store/src/backup.rs`'s unit test proves the online backup
//! beats a naive copy on a synthetic database. This one is the harsher check:
//! it builds the state directory exactly as `hagency init` does — the operator
//! token plus BOTH real databases, with the `-wal`/`-shm` sidecars the store
//! leaves behind — and requires the snapshot to capture every durable file.

use hagency_store::{DomainRepository, Repository, backup, private};

fn initialized_state(root: &std::path::Path) -> std::path::PathBuf {
    let state = root.join("state");
    private::directory(&state).unwrap();
    private::write_new(&state.join("operator.token"), b"operator-token-value").unwrap();
    drop(Repository::open(&state).unwrap());
    drop(DomainRepository::open(&state).unwrap());
    state
}

#[test]
fn native_backup_captures_an_initialized_state() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = initialized_state(root.path());
    // The real store leaves its lock files and both databases behind.
    assert!(state.join("domain.sqlite3").is_file());
    assert!(state.join("custody.sqlite3").is_file());

    let out = root.path().join("snapshot");
    let manifest = backup::snapshot(&state, &out, 1_700_000_000_000).unwrap();
    let captured: Vec<&str> = manifest
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert!(
        captured.contains(&"domain.sqlite3"),
        "the domain database was not captured: {captured:?}"
    );
    assert!(
        captured.contains(&"custody.sqlite3"),
        "the custody database was not captured: {captured:?}"
    );
    assert!(captured.contains(&"operator.token"));

    // Every captured database opens and passes integrity, with no sidecars.
    for name in ["domain.sqlite3", "custody.sqlite3"] {
        let connection = rusqlite::Connection::open(out.join(name)).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "integrity_check", |r| r.get::<_, String>(0))
                .unwrap(),
            "ok",
            "{name} is not a sound snapshot"
        );
        assert!(
            !out.join(format!("{name}-wal")).exists(),
            "{name} left a -wal"
        );
    }

    // And verify accepts what snapshot produced.
    backup::verify(&out).unwrap();
}
