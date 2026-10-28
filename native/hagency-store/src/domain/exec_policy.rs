//! Workspace dirty release, and the durable per-agent execution policy.
//!
//! Two retained behaviours live here, both ported from the TypeScript product:
//!
//! * `clear_workspace_dirty` — `POST /api/router/resources/:id/clear-dirty`
//!   (backend-v2.js:8897-8902 → router/src/store.ts:3297). It releases a
//!   workspace the dispatch gate is holding dirty, and it REFUSES when the
//!   resource is quarantined: an outcome that is unknown must be cleared
//!   through the stopped-dispatch inspection flow, never by this route
//!   (`inspection_required`, backend-v2.js:1929).
//! * `execution_policy` / `set_execution_policy` — `GET`/`PUT
//!   /api/agents/:name/execution-policy` (backend-v2.js:10865-10885). TS keeps
//!   the policy on the agent record (`agent.executionPolicy`); native's agent
//!   IS the engagement, so the policy is keyed by engagement and a missing row
//!   is TS's default `{ yolo: false }` (backend-v2.js:10867 reads
//!   `agent.executionPolicy?.yolo === true`).
mod management;
mod workspaces;
use super::DomainRepository;
use crate::Error;
use hagency_core::{execution::normalize_policy, project::identifier, tasks::clock};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

impl DomainRepository {
    /// Release a dirty workspace. Refuses when the resource is quarantined:
    /// the newest exclusive dispatch on it settled as `outcome_unknown` and is
    /// still unresolved — the same refusal TS spells `inspection_required`.
    /// Returns whether a row was actually released, so a replay is not a
    /// second write.
    pub fn clear_workspace_dirty(&mut self, id: &str, now: u64) -> Result<bool, Error> {
        identifier(id, 128)?;
        clock(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspace_resources WHERE id=?1)",
            [id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(Error::NotFound);
        }
        // TS's quarantine guard is `if (row.dirty_dispatch_id) return
        // refusal('inspection_required', ...)`. Native does not store a
        // `dirty_dispatch_id`; its equivalent fact is an exclusive dispatch on
        // this resource that is still an unresolved `outcome_unknown`.
        let quarantined: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM dispatch_resources r \
             JOIN unresolved_dispatches u ON u.id=r.dispatch_id \
             WHERE r.resource_id=?1 AND r.exclusive=1)",
            [id],
            |r| r.get(0),
        )?;
        if quarantined {
            return Err(Error::Quarantined);
        }
        let released = tx.execute(
            "UPDATE workspace_resources SET dirty=0 WHERE id=?1 AND dirty=1",
            [id],
        )?;
        // The release receipt TS writes (`inspected_at`, store.ts:3312). Written
        // whenever the release is permitted, so a replay re-stamps the same row
        // rather than failing on the primary key.
        tx.execute(
            "INSERT INTO workspace_dirty_releases(resource_id,inspected_at) VALUES(?1,?2) \
             ON CONFLICT(resource_id) DO UPDATE SET inspected_at=?2",
            params![id, now],
        )?;
        tx.commit()?;
        Ok(released == 1)
    }

    /// The framework of the resource an engagement runs, resolved the same way
    /// `agent_roster` resolves it (engagements JOIN resources, parse config).
    /// TS reads `threadSessionFramework(agent)` for the same purpose.
    pub fn agent_framework(&self, engagement: &str) -> Result<String, Error> {
        identifier(engagement, 128)?;
        let config: Option<String> = self
            .db
            .query_row(
                "SELECT r.config FROM engagements e JOIN resources r ON r.id=e.resource_id WHERE e.id=?1",
                [engagement],
                |r| r.get(0),
            )
            .optional()?;
        let config = config.ok_or(Error::NotFound)?;
        let resource: hagency_core::project::Resource = serde_json::from_str(&config)?;
        Ok(resource.framework)
    }

    /// The per-agent execution policy as the console reads it, in the retained
    /// wire shape: `{ executionPolicy: { yolo }, grants, appliesTo }`.
    /// `appliesTo` is TS's constant `'next_dispatch'` — the policy takes effect
    /// on the next dispatch, never retroactively (backend-v2.js:10867-10868).
    pub fn execution_policy(&self, engagement: &str) -> Result<Value, Error> {
        self.check_agent(engagement)?;
        let yolo: Option<bool> = self
            .db
            .query_row(
                "SELECT yolo FROM agent_execution_policies WHERE engagement_id=?1",
                [engagement],
                |r| r.get(0),
            )
            .optional()?;
        // The retained `consoleExecutionGrant` projection, restricted to the
        // columns native's store actually holds. A grant row that native does
        // not model is absent, never invented.
        let grants = self
            .db
            .prepare(
                "SELECT id,scope_kind,scope_key,mode,task_id FROM approval_grants \
                 WHERE engagement_id=?1 AND revoked=0 ORDER BY id LIMIT 100",
            )?
            .query_map([engagement], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "scope": r.get::<_, String>(1)?,
                    "key": r.get::<_, String>(2)?,
                    "mode": r.get::<_, String>(3)?,
                    "taskId": r.get::<_, Option<String>>(4)?,
                    "active": true,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({
            "executionPolicy": { "yolo": yolo.unwrap_or(false) },
            "grants": grants,
            "appliesTo": "next_dispatch",
        }))
    }

    /// Persist the policy through ONE write path. The value is normalized with
    /// the engagement's framework exactly as TS does at the route
    /// (`normalizeExecutionPolicy(req.body?.executionPolicy,
    /// threadSessionFramework(agent))`, backend-v2.js:10875 → the shared
    /// `normalize_policy`, hagency-core/src/execution.rs:18). A value the
    /// retained normalizer rejects is `Invalid` → the route's 400.
    pub fn set_execution_policy(
        &mut self,
        engagement: &str,
        policy: Option<&Value>,
        now: u64,
    ) -> Result<bool, Error> {
        self.check_agent(engagement)?;
        clock(now)?;
        let framework = self.agent_framework(engagement)?;
        let policy = normalize_policy(policy, &framework).map_err(Error::Invalid)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_policy(&tx, engagement, policy.yolo, now)?;
        tx.commit()?;
        Ok(policy.yolo)
    }

    fn check_agent(&self, engagement: &str) -> Result<(), Error> {
        identifier(engagement, 128)?;
        if !self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM engagements WHERE id=?1)",
            [engagement],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::NotFound);
        }
        Ok(())
    }
}


/// The policy of an already started attempt is immutable. Leased attempts
/// without a snapshot read the current owner setting and compare it at start.
/// Original attempts without this table's row preserve their legacy mode/hash.
pub(super) fn dispatch_policy(
    db: &rusqlite::Connection,
    dispatch: &str,
    engagement: &str,
    state: &str,
    framework: &str,
) -> Result<Option<hagency_core::execution::ExecutionPolicy>, Error> {
    let saved: Option<(String, bool)> = db.query_row(
        "SELECT engagement_id,yolo FROM owned_dispatch_execution_policies WHERE dispatch_id=?1",
        [dispatch], |row| Ok((row.get(0)?,row.get(1)?)),
    ).optional()?;
    let yolo = if let Some((owner, value)) = saved {
        if owner != engagement { return Err(Error::Schema); }
        value
    } else if state == "leased" {
        db.query_row("SELECT yolo FROM agent_execution_policies WHERE engagement_id=?1", [engagement], |row| row.get(0)).optional()?.unwrap_or(false)
    } else {
        return Ok(None);
    };
    Ok(Some(normalize_policy(Some(&json!({"yolo":yolo})), framework).map_err(Error::Invalid)?))
}

pub(super) fn capture_dispatch_policy(
    db: &rusqlite::Connection,
    dispatch: &str,
    engagement: &str,
    policy: hagency_core::execution::ExecutionPolicy,
    now: u64,
) -> Result<(), Error> {
    db.execute("INSERT INTO owned_dispatch_execution_policies(dispatch_id,engagement_id,yolo,captured_at) VALUES(?1,?2,?3,?4) ON CONFLICT(dispatch_id) DO NOTHING", params![dispatch,engagement,policy.yolo,now])?;
    let retained: (String,bool) = db.query_row("SELECT engagement_id,yolo FROM owned_dispatch_execution_policies WHERE dispatch_id=?1", [dispatch], |row| Ok((row.get(0)?,row.get(1)?)))?;
    if retained != (engagement.to_owned(),policy.yolo) {return Err(Error::RunnerAuthority);}
    Ok(())
}

fn write_policy(db: &rusqlite::Connection, engagement: &str, yolo: bool, now: u64) -> Result<(), Error> {
    clock(now)?;
    db.execute(
        "INSERT INTO agent_execution_policies(engagement_id,yolo,updated_at) VALUES(?1,?2,?3) ON CONFLICT(engagement_id) DO UPDATE SET yolo=?2, updated_at=?3",
        params![engagement, yolo, now],
    )?;
    Ok(())
}

/// Only the first successful provision transition calls this. Existing choices,
/// including an explicit standard mode, win over the current resource default.
/// An older completed effect returns its original receipt before reaching here.
pub(super) fn inherit_resource_policy(
    db: &rusqlite::Connection,
    engagement: &str,
    resource_id: &str,
) -> Result<(), Error> {
    let resource = super::read_resource(db, resource_id)?;
    if let Some(policy) = super::web_catalog::future_policy(db, resource_id)?.or(resource.execution_policy) {
        policy.validate_for(&resource.framework)?;
        db.execute(
            "INSERT INTO agent_execution_policies(engagement_id,yolo,updated_at) VALUES(?1,?2,?3) ON CONFLICT(engagement_id) DO NOTHING",
            params![engagement, policy.yolo, super::graphs::now_ms()?],
        )?;
    }
    Ok(())
}
