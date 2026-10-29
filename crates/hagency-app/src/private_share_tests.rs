use super::*;
use std::sync::atomic::AtomicUsize;
fn scope() -> ShareScope {
    ShareScope {
        epoch: 7,
        owner: "@owner:fixture.test".parse().unwrap(),
    }
}

#[test]
fn private_share_file_is_private_identity_bound_and_deleted_through_original_directory() {
    let cache = tempfile::tempdir().unwrap();
    let root = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
    let mut file = root
        .stage("../../message.txt", b"private plaintext", SHARE_TTL)
        .unwrap();
    assert_eq!(std::fs::read(&file.path).unwrap(), b"private plaintext");
    let handle = hagency_store::private::open(&file.path, false).unwrap();
    hagency_store::private::check_handle(&handle).unwrap();
    drop(handle);
    file.verify().unwrap();
    let path = file.path.clone();
    file.remove().unwrap();
    assert!(!path.exists());
    root.remove().unwrap();
}

#[test]
fn account_retirement_during_disk_work_never_produces_live_presentation_authority() {
    let cache = tempfile::tempdir().unwrap();
    let root = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
    let checks = AtomicUsize::new(0);
    let mut stored = stage_checked(&root, &scope(), "message.txt", b"fixture", |_| {
        checks.fetch_add(1, Ordering::AcqRel) == 0
    })
    .unwrap();
    assert_eq!(checks.load(Ordering::Acquire), 2);
    assert!(!stored.ticket.live.load(Ordering::Acquire));
    stored.file.remove().unwrap();
    assert!(stage_checked(&root, &scope(), "rejected.txt", b"fixture", |_| false).is_err());
    root.remove().unwrap();
}

#[test]
fn staged_share_is_retained_until_ttl_without_inferring_completion_or_cancel() {
    let cache = tempfile::tempdir().unwrap();
    let root = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
    let mut stored = stage_checked(&root, &scope(), "message.txt", b"fixture", |_| true).unwrap();
    let staged_at = Instant::now();
    assert!(stored.file.expires > staged_at + SHARE_TTL - Duration::from_secs(1));
    assert!(stored.file.path.exists());
    stored.file.verify().unwrap();
    stored.file.expires = staged_at - Duration::from_secs(1);
    assert!(Instant::now() >= stored.file.expires);
    stored.file.remove().unwrap();
    root.remove().unwrap();
}

#[test]
fn crashed_run_is_cleaned_but_another_live_process_lease_is_preserved() {
    let cache = tempfile::tempdir().unwrap();
    let first = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
    let file = first.stage("message.txt", b"fixture", SHARE_TTL).unwrap();
    let path = file.path.clone();
    let second = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
    assert!(
        path.exists(),
        "a live worker's lease must protect its files"
    );
    second.remove().unwrap();
    // Simulate process death: release original handles/lease without cleanup.
    drop(file);
    drop(first);
    let third = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
    assert!(
        !path.exists(),
        "stale process plaintext is removed on startup"
    );
    third.remove().unwrap();
}

#[cfg(unix)]
#[test]
fn legacy_cleanup_does_not_follow_symlink_subtrees_or_delete_unexpired_files() {
    use std::os::unix::fs::symlink;
    let cache = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("keep.txt"), b"outside").unwrap();
    std::fs::create_dir(cache.path().join("temp")).unwrap();
    symlink(outside.path(), cache.path().join("temp/escape")).unwrap();
    std::fs::write(cache.path().join("temp/recent.txt"), b"recent").unwrap();
    files::sweep_legacy(cache.path(), SHARE_TTL).unwrap();
    assert!(cache.path().join("temp/recent.txt").exists());
    files::sweep_legacy(cache.path(), Duration::ZERO).unwrap();
    assert!(!cache.path().join("temp/recent.txt").exists());
    assert_eq!(
        std::fs::read(outside.path().join("keep.txt")).unwrap(),
        b"outside"
    );
}

#[cfg(unix)]
#[test]
fn replaced_file_and_symlink_root_are_refused_without_deleting_targets() {
    use std::os::unix::fs::symlink;
    let cache = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
    let mut file = root.stage("message.txt", b"fixture", SHARE_TTL).unwrap();
    std::fs::remove_file(&file.path).unwrap();
    std::fs::write(&file.path, b"replacement").unwrap();
    assert!(file.verify().is_err());
    assert!(file.remove().is_err());
    assert_eq!(std::fs::read(&file.path).unwrap(), b"replacement");
    drop((file, root));
    let other = tempfile::tempdir().unwrap();
    symlink(outside.path(), other.path().join("private-shares-v1")).unwrap();
    assert!(files::RunDirectory::open(other.path(), SHARE_TTL).is_err());
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[cfg(windows)]
#[test]
fn windows_created_share_file_has_the_store_checked_protected_owner_dacl() {
    let cache = tempfile::tempdir().unwrap();
    let root = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
    let mut file = root.stage("message.txt", b"fixture", SHARE_TTL).unwrap();
    let handle = hagency_store::private::open(&file.path, false).unwrap();
    hagency_store::private::check_handle(&handle).unwrap();
    drop(handle);
    file.remove().unwrap();
    root.remove().unwrap();
}

#[test]
fn production_sweep_covers_ttl_retirement_failed_presentation_and_shutdown() {
    for cause in 0..4 {
        let cache = tempfile::tempdir().unwrap();
        let root = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
        let entry = stage_checked(&root, &scope(), "message.txt", b"fixture", |_| true).unwrap();
        let ticket = entry.ticket.clone();
        let path = entry.file.path.clone();
        let expires = entry.file.expires;
        let mut stored = BTreeMap::from([(ticket.id, entry)]);
        let mut discard = BTreeSet::new();
        if cause == 2 {
            discard.insert(ticket.id);
        }
        let now = if cause == 0 { expires } else { Instant::now() };
        assert_eq!(
            cleanup_entries(
                &mut stored,
                &BTreeSet::new(),
                0,
                false,
                now - Duration::from_secs(1)
            ),
            (0, 0)
        );
        assert!(path.exists(), "presentation alone cannot expire the share");
        assert_eq!(
            cleanup_entries(
                &mut stored,
                &discard,
                if cause == 1 { 8 } else { 0 },
                cause == 3,
                now
            ),
            (7, 0)
        );
        assert!(stored.is_empty());
        assert!(!ticket.live.load(Ordering::Acquire));
        assert!(!path.exists());
        root.remove().unwrap();
    }
}

#[test]
fn production_admission_bounds_queued_running_files_and_total_bytes() {
    let mut state = State::default();
    for _ in 0..MAX_JOBS {
        assert!(state.admits(1));
        state.queue.push_back(Job {
            scope: scope(),
            filename: "fixture".into(),
            bytes: Arc::from([0u8]),
            ready: Box::new(|_| {}),
        });
    }
    assert!(!state.admits(1));
    state.queue.pop_front();
    state.busy = true;
    assert!(
        !state.admits(1),
        "running preparation counts against admission"
    );
    state.queue.clear();
    state.busy = false;
    state.stored = MAX_STAGED_FILES;
    assert!(!state.admits(1));
    state.stored = 0;
    state.used_bytes = MAX_STAGED_BYTES;
    assert!(!state.admits(1));
    state.used_bytes = 0;
    assert!(state.admits(MAX_STAGED_BYTES));
    assert!(!state.admits(MAX_STAGED_BYTES + 1));
}

#[test]
fn transient_unlink_failure_retains_original_identity_and_retries_cleanup() {
    let cache = tempfile::tempdir().unwrap();
    let root = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
    let mut file = root.stage("message.txt", b"fixture", SHARE_TTL).unwrap();
    let path = file.path.clone();
    assert!(
        file.remove_with(|_, _| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)))
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"fixture");
    file.verify().unwrap();
    // The second pass still compares the original ID before a real unlink.
    file.remove().unwrap();
    assert!(!path.exists());
    root.remove().unwrap();
}

#[test]
fn unconfirmed_partial_file_cleanup_keeps_budget_and_blocks_further_writes() {
    let mut state = State {
        used_bytes: 10,
        ..State::default()
    };
    record_stage_failure(&mut state, 7, true);
    assert_eq!(state.used_bytes, 10);
    assert_eq!(state.cleanup_failures, 1);
    assert!(state.poisoned);
    assert!(!state.admits(1));
    record_stage_failure(&mut state, 3, false);
    assert_eq!(
        state.used_bytes, 7,
        "only confirmed-cleaned failures release their own reservation"
    );
    assert!(!state.admits(1));
}

#[test]
fn retained_unknown_native_observations_block_both_staging_and_actual_presentation() {
    let mut state = State {
        native_observations: MAX_STAGED_FILES,
        ..State::default()
    };
    // Deleting all files or changing accounts cannot release an original Child
    // or portal observation that has not actually exited/closed.
    state.stored = 0;
    state.used_bytes = 0;
    assert!(!state.admits(1));
    let mut presents = 0;
    if state.claim_presentation() {
        presents += 1;
    }
    assert_eq!(presents, 0);
    assert_eq!(state.native_observations, MAX_STAGED_FILES);
}

#[test]
fn observed_native_reap_releases_admission_and_inflight_presentation_keeps_its_slot() {
    let mut state = State {
        native_observations: MAX_STAGED_FILES,
        ..State::default()
    };
    // This is the count transition performed only after PendingShare poll/cancel
    // confirms completion; failed cancel leaves the live count unchanged.
    state.native_observations -= 1;
    assert!(state.admits(1));
    assert!(state.claim_presentation());
    assert_eq!(state.retained_native(), MAX_STAGED_FILES);
    assert!(!state.admits(1));
    assert!(!state.claim_presentation());
    state.presenting -= 1;
    assert!(state.admits(1));
}


#[test]
fn empty_directory_cleanup_retries_without_touching_replacement_contents() {
    let cache = tempfile::tempdir().unwrap();
    let root = files::RunDirectory::open(cache.path(), SHARE_TTL).unwrap();
    let mut file = root.stage("message.txt", b"original", SHARE_TTL).unwrap();
    let path = file.path.clone();
    let failed = file.remove_operations(
        |directory, name| directory.remove_file(name),
        |_, _| Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected empty-directory removal failure")),
    );
    assert!(failed.is_err());
    assert!(!path.exists(), "original plaintext was removed before closing its directory");
    std::fs::write(&path, b"replacement must survive").unwrap();
    assert!(file.remove().is_err(), "retry may only remove an empty directory");
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement must survive");
    assert!(file.verify().is_err(), "a removed original is never reconstructed from a replacement");
    std::fs::remove_file(&path).unwrap();
    file.remove().unwrap();
    file.remove().unwrap();
    root.remove().unwrap();
}
