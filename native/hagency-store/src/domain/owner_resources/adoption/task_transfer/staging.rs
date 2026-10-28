use super::*;
fn summary(plan: &Plan) -> Result<&Summary, Error> {
    let s = plan
        .inventory
        .task_history
        .as_ref()
        .ok_or(Error::Quarantined)?;
    if !matches!(plan.version, 2 | 3) {
        return Err(Error::Quarantined);
    }
    s.validate()?;
    Ok(s)
}
fn payload(value: &impl Serialize) -> Result<(String, String), Error> {
    let raw = serde_json::to_string(value)?;
    if raw.len() > ROW_BYTES {
        return Err(Error::Capacity);
    }
    Ok((canonical::digest(&serde_json::to_value(value)?)?, raw))
}
/// Called in the same IMMEDIATE transaction as prepare_adoption. No partial
/// staging plan can survive a writer failure, cancellation or result loss.
pub(in super::super) fn save(
    db: &Connection,
    plan: &Plan,
    history: Option<&History>,
) -> Result<(), Error> {
    match (plan.version, history, plan.inventory.task_history.as_ref()) {
        (1, None, None) => return Ok(()),
        (2 | 3, Some(h), Some(s)) if &h.summary()? == s => {}
        _ => return Err(Error::Quarantined),
    }
    let h = history.ok_or(Error::Quarantined)?;
    for t in &h.tasks {
        let (digest, raw) = payload(t)?;
        db.execute(
            "INSERT INTO owner_operator_task_transfer_tasks VALUES(?1,?2,?3,?4)",
            params![plan.migration_id, t.id, digest, raw],
        )?;
    }
    for c in &h.comments {
        let (digest, raw) = payload(c)?;
        db.execute(
            "INSERT INTO owner_operator_task_transfer_comments VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                plan.migration_id,
                c.sequence,
                c.task_id,
                c.ordinal,
                digest,
                raw
            ],
        )?;
    }
    verify(db, plan)?;
    Ok(())
}
pub(in super::super) fn verify(db: &Connection, plan: &Plan) -> Result<History, Error> {
    let expected = summary(plan)?;
    let mut size = 0usize;
    let mut tasks = Vec::new();
    let mut comments = Vec::new();
    let mut q=db.prepare("SELECT task_id,payload_digest,CASE WHEN length(CAST(payload AS BLOB))<=65536 THEN payload END FROM owner_operator_task_transfer_tasks WHERE migration_id=?1 ORDER BY task_id LIMIT 10001")?;
    let mut rows = q.query([&plan.migration_id])?;
    while let Some(r) = rows.next()? {
        let id: String = r.get(0)?;
        let digest: String = r.get(1)?;
        let raw: String = r.get(2)?;
        size = size.saturating_add(raw.len());
        if size > MAX_BYTES || tasks.len() == MAX_ROWS {
            return Err(Error::Capacity);
        }
        let t: Task = serde_json::from_str(&raw)?;
        if t.id != id || canonical::digest(&serde_json::to_value(&t)?)? != digest {
            return Err(Error::Quarantined);
        }
        tasks.push(t);
    }
    let mut q=db.prepare("SELECT sequence,task_id,ordinal,payload_digest,CASE WHEN length(CAST(payload AS BLOB))<=65536 THEN payload END FROM owner_operator_task_transfer_comments WHERE migration_id=?1 ORDER BY sequence LIMIT 10001")?;
    let mut rows = q.query([&plan.migration_id])?;
    while let Some(r) = rows.next()? {
        let sequence: i64 = r.get(0)?;
        let id: String = r.get(1)?;
        let ordinal: u32 = r.get(2)?;
        let digest: String = r.get(3)?;
        let raw: String = r.get(4)?;
        size = size.saturating_add(raw.len());
        if size > MAX_BYTES || comments.len() == MAX_ROWS {
            return Err(Error::Capacity);
        }
        let c: Comment = serde_json::from_str(&raw)?;
        if c.sequence != sequence
            || c.task_id != id
            || c.ordinal != ordinal
            || canonical::digest(&serde_json::to_value(&c)?)? != digest
        {
            return Err(Error::Quarantined);
        }
        comments.push(c);
    }
    let h = History { tasks, comments };
    if &h.summary()? != expected {
        return Err(Error::Quarantined);
    }
    Ok(h)
}
/// Exact original payload is copied once into inactive destination rows. Its
/// durable receipt is provenance, not task authority until adoption is active.
pub(in super::super) fn import(db: &Connection, plan: &Plan) -> Result<(), Error> {
    if plan.version == 1 && plan.inventory.task_history.is_none() {
        return Ok(());
    }
    let h = verify(db, plan)?;
    let receipt = receipt::expected(plan)?;
    let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_operator_task_histories WHERE profile=?1) OR EXISTS(SELECT 1 FROM owner_operator_tasks WHERE profile=?1) OR EXISTS(SELECT 1 FROM owner_operator_task_comments WHERE profile=?1)",[&plan.inventory.profile],|r|r.get(0))?;
    if exists {
        return Err(Error::Conflict);
    }
    let (digest, raw) = payload(&receipt)?;
    db.execute(
        "INSERT INTO owner_operator_task_histories VALUES(?1,?2,?3,?4)",
        params![plan.inventory.profile, plan.migration_id, digest, raw],
    )?;
    for t in &h.tasks {
        db.execute("INSERT INTO owner_operator_tasks(profile,id,revision,title,description,status,priority,granularity,assignee,created_by,created_at,updated_at,started_at,completed_at,heartbeat_at,waiting_reason,waiting_until,parent_id,labels) VALUES(?1,?2,1,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",params![plan.inventory.profile,t.id,t.title,t.description,t.status,t.priority,t.granularity,t.assignee,t.created_by,t.created_at,t.updated_at,t.started_at,t.completed_at,t.heartbeat_at,t.waiting_reason,t.waiting_until,t.parent_id,serde_json::to_string(&t.labels)?])?;
    }
    for c in &h.comments {
        db.execute(
            "INSERT INTO owner_operator_task_comments VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                plan.inventory.profile,
                c.task_id,
                c.ordinal,
                c.author,
                c.text,
                c.at_ms
            ],
        )?;
    }
    verify_import(db, plan, 2)
}
pub(in super::super) fn verify_import(
    db: &Connection,
    plan: &Plan,
    phase: i64,
) -> Result<(), Error> {
    if plan.version == 1 && plan.inventory.task_history.is_none() {
        return Ok(());
    }
    let h = verify(db, plan)?;
    receipt::saved(db, plan)?;
    if phase >= 4 {
        return Ok(());
    } // Current task rows may legitimately change after activation.
    let profile = &plan.inventory.profile;
    let count:(u64,u64)=db.query_row("SELECT (SELECT count(*) FROM owner_operator_tasks WHERE profile=?1),(SELECT count(*) FROM owner_operator_task_comments WHERE profile=?1)",[profile],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if count != (h.tasks.len() as u64, h.comments.len() as u64) {
        return Err(Error::Quarantined);
    }
    for t in &h.tasks {
        let actual:Task=db.query_row("SELECT id,title,description,status,priority,granularity,assignee,created_by,created_at,updated_at,started_at,completed_at,heartbeat_at,waiting_reason,waiting_until,parent_id,labels,revision FROM owner_operator_tasks WHERE profile=?1 AND id=?2",params![profile,t.id],|r|{
            let revision:i64=r.get(17)?;if revision!=1{return Err(rusqlite::Error::InvalidQuery)}
            let raw:String=r.get(16)?;let labels=serde_json::from_str(&raw).map_err(|e|rusqlite::Error::FromSqlConversionFailure(16,rusqlite::types::Type::Text,Box::new(e)))?;
            Ok(Task{id:r.get(0)?,title:r.get(1)?,description:r.get(2)?,status:r.get(3)?,priority:r.get(4)?,granularity:r.get(5)?,assignee:r.get(6)?,created_by:r.get(7)?,created_at:r.get(8)?,updated_at:r.get(9)?,started_at:r.get(10)?,completed_at:r.get(11)?,heartbeat_at:r.get(12)?,waiting_reason:r.get(13)?,waiting_until:r.get(14)?,parent_id:r.get(15)?,labels})
        })?;
        if &actual != t {
            return Err(Error::Quarantined);
        }
    }
    for c in &h.comments {
        let actual:(String,String,i64)=db.query_row("SELECT author,text,at_ms FROM owner_operator_task_comments WHERE profile=?1 AND task_id=?2 AND ordinal=?3",params![profile,c.task_id,c.ordinal],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        if actual != (c.author.clone(), c.text.clone(), c.at_ms) {
            return Err(Error::Quarantined);
        }
    }
    Ok(())
}
