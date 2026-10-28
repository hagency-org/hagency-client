//! Native bounded history review; no source path or import authority.
use super::*;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorTaskTransferPreview {
    tasks: Summary,
    alerts: Option<alert_transfer::Summary>,
}
impl OperatorTaskTransferPreview {
    pub fn task_count(&self) -> u32 {
        self.tasks.tasks
    }
    pub fn comment_count(&self) -> u32 {
        self.tasks.comments
    }
    /// None preserves the original task-only migration scope.
    pub fn alert_count(&self) -> Option<u32> {
        self.alerts.as_ref().map(|a| a.alerts)
    }
    pub fn alert_note_count(&self) -> Option<u32> {
        self.alerts.as_ref().map(|a| a.notes)
    }
}
impl crate::DomainRepository {
    pub(crate) fn adoption_task_preview(
        &mut self,
        deadline: Instant,
    ) -> Result<OperatorTaskTransferPreview, Error> {
        self.adoption_history_preview_mode(deadline, false)
    }
    pub(crate) fn adoption_history_preview(
        &mut self,
        deadline: Instant,
    ) -> Result<OperatorTaskTransferPreview, Error> {
        self.adoption_history_preview_mode(deadline, true)
    }
    fn adoption_history_preview_mode(
        &mut self,
        deadline: Instant,
        alerts: bool,
    ) -> Result<OperatorTaskTransferPreview, Error> {
        if Instant::now() >= deadline {
            return Err(Error::Busy);
        }
        self.db
            .progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        let result = (|| {
            let tx = self
                .db
                .transaction_with_behavior(TransactionBehavior::Deferred)?;
            let tasks = capture(&tx, deadline)?.summary()?;
            let alerts = if alerts {
                let rows = tx.prepare("SELECT CASE WHEN length(id)<=128 THEN id END,CASE WHEN length(CAST(config AS BLOB))<=16384 THEN config END FROM resources ORDER BY id LIMIT 17")?
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                if !(1..=16).contains(&rows.len()) {
                    return Err(Error::Unqualified);
                }
                let resources = rows
                    .into_iter()
                    .map(|(id, raw)| {
                        let resource: Resource = serde_json::from_str(&raw)?;
                        resource.validate()?;
                        if resource.id() != id {
                            return Err(Error::Quarantined);
                        }
                        Ok(resource)
                    })
                    .collect::<Result<Vec<_>, Error>>()?;
                Some(alert_transfer::capture(&tx, &resources, deadline)?.summary(&resources)?)
            } else {
                None
            };
            tx.commit()?;
            Ok(OperatorTaskTransferPreview { tasks, alerts })
        })();
        self.db.progress_handler(0, None::<fn() -> bool>)?;
        if Instant::now() >= deadline {
            return Err(Error::Busy);
        }
        result
    }
}
impl AdoptionInventory {
    pub fn operator_task_preview(&self) -> Result<OperatorTaskTransferPreview, Error> {
        Ok(OperatorTaskTransferPreview {
            tasks: self.0.task_history.clone().ok_or(Error::Unqualified)?,
            alerts: self.0.alert_history.clone(),
        })
    }
}
