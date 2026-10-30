use super::*;
use serde_json::json;

fn event(id: &str, body: &str, time: u64) -> Event {
    Event {
        value: json!({"type":"m.room.message", "event_id":id,
        "sender":"@a:b", "origin_server_ts":time,
        "content":{"msgtype":"m.text", "body":body}}),
        name: String::new(),
        locked: false,
    }
}

fn redaction(target: &str) -> Event {
    Event {
        value: json!({"type":"m.room.redaction", "event_id":format!("$redact{target}"), "content":{"redacts":target}}),
        name: String::new(),
        locked: false,
    }
}

fn edit(id: &str, target: &str, body: &str, time: u64) -> Event {
    let mut event = event(id, "* edit", time);
    event.value["content"]["m.relates_to"] = json!({"rel_type":"m.replace", "event_id":target});
    event.value["content"]["m.new_content"] = json!({"msgtype":"m.text", "body":body});
    event
}

#[test]
fn encrypted_payloads_tokens_and_coverage_survive_reopen() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("search.sqlite");
    let mut index = Index::open(&path, &[7; 32], "account/device/room")?;
    let coverage = Coverage {
        cursor: Some("secret-pagination-token".into()),
        exhausted: true,
        head_cursor: None,
    };
    index.insert_batch(
        &[event("$secret-event-id", "Confidential 中文项目", 1)],
        Some(&coverage),
    )?;
    drop(index);
    let bytes = std::fs::read(&path)?;
    for secret in [
        "secret-event-id",
        "Confidential",
        "中文项目",
        "@a:b",
        "secret-pagination-token",
    ] {
        assert!(
            !bytes.windows(secret.len()).any(|b| b == secret.as_bytes()),
            "leaked {secret}"
        );
    }
    assert!(Index::open(&path, &[8; 32], "account/device/room").is_err());
    assert!(Index::open(&path, &[7; 32], "another account").is_err());
    let index = Index::open(&path, &[7; 32], "account/device/room")?;
    assert_eq!(index.search("中文项", 0, None, None, || false)?.len(), 1);
    assert_eq!(index.search("项目", 0, None, None, || false)?.len(), 1);
    assert_eq!(
        index.search("CONFIDENTIAL", 0, None, None, || false)?.len(),
        1
    );
    assert_eq!(index.coverage()?.cursor, coverage.cursor);
    assert!(index.coverage()?.exhausted);
    Ok(())
}

#[test]
fn long_account_device_room_path_preserves_encrypted_history() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let parent = dir
        .path()
        .join(format!("account_{}", "a".repeat(64)))
        .join("persistent_state")
        .join("history-search-v1")
        .join("d".repeat(64))
        .join("nested-profile".repeat(8));
    std::fs::create_dir_all(&parent)?;
    let path = parent.join(format!("{}.sqlite3", "e".repeat(64)));
    assert!(path.as_os_str().len() > 300);
    let mut index = Index::open(&path, &[7; 32], "original/account/device/room")?;
    index.insert_batch(
        &[event("$long-path", "original encrypted history", 1)],
        None,
    )?;
    drop(index);
    let bytes = std::fs::read(&path)?;
    assert!(
        !bytes
            .windows(26)
            .any(|b| b == b"original encrypted history")
    );
    assert!(Index::open(&path, &[8; 32], "original/account/device/room").is_err());
    assert!(Index::open(&path, &[7; 32], "other/account/device/room").is_err());
    let index = Index::open(&path, &[7; 32], "original/account/device/room")?;
    assert_eq!(
        index.search("encrypted", 0, None, None, || false)?[0].id,
        "$long-path"
    );
    Ok(())
}

#[test]
fn edits_redactions_locked_upgrades_and_overlap_are_durable() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("search.sqlite");
    let mut index = Index::open(&path, &[1; 32], "room")?;
    let original = event("$original", "obsolete", 1);
    let edit = edit("$edit", "$original", "corrected 项目", 2);
    let mut forged = edit.clone();
    forged.value["event_id"] = "$forged".into();
    forged.value["sender"] = "@mallory:b".into();
    forged.value["origin_server_ts"] = 3.into();
    forged.value["content"]["m.new_content"]["body"] = "forged".into();
    let mut locked = event("$locked", "recovered", 4);
    locked.locked = true;
    index.insert_batch(
        &[
            edit.clone(),
            original.clone(),
            forged,
            original.clone(),
            locked.clone(),
        ],
        None,
    )?;
    assert_eq!(index.stats()?, (1, 1));
    assert!(
        index
            .search("obsolete", 0, None, None, || false)?
            .is_empty()
    );
    assert!(index.search("forged", 0, None, None, || false)?.is_empty());
    assert_eq!(index.search("corrected", 0, None, None, || false)?.len(), 1);
    locked.locked = false;
    index.insert_batch(&[locked], None)?;
    assert_eq!(index.stats()?, (2, 0));
    index.insert_batch(&[redaction("$edit")], None)?;
    assert_eq!(index.search("obsolete", 0, None, None, || false)?.len(), 1);
    index.insert_batch(&[redaction("$original"), redaction("$before")], None)?;
    drop(index);
    let mut index = Index::open(&path, &[1; 32], "room")?;
    index.insert_batch(
        &[original, edit, event("$before", "must stay redacted", 5)],
        None,
    )?;
    assert_eq!(index.stats()?, (1, 0));
    assert_eq!(index.search("", 0, None, None, || false)?[0].id, "$locked");
    Ok(())
}

#[test]
fn equal_timestamp_keyset_pages_have_no_loss_or_duplicates() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut index = Index::open(&dir.path().join("search.sqlite"), &[3; 32], "room")?;
    for start in [0, 1000] {
        let batch: Vec<_> = (start..(start + 1000).min(1205))
            .map(|i| event(&format!("$e{i}"), "中文", 5))
            .collect();
        index.insert_batch(&batch, None)?;
    }
    let mut seen = BTreeSet::new();
    let mut before: Option<(u64, String)> = None;
    loop {
        let mut page = index.search(
            "中文",
            0,
            None,
            before.as_ref().map(|p| (p.0, p.1.as_str())),
            || false,
        )?;
        assert!(page.len() <= 501);
        let more = page.len() > 500;
        page.truncate(500);
        for row in &page {
            assert!(seen.insert(row.id.clone()));
        }
        if !more {
            break;
        }
        let last = page.last().unwrap();
        before = Some((last.timestamp, last.id.clone()));
    }
    assert_eq!(seen.len(), 1205);
    assert!(index.search("", 0, Some(5), None, || false)?.is_empty());
    assert!(index.search("", 0, None, None, || true)?.is_empty());
    Ok(())
}

#[test]
fn filenames_filters_authenticated_corruption_and_rollback() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut index = Index::open(&dir.path().join("search.sqlite"), &[3; 32], "room")?;
    let mut file = event("$file", "caption", 1);
    file.value["content"]["msgtype"] = "m.file".into();
    file.value["content"]["filename"] = "文件.PDF".into();
    index.insert_batch(&[file], None)?;
    assert_eq!(index.search("文件.pdf", 2, None, None, || false)?.len(), 1);
    assert!(index.search("文件", 1, None, None, || false)?.is_empty());
    let huge = event("$huge", &"x".repeat(MAX_EVENT_BYTES), 2);
    assert!(
        index
            .insert_batch(
                &[event("$rollback", "invisible", 3), huge],
                Some(&Coverage {
                    cursor: Some("skipped".into()),
                    exhausted: true,
                    head_cursor: None
                })
            )
            .is_err()
    );
    assert_eq!(index.stats()?, (1, 0));
    assert!(index.coverage()?.cursor.is_none());
    index
        .conn
        .execute("UPDATE messages SET payload=zeroblob(length(payload))", [])?;
    assert!(index.search("", 0, None, None, || false).is_err());
    Ok(())
}

/// This deliberately uses the production encryption/token/transaction/query
/// paths. It is an on-disk storage stress test, not a rendering/network claim.
/// Run with TEMP/TMP and HAGENCY_HISTORY_STRESS_DIR on D:.
#[test]
#[ignore = "explicit million/ten-million on-disk stress run"]
fn million_and_ten_million_encrypted_storage() -> Result<()> {
    let root = std::env::var("HAGENCY_HISTORY_STRESS_DIR")?;
    let root = Path::new(&root);
    ensure!(
        root.is_absolute() && !root.exists(),
        "Use a fresh absolute stress directory"
    );
    std::fs::create_dir_all(root)?;
    let path = root.join("search.sqlite");
    let started = std::time::Instant::now();
    let mut index = Index::open(&path, &[19; 32], "synthetic/stress/room")?;
    for start in (0..10_000_000).step_by(1000) {
        let batch: Vec<_> = (start..start + 1000)
            .map(|i| {
                event(
                    &format!("$e{i}"),
                    if i % 1000 == 0 { "针" } else { "乙" },
                    i as u64,
                )
            })
            .collect();
        index.insert_batch(&batch, None)?;
        let total = start + 1000;
        if total % 100_000 == 0 {
            eprintln!(
                "stored={total} elapsed_s={:.3} bytes={}",
                started.elapsed().as_secs_f64(),
                std::fs::metadata(&path)?.len()
            );
        }
        if total == 1_000_000 || total == 10_000_000 {
            ensure!(index.stats()?.0 == total);
            let query_start = std::time::Instant::now();
            let result = index.search("针", 0, None, None, || false)?;
            ensure!(result.len() == 501 && result[0].timestamp == (total - 1000) as u64);
            let warm_ms = query_start.elapsed().as_secs_f64() * 1000.;
            drop(index);
            index = Index::open(&path, &[19; 32], "synthetic/stress/room")?;
            let reopen_start = std::time::Instant::now();
            ensure!(index.search("针", 0, None, None, || false)?.len() == 501);
            let cold_ms = reopen_start.elapsed().as_secs_f64() * 1000.;
            let all_start = std::time::Instant::now();
            ensure!(index.search("", 0, None, None, || false)?.len() == 501);
            eprintln!(
                "CHECKPOINT total={total} elapsed_s={:.3} bytes={} warm_ms={warm_ms:.3} reopen_ms={cold_ms:.3} latest_ms={:.3}",
                started.elapsed().as_secs_f64(),
                std::fs::metadata(&path)?.len(),
                all_start.elapsed().as_secs_f64() * 1000.
            );
        }
    }
    Ok(())
}

#[test]
fn rebuild_generation_fences_old_workers_and_cursor_updates_merge() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("search.sqlite");
    let mut first = Index::open(&path, &[4; 32], "room")?;
    let mut second = Index::open(&path, &[4; 32], "room")?;
    let initial = Coverage {
        cursor: Some("old".into()),
        exhausted: false,
        head_cursor: Some("head".into()),
    };
    first.insert_batch(&[], Some(&initial))?;
    let generation = first.generation()?;
    let mut forward = initial.clone();
    forward.head_cursor = Some("new-head".into());
    first.insert_page(&[event("$new", "live", 2)], &initial, &forward, &generation)?;
    let mut backward = initial.clone();
    backward.cursor = Some("older".into());
    let merged = second.insert_page(
        &[event("$old", "past", 1)],
        &initial,
        &backward,
        &generation,
    )?;
    assert_eq!(merged.head_cursor, forward.head_cursor);
    assert_eq!(merged.cursor, backward.cursor);
    assert!(
        second
            .insert_page(&[redaction("$new")], &initial, &forward, &generation)
            .is_err()
    );
    assert_eq!(first.stats()?.0, 2);
    Index::mark_stale(&path)?;
    assert!(first.search("", 0, None, None, || false).is_err());
    second.reset_stale()?;
    assert_ne!(generation, second.generation()?);
    assert!(
        first
            .insert_for_generation(&[event("$stale", "do not resurrect", 3)], None, &generation)
            .is_err()
    );
    assert_eq!(first.stats()?, (0, 0));
    Ok(())
}

#[test]
fn tied_edits_profile_upgrade_locked_session_paging_and_blob_binding() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut index = Index::open(&dir.path().join("search.sqlite"), &[4; 32], "room")?;
    index.insert_batch(
        &[
            event("$original", "before", 1),
            edit("$z", "$original", "chosen", 2),
            edit("$a", "$original", "loses", 2),
        ],
        None,
    )?;
    assert_eq!(index.search("chosen", 0, None, None, || false)?.len(), 1);
    index.insert_batch(
        &[edit(
            "$nested",
            "$z",
            "must not become a standalone message",
            3,
        )],
        None,
    )?;
    assert_eq!(index.stats()?.0, 1);
    assert!(
        index
            .search("standalone", 0, None, None, || false)?
            .is_empty()
    );

    let mut named = event("$original", "before", 1);
    named.name = "Resolved Person".into();
    index.insert_batch(&[named], None)?;
    assert_eq!(
        index
            .search("resolved person", 0, None, None, || false)?
            .len(),
        1
    );
    let mut locked = Vec::new();
    for i in 0..205 {
        let mut e = event(&format!("$locked{i}"), "locked", 3 + i);
        e.locked = true;
        e.value["content"]["session_id"] = "one-session".into();
        locked.push(e);
    }
    index.insert_batch(&locked, None)?;
    let mut after = None;
    let mut total = 0;
    loop {
        let page = index.locked_page(Some("one-session"), after.as_deref())?;
        assert!(page.len() <= 100);
        if page.is_empty() {
            break;
        }
        after = page.last().unwrap().value["event_id"]
            .as_str()
            .map(str::to_owned);
        total += page.len();
        let upgraded: Vec<_> = page
            .into_iter()
            .map(|mut e| {
                e.locked = false;
                e
            })
            .collect();
        index.insert_batch(&upgraded, None)?;
    }
    assert_eq!(total, 205);
    assert_eq!(index.stats()?, (206, 0));
    index.insert_batch(&[event("$other", "other", 4)], None)?;
    let key = index.key("$original");
    let other = index.key("$other");
    index.conn.execute(
        "UPDATE events SET payload=(SELECT payload FROM events WHERE id=?1) WHERE id=?2",
        params![&other[..], &key[..]],
    )?;
    assert!(index.event(&key).is_err());
    Ok(())
}

#[test]
fn oversized_duplicates_and_token_expansion_roll_back_without_large_serialization() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut index = Index::open(&dir.path().join("search.sqlite"), &[5; 32], "room")?;
    index.insert_batch(&[event("$one", "body", 1)], None)?;
    let mut duplicate = event("$one", "body", 1);
    duplicate.name = "x".repeat(MAX_EVENT_BYTES);
    assert!(index.insert_batch(&[duplicate], None).is_err());
    assert_eq!(index.stats()?, (1, 0));
    let expanded: String = (0..17000)
        .filter_map(|i| char::from_u32(0x4000 + i))
        .collect();
    assert!(
        index
            .insert_batch(&[event("$expanded", &expanded, 2)], None)
            .is_err()
    );
    assert_eq!(index.stats()?, (1, 0));
    Ok(())
}

/// Reopens the completed real ten-million database with the FINAL code and Git
/// dependency graph. This is connection reopening, not an OS cache flush.
#[test]
#[ignore = "requires completed real ten-million database"]
fn verify_completed_ten_million_with_current_code() -> Result<()> {
    let root = std::env::var("HAGENCY_HISTORY_VERIFY_DIR")?;
    let path = Path::new(&root).join("search.sqlite");
    ensure!(path.is_file(), "Completed stress database is required");
    let open_started = std::time::Instant::now();
    let mut index = Index::open(&path, &[19; 32], "synthetic/stress/room")?;
    ensure!(index.stats()? == (10_000_000, 0));
    eprintln!(
        "FINAL reopen_s={:.3} bytes={}",
        open_started.elapsed().as_secs_f64(),
        std::fs::metadata(&path)?.len()
    );
    let mut before: Option<(u64, String)> = None;
    let mut total = 0;
    let mut pages = 0;
    let started = std::time::Instant::now();
    loop {
        let mut page = index.search(
            "针",
            0,
            None,
            before.as_ref().map(|p| (p.0, p.1.as_str())),
            || false,
        )?;
        let more = page.len() > 500;
        page.truncate(500);
        for message in &page {
            ensure!(message.timestamp == (9_999_000 - total * 1000) as u64);
            total += 1;
        }
        pages += 1;
        if !more {
            break;
        }
        let last = page.last().unwrap();
        before = Some((last.timestamp, last.id.clone()));
    }
    ensure!(total == 10_000 && pages == 20);
    let broad = std::time::Instant::now();
    ensure!(index.search("乙", 0, None, None, || false)?.len() == 501);
    let broad_ms = broad.elapsed().as_secs_f64() * 1000.;
    ensure!(index.search("", 1, None, None, || false)?.is_empty());
    let suffix = std::time::Instant::now();
    ensure!(
        index
            .search("乙不存在", 0, None, None, || false)?
            .is_empty()
    );
    let suffix_ms = suffix.elapsed().as_secs_f64() * 1000.;
    eprintln!(
        "FINAL needle_pages={pages} matches={total} total_ms={:.3} common_ms={broad_ms:.3} absent_suffix_ms={suffix_ms:.3}",
        started.elapsed().as_secs_f64() * 1000.
    );
    // Exercise FINAL insertion/deletion at actual ten-million index depth.
    // Unique IDs make this verification repeatable; only tombstones remain.
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let batch: Vec<_> = (0..1000)
        .map(|i| {
            event(
                &format!("$final-{run}-{i}"),
                "final-code 中文 encrypted insert at ten million",
                10_000_000 + i,
            )
        })
        .collect();
    let final_insert = std::time::Instant::now();
    index.insert_batch(&batch, None)?;
    ensure!(index.stats()?.0 == 10_001_000);
    ensure!(index.search("final-code", 0, None, None, || false)?.len() == 501);
    let inserted_ms = final_insert.elapsed().as_secs_f64() * 1000.;
    let removals: Vec<_> = batch
        .iter()
        .map(|event| redaction(event.value["event_id"].as_str().unwrap()))
        .collect();
    index.insert_batch(&removals, None)?;
    ensure!(index.stats()?.0 == 10_000_000);
    ensure!(
        index
            .search("final-code", 0, None, None, || false)?
            .is_empty()
    );
    eprintln!(
        "FINAL current_code_insert_1000_ms={inserted_ms:.3} restored_messages=10000000 bytes={}",
        std::fs::metadata(&path)?.len()
    );
    Ok(())
}

#[test]
fn edit_author_schema_upgrade_and_disk_full_preserve_transactions() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("search.sqlite");
    let mut index = Index::open(&path, &[6; 32], "room")?;
    index.insert_batch(
        &[
            event("$original", "before", 1),
            edit("$edit", "$original", "after", 2),
        ],
        None,
    )?;
    index.conn.execute_batch("DROP INDEX edit_authors; DROP INDEX missing_edit_authors; ALTER TABLE events DROP COLUMN author;")?;
    drop(index);
    let mut index = Index::open(&path, &[6; 32], "room")?;
    assert_eq!(index.search("after", 0, None, None, || false)?.len(), 1);
    let pages: i64 = index
        .conn
        .query_row("PRAGMA page_count", [], |row| row.get(0))?;
    index.conn.pragma_update(None, "max_page_count", pages)?;
    let batch: Vec<_> = (0..100)
        .map(|i| {
            event(
                &format!("$full{i}"),
                "disk space must not skip this transaction",
                10 + i,
            )
        })
        .collect();
    assert!(
        index
            .insert_batch(
                &batch,
                Some(&Coverage {
                    cursor: Some("must-not-advance".into()),
                    exhausted: true,
                    head_cursor: None
                })
            )
            .is_err()
    );
    assert_eq!(index.stats()?, (1, 0));
    assert!(index.coverage()?.cursor.is_none());
    let plan:Vec<String>=index.conn.prepare("EXPLAIN QUERY PLAN SELECT t.id,m.payload,t.ts FROM tokens t JOIN messages m ON m.id=t.id
        WHERE t.ts=?1 AND t.id>?2 AND t.ts>?3 AND t.token=?4 ORDER BY t.id")?
        .query_map(params![2,&[0u8;32][..],-1,&[0u8;32][..]],|row|row.get(3))?.collect::<rusqlite::Result<_>>()?;
    assert!(
        !plan.iter().any(|line| line.contains("TEMP B-TREE")),
        "{plan:?}"
    );
    assert!(plan.iter().any(|line| line.contains("SEARCH")), "{plan:?}");
    Ok(())
}
