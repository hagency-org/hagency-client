use super::*;
/// Only called inside the already validated source transaction. No pathname or
/// opener is accepted. Columns are byte-bounded before SQLite allocates strings.
pub(in super::super) fn capture(db: &Connection, deadline: Instant) -> Result<History, Error> {
    let bounded = |name: &str, max: usize| {
        format!(
            "CASE WHEN length(CAST({name} AS BLOB))<={} THEN {name} END",
            max * 4
        )
    };
    let columns = [
        ("id", 64),
        ("title", 255),
        ("description", 4096),
        ("status", 16),
        ("priority", 8),
        ("granularity", 16),
        ("assignee", 128),
        ("created_by", 128),
    ];
    let mut fields = columns
        .iter()
        .map(|(n, m)| bounded(n, *m))
        .collect::<Vec<_>>();
    fields.extend(
        [
            "created_at",
            "updated_at",
            "started_at",
            "completed_at",
            "heartbeat_at",
        ]
        .map(String::from),
    );
    fields.extend(
        [
            ("waiting_reason", 1024),
            ("waiting_until", 64),
            ("parent_id", 64),
            ("labels", 2048),
        ]
        .map(|(n, m)| bounded(n, m)),
    );
    // Nullable fields must distinguish NULL from an oversized non-NULL value.
    let invalid:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM operator_tasks WHERE length(CAST(assignee AS BLOB))>512 OR length(CAST(created_by AS BLOB))>512 OR length(CAST(waiting_reason AS BLOB))>4096 OR length(CAST(waiting_until AS BLOB))>256 OR length(CAST(parent_id AS BLOB))>256)",[],|r|r.get(0))?;
    if invalid {
        return Err(Error::Capacity);
    }
    let sql = format!(
        "SELECT {} FROM operator_tasks ORDER BY id LIMIT {}",
        fields.join(","),
        MAX_ROWS + 1
    );
    let mut tasks = Vec::new();
    let mut size = 0usize;
    let mut query = db.prepare(&sql)?;
    let mut rows = query.query([])?;
    while let Some(r) = rows.next()? {
        if Instant::now() >= deadline {
            return Err(Error::Busy);
        }
        let raw: String = r.get(16)?;
        let t = Task {
            id: r.get(0)?,
            title: r.get(1)?,
            description: r.get(2)?,
            status: r.get(3)?,
            priority: r.get(4)?,
            granularity: r.get(5)?,
            assignee: r.get(6)?,
            created_by: r.get(7)?,
            created_at: r.get(8)?,
            updated_at: r.get(9)?,
            started_at: r.get(10)?,
            completed_at: r.get(11)?,
            heartbeat_at: r.get(12)?,
            waiting_reason: r.get(13)?,
            waiting_until: r.get(14)?,
            parent_id: r.get(15)?,
            labels: serde_json::from_str(&raw)?,
        };
        size = size.saturating_add(serde_json::to_vec(&t)?.len());
        if size > MAX_BYTES || tasks.len() == MAX_ROWS {
            return Err(Error::Capacity);
        }
        tasks.push(t);
    }
    let mut ordinals = std::collections::BTreeMap::<String, u32>::new();
    let mut comments = Vec::new();
    let sql = format!(
        "SELECT sequence,{}, {}, {},created_at FROM operator_task_comments ORDER BY sequence LIMIT {}",
        bounded("task_id", 64),
        bounded("author", 128),
        bounded("body", 4096),
        MAX_ROWS + 1
    );
    let mut query = db.prepare(&sql)?;
    let mut rows = query.query([])?;
    while let Some(r) = rows.next()? {
        if Instant::now() >= deadline {
            return Err(Error::Busy);
        }
        let task_id: String = r.get(1)?;
        let ordinal = ordinals.entry(task_id.clone()).or_default();
        let c = Comment {
            sequence: r.get(0)?,
            task_id,
            ordinal: *ordinal,
            author: r.get(2)?,
            text: r.get(3)?,
            at_ms: r.get(4)?,
        };
        *ordinal += 1;
        size = size.saturating_add(serde_json::to_vec(&c)?.len());
        if size > MAX_BYTES || comments.len() == MAX_ROWS {
            return Err(Error::Capacity);
        }
        comments.push(c);
    }
    let h = History { tasks, comments };
    h.summary()?;
    if Instant::now() >= deadline {
        return Err(Error::Busy);
    }
    Ok(h)
}
