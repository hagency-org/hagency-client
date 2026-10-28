use super::*;
/// The caller holds a schema-validated source transaction and finite deadline.
/// Check byte lengths before loading fields, including nullable fields.
pub(in super::super) fn capture(
    db: &Connection,
    resources: &[Resource],
    deadline: Instant,
) -> Result<History, Error> {
    if db.query_row("SELECT EXISTS(SELECT 1 FROM ceiling_alerts WHERE length(CAST(dedupe_key AS BLOB))>1024 OR length(CAST(resource_id AS BLOB))>512 OR length(CAST(summary AS BLOB))>16384 OR length(CAST(detail AS BLOB))>16384 OR length(CAST(runbook AS BLOB))>8192 OR length(CAST(impact AS BLOB))>8192 OR length(CAST(recovery_condition AS BLOB))>8192 OR length(CAST(resolved_by AS BLOB))>512 OR length(CAST(status AS BLOB))>64 OR length(CAST(note AS BLOB))>8192 OR length(CAST(transitioned_by AS BLOB))>512 OR length(CAST(alert_type AS BLOB))>512 OR length(CAST(severity AS BLOB))>64 OR length(CAST(source AS BLOB))>64 OR length(CAST(source_agent AS BLOB))>512 OR length(CAST(assignee AS BLOB))>512 OR length(CAST(linked_task_id AS BLOB))>512 OR length(CAST(original_severity AS BLOB))>64 OR length(CAST(missing_actionable_fields AS BLOB))>16384 OR length(CAST(tags AS BLOB))>16384 OR length(CAST(owner AS BLOB))>512)", [], |r| r.get::<_, bool>(0))? { return Err(Error::Capacity); }
    let mut q = db.prepare("SELECT dedupe_key,resource_id,summary,detail,runbook,impact,recovery_condition,occurrences,first_seen_ms,last_seen_ms,resolved_at_ms,resolved_by,status,note,transitioned_at_ms,transitioned_by,alert_type,severity,source,source_agent,assignee,suppress_until_ms,linked_task_id,original_severity,missing_actionable_fields,tags,owner FROM ceiling_alerts ORDER BY dedupe_key LIMIT 1001")?;
    let mut rows = q.query([])?;
    let mut alerts = Vec::new();
    let mut bytes = 0usize;
    while let Some(r) = rows.next()? {
        if Instant::now() >= deadline {
            return Err(Error::Busy);
        }
        let a = Alert {
            dedupe_key: r.get(0)?,
            resource_id: r.get(1)?,
            summary: r.get(2)?,
            detail: r.get(3)?,
            runbook: r.get(4)?,
            impact: r.get(5)?,
            recovery_condition: r.get(6)?,
            occurrences: r.get(7)?,
            first_seen_ms: r.get(8)?,
            last_seen_ms: r.get(9)?,
            resolved_at_ms: r.get(10)?,
            resolved_by: r.get(11)?,
            status: r.get(12)?,
            note: r.get(13)?,
            transitioned_at_ms: r.get(14)?,
            transitioned_by: r.get(15)?,
            alert_type: r.get(16)?,
            severity: r.get(17)?,
            source: r.get(18)?,
            source_agent: r.get(19)?,
            assignee: r.get(20)?,
            suppress_until_ms: r.get(21)?,
            linked_task_id: r.get(22)?,
            original_severity: r.get(23)?,
            missing_actionable_fields: r.get(24)?,
            tags: r.get(25)?,
            owner: r.get(26)?,
        };
        let size = serde_json::to_vec(&a)?.len();
        bytes = bytes.saturating_add(size);
        if size > ROW_BYTES || bytes > MAX_BYTES || alerts.len() == MAX_ALERTS {
            return Err(Error::Capacity);
        }
        alerts.push(a);
    }
    if db.query_row("SELECT EXISTS(SELECT 1 FROM ceiling_alert_notes WHERE length(CAST(dedupe_key AS BLOB))>1024 OR length(CAST(author AS BLOB))>512 OR length(CAST(text AS BLOB))>8192)", [], |r| r.get::<_, bool>(0))? { return Err(Error::Capacity); }
    let mut q = db.prepare("SELECT dedupe_key,seq,author,text,ts_ms FROM ceiling_alert_notes ORDER BY dedupe_key,seq LIMIT 10001")?;
    let mut rows = q.query([])?;
    let mut notes = Vec::new();
    while let Some(r) = rows.next()? {
        if Instant::now() >= deadline {
            return Err(Error::Busy);
        }
        let n = Note {
            dedupe_key: r.get(0)?,
            seq: r.get(1)?,
            author: r.get(2)?,
            text: r.get(3)?,
            ts_ms: r.get(4)?,
        };
        bytes = bytes.saturating_add(serde_json::to_vec(&n)?.len());
        if bytes > MAX_BYTES || notes.len() == MAX_NOTES {
            return Err(Error::Capacity);
        }
        notes.push(n);
    }
    let h = History { alerts, notes };
    h.summary(resources)?;
    if Instant::now() >= deadline {
        return Err(Error::Busy);
    }
    Ok(h)
}
