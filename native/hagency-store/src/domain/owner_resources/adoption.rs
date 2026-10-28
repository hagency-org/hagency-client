//! Disabled-by-default definition adoption. This journal never opens/fences a
//! source and cannot treat a serialized inventory as native authority.
mod archives;
mod custody;
mod inventory;
mod empty_device;
pub use empty_device::EmptyDeviceInventory;
mod empty_initialization;
pub(crate) use empty_initialization::EmptyDeviceCustodyRequest;
pub use empty_initialization::VerifiedEmptyDeviceJournal;
pub use empty_initialization::{EmptyDeviceInitializationSummary, PreparedEmptyDeviceInitialization, EmptyDeviceTransfer, VerifiedEmptyDeviceArchives};
mod journal;
#[cfg(feature = "resource-adoption")]
mod management;
#[cfg(feature = "resource-adoption")]
pub use management::AdoptedManagementStore;
mod schema;
mod task_transfer;
mod alert_transfer;
#[cfg(all(test, feature = "resource-adoption"))]
mod tests;
mod writer;
use super::*;
pub use archives::{AdoptionTransfer, VerifiedAdoptionArchives};
use hagency_core::project::identifier;
pub use inventory::AdoptionInventory;
pub use task_transfer::OperatorTaskTransferPreview;
pub(crate) use journal::CustodyRequest;
pub use journal::VerifiedAdoptionJournal;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Inventory {
    source_schema: i32,
    profile: String,
    owner: String,
    resources: Vec<Resource>,
    seat: Seat,
    custody_digest: String,
    custody_rows: u32,
    personal_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_history: Option<task_transfer::Summary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alert_history: Option<alert_transfer::Summary>,
}

/// Display-only, stable resource selection. No provider subject, source path,
/// private command payload or proof can be round-tripped from a mini-app.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionSummary {
    pub migration_id: String,
    pub phase: &'static str,
    pub resource_ids: Vec<String>,
    pub resource_revisions: Vec<String>,
    pub monthly_account_tokens: u64,
    pub introduces_shared_account_ceiling: bool,
    pub completed_private_records: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transferred_task_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transferred_comment_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transferred_alert_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transferred_alert_note_count: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Plan {
    version: u16,
    migration_id: String,
    registry_id: String,
    provider_digest: String,
    runtime: String,
    source_binding: String,
    inventory_digest: String,
    inventory: Inventory,
    target_seat: Seat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alert_history: Option<alert_transfer::History>,
}
impl Plan {
    fn digest(&self) -> Result<String, Error> {
        Ok(canonical::digest(&serde_json::to_value(self)?)?)
    }
    fn summary(&self, phase: i64) -> Result<AdoptionSummary, Error> {
        Ok(AdoptionSummary {
            migration_id: self.migration_id.clone(),
            phase: phase_name(phase)?,
            resource_ids: self.inventory.resources.iter().map(Resource::id).collect(),
            resource_revisions: self
                .inventory
                .resources
                .iter()
                .map(crate::resource_publication_revision)
                .collect::<Result<_, _>>()?,
            monthly_account_tokens: self
                .target_seat
                .declaration
                .as_ref()
                .and_then(|d| d.quota_tokens)
                .ok_or(Error::NoCeiling)?
                .into(),
            introduces_shared_account_ceiling: self.inventory.seat.declaration.is_none(),
            completed_private_records: self.inventory.custody_rows,
            transferred_task_count: self.inventory.task_history.as_ref().map(|h| h.tasks),
            transferred_comment_count: self.inventory.task_history.as_ref().map(|h| h.comments),
            transferred_alert_count: self.inventory.alert_history.as_ref().map(|h| h.alerts),
            transferred_alert_note_count: self.inventory.alert_history.as_ref().map(|h| h.notes),
        })
    }
}
fn phase_name(phase: i64) -> Result<&'static str, Error> {
    match phase {
        0 => Ok("prepared"),
        1 => Ok("sources_fenced"),
        2 => Ok("imported_inactive"),
        3 => Ok("custody_ready"),
        4 => Ok("active"),
        _ => Err(Error::Schema),
    }
}

/// Trusted host confirmation, independent of the retired source's Session.
/// The gate must retain the exact installation/source locks and SDK epoch.
#[derive(Clone)]
pub struct AdoptionAuthority {
    registry: OwnerResourceRegistry,
    profile: ProfileAuthority,
    expires: Instant,
    native: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl AdoptionAuthority {
    pub fn new(
        registry: OwnerResourceRegistry,
        profile: ProfileAuthority,
        expires: Instant,
        native: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        if expires > Instant::now() + Duration::from_secs(60) {
            return Err(Error::LocalAuthority);
        }
        let value = Self {
            registry,
            profile,
            expires,
            native: Arc::new(native),
        };
        value.check()?;
        Ok(value)
    }
    fn check(&self) -> Result<(), Error> {
        self.profile.check()?;
        if Instant::now() >= self.expires
            || self.registry.0.closed.load(Ordering::Acquire)
            || !(self.native)()
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}

/// Native-only preparation; no Deserialize or raw payload constructor. It retains
/// the original finite authority, provider proof and complete captured history.
pub struct PreparedResourceAdoption {
    authority: AdoptionAuthority,
    provider: VerifiedProviderAccount,
    plan: Plan,
    history: Option<task_transfer::History>,
}
impl PreparedResourceAdoption {
    pub fn review_summary(&self) -> Result<AdoptionSummary, Error> { self.plan.summary(0) }
    pub fn plan_digest(&self) -> Result<String, Error> {
        self.plan.digest()
    }
    pub fn inventory_digest(&self) -> &str {
        &self.plan.inventory_digest
    }
}

impl OwnerResourceRegistry {
    /// Original v1 API and ordering remain available to existing native callers.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_resource_adoption(
        &self,
        authority: AdoptionAuthority,
        migration_id: String,
        provider: VerifiedProviderAccount,
        inventory: AdoptionInventory,
        source_binding: String,
        monthly_account_tokens: u64,
    ) -> Result<AdoptionSummary, Error> {
        let prepared = self.resource_adoption_preparation(
            authority,
            migration_id,
            provider,
            inventory,
            source_binding,
            monthly_account_tokens,
        )?;
        self.commit_resource_adoption_preparation(prepared).await
    }
    /// Builds an opaque exact preparation without any database write. The native
    /// host can durably bind its original initializer before committing this plan.
    #[allow(clippy::too_many_arguments)]
    pub fn resource_adoption_preparation(
        &self,
        authority: AdoptionAuthority,
        migration_id: String,
        provider: VerifiedProviderAccount,
        inventory: AdoptionInventory,
        source_binding: String,
        monthly_account_tokens: u64,
    ) -> Result<PreparedResourceAdoption, Error> {
        authority.check()?;
        if !Arc::ptr_eq(&self.0, &authority.registry.0) || Instant::now() >= provider.expires {
            return Err(Error::LocalAuthority);
        }
        identifier(&migration_id, 128)?;
        hash(&source_binding)?;
        let AdoptionInventory(inventory, history, alerts) = inventory;
        if inventory.profile != authority.profile.digest
            || inventory.owner != authority.profile.account
            || inventory
                .resources
                .iter()
                .any(|r| r.framework != provider.runtime)
        {
            return Err(Error::LocalAuthority);
        }
        let tokens: hagency_core::allocation::Tokens = monthly_account_tokens.try_into()?;
        if monthly_account_tokens == 0 {
            return Err(Error::NoCeiling);
        }
        let target_seat: Seat = serde_json::from_value(
            json!({"id":inventory.seat.id,"declaration":{"quotaTokens":tokens,"period":"monthly"}}),
        )?;
        if inventory.seat.declaration.is_some()
            && serde_json::to_value(&inventory.seat)? != serde_json::to_value(&target_seat)?
        {
            return Err(Error::Conflict);
        }
        let plan = Plan {
            version: if alerts.is_some() { 3 } else if history.is_some() { 2 } else { 1 },
            migration_id,
            registry_id: self.id().into(),
            provider_digest: provider.digest.clone(),
            runtime: provider.runtime.clone(),
            source_binding,
            inventory_digest: canonical::digest(&serde_json::to_value(&inventory)?)?,
            inventory,
            target_seat,
            alert_history: alerts,
        };
        let encoded = serde_json::to_vec(&plan)?;
        if encoded.len() > 256 * 1024 {
            return Err(Error::Capacity);
        }
        Ok(PreparedResourceAdoption {
            authority,
            provider,
            plan,
            history,
        })
    }
    /// Read-only current-history check before native original-intent persistence.
    /// The commit still rechecks under its actual serialized writer transaction.
    pub async fn validate_resource_adoption_preparation(
        &self,
        prepared: &PreparedResourceAdoption,
    ) -> Result<(), Error> {
        if !Arc::ptr_eq(&self.0, &prepared.authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        prepared.authority.check()?;
        let authority = prepared.authority.clone();
        let plan = prepared.plan.clone();
        let expires = prepared.provider.expires;
        self.0
            .store
            .call(262144, move |db| {
                authority.check()?;
                if Instant::now() >= expires {
                    return Err(Error::LocalAuthority);
                }
                let tx = db.db.unchecked_transaction()?;
                writer::prepare_preflight(&tx, &authority, &plan)?;
                authority.check()?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        prepared.authority.check()?;
        if Instant::now() >= expires {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    /// Saves only the exact previously built plan/staging. No source mutation.
    pub async fn commit_resource_adoption_preparation(
        &self,
        prepared: PreparedResourceAdoption,
    ) -> Result<AdoptionSummary, Error> {
        let PreparedResourceAdoption {
            authority,
            provider,
            plan,
            history,
        } = prepared;
        authority.check()?;
        if !Arc::ptr_eq(&self.0, &authority.registry.0) || plan.registry_id != self.id() {
            return Err(Error::LocalAuthority);
        }
        let encoded = serde_json::to_vec(&plan)?;
        let job_bytes = encoded.len()
            + plan
                .inventory
                .task_history
                .as_ref()
                .map_or(0, |h| h.bytes as usize);
        let after = authority.clone();
        let result = self
            .0
            .store
            .call(job_bytes as u32, move |db| {
                authority.check()?;
                if Instant::now() >= provider.expires {
                    return Err(Error::LocalAuthority);
                }
                db.prepare_adoption(&authority, &plan, history.as_ref())
            })
            .await?;
        after.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
    /// Recovery reports the original phase; it grants no source execution.
    pub async fn resource_adoption(
        &self,
        authority: AdoptionAuthority,
        migration_id: String,
    ) -> Result<AdoptionSummary, Error> {
        authority.check()?;
        if !Arc::ptr_eq(&self.0, &authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        identifier(&migration_id, 128)?;
        let after = authority.clone();
        let result = self
            .0
            .store
            .call(4096, move |db| {
                authority.check()?;
                let (plan, phase) = writer::read(&db.db, &authority, &migration_id)?;
                if matches!(plan.version, 2 | 3) {
                    task_transfer::verify(&db.db, &plan)?;
                }
                authority.check()?;
                plan.summary(phase)
            })
            .await?;
        after.check()?;
        Ok(result)
    }
}
