//! Bounded resource reads and atomic CAS/receipt for retained alert triage.
use super::*;
use crate::management::{CommandReceipt, ResourceAlert, ResourceAlertPage, ResourceAlertQuery, ResourceAlertDetail, ResourceAlertDetailQuery, ResourceAlertNote, TriageResourceAlert};
use rusqlite::Connection;

#[cfg(test)]
#[path = "web_overview_tests.rs"]
mod web_overview_tests;

fn projection(db:&Connection,alert:CeilingAlert)->Result<ResourceAlert,Error>{
    let last_note:u64=db.query_row("SELECT COALESCE(MAX(seq),0) FROM ceiling_alert_notes WHERE dedupe_key=?1",[&alert.dedupe_key],|r|r.get(0))?;
    // Covers observations, legal state, metadata and appended history. A new
    // sweep or note invalidates a review even if the status itself is unchanged.
    let revision=hagency_core::canonical::digest(&serde_json::json!({"alert":alert,"lastNote":last_note}))?;
    let mut budget=std::collections::BTreeMap::new();
    for key in ["ceilingTokens","committedTokens","measuredTokens","drawnTokens","overByTokens"] {
        if let Some(value)=alert.detail.get(key).and_then(serde_json::Value::as_u64).filter(|v|*v<=JSON_SAFE_MAX){budget.insert(key.into(),value);}
    }
    let result=ResourceAlert{id:alert.dedupe_key,resource_id:alert.resource_id,revision,summary:alert.summary,runbook:alert.runbook,impact:alert.impact,recovery_condition:alert.recovery_condition,next:allowed_transitions(&alert.status).iter().map(|s|(*s).into()).collect(),status:alert.status,severity:alert.severity,occurrences:alert.occurrences,first_seen_ms:alert.first_seen_ms,last_seen_ms:alert.last_seen_ms,resolved:alert.resolved,note:alert.note,assignee:alert.assignee,suppress_until_ms:alert.suppress_until_ms,budget};
    bounded_result(&result,16*1024)?;Ok(result)
}
fn bounded_result(value:&impl Serialize,limit:usize)->Result<(),Error>{
    if serde_json::to_vec(value)?.len()>limit {return Err(Error::Capacity);}Ok(())
}
fn scoped_alert(db:&Connection,resource:&str,key:&str)->Result<CeilingAlert,Error>{
    let alert=read_alert(db,key)?;
    if alert.resource_id!=resource {return Err(Error::NotFound);}Ok(alert)
}
impl DomainRepository {
    pub(crate) fn management_web_alerts(&self, profile: Option<&str>, query: &crate::management::WebAlertsQuery, check: impl Fn(&Connection) -> Result<(), Error>) -> Result<crate::management::WebAlertsPage, Error> {
        use crate::management::{WebAlertCounts, WebAlertsPage};
        query.validate()?;
        let tx = self.db.unchecked_transaction()?;
        check(&tx)?;
        // The canonical database is shared. Both counters and page rows obey
        // the same active resource grant and adoption-readiness boundary.
        let visible = "EXISTS(SELECT 1 FROM resources r WHERE r.id=ceiling_alerts.resource_id AND (?1 IS NULL OR (EXISTS(SELECT 1 FROM owner_resource_grants g WHERE g.resource_id=r.id AND g.profile=?1 AND g.active=1) AND NOT EXISTS(SELECT 1 FROM owner_resource_adoptions a WHERE a.phase<4 AND json_extract(a.plan,'$.targetSeat.id')=json_extract(r.config,'$.seatId')))))";
        let mut counts = WebAlertCounts {
            by_status: ALERT_STATUSES.into_iter().map(|s| (s.into(), 0)).collect(),
            open_by_severity: ALERT_SEVERITIES.into_iter().map(|s| (s.into(), 0)).collect(),
            ..Default::default()
        };
        let mut statement = tx.prepare(&format!("SELECT status,severity,COUNT(*) FROM ceiling_alerts WHERE {visible} GROUP BY status,severity"))?;
        let rows = statement.query_map([profile], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, u64>(2)?)))?;
        for row in rows {
            let (status, severity, n) = row?;
            counts.total = counts.total.checked_add(n).filter(|n| *n <= JSON_SAFE_MAX).ok_or(Error::Capacity)?;
            *counts.by_status.entry(status.clone()).or_default() += n;
            if status == "open" { *counts.open_by_severity.entry(severity).or_default() += n; }
        }
        drop(statement);
        let filters = "(?2 IS NULL OR status=?2) AND (?3 IS NULL OR severity=?3) AND (?4 IS NULL OR resource_id=?4)";
        let filtered_count = tx.query_row(&format!("SELECT COUNT(*) FROM ceiling_alerts WHERE {visible} AND {filters}"), params![profile, query.status, query.severity, query.agent], |r| r.get(0))?;
        let mut statement = tx.prepare(&format!("SELECT {ALERT_COLUMNS} FROM ceiling_alerts WHERE {visible} AND {filters} AND dedupe_key>?5 ORDER BY dedupe_key LIMIT 17"))?;
        let mut rows = statement.query_map(params![profile, query.status, query.severity, query.agent, query.after.as_deref().unwrap_or("")], alert_from_row)?.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let more = rows.len() > 16;
        rows.truncate(16);
        let alerts = rows.into_iter().map(|a| projection(&tx, a)).collect::<Result<Vec<_>, _>>()?;
        let next_after = more.then(|| alerts.last().expect("full alert page").id.clone());
        let result = WebAlertsPage { alerts, counts, filtered_count, next_after, observed_at_ms: crate::domain::graphs::now_ms()? };
        bounded_result(&result, 128 * 1024)?;
        check(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    pub(crate) fn management_resource_alerts(&self,query:&ResourceAlertQuery,check:impl Fn(&Connection)->Result<(),Error>)->Result<ResourceAlertPage,Error>{
        query.validate()?;
        let tx=self.db.unchecked_transaction()?;check(&tx)?;
        let mut stmt=tx.prepare(&format!("SELECT {ALERT_COLUMNS} FROM ceiling_alerts WHERE resource_id=?1 AND dedupe_key>?2 AND (?3 IS NULL OR status=?3) ORDER BY dedupe_key LIMIT 17"))?;
        let mut rows=stmt.query_map(params![query.resource_id,query.after,query.status],alert_from_row)?.collect::<Result<Vec<_>,_>>()?;drop(stmt);
        let more=rows.len()>16;rows.truncate(16);
        let alerts=rows.into_iter().map(|a|projection(&tx,a)).collect::<Result<Vec<_>,_>>()?;
        let next_after=if more {alerts.last().map(|a|a.id.clone())} else {None};
        let result=ResourceAlertPage{resource_id:query.resource_id.clone(),alerts,next_after,observed_at_ms:crate::domain::graphs::now_ms()?};
        bounded_result(&result,128*1024)?;check(&tx)?;tx.commit()?;Ok(result)
    }
    pub(crate) fn management_resource_alert(&self,query:&ResourceAlertDetailQuery,check:impl Fn(&Connection)->Result<(),Error>)->Result<ResourceAlertDetail,Error>{
        query.validate()?;
        let tx=self.db.unchecked_transaction()?;check(&tx)?;
        let alert=projection(&tx,scoped_alert(&tx,&query.resource_id,&query.id)?)?;
        let mut stmt=tx.prepare("SELECT seq,author,text,ts_ms FROM ceiling_alert_notes WHERE dedupe_key=?1 AND seq>?2 ORDER BY seq LIMIT 17")?;
        let mut notes=stmt.query_map(params![query.id,query.after_note],|r|Ok(ResourceAlertNote{sequence:r.get(0)?,author:r.get(1)?,text:r.get(2)?,at_ms:r.get(3)?}))?.collect::<Result<Vec<_>,_>>()?;drop(stmt);
        let more=notes.len()>16;notes.truncate(16);
        let next_note=if more {notes.last().map(|n|n.sequence)} else {None};
        let result=ResourceAlertDetail{alert,notes,next_note};bounded_result(&result,64*1024)?;
        check(&tx)?;tx.commit()?;Ok(result)
    }
    pub(crate) fn management_triage_alert(&mut self,input:&TriageResourceAlert,actor:&str,receipt:CommandReceipt,check:impl Fn(&Connection)->Result<(),Error>)->Result<ResourceAlert,Error>{
        input.validate()?;
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        check(&tx)?;
        // Recover after current resource authority, before checking the mutable
        // alert row: a newer observation or retention cannot replay the effect.
        if let Some(result)=receipt.lookup::<ResourceAlert>(&tx)?{check(&tx)?;return Ok(result);}
        let current=projection(&tx,scoped_alert(&tx,&input.resource_id,&input.id)?)?;
        if current.revision!=input.expected_revision {return Err(Error::Conflict);}
        let to=ALERT_STATUSES.iter().copied().find(|s|*s==input.to).ok_or(hagency_core::InvalidInput("invalid alert status"))?;
        let changed=transition_in_transaction(&tx,AlertTransition{key:input.id.clone(),to,actor:actor.into(),note:input.note.clone(),assignee:(to=="assigned").then(||actor.into()),suppress_until_ms:None,now:crate::domain::graphs::now_ms()?})?;
        let result=projection(&tx,changed)?;
        receipt.record(&tx,&result)?;check(&tx)?;tx.commit().map_err(|_|Error::OutcomeUnknown)?;Ok(result)
    }
}
