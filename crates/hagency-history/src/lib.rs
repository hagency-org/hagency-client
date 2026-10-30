//! Bounded-memory, encrypted Matrix history search. All calls are blocking.
//!
//! Payloads and the random index secret use the SDK's authenticated StoreCipher.
//! Only keyed event/substring identifiers, timestamps, counts and relationships
//! are visible to SQLite. Equality/frequency/access-pattern leakage is inherent
//! in this blind index; this is not an oblivious database or SQLCipher.
//! No plaintext FTS, raw event ID, sender, filename or message is written.
use std::{collections::BTreeSet, path::Path};

use anyhow::{Context, Result, ensure};
use matrix_sdk_store_encryption::StoreCipher;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_EVENT_BYTES: usize = 256 * 1024;
const PAGE_SIZE: usize = 501;
const MAX_DISK_BYTES: i64 = 32 * 1024 * 1024 * 1024;

fn validate_serialized(value: &impl Serialize, limit: usize) -> Result<()> {
    struct Budget(usize);
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(std::io::Error::other(
                    "History record exceeds serialization budget",
                ));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Budget(limit), value)?;
    Ok(())
}

fn bounded_blob(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<Vec<u8>> {
    let bytes = row.get_ref(column)?.as_blob()?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Blob,
            Box::new(std::io::Error::other(
                "History encrypted record exceeds budget",
            )),
        ));
    }
    Ok(bytes.to_vec())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub value: Value,
    pub name: String,
    pub locked: bool,
}

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub sender: String,
    pub name: String,
    pub timestamp: u64,
    pub content: Value,
}

impl Message {
    fn fields(&self) -> [&str; 3] {
        [
            self.content["body"].as_str().unwrap_or_default(),
            &self.name,
            self.content["filename"].as_str().unwrap_or_default(),
        ]
    }

    fn matches(&self, query: &str, filter: u8) -> bool {
        let kind = self.content["msgtype"].as_str().unwrap_or_default();
        (match filter {
            0 => true,
            1 => matches!(kind, "m.image" | "m.video"),
            2 => kind == "m.file",
            3 => kind == "m.audio",
            _ => false,
        }) && (query.is_empty()
            || self
                .fields()
                .iter()
                .any(|s| s.to_lowercase().contains(query)))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Coverage {
    pub cursor: Option<String>,
    pub exhausted: bool,
    /// Independent /messages forward cursor for crash-safe live catch-up.
    #[serde(default)]
    pub head_cursor: Option<String>,
}

/// One connection per open room panel, never one connection per historical room.
pub struct Index {
    conn: Connection,
    cipher: StoreCipher,
}

impl Index {
    /// `wrapping_key` must come from the authenticated account's protected secret,
    /// and be domain separated by homeserver, user and device. It is never saved.
    pub fn open(path: &Path, wrapping_key: &[u8; 32], identity: &str) -> Result<Self> {
        // Account/device/room hashes can exceed the default Windows VFS path
        // budget. Its wide Win32 calls also need a verbatim absolute path;
        // canonicalize the existing parent so creation keeps the original name.
        #[cfg(windows)]
        let path = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()?
            .join(
                path.file_name()
                    .context("History database filename missing")?,
            );
        // Keep SQLite's ordinary locking and journal implementation.
        #[cfg(windows)]
        let conn = Connection::open_with_flags_and_vfs(
            &path,
            rusqlite::OpenFlags::default(),
            "win32-longpath",
        )?;
        #[cfg(not(windows))]
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE;
            PRAGMA synchronous=FULL; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;
            PRAGMA secure_delete=ON; PRAGMA temp_store=FILE;
            CREATE TABLE IF NOT EXISTS metadata(k TEXT PRIMARY KEY, v BLOB NOT NULL);",
        )?;
        let pages =
            MAX_DISK_BYTES / conn.query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0))?;
        conn.pragma_update(None, "max_page_count", pages)?;
        let tx =
            rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        let exported: Option<Vec<u8>> = conn
            .query_row("SELECT v FROM metadata WHERE k='cipher-v1'", [], |r| {
                bounded_blob(r, 0)
            })
            .optional()?;
        let cipher = if let Some(exported) = exported {
            StoreCipher::import_with_key(wrapping_key, &exported)
                .context("Cannot unlock history index")?
        } else {
            let cipher = StoreCipher::new()?;
            conn.execute(
                "INSERT INTO metadata VALUES('cipher-v1',?1)",
                [cipher.export_with_key(wrapping_key)?],
            )?;
            conn.execute(
                "INSERT INTO metadata VALUES('identity',?1)",
                [cipher.encrypt_value(&identity)?],
            )?;
            cipher
        };
        let stored_identity: Vec<u8> =
            conn.query_row("SELECT v FROM metadata WHERE k='identity'", [], |r| {
                bounded_blob(r, 0)
            })?;
        ensure!(
            cipher.decrypt_value::<String>(&stored_identity)? == identity,
            "History index identity mismatch"
        );
        conn.execute_batch("CREATE TABLE IF NOT EXISTS events(
            id BLOB PRIMARY KEY, target BLOB, ts INTEGER NOT NULL, kind INTEGER NOT NULL,
            payload BLOB NOT NULL, author BLOB) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS relations ON events(target,ts DESC,id DESC);
            CREATE INDEX IF NOT EXISTS locked_events ON events(id) WHERE kind=2;
            CREATE TABLE IF NOT EXISTS locked_sessions(session BLOB,id BLOB,PRIMARY KEY(session,id),
                FOREIGN KEY(id) REFERENCES events(id) ON DELETE CASCADE) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS redactions(id BLOB PRIMARY KEY) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS messages(id BLOB PRIMARY KEY,ts INTEGER NOT NULL,payload BLOB NOT NULL) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS chronology ON messages(ts DESC,id);
            CREATE TABLE IF NOT EXISTS tokens(token BLOB,ts INTEGER,id BLOB,
                PRIMARY KEY(token,ts DESC,id), FOREIGN KEY(id) REFERENCES messages(id) ON DELETE CASCADE) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS message_tokens ON tokens(id);
            CREATE TABLE IF NOT EXISTS stats(singleton INTEGER PRIMARY KEY CHECK(singleton=1),messages INTEGER,locked INTEGER);
            INSERT OR IGNORE INTO stats VALUES(1,0,0);
            CREATE TRIGGER IF NOT EXISTS message_added AFTER INSERT ON messages BEGIN UPDATE stats SET messages=messages+1; END;
            CREATE TRIGGER IF NOT EXISTS message_removed AFTER DELETE ON messages BEGIN UPDATE stats SET messages=messages-1; END;
            CREATE TRIGGER IF NOT EXISTS locked_added AFTER INSERT ON events WHEN NEW.kind=2 BEGIN UPDATE stats SET locked=locked+1; END;
            CREATE TRIGGER IF NOT EXISTS locked_removed AFTER DELETE ON events WHEN OLD.kind=2 BEGIN UPDATE stats SET locked=locked-1; END;")?;
        let has_author = conn
            .prepare("PRAGMA table_info(events)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|name| name == "author");
        if !has_author {
            conn.execute_batch("ALTER TABLE events ADD COLUMN author BLOB;")?;
        }
        conn.execute_batch("CREATE INDEX IF NOT EXISTS edit_authors ON events(target,author,ts DESC,id DESC) WHERE kind=1;
            CREATE INDEX IF NOT EXISTS missing_edit_authors ON events(id) WHERE kind=1 AND author IS NULL;")?;
        // Upgrade older derived indexes one authenticated edit at a time.
        loop {
            let missing=conn.query_row("SELECT id,payload FROM events WHERE kind=1 AND author IS NULL ORDER BY id LIMIT 1",[],
                |row|Ok((bounded_blob(row,0)?,bounded_blob(row,1)?))).optional()?;
            let Some((id, blob)) = missing else {
                break;
            };
            let event: Event = cipher.decrypt_value(&blob)?;
            ensure!(
                event.value["event_id"]
                    .as_str()
                    .is_some_and(|event_id| cipher
                        .hash_key("event-v1", event_id.as_bytes())
                        .as_slice()
                        == id),
                "History migration binding mismatch"
            );
            let sender = event.value["sender"].as_str().unwrap_or_default();
            conn.execute(
                "UPDATE events SET author=?1 WHERE id=?2",
                params![&cipher.hash_key("sender-v1", sender.as_bytes())[..], id],
            )?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO metadata VALUES('generation-v1',?1)",
            [cipher.encrypt_value(&"generation")?],
        )?;
        tx.commit()?;
        Ok(Self { conn, cipher })
    }

    fn key(&self, id: &str) -> [u8; 32] {
        self.cipher.hash_key("event-v1", id.as_bytes())
    }

    pub fn stats(&self) -> Result<(usize, usize)> {
        Ok(self.conn.query_row(
            "SELECT messages,locked FROM stats WHERE singleton=1",
            [],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
        )?)
    }

    pub fn coverage(&self) -> Result<Coverage> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT v FROM metadata WHERE k='coverage'", [], |r| {
                bounded_blob(r, 0)
            })
            .optional()?;
        blob.map(|v| self.cipher.decrypt_value(&v).map_err(Into::into))
            .unwrap_or_else(|| Ok(Coverage::default()))
    }

    /// A missed sync broadcast or failed update invalidates this derived cache.
    /// Existing readers also check this marker before returning results.
    pub fn mark_stale(path: &Path) -> Result<()> {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute("INSERT OR REPLACE INTO metadata VALUES('stale',X'01')", [])?;
        Ok(())
    }

    pub fn is_stale(&self) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM metadata WHERE k='stale')",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn reset_stale(&mut self) -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        if self.is_stale()? {
            self.conn.execute_batch("DELETE FROM tokens; DELETE FROM messages; DELETE FROM locked_sessions; DELETE FROM events; DELETE FROM redactions;
                DELETE FROM metadata WHERE k IN ('stale','coverage');")?;
            self.conn.execute(
                "UPDATE metadata SET v=?1 WHERE k='generation-v1'",
                [self.cipher.encrypt_value(&"generation")?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Events and continuation are committed together; a crash cannot skip a page.
    /// The caller must bound a batch (the production caller uses 100 events).
    pub fn insert_batch(&mut self, events: &[Event], coverage: Option<&Coverage>) -> Result<()> {
        self.insert_checked(events, coverage, None, None)
            .map(|_| ())
    }

    pub fn generation(&self) -> Result<Vec<u8>> {
        Ok(self.conn.query_row(
            "SELECT v FROM metadata WHERE k='generation-v1'",
            [],
            |row| bounded_blob(row, 0),
        )?)
    }

    pub fn insert_for_generation(
        &mut self,
        events: &[Event],
        coverage: Option<&Coverage>,
        generation: &[u8],
    ) -> Result<()> {
        self.insert_checked(events, coverage, Some(generation), None)
            .map(|_| ())
    }

    /// Merge independent forward/backward cursor changes under the write
    /// transaction. A concurrent update to the same cursor must be retried.
    pub fn insert_page(
        &mut self,
        events: &[Event],
        previous: &Coverage,
        next: &Coverage,
        generation: &[u8],
    ) -> Result<Coverage> {
        self.insert_checked(events, Some(next), Some(generation), Some(previous))
    }

    fn insert_checked(
        &mut self,
        events: &[Event],
        coverage: Option<&Coverage>,
        generation: Option<&[u8]>,
        previous: Option<&Coverage>,
    ) -> Result<Coverage> {
        ensure!(
            events.len() <= 1000,
            "History index batch exceeds 1000 events"
        );
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        ensure!(
            !self.is_stale()?,
            "History index missed updates; search again to rebuild"
        );
        if let Some(generation) = generation {
            ensure!(
                self.generation()?.as_slice() == generation,
                "History index generation changed; retry"
            );
        }
        let mut merged = self.coverage()?;
        if let Some(next) = coverage {
            if let Some(previous) = previous {
                if next.head_cursor != previous.head_cursor {
                    ensure!(
                        merged.head_cursor == previous.head_cursor,
                        "History forward cursor changed; retry"
                    );
                    merged.head_cursor = next.head_cursor.clone();
                }
                if next.cursor != previous.cursor || next.exhausted != previous.exhausted {
                    ensure!(
                        merged.cursor == previous.cursor && merged.exhausted == previous.exhausted,
                        "History backward cursor changed; retry"
                    );
                    merged.cursor = next.cursor.clone();
                    merged.exhausted = next.exhausted;
                }
            } else {
                merged = next.clone();
            }
            validate_serialized(&merged, 64 * 1024)?;
        }
        for event in events {
            self.insert(event)?;
        }
        if coverage.is_some() {
            self.conn.execute(
                "INSERT OR REPLACE INTO metadata VALUES('coverage',?1)",
                [self.cipher.encrypt_value(&merged)?],
            )?;
        }
        tx.commit()?;
        Ok(merged)
    }

    fn event(&self, key: &[u8]) -> Result<Option<Event>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT payload FROM events WHERE id=?1", [key], |r| {
                bounded_blob(r, 0)
            })
            .optional()?;
        let event: Option<Event> = blob
            .map(|v| self.cipher.decrypt_value(&v).map_err(anyhow::Error::from))
            .transpose()?;
        if let Some(event) = &event {
            ensure!(
                event.value["event_id"]
                    .as_str()
                    .is_some_and(|id| self.key(id).as_slice() == key),
                "History event binding mismatch"
            );
        }
        Ok(event)
    }

    /// At most 100 encrypted source events per decryption retry; never load
    /// the entire historical UTD set into a Vec.
    pub fn locked_page(&self, session: Option<&str>, after: Option<&str>) -> Result<Vec<Event>> {
        let after = after.map(|id| self.key(id)).unwrap_or([0; 32]);
        let mut events = Vec::new();
        let keys: Vec<Vec<u8>> = if let Some(session) = session {
            self.conn.prepare_cached("SELECT id FROM locked_sessions WHERE session=?1 AND id>?2 ORDER BY id LIMIT 100")?
                .query_map(params![&self.cipher.hash_key("session-v1",session.as_bytes())[..],&after[..]],|r|r.get(0))?.collect::<rusqlite::Result<_>>()?
        } else {
            self.conn
                .prepare_cached(
                    "SELECT id FROM events WHERE kind=2 AND id>?1 ORDER BY id LIMIT 100",
                )?
                .query_map([&after[..]], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for key in keys {
            if let Some(event) = self.event(&key)? {
                events.push(event);
            }
        }
        Ok(events)
    }

    pub fn locked_session(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .event(&self.key(id))?
            .filter(|event| event.locked)
            .and_then(|event| {
                event
                    .value
                    .pointer("/content/session_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }))
    }

    fn redacted(&self, key: &[u8]) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM redactions WHERE id=?1)",
            [key],
            |r| r.get(0),
        )?)
    }

    fn redact(&self, key: &[u8]) -> Result<()> {
        self.conn
            .execute("INSERT OR IGNORE INTO redactions VALUES(?1)", [key])?;
        let parent: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT target FROM events WHERE id=?1", [key], |r| r.get(0))
            .optional()?
            .flatten();
        // Remove encrypted source too. Never resurrect it from an overlapping page.
        self.conn.execute("DELETE FROM events WHERE id=?1", [key])?;
        self.conn
            .execute("DELETE FROM messages WHERE id=?1", [key])?;
        if let Some(parent) = parent {
            self.rebuild(&parent)?;
        }
        Ok(())
    }

    fn insert(&self, event: &Event) -> Result<()> {
        validate_serialized(event, MAX_EVENT_BYTES)?;
        let value = &event.value;
        let Some(id) = value["event_id"].as_str() else {
            return Ok(());
        };
        let key = self.key(id);
        if value.pointer("/unsigned/redacted_because").is_some() {
            return self.redact(&key);
        }
        if self.redacted(&key)? {
            return Ok(());
        }
        if value["type"] == "m.room.redaction" {
            if let Some(target) = value
                .get("redacts")
                .or_else(|| value.pointer("/content/redacts"))
                .and_then(Value::as_str)
            {
                self.redact(&self.key(target))?;
            }
            return Ok(());
        }
        if !event.locked && value["type"] != "m.room.message" {
            return Ok(());
        }
        let timestamp = value["origin_server_ts"].as_u64().unwrap_or(0);
        ensure!(timestamp <= i64::MAX as u64, "Invalid history timestamp");
        let replacement = (value
            .pointer("/content/m.relates_to/rel_type")
            .and_then(Value::as_str)
            == Some("m.replace"))
        .then(|| {
            value
                .pointer("/content/m.relates_to/event_id")
                .and_then(Value::as_str)
        })
        .flatten();
        let target = replacement.map(|id| self.key(id));
        let kind = if event.locked {
            2
        } else if target.is_some() {
            1
        } else {
            0
        };
        let old_kind: Option<u8> = self
            .conn
            .query_row("SELECT kind FROM events WHERE id=?1", [&key[..]], |r| {
                r.get(0)
            })
            .optional()?;
        if old_kind.is_some_and(|old| old != 2 || event.locked) {
            // The sync watcher may have indexed a sender ID before the history
            // panel resolved its display name. Never replace a known name with
            // the watcher's unresolved fallback.
            if kind == 0
                && old_kind == Some(0)
                && event.name != value["sender"].as_str().unwrap_or_default()
                && let Some(mut previous) = self.event(&key)?
                && previous.name != event.name
            {
                previous.name = event.name.clone();
                validate_serialized(&previous, MAX_EVENT_BYTES)?;
                self.conn.execute(
                    "UPDATE events SET payload=?1 WHERE id=?2",
                    params![self.cipher.encrypt_value(&previous)?, &key[..]],
                )?;
                self.rebuild(&key)?;
            }
            return Ok(());
        }
        self.conn
            .execute("DELETE FROM events WHERE id=?1", [&key[..]])?;
        let author = (kind == 1).then(|| {
            self.cipher.hash_key(
                "sender-v1",
                value["sender"].as_str().unwrap_or_default().as_bytes(),
            )
        });
        self.conn.execute(
            "INSERT INTO events VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                &key[..],
                target.as_ref().map(|v| &v[..]),
                timestamp as i64,
                kind,
                self.cipher.encrypt_value(event)?,
                author.as_ref().map(|hash| &hash[..])
            ],
        )?;
        if event.locked
            && let Some(session) = value.pointer("/content/session_id").and_then(Value::as_str)
        {
            self.conn.execute(
                "INSERT OR IGNORE INTO locked_sessions VALUES(?1,?2)",
                params![
                    &self.cipher.hash_key("session-v1", session.as_bytes())[..],
                    &key[..]
                ],
            )?;
        }
        if !event.locked {
            self.rebuild(target.as_ref().map(|v| &v[..]).unwrap_or(&key))?;
        }
        Ok(())
    }

    fn rebuild(&self, key: &[u8]) -> Result<()> {
        self.conn
            .execute("DELETE FROM messages WHERE id=?1", [key])?;
        if self.redacted(key)? {
            return Ok(());
        }
        let Some(event) = self.event(key)? else {
            return Ok(());
        };
        if event.locked
            || event
                .value
                .pointer("/content/m.relates_to/rel_type")
                .and_then(Value::as_str)
                == Some("m.replace")
        {
            return Ok(());
        }
        let value = event.value;
        let (Some(id), Some(sender), Some(timestamp)) = (
            value["event_id"].as_str(),
            value["sender"].as_str(),
            value["origin_server_ts"].as_u64(),
        ) else {
            return Ok(());
        };
        let mut content = value["content"].clone();
        // Read one edit at a time. Even millions of edits to one event use O(1)
        // decoded records, and forged edits cannot hide the authorized one.
        let mut edits = self.conn.prepare_cached(
            "SELECT e.payload,e.id,e.ts FROM events e LEFT JOIN redactions r ON r.id=e.id
            WHERE e.target=?1 AND e.author=?2 AND e.kind=1 AND r.id IS NULL ORDER BY e.ts DESC,e.id DESC",
        )?;
        let mut rows = edits.query(params![
            key,
            &self.cipher.hash_key("sender-v1", sender.as_bytes())[..]
        ])?;
        let mut selected: Option<Event> = None;
        while let Some(row) = rows.next()? {
            let blob = row.get_ref(0)?.as_blob()?;
            ensure!(
                blob.len() <= 2 * 1024 * 1024,
                "History edit exceeds encrypted record budget"
            );
            let edit: Event = self.cipher.decrypt_value(blob)?;
            let edit_id = edit.value["event_id"].as_str().unwrap_or_default();
            let edit_ts = edit.value["origin_server_ts"].as_u64().unwrap_or(0);
            ensure!(
                self.key(edit_id).as_slice() == row.get_ref(1)?.as_blob()?
                    && edit_ts == row.get::<_, i64>(2)? as u64
                    && edit
                        .value
                        .pointer("/content/m.relates_to/event_id")
                        .and_then(Value::as_str)
                        .is_some_and(|target| self.key(target).as_slice() == key),
                "History edit binding mismatch"
            );
            if selected.as_ref().is_some_and(|selected| {
                selected.value["origin_server_ts"].as_u64().unwrap_or(0) > edit_ts
            }) {
                break;
            }
            if edit.value["sender"].as_str() == Some(sender) {
                let replacement = &edit.value["content"]["m.new_content"];
                if replacement["msgtype"].is_string()
                    && replacement["body"].is_string()
                    && selected.as_ref().is_none_or(|previous| {
                        previous.value["event_id"].as_str().unwrap_or_default() < edit_id
                    })
                {
                    selected = Some(edit);
                }
            }
        }
        if let Some(edit) = selected {
            content = edit.value["content"]["m.new_content"].clone();
        }
        if !content["msgtype"].is_string() || !content["body"].is_string() {
            return Ok(());
        }
        let message = Message {
            id: id.into(),
            sender: sender.into(),
            name: event.name,
            timestamp,
            content,
        };
        self.conn.execute(
            "INSERT INTO messages VALUES(?1,?2,?3)",
            params![key, timestamp as i64, self.cipher.encrypt_value(&message)?],
        )?;
        let mut insert = self
            .conn
            .prepare_cached("INSERT INTO tokens VALUES(?1,?2,?3)")?;
        for token in self.tokens(&message)? {
            insert.execute(params![&token[..], timestamp as i64, key])?;
        }
        Ok(())
    }

    fn tokens(&self, message: &Message) -> Result<BTreeSet<[u8; 32]>> {
        let mut tokens = BTreeSet::new();
        let filter = match message.content["msgtype"].as_str().unwrap_or_default() {
            "m.image" | "m.video" => 1,
            "m.file" => 2,
            "m.audio" => 3,
            _ => 0,
        };
        if filter != 0 {
            tokens.insert(self.cipher.hash_key("filter-v1", &[filter]));
        }
        for field in message.fields() {
            let lower = field.to_lowercase();
            let mut chars = lower.char_indices().map(|(i, _)| i).peekable();
            while let Some(start) = chars.next() {
                let mut ends = chars.clone();
                for _ in 0..3 {
                    let end = ends.next().unwrap_or(lower.len());
                    tokens.insert(
                        self.cipher
                            .hash_key("substring-v1", &lower.as_bytes()[start..end]),
                    );
                    ensure!(
                        tokens.len() <= 16384,
                        "History event exceeds substring token budget"
                    );
                    if end == lower.len() {
                        break;
                    }
                }
            }
        }
        Ok(tokens)
    }

    /// Keyset pagination reads/decrypts one candidate at a time and returns <=501.
    /// No full-table materialization, OFFSET or in-memory sort. The cursor's ID
    /// is ordered by its keyed digest (stable within this index).
    pub fn search(
        &self,
        query: &str,
        filter: u8,
        cutoff: Option<u64>,
        before: Option<(u64, &str)>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Vec<Message>> {
        // The read transaction prevents a sync redaction/reset racing a query.
        let tx = self.conn.unchecked_transaction()?;
        ensure!(
            !self.is_stale()?,
            "History index missed updates; search again to rebuild"
        );
        ensure!(query.len() <= 4096, "History query exceeds 4096-byte limit");
        let started = std::time::Instant::now();
        let query = query.trim().to_lowercase();
        let characters: Vec<_> = query
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(query.len()))
            .collect();
        let length = characters.len() - 1;
        let width = length.min(3);
        let grams = length.saturating_sub(width) + 1;
        let mut token = self
            .cipher
            .hash_key("substring-v1", &query.as_bytes()[..characters[width]]);
        // Sample at most 32 grams and count only the first 1024 postings.
        // A rare suffix should not decrypt millions of common-prefix matches.
        let samples = grams.min(32);
        ensure!(filter <= 3, "Invalid history filter");
        let use_tokens = !query.is_empty() || filter != 0;
        let mut best = usize::MAX;
        if filter != 0 {
            token = self.cipher.hash_key("filter-v1", &[filter]);
            best = self.conn.query_row(
                "SELECT count(*) FROM (SELECT 1 FROM tokens WHERE token=?1 LIMIT 1024)",
                [&token[..]],
                |row| row.get(0),
            )?;
            if best == 0 {
                tx.commit()?;
                return Ok(Vec::new());
            }
        }
        if !query.is_empty() {
            for sample in 0..samples {
                if cancelled() {
                    return Ok(Vec::new());
                }
                let offset = if samples == 1 {
                    0
                } else {
                    sample * (grams - 1) / (samples - 1)
                };
                let candidate = self.cipher.hash_key(
                    "substring-v1",
                    &query.as_bytes()[characters[offset]..characters[offset + width]],
                );
                let count: usize = self.conn.query_row(
                    "SELECT count(*) FROM (SELECT 1 FROM tokens WHERE token=?1 LIMIT 1024)",
                    [&candidate[..]],
                    |row| row.get(0),
                )?;
                if count < best {
                    best = count;
                    token = candidate;
                }
                if count == 0 {
                    tx.commit()?;
                    return Ok(Vec::new());
                }
            }
        }
        let mut candidates = 0usize;
        let before_ts = before.map_or(i64::MAX, |p| p.0.min(i64::MAX as u64) as i64);
        let before_key = before.map(|p| self.key(p.1));
        let cutoff = cutoff.map_or(-1, |t| t.min(i64::MAX as u64) as i64);
        let mut queries: Vec<(&str, Vec<rusqlite::types::Value>)> = Vec::new();
        if let Some(key) = before_key {
            if !use_tokens {
                queries.push(("SELECT id,payload,ts FROM messages WHERE ts=?1 AND id>?2 AND ts>?3 ORDER BY id",
                    vec![before_ts.into(),key.to_vec().into(),cutoff.into()]));
            } else {
                queries.push((
                    "SELECT t.id,m.payload,t.ts FROM tokens t JOIN messages m ON m.id=t.id
                    WHERE t.ts=?1 AND t.id>?2 AND t.ts>?3 AND t.token=?4 ORDER BY t.id",
                    vec![
                        before_ts.into(),
                        key.to_vec().into(),
                        cutoff.into(),
                        token.to_vec().into(),
                    ],
                ));
            }
        }
        // Split the equal-time tie from the older range: an OR predicate would
        // rescan the entire equal-time prefix on every page (quadratic for a
        // million messages sharing a timestamp).
        let upper = if before.is_some() {
            before_ts
        } else {
            i64::MAX
        };
        if !use_tokens {
            queries.push((
                if before.is_some() {
                    "SELECT id,payload,ts FROM messages WHERE ts>?1 AND ts<?2 ORDER BY ts DESC,id"
                } else {
                    "SELECT id,payload,ts FROM messages WHERE ts>?1 AND ts<=?2 ORDER BY ts DESC,id"
                },
                vec![cutoff.into(), upper.into()],
            ));
        } else {
            queries.push((
                if before.is_some() {
                    "SELECT t.id,m.payload,t.ts FROM tokens t JOIN messages m ON m.id=t.id
                    WHERE t.ts>?1 AND t.ts<?2 AND t.token=?3 ORDER BY t.ts DESC,t.id"
                } else {
                    "SELECT t.id,m.payload,t.ts FROM tokens t JOIN messages m ON m.id=t.id
                    WHERE t.ts>?1 AND t.ts<=?2 AND t.token=?3 ORDER BY t.ts DESC,t.id"
                },
                vec![cutoff.into(), upper.into(), token.to_vec().into()],
            ));
        }
        let mut result = Vec::new();
        for (sql, values) in queries {
            let mut stmt = self.conn.prepare_cached(sql)?;
            let mut rows = stmt.query(rusqlite::params_from_iter(values))?;
            while let Some(row) = rows.next()? {
                if cancelled() {
                    return Ok(Vec::new());
                }
                candidates += 1;
                ensure!(
                    candidates <= 32768 && started.elapsed() < std::time::Duration::from_secs(3),
                    "History query is too broad; refine the text or attachment filter"
                );
                let blob = row.get_ref(1)?.as_blob()?;
                ensure!(
                    blob.len() <= 2 * 1024 * 1024,
                    "History payload exceeds encrypted record budget"
                );
                let message: Message = self.cipher.decrypt_value(blob)?;
                ensure!(
                    self.key(&message.id).as_slice() == row.get_ref(0)?.as_blob()?
                        && message.timestamp == row.get::<_, i64>(2)? as u64,
                    "History message binding mismatch"
                );
                if message.matches(&query, filter) {
                    result.push(message);
                }
                if result.len() == PAGE_SIZE {
                    break;
                }
            }
            if result.len() == PAGE_SIZE {
                break;
            }
        }
        tx.commit()?;
        Ok(result)
    }
}
