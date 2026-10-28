//! Never release another resource's workspace or erase unresolved custody.
use super::*;
use crate::management::{
    ClearWorkspaceDirty, CommandReceipt, WorkspaceListQuery, WorkspacePage, WorkspaceQuery,
    WorkspaceRelease, WorkspaceReleaseBlock, WorkspaceStatus,
};
use rusqlite::Connection;

fn snapshot(
    db: &Connection,
    query: &WorkspaceQuery,
    check: &impl Fn(&Connection, &str) -> Result<(), Error>,
) -> Result<WorkspaceStatus, Error> {
    check(db, &query.resource_id)?;
    let dirty: bool = db
        .query_row(
            "SELECT dirty FROM workspace_resources WHERE id=?1",
            [&query.workspace_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(Error::NotFound)?;
    // Inspect every retained association. Do not infer exclusive ownership from
    // one selected agent or from the current lease alone. Resource names/paths
    // belonging to other owners never enter the public snapshot.
    let mut resources=db.prepare("SELECT DISTINCT e.resource_id FROM dispatch_resources r JOIN runner_dispatches d ON d.id=r.dispatch_id JOIN runner_sessions s ON s.id=d.session_id JOIN engagements e ON e.id=s.engagement_id WHERE r.resource_id=?1 ORDER BY e.resource_id")?;
    let mut associated = false;
    let mut shared_authority = true;
    let mut count = 0u64;
    for resource in resources.query_map([&query.workspace_id], |r| r.get::<_, String>(0))? {
        let resource = resource?;
        count += 1;
        associated |= resource == query.resource_id;
        match check(db, &resource) {
            Ok(()) => {}
            Err(Error::LocalAuthority) => shared_authority = false,
            Err(error) => return Err(error),
        }
    }
    if !associated {
        return Err(Error::NotFound);
    }
    let leases: u64 = db.query_row(
        "SELECT count(*) FROM resource_leases WHERE resource_id=?1",
        [&query.workspace_id],
        |r| r.get(0),
    )?;
    let quarantined:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM dispatch_resources r JOIN unresolved_dispatches u ON u.id=r.dispatch_id WHERE r.resource_id=?1 AND r.exclusive=1)",[&query.workspace_id],|r|r.get(0))?;
    let released = db
        .query_row(
            "SELECT inspected_at FROM workspace_dirty_releases WHERE resource_id=?1",
            [&query.workspace_id],
            |r| r.get::<_, u64>(0),
        )
        .optional()?;
    let block = if !shared_authority {
        Some(WorkspaceReleaseBlock::SharedResourceAuthority)
    } else if quarantined {
        Some(WorkspaceReleaseBlock::InspectionRequired)
    } else if leases > 0 {
        Some(WorkspaceReleaseBlock::ActiveExecution)
    } else {
        None
    };
    let mut result = WorkspaceStatus {
        resource_id: query.resource_id.clone(),
        workspace_id: query.workspace_id.clone(),
        dirty,
        active_leases: leases,
        quarantined,
        shared_resource_count: count,
        release_block: block,
        last_released_at: released,
        revision: String::new(),
    };
    // Stream a revision over exact attempts, not just counts: a finished task
    // followed by a new one must invalidate the old review, even with equal
    // counts/dirty flags. Memory does not grow with the workspace's history.
    let mut revision = hagency_core::canonical::digest(&json!(result))?;
    let mut statement=db.prepare("SELECT r.dispatch_id,r.exclusive,d.fence,d.state,s.engagement_id,e.resource_id,l.exclusive,EXISTS(SELECT 1 FROM unresolved_dispatches u WHERE u.id=d.id) FROM dispatch_resources r JOIN runner_dispatches d ON d.id=r.dispatch_id JOIN runner_sessions s ON s.id=d.session_id JOIN engagements e ON e.id=s.engagement_id LEFT JOIN resource_leases l ON l.resource_id=r.resource_id AND l.dispatch_id=r.dispatch_id WHERE r.resource_id=?1 ORDER BY r.dispatch_id")?;
    let mut rows = statement.query([&query.workspace_id])?;
    while let Some(row) = rows.next()? {
        revision = hagency_core::canonical::digest(&json!([
            revision,
            row.get::<_, String>(0)?,
            row.get::<_, bool>(1)?,
            row.get::<_, u64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<bool>>(6)?,
            row.get::<_, bool>(7)?
        ]))?;
    }
    result.revision = revision;
    check(db, &query.resource_id)?;
    Ok(result)
}

impl DomainRepository {
    pub(crate) fn management_workspaces(
        &self,
        query: &WorkspaceListQuery,
        check: impl Fn(&Connection, &str) -> Result<(), Error>,
    ) -> Result<WorkspacePage, Error> {
        query.validate()?;
        let tx = self.db.unchecked_transaction()?;
        check(&tx, &query.resource_id)?;
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE id=?1)",
            [&query.resource_id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::NotFound);
        }
        let mut statement=tx.prepare("SELECT DISTINCT r.resource_id FROM dispatch_resources r JOIN runner_dispatches d ON d.id=r.dispatch_id JOIN runner_sessions s ON s.id=d.session_id JOIN engagements e ON e.id=s.engagement_id WHERE e.resource_id=?1 AND r.resource_id>?2 ORDER BY r.resource_id LIMIT 17")?;
        let ids = statement
            .query_map(params![query.resource_id, query.after], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let more = ids.len() > 16;
        let workspaces = ids
            .into_iter()
            .take(16)
            .map(|workspace_id| {
                snapshot(
                    &tx,
                    &WorkspaceQuery {
                        resource_id: query.resource_id.clone(),
                        workspace_id,
                    },
                    &check,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let next_after = if more {
            workspaces.last().map(|w| w.workspace_id.clone())
        } else {
            None
        };
        let result = WorkspacePage {
            resource_id: query.resource_id.clone(),
            workspaces,
            next_after,
        };
        if serde_json::to_vec(&result)?.len() > 128 * 1024 {
            return Err(Error::Capacity);
        }
        check(&tx, &query.resource_id)?;
        tx.commit()?;
        Ok(result)
    }
    pub(crate) fn management_workspace(
        &self,
        query: &WorkspaceQuery,
        check: impl Fn(&Connection, &str) -> Result<(), Error>,
    ) -> Result<WorkspaceStatus, Error> {
        query.validate()?;
        let tx = self.db.unchecked_transaction()?;
        let result = snapshot(&tx, query, &check)?;
        tx.commit()?;
        Ok(result)
    }
    pub(crate) fn management_clear_workspace_dirty(
        &mut self,
        input: &ClearWorkspaceDirty,
        receipt: CommandReceipt,
        check: impl Fn(&Connection, &str) -> Result<(), Error>,
    ) -> Result<WorkspaceRelease, Error> {
        input.validate()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = snapshot(&tx, &input.query(), &check)?;
        if current.release_block == Some(WorkspaceReleaseBlock::SharedResourceAuthority) {
            return Err(Error::LocalAuthority);
        }
        // The original receipt is descriptive: retrying never clears a newer
        // dirty mark or circumvents a new attempt's quarantine/lease.
        if let Some(original) = receipt.lookup::<WorkspaceRelease>(&tx)? {
            return Ok(original);
        }
        if current.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        match current.release_block {
            Some(WorkspaceReleaseBlock::InspectionRequired) => {
                return Err(Error::WorkspaceInspectionRequired);
            }
            Some(WorkspaceReleaseBlock::ActiveExecution) => return Err(Error::WorkspaceActive),
            Some(WorkspaceReleaseBlock::SharedResourceAuthority) => {
                return Err(Error::LocalAuthority);
            }
            None => {}
        }
        let cleared = tx.execute(
            "UPDATE workspace_resources SET dirty=0 WHERE id=?1 AND dirty=1",
            [&input.workspace_id],
        )? == 1;
        tx.execute("INSERT INTO workspace_dirty_releases(resource_id,inspected_at) VALUES(?1,?2) ON CONFLICT(resource_id) DO UPDATE SET inspected_at=?2",params![input.workspace_id,crate::domain::graphs::now_ms()?])?;
        let result = WorkspaceRelease {
            workspace: snapshot(&tx, &input.query(), &check)?,
            cleared,
        };
        // Recheck every affected authority immediately before the commit.
        if result.workspace.release_block == Some(WorkspaceReleaseBlock::SharedResourceAuthority) {
            return Err(Error::LocalAuthority);
        }
        receipt.record(&tx, &result)?;
        check(&tx, &input.resource_id)?;
        tx.commit().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
