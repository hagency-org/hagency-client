//! Bounded closed custody validation. Terminal replies remain recoverable;
//! erased acknowledgements are tombstones, never evidence of a new effect.
use super::*;
use crate::management::{Intent, IntentOperation, IntentReply};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DefinitionReceipt {
    resource_id: String,
    revision: String,
    published: bool,
}
fn identity(actor: &str, command: &str, digest: &str) -> Result<(), Error> {
    hash(actor)?;
    identifier(command, 128)?;
    hash(digest)
}
pub(super) fn collect(db: &Connection, resources: &[Resource], alerts: bool) -> Result<(String, u32), Error> {
    if db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM management_intents WHERE length(CAST(input AS BLOB))>{} OR length(CAST(reply AS BLOB))>65536)",crate::management::intents::INPUT_LIMIT_SQL),[],|r|r.get::<_,bool>(0))? { return Err(Error::Capacity); }
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    let mut add = |value: Value| -> Result<(), Error> {
        bytes = bytes.saturating_add(serde_json::to_vec(&value)?.len());
        if bytes > 1024 * 1024 || rows.len() >= 10_000 {
            return Err(Error::Capacity);
        }
        rows.push(value);
        Ok(())
    };
    let mut q=db.prepare("SELECT CASE WHEN length(actor_scope)=64 THEN actor_scope END,CASE WHEN length(command_id)<=128 THEN command_id END,CASE WHEN length(operation)<=64 THEN operation END,CASE WHEN length(digest)=64 THEN digest END,CASE WHEN length(CAST(result AS BLOB))<=65536 THEN result END FROM management_commands ORDER BY actor_scope,command_id")?;
    for row in q.query_map([], |r| {
        Ok([
            r.get::<_, String>(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
        ])
    })? {
        let row = row?;
        identity(&row[0], &row[1], &row[3])?;
        if row[2] == "alert.triage" && alerts {
            let raw: Value = serde_json::from_str(&row[4])?;
            let receipt: crate::management::ResourceAlert = serde_json::from_value(raw.clone())?;
            // Closed original result: unknown fields or another resource cannot
            // become a recoverable completed command under this transfer.
            if serde_json::to_value(&receipt)? != raw || !resources.iter().any(|r| r.id() == receipt.resource_id) || serde_json::to_vec(&receipt)?.len() > 16 * 1024 { return Err(Error::Quarantined); }
            crate::management::ResourceAlertDetailQuery { resource_id: receipt.resource_id.clone(), id: receipt.id.clone(), after_note: 0 }.validate()?;
            hash(&receipt.revision)?;
            if !crate::ALERT_STATUSES.contains(&receipt.status.as_str()) || !crate::domain::ceiling_alerts::ALERT_SEVERITIES.contains(&receipt.severity.as_str()) || receipt.occurrences == 0 || [Some(receipt.occurrences), Some(receipt.first_seen_ms), Some(receipt.last_seen_ms), receipt.suppress_until_ms].into_iter().flatten().any(|v| v > JSON_SAFE_MAX) || receipt.budget.iter().any(|(k,v)| !["ceilingTokens","committedTokens","measuredTokens","drawnTokens","overByTokens"].contains(&k.as_str()) || *v > JSON_SAFE_MAX) { return Err(Error::Quarantined); }
            if receipt.next != crate::domain::ceiling_alerts::allowed_transitions(&receipt.status).iter().map(|s| (*s).to_owned()).collect::<Vec<_>>() { return Err(Error::Quarantined); }
            add(json!({"kind":"command","row":row}))?;
            continue;
        }
        if !matches!(
            row[2].as_str(),
            "resource.enroll" | "resource.configure" | "resource.publish"
        ) {
            return Err(Error::Quarantined);
        }
        let receipt: DefinitionReceipt = serde_json::from_str(&row[4])?;
        hash(&receipt.revision)?;
        if !resources.iter().any(|r| r.id() == receipt.resource_id) {
            return Err(Error::Quarantined);
        }
        // The publication flag and old revision are historical values. A
        // later completed edit may legitimately change the current definition.
        let _historical_publication = receipt.published;
        add(json!({"kind":"command","row":row}))?;
    }
    let mut q=db.prepare(&format!("SELECT CASE WHEN length(actor_scope)=64 THEN actor_scope END,CASE WHEN length(command_id)<=128 THEN command_id END,CASE WHEN length(operation)<=64 THEN operation END,CASE WHEN length(digest)=64 THEN digest END,CASE WHEN length(CAST(input AS BLOB))<={} THEN input END,CASE WHEN length(CAST(reply AS BLOB))<=65536 THEN reply END,state FROM management_intents ORDER BY actor_scope,command_id",crate::management::intents::INPUT_LIMIT_SQL))?;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, String>(6)?,
        ))
    })? {
        let (actor, id, operation, digest, input, reply, state) = row?;
        identity(&actor, &id, &digest)?;
        let op: IntentOperation = serde_json::from_str(&operation)?;
        match state.as_str() {
            "acknowledged" if input.is_none() && reply.is_none() => {}
            "replied" => {
                let input: Value =
                    serde_json::from_str(input.as_deref().ok_or(Error::Quarantined)?)?;
                let intent = Intent::new(op, id.clone(), &input)?;
                if !intent.matches_digest(&digest) {
                    return Err(Error::Quarantined);
                }
                match serde_json::from_str::<IntentReply>(
                    reply.as_deref().ok_or(Error::Quarantined)?,
                )? {
                    IntentReply::Success(value) if value.is_object() => {}
                    IntentReply::Failure { status, code }
                        if (400..600).contains(&status)
                            && !code.is_empty()
                            && code.len() <= 128
                            && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') => {}
                    _ => return Err(Error::Quarantined),
                }
            }
            _ => return Err(Error::Quarantined),
        }
        add(
            json!({"kind":"intent","row":[actor,id,operation,digest],"input":input,"reply":reply,"state":state}),
        )?;
    }
    let count = rows.len() as u32;
    Ok((canonical::digest(&Value::Array(rows))?, count))
}
