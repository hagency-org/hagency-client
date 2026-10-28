//! V2 preparation closes the descriptor/commit crash gap without changing v1.
use super::*;
use crate::console::native::AdoptedManagementSelection;
use crate::console::native::adoption_handoff::{TaskAdoptionPrepare, TaskPrepareReview};
use hagency_store::owner_resources::adoption::OperatorTaskTransferPreview;
use hagency_store::owner_resources::adoption::PreparedResourceAdoption;

pub(super) fn saved(
    owner: &NativeOwner,
    registry: OwnerResourceRegistry,
    migration: &str,
    profile: &str,
    device: &str,
) -> Result<TaskAdoptionPrepare, NativeError> {
    let selection =
        AdoptedManagementSelection::recover_original(owner, registry, migration, profile)?;
    Ok(selection.original_task_prepare(device)?.clone())
}
fn matches(
    record: &TaskAdoptionPrepare,
    prepared: &PreparedResourceAdoption,
) -> Result<(), NativeError> {
    if prepared.plan_digest().map_err(store)? != record.plan_digest
        || prepared.inventory_digest() != record.inventory_digest
    {
        return Err(err("source_adoption_review_changed"));
    }
    Ok(())
}
pub(super) fn matches_transfer(
    record: &TaskAdoptionPrepare,
    transfer: &AdoptionTransfer,
) -> Result<(), NativeError> {
    if !transfer.includes_operator_tasks()
        || transfer.plan_digest().map_err(store)? != record.plan_digest
        || transfer.inventory_digest() != record.inventory_digest
        || transfer.source_binding() != record.source_binding
    {
        return Err(err("source_adoption_review_changed"));
    }
    Ok(())
}
pub(super) struct StagedTaskAdoption {
    pub source: Arc<QuiescedSources>,
    pub authority: AdoptionAuthority,
    pub binding: String,
    pub custody: Arc<JournalCustody>,
    pub migration_id: String,
    pub prepared: PreparedResourceAdoption,
}
impl StagedTaskAdoption {
    async fn commit(self) -> Result<NativeResourceAdoption, NativeError> {
        self.source
            .registry
            .commit_resource_adoption_preparation(self.prepared)
            .await
            .map_err(store)?;
        self.source
            .adoption_with(
                self.authority,
                self.binding,
                self.migration_id,
                self.custody,
            )
            .await
    }
}
impl QuiescedSources {
    /// Explicit native v2 preparation. The original initializer/selection is
    /// persisted before a canonical write can be issued. No caller JSON proof.
    pub async fn prepare_resource_adoption_with_operator_tasks(
        self: &Arc<Self>,
        migration_id: String,
        provider: VerifiedProviderAccount,
        monthly_account_tokens: u64,
    ) -> Result<NativeResourceAdoption, NativeError> {
        if self.owner.0.adoption_recovery.get().is_some() {
            return Err(err("source_adoption_recovery_only"));
        }
        self.stage_task_adoption(migration_id, provider, monthly_account_tokens, None)
            .await?
            .commit()
            .await
    }
    /// Recover only the saved original v2 preparation. Existing canonical
    /// staging wins; absent staging allows exact pre-fence-only recapture under
    /// the same original native device/profile/source custody and provider proof.
    pub async fn resume_resource_adoption_with_operator_tasks(
        self: &Arc<Self>,
        migration_id: String,
        provider: VerifiedProviderAccount,
    ) -> Result<NativeResourceAdoption, NativeError> {
        super::super::super::key(&migration_id)?;
        self.check()?;
        if self
            .owner
            .0
            .adoption_recovery
            .get()
            .is_some_and(|id| id != &migration_id)
        {
            return Err(err("source_adoption_scope_changed"));
        }
        let record = saved(
            &self.owner,
            self.registry.clone(),
            &migration_id,
            &self.profile,
            &self.device_id,
        )?;
        let (authority, binding, custody) = self.adoption_authority()?;
        if binding != record.source_binding {
            return Err(err("source_adoption_binding_changed"));
        }
        let found = self
            .registry
            .find_resource_adoption_transfer(authority.clone(), migration_id.clone())
            .await
            .map_err(store)?;
        self.check()?;
        if let Some(transfer) = found {
            matches_transfer(&record, &transfer)?;
            return self
                .adoption_with(authority, binding, migration_id, custody)
                .await;
        }
        // Release this temporary custody before reacquiring it for recapture.
        drop(authority);
        drop(custody);
        self.stage_task_adoption(
            migration_id,
            provider,
            record.monthly_account_tokens,
            Some(record),
        )
        .await?
        .commit()
        .await
    }
    pub(super) async fn stage_task_adoption(
        self: &Arc<Self>,
        migration_id: String,
        provider: VerifiedProviderAccount,
        monthly_account_tokens: u64,
        expected: Option<TaskAdoptionPrepare>,
    ) -> Result<StagedTaskAdoption, NativeError> {
        super::super::super::key(&migration_id)?;
        self.stage_task_adoption_inner(
            migration_id,
            provider,
            monthly_account_tokens,
            expected,
            None,
        )
        .await
    }
    pub(in crate::console::native) async fn prepare_reviewed_task_adoption(
        self: &Arc<Self>,
        migration_id: String,
        provider: VerifiedProviderAccount,
        monthly_account_tokens: u64,
        revisions: &[(String, String)],
        preview: &OperatorTaskTransferPreview,
    ) -> Result<NativeResourceAdoption, NativeError> {
        if self.owner.0.adoption_recovery.get().is_some() {
            return Err(err("source_adoption_recovery_only"));
        }
        self.stage_task_adoption_inner(
            migration_id,
            provider,
            monthly_account_tokens,
            None,
            Some((revisions, preview)),
        )
        .await?
        .commit()
        .await
    }
    async fn stage_task_adoption_inner(
        self: &Arc<Self>,
        migration_id: String,
        provider: VerifiedProviderAccount,
        monthly_account_tokens: u64,
        expected: Option<TaskAdoptionPrepare>,
        reviewed: Option<(&[(String, String)], &OperatorTaskTransferPreview)>,
    ) -> Result<StagedTaskAdoption, NativeError> {
        super::super::super::key(&migration_id)?;
        let (authority, binding, custody) = self.adoption_authority()?;
        // Only the explicit fresh review or original saved review selects v3.
        // Existing v2 preparations never gain a new transfer on restart.
        let alerts = reviewed.is_some_and(|(_, p)| p.alert_count().is_some())
            || expected
                .as_ref()
                .and_then(|e| e.review.as_ref())
                .is_some_and(|r| r.alerts.is_some());
        let source = self.clone();
        let inventory = tokio::task::spawn_blocking(move || {
            source
                .check()
                .map_err(|_| "source_quiescence_authority_changed")?;
            let files = source
                .files
                .lock()
                .map_err(|_| "source_quiescence_unknown")?;
            if alerts {
                files.inventory_with_operator_history(
                    &source.profile,
                    &source.owner.0.expected_owner,
                )
            } else {
                files.inventory_with_operator_tasks(&source.profile, &source.owner.0.expected_owner)
            }
        })
        .await
        .map_err(|_| err("source_quiescence_unknown"))?
        .map_err(err)?;
        self.check()?;
        if let Some((revisions, preview)) = reviewed
            && (!inventory.matches_review(revisions).map_err(store)?
                || &inventory.operator_task_preview().map_err(store)? != preview)
        {
            return Err(err("source_adoption_review_changed"));
        }
        let prepared = self
            .registry
            .resource_adoption_preparation(
                authority.clone(),
                migration_id.clone(),
                provider,
                inventory,
                binding.clone(),
                monthly_account_tokens,
            )
            .map_err(store)?;
        let mut record = TaskAdoptionPrepare {
            source_binding: binding.clone(),
            plan_digest: prepared.plan_digest().map_err(store)?,
            inventory_digest: prepared.inventory_digest().into(),
            monthly_account_tokens,
            review: Some(TaskPrepareReview::from_summary(
                &prepared.review_summary().map_err(store)?,
            )?),
        };
        if let Some(expected) = expected {
            matches(&expected, &prepared)?;
            if expected.review.is_none() {
                record.review = None;
            }
            if expected != record {
                return Err(err("source_adoption_review_changed"));
            }
        }
        self.registry
            .validate_resource_adoption_preparation(&prepared)
            .await
            .map_err(store)?;
        self.check()?;
        let selection = AdoptedManagementSelection::prepared_tasks(
            &self.owner,
            self.registry.clone(),
            migration_id.clone(),
            self.profile.clone(),
            self.device_id.clone(),
            record,
        )?;
        self.check()?;
        selection.persist(&self.owner)?;
        self.check()?;
        Ok(StagedTaskAdoption {
            source: self.clone(),
            authority,
            binding,
            custody,
            migration_id,
            prepared,
        })
    }
}

impl NativeOwner {
    /// Only a previously saved v2 record can request this bounded descriptive
    /// read. Fresh SDK/device and exact retained source identity are required on
    /// both sides; no source worker is closed and no initializer is written.
    pub(in crate::console::native) async fn saved_task_activation_preview(
        &self,
        profile: &str,
        device: &crate::console::AuthorizedDevice,
        record: &TaskAdoptionPrepare,
    ) -> Result<TaskPrepareReview, NativeError> {
        super::super::verify_source(self, device).await?;
        self.0
            .host
            .check_state_lock()
            .map_err(|_| err("source_root_changed"))?;
        let root = self.0.host.state_directory().to_owned();
        let profile = profile.to_owned();
        let owner = self.0.expected_owner.clone();
        let record = record.clone();
        let value = tokio::task::spawn_blocking(move || {
            let sources = super::super::files::Sources::capture(&root, &profile).map_err(err)?;
            sources
                .saved_task_preview(
                    &profile,
                    &owner,
                    &record.source_binding,
                    &record.inventory_digest,
                    record.review.as_ref().is_some_and(|r| r.alerts.is_some()),
                )
                .map_err(err)
        })
        .await
        .map_err(|_| err("source_adoption_preview_unavailable"))??;
        self.0
            .host
            .check_state_lock()
            .map_err(|_| err("source_root_changed"))?;
        super::super::verify_source(self, device).await?;
        Ok(value)
    }
}
