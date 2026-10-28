//! Scoped binding snapshots and an atomic removal/receipt using original rows.
use super::*;
use crate::management::{
    CommandReceipt, ResourceApprovalBinding, ResourceApprovalBindingPage,
    ResourceApprovalBindingQuery, RetireResourceApprovalBinding, RetiredResourceApprovalBinding,
};

const COLUMNS: &str = "e.resource_id,e.id,json_extract(e.projection,'$.agentName'),room.fleet_id,room.project_id,room.server_name,room.room_id,room.owner_mxid,b.room_generation,b.incarnation,EXISTS(SELECT 1 FROM current_approval_bindings c WHERE c.engagement_id=b.engagement_id)";
const JOINS: &str = "approval_bindings b JOIN approval_rooms room ON room.server_name=b.server_name AND room.room_id=b.room_id JOIN engagements e ON e.id=b.engagement_id LEFT JOIN projects p ON p.fleet_id=e.fleet_id AND p.id=e.project_id AND p.generation=e.generation";
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ResourceApprovalBinding> {
    Ok(ResourceApprovalBinding {
        resource_id: r.get(0)?,
        engagement_id: r.get(1)?,
        agent: r.get(2)?,
        fleet_id: r.get(3)?,
        project_id: r.get(4)?,
        server_name: r.get(5)?,
        room_id: r.get(6)?,
        owner_mxid: r.get(7)?,
        room_generation: r.get(8)?,
        incarnation: r.get(9)?,
        active: r.get(10)?,
        revision: String::new(),
    })
}
fn project(mut row: ResourceApprovalBinding) -> Result<ResourceApprovalBinding, Error> {
    if row.incarnation == 0
        || row.incarnation > hagency_core::JSON_SAFE_MAX
        || row.room_generation == 0
        || row.room_generation > hagency_core::JSON_SAFE_MAX
        || serde_json::to_vec(&row)?.len() > 8 * 1024
    {
        return Err(Error::Capacity);
    }
    row.revision = canonical::digest(&serde_json::to_value(&row)?)?;
    Ok(row)
}
impl DomainRepository {
    pub(crate) fn management_approval_bindings(
        &self,
        query: &ResourceApprovalBindingQuery,
        check: impl Fn(&Connection) -> Result<(), Error>,
    ) -> Result<ResourceApprovalBindingPage, Error> {
        query.validate()?;
        let tx = self.db.unchecked_transaction()?;
        check(&tx)?;
        let mut statement = tx.prepare(&format!("SELECT {COLUMNS} FROM {JOINS} WHERE e.resource_id=?1 AND e.id>?2 AND (?3='' OR json_extract(e.projection,'$.agentName')=?3) AND (?4='' OR e.project_id=?4 OR p.room_id=?4) ORDER BY e.id LIMIT 17"))?;
        let mut rows = statement
            .query_map(
                params![query.resource_id, query.after, query.agent, query.project],
                row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let more = rows.len() > 16;
        rows.truncate(16);
        let bindings = rows
            .into_iter()
            .map(project)
            .collect::<Result<Vec<_>, _>>()?;
        let next_after = if more {
            bindings.last().map(|r| r.engagement_id.clone())
        } else {
            None
        };
        let result = ResourceApprovalBindingPage {
            resource_id: query.resource_id.clone(),
            agent: query.agent.clone(),
            project: query.project.clone(),
            bindings,
            next_after,
            observed_at_ms: crate::domain::graphs::now_ms()?,
        };
        if serde_json::to_vec(&result)?.len() > 128 * 1024 {
            return Err(Error::Capacity);
        }
        check(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    pub(crate) fn management_retire_approval_binding(
        &mut self,
        input: &RetireResourceApprovalBinding,
        receipt: CommandReceipt,
        check: impl Fn(&Connection) -> Result<(), Error>,
    ) -> Result<RetiredResourceApprovalBinding, Error> {
        input.validate()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check(&tx)?;
        // Recover the original removal before consulting a possible successor
        // binding. Retrying an old result must never remove that successor.
        if let Some(result) = receipt.lookup::<RetiredResourceApprovalBinding>(&tx)? {
            check(&tx)?;
            return Ok(result);
        }
        let binding = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM {JOINS} WHERE e.resource_id=?1 AND e.id=?2"),
                params![input.resource_id, input.engagement_id],
                row,
            )
            .optional()?
            .ok_or(Error::NotFound)?;
        let binding = project(binding)?;
        if binding.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        let revoked = tx.execute(
            "UPDATE approval_grants SET revoked=1 WHERE engagement_id=?1 AND revoked=0",
            [&input.engagement_id],
        )?;
        if tx.execute(
            "DELETE FROM approval_bindings WHERE engagement_id=?1",
            [&input.engagement_id],
        )? != 1
        {
            return Err(Error::Conflict);
        }
        let result = RetiredResourceApprovalBinding {
            binding,
            revoked_grants: revoked as u64,
            unbound: true,
        };
        receipt.record(&tx, &result)?;
        check(&tx)?;
        tx.commit().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
