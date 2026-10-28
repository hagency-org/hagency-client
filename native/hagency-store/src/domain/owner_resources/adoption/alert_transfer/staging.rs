use super::*;
/// The original complete history lives in the bounded immutable plan. The
/// inventory holds its digest/counts, and source fencing binds that inventory.
pub(in super::super) fn verify(plan: &Plan) -> Result<Option<&History>, Error> {
    match (
        plan.version,
        &plan.alert_history,
        &plan.inventory.alert_history,
    ) {
        (1 | 2, None, None) => Ok(None),
        (3, Some(history), Some(summary)) if plan.inventory.task_history.is_some() => {
            summary.validate()?;
            if &history.summary(&plan.inventory.resources)? != summary {
                return Err(Error::Quarantined);
            }
            Ok(Some(history))
        }
        _ => Err(Error::Quarantined),
    }
}
pub(in super::super) fn conflicts(db: &Connection, plan: &Plan) -> Result<(), Error> {
    let Some(history) = verify(plan)? else {
        return Ok(());
    };
    for a in &history.alerts {
        if db.query_row("SELECT EXISTS(SELECT 1 FROM ceiling_alerts WHERE dedupe_key=?1) OR EXISTS(SELECT 1 FROM ceiling_alert_notes WHERE dedupe_key=?1)", [&a.dedupe_key], |r| r.get::<_, bool>(0))? { return Err(Error::Conflict); }
    }
    for resource in &plan.inventory.resources {
        if db.query_row(
            "SELECT EXISTS(SELECT 1 FROM ceiling_alerts WHERE resource_id=?1)",
            [resource.id()],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::Conflict);
        }
    }
    Ok(())
}
/// Within the resource import's immediate transaction; never upsert a collision.
pub(in super::super) fn import(db: &Connection, plan: &Plan) -> Result<(), Error> {
    let Some(history) = verify(plan)? else {
        return Ok(());
    };
    conflicts(db, plan)?;
    for a in &history.alerts {
        db.execute("INSERT INTO ceiling_alerts(dedupe_key,resource_id,summary,detail,runbook,impact,recovery_condition,occurrences,first_seen_ms,last_seen_ms,resolved_at_ms,resolved_by,status,note,transitioned_at_ms,transitioned_by,alert_type,severity,source,source_agent,assignee,suppress_until_ms,linked_task_id,original_severity,missing_actionable_fields,tags,owner) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27)", params![a.dedupe_key,a.resource_id,a.summary,a.detail,a.runbook,a.impact,a.recovery_condition,a.occurrences,a.first_seen_ms,a.last_seen_ms,a.resolved_at_ms,a.resolved_by,a.status,a.note,a.transitioned_at_ms,a.transitioned_by,a.alert_type,a.severity,a.source,a.source_agent,a.assignee,a.suppress_until_ms,a.linked_task_id,a.original_severity,a.missing_actionable_fields,a.tags,a.owner])?;
    }
    for n in &history.notes {
        db.execute("INSERT INTO ceiling_alert_notes(dedupe_key,seq,author,text,ts_ms) VALUES(?1,?2,?3,?4,?5)", params![n.dedupe_key,n.seq,n.author,n.text,n.ts_ms])?;
    }
    verify_import(db, plan, 2)
}
pub(in super::super) fn verify_import(
    db: &Connection,
    plan: &Plan,
    phase: i64,
) -> Result<(), Error> {
    let Some(history) = verify(plan)? else {
        return Ok(());
    };
    if phase >= 4 {
        return Ok(());
    } // Subsequent observation/triage/retention is legitimate.
    let mut keys = Vec::new();
    for resource in &plan.inventory.resources {
        let mut q = db.prepare("SELECT dedupe_key FROM ceiling_alerts WHERE resource_id=?1 ORDER BY dedupe_key LIMIT 1001")?;
        keys.extend(
            q.query_map([resource.id()], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    keys.sort();
    if keys
        != history
            .alerts
            .iter()
            .map(|a| a.dedupe_key.clone())
            .collect::<Vec<_>>()
    {
        return Err(Error::Quarantined);
    }
    let mut notes = Vec::new();
    for a in &history.alerts {
        let actual = db.query_row("SELECT dedupe_key,resource_id,summary,detail,runbook,impact,recovery_condition,occurrences,first_seen_ms,last_seen_ms,resolved_at_ms,resolved_by,status,note,transitioned_at_ms,transitioned_by,alert_type,severity,source,source_agent,assignee,suppress_until_ms,linked_task_id,original_severity,missing_actionable_fields,tags,owner FROM ceiling_alerts WHERE dedupe_key=?1", [&a.dedupe_key], |r| Ok(Alert {dedupe_key:r.get(0)?,resource_id:r.get(1)?,summary:r.get(2)?,detail:r.get(3)?,runbook:r.get(4)?,impact:r.get(5)?,recovery_condition:r.get(6)?,occurrences:r.get(7)?,first_seen_ms:r.get(8)?,last_seen_ms:r.get(9)?,resolved_at_ms:r.get(10)?,resolved_by:r.get(11)?,status:r.get(12)?,note:r.get(13)?,transitioned_at_ms:r.get(14)?,transitioned_by:r.get(15)?,alert_type:r.get(16)?,severity:r.get(17)?,source:r.get(18)?,source_agent:r.get(19)?,assignee:r.get(20)?,suppress_until_ms:r.get(21)?,linked_task_id:r.get(22)?,original_severity:r.get(23)?,missing_actionable_fields:r.get(24)?,tags:r.get(25)?,owner:r.get(26)?}))?;
        if &actual != a {
            return Err(Error::Quarantined);
        }
        let mut q = db.prepare("SELECT dedupe_key,seq,author,text,ts_ms FROM ceiling_alert_notes WHERE dedupe_key=?1 ORDER BY seq LIMIT 10001")?;
        notes.extend(
            q.query_map([&a.dedupe_key], |r| {
                Ok(Note {
                    dedupe_key: r.get(0)?,
                    seq: r.get(1)?,
                    author: r.get(2)?,
                    text: r.get(3)?,
                    ts_ms: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?,
        );
        if notes.len() > MAX_NOTES {
            return Err(Error::Capacity);
        }
    }
    if notes != history.notes {
        return Err(Error::Quarantined);
    }
    Ok(())
}
