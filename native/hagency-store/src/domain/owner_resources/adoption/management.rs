//! Canonical definitions and original private custody are separate authorities.
use super::*;
mod resource_templates;
mod resource_publication;
mod web_catalog;
mod resource_name;
mod resource_edit;
mod resource_ceiling;
mod resource_create;
mod alerts;
mod approval_bindings;
mod tool_approvals;
mod execution_policy;
mod workspaces;
mod stopped;
mod accounts;
mod operator_tasks;
use crate::management::{
    ArchiveJournalBinding, JournalCommand, PROTOCOL, PendingIntent, PrivateManagementJournal,
    ResourceDetail, ResourcePage, ResourceQuery, ResourceSummary,
};

/// Produced only from an active original adoption plus its actual archive
/// worker. No path, SQL, profile or access claim can be deserialized into it.
#[derive(Clone)]
pub struct AdoptedManagementStore {
    registry: OwnerResourceRegistry,
    profile: String,
    owner: String,
    journal: PrivateManagementJournal,
    binding: ArchiveJournalBinding,
    task_history: Option<operator_tasks::TaskBinding>,
}
impl AdoptedManagementStore {
    pub(crate) async fn record_contribution_attempt(
        &self,
        profile: &ProfileAuthority,
        resource_id: String,
        expected_revision: String,
        command_id: String,
    ) -> Result<(), Error> {
        self.check(profile)?;
        let inherited = profile.clone();
        let backing = self.clone();
        let authority = profile.clone().with_native_gate(move || backing.check(&inherited).is_ok())?;
        self.registry.record_contribution_attempt(authority, resource_id, expected_revision, command_id).await?;
        self.check(profile)
    }
    pub(crate) fn enrollment_registry(&self,profile:&ProfileAuthority)->Result<OwnerResourceRegistry,Error>{
        self.check(profile)?;
        Ok(self.registry.clone())
    }
    #[cfg(feature = "shared-intake")]
    pub fn native_owner_cleanup_registry(&self, profile: &ProfileAuthority) -> Result<OwnerResourceRegistry, Error> {
        self.check(profile)?;
        Ok(self.registry.clone())
    }
    pub fn matches_registry(&self, registry: &OwnerResourceRegistry) -> bool {
        Arc::ptr_eq(&self.registry.0, &registry.0)
    }
    pub fn matches_profile(&self, profile: &str, account: &str) -> bool {
        self.profile == profile && self.owner == account
    }
    pub(crate) fn identity(&self) -> (&str, &str) {
        (&self.profile, &self.owner)
    }
    pub fn is_live(&self) -> bool {
        !self.registry.0.closed.load(Ordering::Acquire) && self.journal.writer_open()
    }
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.journal.shutdown().await
    }
    fn check(&self, profile: &ProfileAuthority) -> Result<(), Error> {
        profile.check()?;
        if !self.is_live() || !self.matches_profile(&profile.digest, &profile.account) {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    pub(crate) async fn journal(
        &self,
        command: JournalCommand,
    ) -> Result<Vec<PendingIntent>, Error> {
        if !self.is_live() {
            return Err(Error::LocalAuthority);
        }
        self.journal.adopted_command(&self.binding, command).await
    }
    pub(crate) async fn resource(
        &self,
        profile: &ProfileAuthority,
        id: String,
    ) -> Result<ResourceSummary, Error> {
        self.check(profile)?;
        let authority = profile.clone();
        let backing = self.clone();
        let result = self.registry.0.store.call(4096, move |db| {
            backing.check(&authority)?;
            let tx = db.db.unchecked_transaction()?;
            authorize_resource(&tx, &authority, &id)?;
            let mut value = db.management_resource(&id, graphs::now_ms()?)?;
            value.local_name = Some(backing.name_for(&tx, &id)?);
            crate::domain::web_catalog::filter_capabilities(&tx, &authority.digest, &mut value.capabilities)?;
            backing.check(&authority)?;
            tx.commit()?;
            Ok(value)
        }).await?;
        self.check(profile)?;
        Ok(result)
    }
    pub(crate) async fn verify_connection(
        &self,
        profile: &ProfileAuthority,
        id: String,
        revision: String,
        provider: VerifiedProviderAccount,
    ) -> Result<ResourceSummary, Error> {
        self.check(profile)?;
        let binding = self
            .registry
            .verify_provider_resource(profile.clone(), id.clone(), provider)
            .await?;
        if binding.resource_revision() != revision {
            return Err(Error::Conflict);
        }
        let expires = binding.expires;
        let authority = profile.clone();
        let backing = self.clone();
        let result = self
            .registry
            .0
            .store
            .call(4096, move |db| {
                backing.check(&authority)?;
                binding.check_canonical(&db.db, &backing.registry.0.id)?;
                let result = db.management_resource(&id, graphs::now_ms()?)?;
                if result.revision != revision {
                    return Err(Error::Conflict);
                }
                binding.check()?;
                backing.check(&authority)?;
                Ok(result)
            })
            .await?;
        self.check(profile)?;
        if Instant::now() >= expires {
            return Err(Error::LocalAuthority);
        }
        Ok(result)
    }
    pub(crate) async fn detail(
        &self,
        profile: &ProfileAuthority,
        id: String,
    ) -> Result<ResourceDetail, Error> {
        self.check(profile)?;
        let authority = profile.clone();
        let backing = self.clone();
        let result = self
            .registry
            .0
            .store
            .call(4096, move |db| {
                backing.check(&authority)?;
                let tx = db.db.unchecked_transaction()?;
                authorize_resource(&tx, &authority, &id)?;
                let mut result = db.management_resource_detail_in_snapshot(&id, graphs::now_ms()?)?;
                result.resource.local_name = Some(backing.name_for(&tx, &id)?);
                crate::domain::web_catalog::filter_capabilities(&tx, &authority.digest, &mut result.resource.capabilities)?;
                result.editing = resource_edit::editing(&tx, backing.registry.id(), &id)?;
                result.ceiling_amendment = resource_ceiling::amendment_state(
                    &tx,
                    backing.registry.id(),
                    &read_resource(&tx, &id)?,
                    result.observed_at_ms,
                )?;
                if serde_json::to_vec(&result)?.len() > 64 * 1024 {
                    return Err(Error::Capacity);
                }
                backing.check(&authority)?;
                tx.commit()?;
                Ok(result)
            })
            .await?;
        self.check(profile)?;
        Ok(result)
    }
    pub(crate) async fn resources(
        &self,
        profile: &ProfileAuthority,
        query: ResourceQuery,
    ) -> Result<ResourcePage, Error> {
        self.check(profile)?;
        query.validate()?;
        let authority = profile.clone();
        let backing = self.clone();
        let result=self.registry.0.store.call(4096,move|db|{
            backing.check(&authority)?;
            let tx=db.db.unchecked_transaction()?;
            // Query only authorized IDs. A page cursor cannot enumerate another
            // profile's definitions, model labels or resource cardinality.
            let ids=tx.prepare("SELECT r.id FROM resources r JOIN owner_resource_grants g ON g.resource_id=r.id WHERE g.profile=?1 AND g.active=1 AND r.id>?2 ORDER BY r.id LIMIT 101")?
                .query_map(params![authority.digest,query.after],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            let has_more=ids.len()>100;
            let at=graphs::now_ms()?;let mut resources=Vec::new();let mut next_after=None;
            for id in ids.into_iter().take(100){
                authorize_resource(&tx,&authority,&id)?;
                let mut value=db.management_resource(&id,at)?;
                value.local_name=Some(backing.name_for(&tx,&id)?);
                crate::domain::web_catalog::filter_capabilities(&tx,&authority.digest,&mut value.capabilities)?;
                let matches=query.framework.as_ref().is_none_or(|v|v==&value.framework)
                    && query.capability.as_ref().is_none_or(|v|value.capabilities.contains(v))
                    && query.published.is_none_or(|v|v==value.published);
                if matches {
                    if resources.len()==query.limit{break;}
                    resources.push(value);
                }
                next_after=Some(id);
            }
            if resources.len()<query.limit && !has_more{next_after=None;}
            let result=ResourcePage{protocol:PROTOCOL,observed_at_ms:at,resources,next_after};
            if serde_json::to_vec(&result)?.len()>64*1024{return Err(Error::Capacity);}
            backing.check(&authority)?;tx.commit()?;Ok(result)
        }).await?;
        self.check(profile)?;
        Ok(result)
    }
}
impl AdoptionTransfer {
    /// History can legitimately have new pending commands or acknowledged old
    /// replies after activation. Do not compare its mutable custody snapshot to
    /// the pre-import digest again; require the exact permanent archive binding.
    pub async fn adopted_management(
        &self,
        journal: PrivateManagementJournal,
    ) -> Result<AdoptedManagementStore, Error> {
        self.authority.check()?;
        let binding = self.archive_binding()?;
        journal.require_binding(&binding)?;
        let authority = self.authority.clone();
        let original = self.plan.clone();
        let task_history = self.authority
            .registry
            .0
            .store
            .call(4096, move |db| {
                authority.check()?;
                let (plan, phase) = writer::read(&db.db, &authority, &original.migration_id)?;
                if phase != 4 || plan.digest()? != original.digest()? {
                    return Err(Error::LocalAuthority);
                }
                authority.check()?;
                db.initialize_empty_operator_history(&authority, &original)
            })
            .await?;
        let after = |error| if task_history.is_some() { Error::OutcomeUnknown } else { error };
        self.authority.check().map_err(after)?;
        journal.require_binding(&binding).map_err(after)?;
        Ok(AdoptedManagementStore {
            registry: self.authority.registry.clone(),
            profile: self.plan.inventory.profile.clone(),
            owner: self.plan.inventory.owner.clone(),
            journal,
            binding,
            task_history,
        })
    }
}

impl AdoptedManagementStore {
    pub(crate) async fn resource_namespace(
        &self, profile: &ProfileAuthority, id: String, revision: String,
    ) -> Result<Option<ResourceNamespace>, Error> {
        self.check(profile)?;
        let result=self.registry.resource_namespace(profile.clone(),id,revision).await?;
        self.check(profile)?;
        Ok(result)
    }
}

impl AdoptedManagementStore {
    pub(super) async fn from_empty_device(
        authority: &AdoptionAuthority,
        original: &empty_initialization::EmptyPlan,
        journal: PrivateManagementJournal,
        binding: ArchiveJournalBinding,
    ) -> Result<Self, Error> {
        authority.check()?;
        journal.require_binding(&binding)?;
        let check = authority.clone();
        let plan = original.clone();
        let task_history = authority.registry.0.store.call(4096, move |db| {
            db.initialize_empty_device_operator_history(&check, &plan)
        }).await?;
        authority.check().map_err(|_| Error::OutcomeUnknown)?;
        journal.require_binding(&binding).map_err(|_| Error::OutcomeUnknown)?;
        Ok(Self {
            registry: authority.registry.clone(), profile: original.inventory.profile.clone(),
            owner: original.inventory.owner.clone(), journal, binding, task_history: Some(task_history),
        })
    }
}

mod web_connections;
