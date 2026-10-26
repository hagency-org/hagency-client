//! Owner-scoped original delivery history. Refresh can add evidence, never custody.
use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub scope: Scope,
    pub dispatch_id: String,
    pub execution_id: String,
    pub created_at: i64,
    pub record: Record,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub records: Vec<Entry>,
    pub next_cursor: Option<String>,
}
impl Ledger {
    pub fn sent_file_record(&self, owner: &str, agent: &str, id: &str) -> Result<Entry> {
        self.owner(owner)?;
        crate::key(agent)?;
        crate::key(id)?;
        let row:Option<(String,String,String,i64)>=self.db.query_row("SELECT s.scope,s.dispatch,s.execution,a.created_at FROM sent_files s JOIN approval_audit a ON a.id=s.audit WHERE s.id=? AND a.agent=?",(id,agent),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let (encoded, dispatch_id, execution_id, created_at) = row.ok_or(Error::Unauthorized)?;
        let scope: Scope = serde_json::from_str(&encoded)?;
        if scope.agent != agent {
            return Err(Error::Unauthorized);
        }
        crate::verify(&self.db, &scope)?;
        let record =
            record(&self.db, &scope, &dispatch_id, &execution_id, id)?.ok_or(Error::Unknown)?;
        Ok(Entry {
            scope,
            dispatch_id,
            execution_id,
            created_at,
            record,
        })
    }
    pub fn sent_file_history(
        &self,
        owner: &str,
        agent: &str,
        cursor: Option<&str>,
    ) -> Result<Page> {
        self.owner(owner)?;
        crate::key(agent)?;
        let before = cursor
            .map(|id| {
                self.sent_file_record(owner, agent, id)
                    .map(|e| e.created_at)
            })
            .transpose()?;
        let mut ids=self.db.prepare("SELECT s.id FROM sent_files s JOIN approval_audit a ON a.id=s.audit WHERE a.agent=?1 AND (?2 IS NULL OR a.created_at<?2 OR (a.created_at=?2 AND s.id<?3)) ORDER BY a.created_at DESC,s.id DESC LIMIT 101")?.query_map((agent,before,cursor),|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let more = ids.len() > 100;
        ids.truncate(100);
        let next_cursor = more.then(|| ids.last().cloned()).flatten();
        let records = ids
            .iter()
            .map(|id| self.sent_file_record(owner, agent, id))
            .collect::<Result<Vec<_>>>()?;
        Ok(Page {
            records,
            next_cursor,
        })
    }
    /// Safe host-fetched evidence only; never infer a cancelled delivery from a
    /// missing response, ordinary 404 or a still-running preparation.
    pub fn observe_file_lookup(
        &mut self,
        scope: &Scope,
        dispatch: &str,
        execution: &str,
        id: &str,
        observation: FileObservation,
        now: i64,
    ) -> Result<Record> {
        crate::verify(&self.db, scope)?;
        let current =
            record(&self.db, scope, dispatch, execution, id)?.ok_or(Error::Unauthorized)?;
        let query = FileLookup {
            dispatch_id: dispatch.into(),
            execution_id: execution.into(),
            request_id: current.request_id.clone(),
        };
        observation.validate(&query, &current.manifest)?;
        if now < 0 {
            return Err(Error::Invalid("file observation time"));
        }
        if let Some(receipt) = &observation.delivery {
            return self.observe_sent_file(scope, dispatch, execution, id, receipt.clone());
        }
        if !observation.preparation_closed {
            if current.absence.is_some() {
                return Err(Error::Conflict);
            }
            return Ok(current);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = record(&tx, scope, dispatch, execution, id)?.ok_or(Error::Unauthorized)?;
        if current.remote.is_some() {
            return Err(Error::Conflict);
        }
        if current.absence.is_none() {
            tx.execute(
                "INSERT INTO sent_file_absence(id,proof,observed_at) VALUES(?,?,?)",
                (id, json(&observation)?, now),
            )?;
        }
        let result = record(&tx, scope, dispatch, execution, id)?.ok_or(Error::Unknown)?;
        tx.commit()?;
        Ok(result)
    }
}
