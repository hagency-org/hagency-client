//! Canonical operator task history. The initializer is native-only and uses
//! the exact active v1 empty-history or v2 staged-task contract; readers never open archives.
use super::*;
use super::super::empty_initialization::{self, EmptyPlan};
mod commands;
use crate::management::{
    OperatorBoardPage, OperatorBoardQuery, OperatorTaskDetail, OperatorTaskDetailQuery,
    OperatorTaskPage, OperatorTaskQuery, TASK_COMMENT_PAGE, TASK_READ_PAGE,
};
use crate::{OperatorTask, OperatorTaskComment, TASK_STATUSES};
use std::collections::BTreeMap;

#[derive(Clone)]
pub(super) enum TaskBinding {
    Migration(Arc<Plan>),
    EmptyDevice(Arc<EmptyPlan>),
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmptyHistoryReceipt {
    version: u16,
    kind: String,
    migration_id: String,
    registry_id: String,
    profile_digest: String,
    owner_mxid: String,
    plan_digest: String,
    inventory_digest: String,
    source_schema: i32,
    source_evidence: String,
    custody_evidence: String,
    original_tasks: u64,
    original_comments: u64,
    original_projects: u64,
    original_engagements: u64,
}
fn manifest(plan: &Plan, kind: &str) -> Value {
    json!({"version":1,"kind":kind,"migrationId":plan.migration_id,
        "registryId":plan.registry_id,"profileDigest":plan.inventory.profile,
        "ownerMxid":plan.inventory.owner,"inventoryDigest":plan.inventory_digest,
        "sourceSchema":plan.inventory.source_schema})
}
fn expected(
    db: &Connection,
    authority: &AdoptionAuthority,
    original: &Plan,
) -> Result<Option<EmptyHistoryReceipt>, Error> {
    authority.check()?;
    let (plan, phase) = writer::read(db, authority, &original.migration_id)?;
    if phase != 4 || plan.digest()? != original.digest()? {
        return Err(Error::LocalAuthority);
    }
    // This selects a reviewed contract, not authority from a schema number.
    // v1 collect rejected nonempty tasks/comments/projects/engagements before
    // producing the sealed plan and verified source/custody evidence below.
    if plan.version != 1 || !matches!(plan.inventory.source_schema, 77..=83) {
        return Ok(None);
    }
    if canonical::digest(&serde_json::to_value(&plan.inventory)?)? != plan.inventory_digest {
        return Err(Error::Quarantined);
    }
    let plan_digest = plan.digest()?;
    let source_evidence = canonical::digest(&json!({"plan":plan_digest,
        "management":manifest(&plan,"management"),"personal":manifest(&plan,"personal")}))?;
    let custody_evidence = canonical::digest(&json!({"plan":plan_digest,
        "custody":plan.inventory.custody_digest,"count":plan.inventory.custody_rows}))?;
    let (source, custody, result): (String, String, String) = db.query_row(
        "SELECT CASE WHEN length(source_evidence)=64 THEN source_evidence END,CASE WHEN length(custody_evidence)=64 THEN custody_evidence END,CASE WHEN length(CAST(result AS BLOB))<=262144 THEN result END FROM owner_resource_adoptions WHERE id=?1",
        [&plan.migration_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    )?;
    if source != source_evidence
        || custody != custody_evidence
        || serde_json::from_str::<Value>(&result)? != serde_json::to_value(plan.summary(4)?)?
    {
        return Err(Error::Quarantined);
    }
    authority.check()?;
    Ok(Some(EmptyHistoryReceipt {
        version: 1,
        kind: "empty_resource_adoption_v1".into(),
        migration_id: plan.migration_id,
        registry_id: plan.registry_id,
        profile_digest: plan.inventory.profile,
        owner_mxid: plan.inventory.owner,
        plan_digest,
        inventory_digest: plan.inventory_digest,
        source_schema: plan.inventory.source_schema,
        source_evidence,
        custody_evidence,
        original_tasks: 0,
        original_comments: 0,
        original_projects: 0,
        original_engagements: 0,
    }))
}
fn saved(db: &Connection, receipt: &EmptyHistoryReceipt) -> Result<bool, Error> {
    let row: Option<(String, String, String)> = db.query_row(
        "SELECT migration_id,receipt_digest,CASE WHEN length(CAST(receipt AS BLOB))<=8192 THEN receipt END FROM owner_operator_task_histories WHERE profile=?1",
        [&receipt.profile_digest],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?;
    let Some((migration, digest, raw)) = row else {
        return Ok(false);
    };
    let actual: EmptyHistoryReceipt = serde_json::from_str(&raw)?;
    if migration != receipt.migration_id
        || &actual != receipt
        || canonical::digest(&serde_json::to_value(actual)?)? != digest
    {
        return Err(Error::Quarantined);
    }
    Ok(true)
}
impl DomainRepository {
    /// Called only by trusted adopted backing construction, never a script read.
    pub(super) fn initialize_empty_operator_history(
        &mut self,
        authority: &AdoptionAuthority,
        original: &Plan,
    ) -> Result<Option<TaskBinding>, Error> {
        authority.check()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Recheck after SQLite's writer wait and again immediately pre-commit.
        authority.check()?;
        if matches!(original.version, 2 | 3) && original.inventory.task_history.is_some() {
            task_transfer::authorize_active(&tx, authority, original)?;
            task_transfer::verify(&tx, original)?;
            tx.commit()?;
            authority.check()?;
            return Ok(Some(TaskBinding::Migration(Arc::new(original.clone()))));
        }
        let Some(receipt) = expected(&tx, authority, original)? else {
            return Ok(None);
        };
        if !saved(&tx, &receipt)? {
            let has_rows: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_operator_tasks WHERE profile=?1) OR EXISTS(SELECT 1 FROM owner_operator_task_comments WHERE profile=?1)",[&receipt.profile_digest],|r|r.get(0))?;
            if has_rows {
                return Err(Error::Quarantined);
            }
            tx.execute("INSERT INTO owner_operator_task_histories(profile,migration_id,receipt_digest,receipt) VALUES(?1,?2,?3,?4)",params![receipt.profile_digest,receipt.migration_id,canonical::digest(&serde_json::to_value(&receipt)?)?,serde_json::to_string(&receipt)?])?;
        }
        authority.check()?;
        tx.commit()?;
        authority.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(Some(TaskBinding::Migration(Arc::new(original.clone()))))
    }
}
fn empty_device_receipt(
    db: &Connection, authority: &AdoptionAuthority, original: &EmptyPlan,
) -> Result<EmptyHistoryReceipt, Error> {
    let (plan, phase) = empty_initialization::read_state(db, authority, &original.initialization_id)?;
    if phase != 4 || plan.digest()? != original.digest()? { return Err(Error::LocalAuthority); }
    Ok(EmptyHistoryReceipt {
        version: 2,
        kind: "empty_device_initialization_v4".into(),
        plan_digest: plan.digest()?,
        source_evidence: empty_initialization::source_evidence(&plan)?,
        custody_evidence: empty_initialization::custody_evidence(&plan)?,
        migration_id: plan.initialization_id,
        registry_id: plan.registry_id,
        profile_digest: plan.inventory.profile,
        owner_mxid: plan.inventory.owner,
        inventory_digest: plan.inventory_digest,
        source_schema: plan.inventory.source_schema,
        original_tasks: 0, original_comments: 0, original_projects: 0, original_engagements: 0,
    })
}
impl DomainRepository {
    pub(super) fn initialize_empty_device_operator_history(
        &mut self, authority: &AdoptionAuthority, original: &EmptyPlan,
    ) -> Result<TaskBinding, Error> {
        authority.check()?;
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let receipt = empty_device_receipt(&tx, authority, original)?;
        if !saved(&tx, &receipt)? {
            let has_rows: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_operator_tasks WHERE profile=?1) OR EXISTS(SELECT 1 FROM owner_operator_task_comments WHERE profile=?1)", [&receipt.profile_digest], |r| r.get(0))?;
            if has_rows { return Err(Error::Quarantined); }
            tx.execute("INSERT INTO owner_operator_task_histories(profile,migration_id,receipt_digest,receipt) VALUES(?1,?2,?3,?4)", params![receipt.profile_digest,receipt.migration_id,canonical::digest(&serde_json::to_value(&receipt)?)?,serde_json::to_string(&receipt)?])?;
        }
        authority.check()?;
        tx.commit()?;
        authority.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(TaskBinding::EmptyDevice(Arc::new(original.clone())))
    }
}
impl AdoptedManagementStore {
    fn task_authority(&self, profile: &ProfileAuthority) -> Result<AdoptionAuthority, Error> {
        self.check(profile)?;
        let retained = self.clone();
        AdoptionAuthority::new(
            self.registry.clone(),
            profile.clone(),
            Instant::now() + Duration::from_secs(30),
            move || retained.is_live(),
        )
    }
    fn task_read_proof(
        &self,
        db: &Connection,
        authority: &AdoptionAuthority,
    ) -> Result<bool, Error> {
        let Some(binding) = &self.task_history else {
            return Ok(false);
        };
        let receipt = match binding {
            TaskBinding::Migration(original) if matches!(original.version, 2 | 3) => {
                task_transfer::authorize_active(db, authority, original)?;
                return Ok(true);
            }
            TaskBinding::Migration(original) => {
                let Some(receipt) = expected(db, authority, original)? else { return Ok(false); };
                receipt
            }
            TaskBinding::EmptyDevice(original) => empty_device_receipt(db, authority, original)?,
        };
        saved(db, &receipt)
    }
    pub(crate) async fn operator_task_page(
        &self,
        profile: &ProfileAuthority,
        query: OperatorTaskQuery,
        observed: u64,
    ) -> Result<Option<OperatorTaskPage>, Error> {
        query.validate()?;
        let authority = self.task_authority(profile)?;
        let backing = self.clone();
        let after = authority.clone();
        let value=self.registry.0.store.call(1024,move|db|{
            let tx=db.db.unchecked_transaction()?;
            if !backing.task_read_proof(&tx,&authority)? { return Ok(None) }
            let statuses=query.status.as_deref().unwrap_or_default().split(',').map(str::trim).filter(|s|!s.is_empty()).collect::<Vec<_>>();
            let status_json=serde_json::to_string(&statuses)?;
            let mut statement=tx.prepare(&format!("{SELECT} WHERE profile=?1 AND (?2 IS NULL OR ?2='' OR assignee=?2) AND (?3='[]' OR status IN (SELECT value FROM json_each(?3))) AND (?4 IS NULL OR ?4='' OR priority=?4) AND (?5 IS NULL OR ?5='' OR EXISTS(SELECT 1 FROM json_each(labels) WHERE value=?5)) ORDER BY created_at DESC,id DESC LIMIT 17 OFFSET ?6"))?;
            let tasks=statement.query_map(params![authority.profile.digest,query.assignee,status_json,query.priority,query.label,query.offset],task_row)?.collect::<Result<Vec<_>,_>>()?;
            drop(statement);
            let more=tasks.len()>TASK_READ_PAGE;
            let tasks=tasks.into_iter().take(TASK_READ_PAGE).map(|task|task.map(Into::into)).collect::<Result<Vec<_>,_>>()?;
            let value=OperatorTaskPage{write_supported:true,write_operations:vec![],supported:true,reason:None,next_offset:more.then_some(query.offset+tasks.len() as u64),tasks,observed_at_ms:observed,unavailable:vec!["health"]};
            authority.check()?;tx.commit()?;Ok(Some(value))
        }).await?;
        after.check()?;
        self.check(profile)?;
        Ok(value)
    }
    pub(crate) async fn operator_task_detail(
        &self,
        profile: &ProfileAuthority,
        query: OperatorTaskDetailQuery,
        observed: u64,
    ) -> Result<Option<OperatorTaskDetail>, Error> {
        query.validate()?;
        let authority = self.task_authority(profile)?;
        let backing = self.clone();
        let after = authority.clone();
        let value=self.registry.0.store.call(512,move|db|{
            let tx=db.db.unchecked_transaction()?;
            if !backing.task_read_proof(&tx,&authority)? { return Ok(None) }
            let mut task=tx.query_row(&format!("{SELECT} WHERE profile=?1 AND id=?2"),params![authority.profile.digest,query.id],task_row).optional()?.ok_or(Error::NotFound)??;
            let comment_count=tx.query_row("SELECT COUNT(*) FROM owner_operator_task_comments WHERE profile=?1 AND task_id=?2",params![authority.profile.digest,query.id],|r|r.get::<_,usize>(0))?;
            if comment_count>crate::MAX_TASK_COMMENTS {return Err(Error::Capacity)}
            let mut statement=tx.prepare("SELECT author,text,at_ms FROM owner_operator_task_comments WHERE profile=?1 AND task_id=?2 ORDER BY ordinal LIMIT ?3 OFFSET ?4")?;
            task.comments=statement.query_map(params![authority.profile.digest,query.id,TASK_COMMENT_PAGE as u64,query.comment_offset as u64],|r|Ok(OperatorTaskComment{author:r.get(0)?,text:r.get(1)?,ts:time(r.get(2)?)}))?.collect::<Result<Vec<_>,_>>()?;
            drop(statement);
            let end=query.comment_offset+task.comments.len();
            let revision=tx.query_row("SELECT revision FROM owner_operator_tasks WHERE profile=?1 AND id=?2",params![authority.profile.digest,query.id],|r|r.get(0))?;
            let value=OperatorTaskDetail{write_supported:true,write_operations:vec![],revision:Some(revision),supported:true,reason:None,next:crate::operator_transitions(&task.status).iter().map(|s|(*s).into()).collect(),task:Some(task),comment_count,next_comment_offset:(end<comment_count).then_some(end),observed_at_ms:observed,unavailable:vec!["health"]};
            authority.check()?;tx.commit()?;Ok(Some(value))
        }).await?;
        after.check()?;
        self.check(profile)?;
        Ok(value)
    }
    pub(crate) async fn operator_board(
        &self,
        profile: &ProfileAuthority,
        query: OperatorBoardQuery,
        observed: u64,
    ) -> Result<Option<OperatorBoardPage>, Error> {
        query.validate()?;
        let authority = self.task_authority(profile)?;
        let backing = self.clone();
        let after = authority.clone();
        let value=self.registry.0.store.call(512,move|db|{
            let tx=db.db.unchecked_transaction()?;
            if !backing.task_read_proof(&tx,&authority)? { return Ok(None) }
            // The reviewed v1 source contract also excludes projects and
            // engagements. No central project or running-agent inference.
            if query.project_id.is_some() {return Err(Error::NotFound)}
            let mut totals=BTreeMap::new();
            for status in TASK_STATUSES {totals.insert(status.into(),tx.query_row("SELECT COUNT(*) FROM owner_operator_tasks WHERE profile=?1 AND status=?2",params![authority.profile.digest,status],|r|r.get(0))?);}
            let value=OperatorBoardPage{supported:true,reason:None,projects:vec![],next_after:None,project_count:0,agent_count:0,task_totals:totals,observed_at_ms:observed,stale_after_ms:300_000,unavailable:vec!["createdAt","binding","health","repositories","worktrees","specs","issues","activity"]};
            authority.check()?;tx.commit()?;Ok(Some(value))
        }).await?;
        after.check()?;
        self.check(profile)?;
        Ok(value)
    }
}
const SELECT: &str = "SELECT id,title,description,status,priority,granularity,assignee,created_by,created_at,updated_at,started_at,completed_at,heartbeat_at,waiting_reason,waiting_until,parent_id,labels FROM owner_operator_tasks";
fn time(ms: u64) -> String {
    crate::domain::operator_tasks::iso8601(ms)
}
fn task_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<OperatorTask, Error>> {
    let labels: String = r.get(16)?;
    let labels = match serde_json::from_str::<Vec<String>>(&labels) {
        Ok(labels) if labels.len() <= 20 && labels.iter().all(|s| s.chars().count() <= 64) => {
            labels
        }
        _ => return Ok(Err(Error::Quarantined)),
    };
    Ok(Ok(OperatorTask {
        id: r.get(0)?,
        title: r.get(1)?,
        description: r.get(2)?,
        status: r.get(3)?,
        priority: r.get(4)?,
        granularity: r.get(5)?,
        assignee: r.get(6)?,
        created_by: r.get(7)?,
        created_at: time(r.get(8)?),
        updated_at: time(r.get(9)?),
        started_at: r.get::<_, Option<u64>>(10)?.map(time),
        completed_at: r.get::<_, Option<u64>>(11)?.map(time),
        heartbeat_at: r.get::<_, Option<u64>>(12)?.map(time),
        waiting_reason: r.get(13)?,
        waiting_until: r.get(14)?,
        parent_id: r.get(15)?,
        labels,
        comments: vec![],
    }))
}
