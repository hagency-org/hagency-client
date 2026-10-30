//! Canonical retained definitions; approvals and agent snapshots are never edited.
use super::*;
use crate::management::{
    CommandReceipt, ResourceTemplate, ResourceTemplatePage, ResourceTemplateDetail,
    ResourceTemplateCommand, ResourceTemplateResult,
};
use sha2::{Digest, Sha256};

#[derive(Serialize)]
struct Definition {
    id: String,
    name: String,
    role: String,
    enabled: bool,
    created_at: u64,
}
fn definitions(db: &Connection, resource: &str) -> Result<Vec<Definition>, Error> {
    let mut q=db.prepare("SELECT id,name,role,enabled,created_at FROM agent_definitions WHERE resource_id=?1 ORDER BY id LIMIT 201")?;
    let rows = q
        .query_map([resource], |r| {
            Ok(Definition {
                id: r.get(0)?,
                name: r.get(1)?,
                role: r.get(2)?,
                enabled: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() > 200 {
        return Err(Error::Capacity);
    }
    for row in &rows {
        identifier(&row.id, 128)?;
        if row.name.len() > 64 || row.role.len() > 128 {
            return Err(Error::Schema);
        }
    }
    Ok(rows)
}
fn revision(resource: &Resource, rows: &[Definition]) -> Result<String, Error> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            crate::resource_publication_revision(resource)?,
            rows
        ))?)
    ))
}
fn roles(db: &Connection, resource: &Resource) -> Result<Vec<String>, Error> {
    if !resource.provisionable()
        || crate::domain::resource_ceiling::effective_ceiling_tokens(db, resource)?.is_none()
    {
        return Ok(vec![]);
    }
    Ok(hagency_core::qualification::roles()
        .filter(|r| hagency_core::qualification::qualifies(&resource.profile(), r, None))
        .map(str::to_owned)
        .collect())
}
fn use_counts(db: &Connection, name: &str) -> Result<(u64, u64, bool), Error> {
    let active: u64 = db.query_row(
        "SELECT COUNT(*) FROM engagements WHERE name=?1 AND state='active'",
        [name],
        |r| r.get(0),
    )?;
    let pending: u64 = db.query_row(
        "SELECT COUNT(*) FROM engagements WHERE name=?1 AND state IN('pending','reserved')",
        [name],
        |r| r.get(0),
    )?;
    // Native managed approvals keep immutable definition snapshots. Any retained
    // name remains protected, including fenced work awaiting final settlement.
    let (managed,qualified):(u64,u64)=db.query_row("SELECT COUNT(*),COALESCE(SUM(CASE WHEN a.permanent_fence=0 AND e.state='qualified' THEN 1 ELSE 0 END),0) FROM owner_managed_agents a LEFT JOIN owner_managed_effects e ON e.child_id=a.id WHERE json_extract(a.original_command,'$.definition.name')=?1",[name],|r|Ok((r.get(0)?,r.get(1)?)))?;
    Ok((
        active + qualified,
        pending + managed.saturating_sub(qualified),
        active + pending + managed > 0,
    ))
}
fn project(db: &Connection, row: &Definition) -> Result<(ResourceTemplate, bool), Error> {
    let (active, pending, used) = use_counts(db, &row.name)?;
    Ok((
        ResourceTemplate {
            id: row.id.clone(),
            name: row.name.clone(),
            role: row.role.clone(),
            enabled: row.enabled,
            created_at: row.created_at,
            status: if active > 0 {
                "provisioned"
            } else if pending > 0 {
                "reserved"
            } else {
                "defined"
            }
            .into(),
            active_engagements: active,
        },
        used,
    ))
}
impl AdoptedManagementStore {
    pub(crate) async fn resource_templates(
        &self,
        profile: &ProfileAuthority,
        id: String,
        after: Option<String>,
    ) -> Result<ResourceTemplatePage, Error> {
        self.check(profile)?;
        let backing = self.clone();
        let authority = profile.clone();
        let result = self
            .registry
            .0
            .store
            .call(64 * 1024, move |db| {
                backing.check(&authority)?;
                let tx = db.db.unchecked_transaction()?;
                authorize_resource(&tx, &authority, &id)?;
                let resource = read_resource(&tx, &id)?;
                let rows = definitions(&tx, &id)?;
                let revision = revision(&resource, &rows)?;
                let roles = roles(&tx, &resource)?;
                let remaining: Vec<_> = rows
                    .iter()
                    .filter(|r| after.as_ref().is_none_or(|a| r.id > *a))
                    .take(17)
                    .collect();
                let items = remaining
                    .iter()
                    .take(16)
                    .map(|r| project(&tx, r).map(|r| r.0))
                    .collect::<Result<Vec<_>, _>>()?;
                let next_after =
                    (remaining.len() > 16).then(|| items.last().expect("16 rows").id.clone());
                let name = backing.name_for(&tx, &id)?.name;
                let value = ResourceTemplatePage {
                    resource_id: id,
                    resource_name: if name.is_empty() {
                        resource.model
                    } else {
                        name
                    },
                    revision,
                    items,
                    next_after,
                    can_create: rows.len() < 200 && !roles.is_empty(),
                    roles,
                };
                backing.check(&authority)?;
                tx.commit()?;
                Ok(value)
            })
            .await?;
        self.check(profile)?;
        Ok(result)
    }
    pub(crate) async fn resource_template(
        &self,
        profile: &ProfileAuthority,
        resource_id: String,
        id: String,
    ) -> Result<ResourceTemplateDetail, Error> {
        self.check(profile)?;
        let backing = self.clone();
        let authority = profile.clone();
        let result = self
            .registry
            .0
            .store
            .call(64 * 1024, move |db| {
                backing.check(&authority)?;
                let tx = db.db.unchecked_transaction()?;
                authorize_resource(&tx, &authority, &resource_id)?;
                let resource = read_resource(&tx, &resource_id)?;
                let rows = definitions(&tx, &resource_id)?;
                let row = rows.iter().find(|r| r.id == id).ok_or(Error::NotFound)?;
                let (definition, used) = project(&tx, row)?;
                let value = ResourceTemplateDetail {
                    resource_id,
                    revision: revision(&resource, &rows)?,
                    definition,
                    roles: roles(&tx, &resource)?,
                    can_edit_identity: !used,
                    can_toggle: true,
                    can_delete: !used,
                };
                backing.check(&authority)?;
                tx.commit()?;
                Ok(value)
            })
            .await?;
        self.check(profile)?;
        Ok(result)
    }
    pub(crate) async fn apply_resource_template(
        &self,
        profile: &ProfileAuthority,
        input: ResourceTemplateCommand,
        receipt: CommandReceipt,
    ) -> Result<ResourceTemplateResult, Error> {
        input.validate()?;
        self.check(profile)?;
        let backing = self.clone();
        let authority = profile.clone();
        let result=self.registry.0.store.call(64*1024,move|db|{
            backing.check(&authority)?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            backing.check(&authority)?;authorize_resource(&tx,&authority,&input.resource_id)?;
            // Original result precedes CAS, existence, and current eligibility.
            if let Some(result)=receipt.lookup(&tx)? {backing.check(&authority)?;return Ok(result)}
            let resource=read_resource(&tx,&input.resource_id)?;let rows=definitions(&tx,&input.resource_id)?;
            if revision(&resource,&rows)?!=input.expected_revision {return Err(Error::Conflict)}
            let old=rows.iter().find(|r|r.id==input.definition_id);
            let change=&input.change;let creating=change.operation=="create";let deleted=change.operation=="delete";
            if creating {
                if old.is_some(){return Err(Error::Conflict)}
                if rows.len()>=200{return Err(Error::Capacity)}
            } else {
                let old=old.ok_or(Error::NotFound)?;
                if deleted && (old.name!=change.name||old.role!=change.role||old.enabled!=change.enabled){return Err(Error::Conflict)}
                if use_counts(&tx,&old.name)?.2 && (deleted||old.name!=change.name||old.role!=change.role){return Err(Error::State)}
            }
            if deleted {
                tx.execute("DELETE FROM agent_definitions WHERE resource_id=?1 AND id=?2",params![input.resource_id,input.definition_id])?;
            } else {
                // Toggle-only changes retain old snapshots and remain available
                // even after a resource no longer qualifies for new agents.
                let identity_changed=old.is_none_or(|old|old.name!=change.name||old.role!=change.role);
                if (identity_changed||change.enabled)&&!roles(&tx,&resource)?.contains(&change.role){return Err(Error::Unqualified)}
                let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_definitions WHERE name=?1 AND id<>?2)",params![change.name,input.definition_id],|r|r.get(0))?;
                if duplicate || (identity_changed&&use_counts(&tx,&change.name)?.2){return Err(Error::Conflict)}
                if creating {
                    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|Error::LocalAuthority)?.as_millis();
                    let now=u64::try_from(now).map_err(|_|Error::Capacity)?;
                    tx.execute("INSERT INTO agent_definitions(id,resource_id,name,role,enabled,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![input.definition_id,input.resource_id,change.name,change.role,change.enabled,now])?;
                } else {
                    tx.execute("UPDATE agent_definitions SET name=?3,role=?4,enabled=?5 WHERE resource_id=?1 AND id=?2",params![input.resource_id,input.definition_id,change.name,change.role,change.enabled])?;
                }
            }
            let result=ResourceTemplateResult{command_id:input.command_id,resource_id:input.resource_id,definition_id:input.definition_id,deleted,revision:revision(&resource,&definitions(&tx,&resource.id())?)?};
            receipt.record(&tx,&result)?;backing.check(&authority)?;tx.commit()?;Ok(result)
        }).await?;
        self.check(profile).map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
