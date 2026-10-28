//! Bounded projection of the retained operator board; no dispatch or file inspection.
use super::DomainRepository;
use crate::{
    Error, TASK_STATUSES,
    management::{OperatorBoardPage, OperatorBoardProject, OperatorBoardQuery, TASK_READ_PAGE},
};
use rusqlite::params;
use std::collections::BTreeMap;
impl DomainRepository {
    pub(crate) fn management_operator_board(
        &self,
        q: OperatorBoardQuery,
        at: u64,
    ) -> Result<OperatorBoardPage, Error> {
        let tx = self.db.unchecked_transaction()?;
        let project_count = tx.query_row("SELECT COUNT(DISTINCT id) FROM projects", [], |r| r.get(0))?;
        let agent_count=tx.query_row("SELECT COUNT(*) FROM (SELECT DISTINCT e.project_id,e.name FROM engagements e JOIN projects p ON p.id=e.project_id WHERE e.state='active')",[],|r|r.get(0))?;
        let mut task_totals = BTreeMap::new();
        for status in TASK_STATUSES {
            task_totals.insert(
                status.to_owned(),
                tx.query_row(
                    "SELECT COUNT(*) FROM operator_tasks WHERE status=?1",
                    [status],
                    |r| r.get(0),
                )?,
            );
        }
        let mut statement=tx.prepare("SELECT DISTINCT id FROM projects WHERE (?1 IS NULL OR id=?1) AND (?2 IS NULL OR id>?2) ORDER BY id LIMIT ?3")?;
        let ids = statement
            .query_map(
                params![q.project_id, q.after, (TASK_READ_PAGE + 1) as u64],
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if q.project_id.is_some() && ids.is_empty() {
            return Err(Error::NotFound);
        }
        let more = ids.len() > TASK_READ_PAGE;
        let mut projects = Vec::new();
        for id in ids.into_iter().take(TASK_READ_PAGE) {
            let count=tx.query_row("SELECT COUNT(DISTINCT name) FROM engagements WHERE project_id=?1 AND state='active'",[&id],|r|r.get(0))?;
            let limit = if q.project_id.is_some() { 16 } else { 4 };
            let mut s=tx.prepare("SELECT DISTINCT name FROM engagements WHERE project_id=?1 AND state='active' AND (?2 IS NULL OR name>?2) ORDER BY name LIMIT ?3")?;
            let mut agents = s
                .query_map(params![id, q.agent_after, limit + 1], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(s);
            let more_agents = agents.len() > limit as usize;
            agents.truncate(limit as usize);
            let next_agent_after = if more_agents {
                agents.last().cloned()
            } else {
                None
            };
            let mut lanes = BTreeMap::new();
            for status in TASK_STATUSES {
                lanes.insert(status.to_owned(),tx.query_row("SELECT COUNT(*) FROM operator_tasks t WHERE t.status=?1 AND t.assignee IN (SELECT name FROM engagements WHERE project_id=?2 AND state='active')",params![status,id],|r|r.get(0))?);
            }
            projects.push(OperatorBoardProject {
                id: id.clone(),
                name: id,
                agents,
                agent_count: count,
                next_agent_after,
                task_lanes: lanes,
            });
        }
        let next_after = if more {
            projects.last().map(|p| p.id.clone())
        } else {
            None
        };
        tx.commit()?;
        Ok(OperatorBoardPage {
            supported: true,
            reason: None,
            projects,
            next_after,
            project_count,
            agent_count,
            task_totals,
            observed_at_ms: at,
            stale_after_ms: 300_000,
            unavailable: vec![
                "createdAt",
                "binding",
                "health",
                "repositories",
                "worktrees",
                "specs",
                "issues",
                "activity",
            ],
        })
    }
}
