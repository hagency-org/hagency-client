//! Persist the original native choice before its canonical prepared record.
//! Optional-source fencing, original journal custody and active handoff.
use super::*;
use crate::console::native::AdoptedManagementSelection;
use crate::console::native::adoption_handoff::EmptyDevicePrepare;
use hagency_store::owner_resources::adoption::{
    AdoptionAuthority, EmptyDeviceInitializationSummary, PreparedEmptyDeviceInitialization,
    EmptyDeviceTransfer,
};

fn store(error: hagency_store::Error) -> NativeError {
    crate::console::native::management::store_error(error)
}

impl NativeOwner {
    /// Source-only restart validation before any writer is opened. A missing
    /// canonical prepared row is eligible only if the exact original native
    /// descriptor can be reconstructed from the unchanged unfenced sources.
    pub(in crate::console::native) async fn validate_empty_initialization_recovery(
        &self,
        registry: &OwnerResourceRegistry,
        authority: AdoptionAuthority,
        initialization: &str,
        profile: String,
        record: EmptyDevicePrepare,
    ) -> Result<(), NativeError> {
        if let Some(transfer) = registry
            .find_empty_device_transfer(
                authority.clone(),
                initialization.into(),
                record.plan_digest.clone(),
            )
            .await
            .map_err(store)?
        {
            record.matches_transfer(&transfer)?;
            let active = transfer.summary().await.map_err(store)?.phase == "active";
            let root = self.0.host.state_directory().to_owned();
            return tokio::task::spawn_blocking(move || {
                let sources = files::Sources::capture(&root, &profile)?;
                if active {
                    sources.inspect_active_empty_transfer(&transfer, &profile)
                } else {
                    sources.inspect_empty_transfer(&transfer, &profile)
                }
            })
            .await
            .map_err(|_| err("source_quiescence_unknown"))?
            .map_err(err);
        }
        let root = self.0.host.state_directory().to_owned();
        let owner = self.0.expected_owner.clone();
        let (binding, inventory) = tokio::task::spawn_blocking(move || {
            files::Sources::capture(&root, &profile)?.empty_device_inventory(&profile, &owner)
        })
        .await
        .map_err(|_| err("source_quiescence_unknown"))?
        .map_err(err)?;
        let prepared = registry
            .empty_device_preparation(authority, initialization.into(), inventory, binding.clone())
            .map_err(store)?;
        if EmptyDevicePrepare::from_prepared(binding, &prepared)? != record {
            return Err(err("empty_device_review_changed"));
        }
        registry
            .validate_empty_device_preparation(&prepared)
            .await
            .map_err(store)
    }
}

/// Holds the same original native source and mutually exclusive handoff slot.
/// A prepared result does not mean provider authorization or active management.
pub struct NativeEmptyDeviceInitialization {
    source: Arc<QuiescedSources>,
    custody: Arc<adoption::JournalCustody>,
    drain: Mutex<adoption::JournalDrain>,
    transfer: Arc<EmptyDeviceTransfer>,
    lane: Mutex<()>,
}
impl NativeEmptyDeviceInitialization {
    /// Activate the original empty installation without a provider call or
    /// quota creation. Every retry continues the same persisted initialization.
    pub async fn advance(&self) -> Result<EmptyDeviceInitializationSummary, NativeError> {
        let _lane = self.lane.lock().await;
        if !matches!(*self.drain.lock().await, adoption::JournalDrain::Idle) {
            return Err(err("source_adoption_custody_closed"));
        }
        let summary = self.summary().await?;
        if summary.phase == "active" {
            return Ok(summary);
        }
        self.original_selection()?;
        if matches!(summary.phase, "prepared" | "sources_fenced") {
            self.fence_sources_inner().await?;
        }
        self.source
            .registry
            .prepare_empty_device_registry(&self.transfer)
            .await
            .map_err(store)?;
        let journal = self
            .custody
            .open_journal(
                &self.source,
                self.transfer.archive_binding().map_err(store)?,
            )
            .await?;
        self.source
            .registry
            .record_empty_device_custody(
                self.transfer
                    .verify_private_journal(&journal)
                    .await
                    .map_err(store)?,
            )
            .await
            .map_err(store)?;
        let result = self
            .source
            .registry
            .activate_empty_device(
                self.transfer
                    .verify_private_journal(&journal)
                    .await
                    .map_err(store)?,
            )
            .await
            .map_err(store)?;
        self.source.check()?;
        Ok(result)
    }
    fn original_selection(&self) -> Result<AdoptedManagementSelection, NativeError> {
        let selection = AdoptedManagementSelection::recover_original(
            &self.source.owner,
            self.source.registry.clone(),
            self.transfer.initialization_id(),
            &self.source.profile,
        )?;
        selection
            .original_empty_prepare(&self.source.device_id)?
            .matches_transfer(&self.transfer)?;
        Ok(selection)
    }
    pub async fn finish_handoff(&self) -> Result<AdoptedManagementSelection, NativeError> {
        if self.summary().await?.phase != "active" {
            return Err(err("source_adoption_not_active"));
        }
        self.drain_journal().await?;
        self.source.check()?;
        self.original_selection()
    }
    pub async fn drain_journal(&self) -> Result<(), NativeError> {
        let _lane = self.lane.lock().await;
        self.custody.drain_journal(&self.source, &self.drain).await
    }
    pub async fn summary(&self) -> Result<EmptyDeviceInitializationSummary, NativeError> {
        self.source.check()?;
        let result = self.transfer.summary().await.map_err(store)?;
        self.source.check()?;
        Ok(result)
    }
    /// Sync original backups, publish permanent legacy refusal, fence only
    /// sources actually present, then record verified source evidence.
    pub async fn fence_sources(&self) -> Result<EmptyDeviceInitializationSummary, NativeError> {
        let _lane = self.lane.lock().await;
        if !matches!(*self.drain.lock().await, adoption::JournalDrain::Idle) {
            return Err(err("source_adoption_custody_closed"));
        }
        self.fence_sources_inner().await
    }
    async fn fence_sources_inner(&self) -> Result<EmptyDeviceInitializationSummary, NativeError> {
        self.source.check()?;
        let source = self.source.clone();
        let transfer = self.transfer.clone();
        let proof = tokio::task::spawn_blocking(move || {
            source.check()?;
            let proof = source
                .files
                .lock()
                .map_err(|_| err("source_quiescence_unknown"))?
                .fence_empty_device(
                    &transfer,
                    source.registry.id(),
                    &source.profile,
                    &source.gate,
                    &source.lease,
                )
                .map_err(err)?;
            source.check()?;
            Ok::<_, NativeError>(proof)
        })
        .await
        .map_err(|_| err("source_quiescence_unknown"))??;
        let result = self
            .source
            .registry
            .record_empty_device_sources(proof)
            .await
            .map_err(store)?;
        self.source.check()?;
        Ok(result)
    }
}

struct Staged {
    source: Arc<QuiescedSources>,
    custody: Arc<adoption::JournalCustody>,
    authority: AdoptionAuthority,
    prepared: PreparedEmptyDeviceInitialization,
}
impl Staged {
    async fn commit(self) -> Result<NativeEmptyDeviceInitialization, NativeError> {
        self.source.check()?;
        let digest = self.prepared.plan_digest().map_err(store)?;
        let id = self.prepared.review_summary().initialization_id;
        self.source
            .registry
            .commit_empty_device_preparation(self.prepared)
            .await
            .map_err(store)?;
        let transfer = self
            .source
            .registry
            .empty_device_transfer(self.authority, id, digest)
            .await
            .map_err(store)?;
        self.source.check()?;
        Ok(NativeEmptyDeviceInitialization {
            source: self.source,
            custody: self.custody,
            drain: Mutex::new(adoption::JournalDrain::Idle),
            transfer: Arc::new(transfer),
            lane: Mutex::new(()),
        })
    }
}

impl QuiescedSources {
    /// The native review must be the same one shown before draining the original
    /// writer. Source identities and complete inventory are recaptured here.
    pub async fn prepare_empty_device_initialization(
        self: &Arc<Self>,
        initialization_id: String,
        review: &EmptyDeviceReview,
    ) -> Result<NativeEmptyDeviceInitialization, NativeError> {
        crate::console::native::key(&initialization_id)?;
        if self.owner.0.adoption_recovery.get().is_some() {
            return Err(err("source_adoption_recovery_only"));
        }
        AdoptedManagementSelection::require_no_task_prepare(&self.owner)?;
        let current = self.validate_empty_device_review(review).await?;
        self.stage_empty_initialization(initialization_id, current, None)
            .await?
            .commit()
            .await
    }

    /// Recover only the exact saved native selection with a freshly authorized
    /// source gate. If the canonical commit never occurred, its original plan is
    /// recaptured and matched before commit; a replacement setup is not created.
    pub async fn resume_empty_device_initialization(
        self: &Arc<Self>,
        initialization_id: String,
    ) -> Result<NativeEmptyDeviceInitialization, NativeError> {
        crate::console::native::key(&initialization_id)?;
        self.check()?;
        if self
            .owner
            .0
            .adoption_recovery
            .get()
            .is_some_and(|id| id != &initialization_id)
        {
            return Err(err("source_adoption_scope_changed"));
        }
        let selection = AdoptedManagementSelection::recover_original(
            &self.owner,
            self.registry.clone(),
            &initialization_id,
            &self.profile,
        )?;
        let record = selection.original_empty_prepare(&self.device_id)?.clone();
        let (authority, binding, custody) = self.empty_device_authority()?;
        if binding != record.source_binding {
            return Err(err("source_adoption_binding_changed"));
        }
        if let Some(transfer) = self
            .registry
            .find_empty_device_transfer(
                authority.clone(),
                initialization_id.clone(),
                record.plan_digest.clone(),
            )
            .await
            .map_err(store)?
        {
            record.matches_transfer(&transfer)?;
            let active = transfer.summary().await.map_err(store)?.phase == "active";
            let transfer = Arc::new(transfer);
            let original = transfer.clone();
            let source = self.clone();
            tokio::task::spawn_blocking(move || {
                source.check()?;
                let sources = source
                    .files
                    .lock()
                    .map_err(|_| err("source_quiescence_unknown"))?
                    .identities();
                if active {
                    sources.inspect_active_empty_transfer(&original, &source.profile)
                } else {
                    sources.inspect_empty_transfer(&original, &source.profile)
                }
                .map_err(err)?;
                source.check()
            })
            .await
            .map_err(|_| err("source_quiescence_unknown"))??;
            return Ok(NativeEmptyDeviceInitialization {
                source: self.clone(),
                custody,
                drain: Mutex::new(adoption::JournalDrain::Idle),
                transfer,
                lane: Mutex::new(()),
            });
        }
        drop(authority);
        drop(custody);
        let current = self.empty_device_inventory().await?;
        self.stage_empty_initialization(initialization_id, current, Some(record))
            .await?
            .commit()
            .await
    }

    async fn stage_empty_initialization(
        self: &Arc<Self>,
        initialization_id: String,
        current: EmptyDeviceSource,
        expected: Option<EmptyDevicePrepare>,
    ) -> Result<Staged, NativeError> {
        self.check()?;
        if !Arc::ptr_eq(self, &current.source) {
            return Err(err("empty_device_review_scope_changed"));
        }
        let (authority, binding, custody) = self.empty_device_authority()?;
        if binding != current.binding {
            return Err(err("source_adoption_binding_changed"));
        }
        let prepared = self
            .registry
            .empty_device_preparation(
                authority.clone(),
                initialization_id.clone(),
                current.inventory,
                binding.clone(),
            )
            .map_err(store)?;
        let record = EmptyDevicePrepare::from_prepared(binding, &prepared)?;
        if expected.as_ref().is_some_and(|old| old != &record) {
            return Err(err("empty_device_review_changed"));
        }
        self.registry
            .validate_empty_device_preparation(&prepared)
            .await
            .map_err(store)?;
        self.check()?;
        let selection = AdoptedManagementSelection::prepared_empty_device(
            &self.owner,
            self.registry.clone(),
            initialization_id,
            self.profile.clone(),
            self.device_id.clone(),
            record,
        )?;
        // Atomic create-once publication, including directory sync, precedes
        // the first canonical write. Existing v1/v2 descriptors are preserved.
        selection.persist(&self.owner)?;
        self.check()?;
        Ok(Staged {
            source: self.clone(),
            custody,
            authority,
            prepared,
        })
    }
}

#[cfg(test)]
mod tests;
