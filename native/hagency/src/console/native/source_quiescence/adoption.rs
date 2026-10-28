//! Disabled native-only original-ID adoption. No mini-app route or auto-import.
use super::*;
mod task_prepare;
mod journal_handoff;
#[cfg(test)]
mod tests;
use hagency_store::management::PrivateManagementJournal;
use hagency_store::owner_resources::adoption::{AdoptionAuthority, AdoptionSummary, AdoptionTransfer};
use hagency_store::owner_resources::{ProfileAuthority, VerifiedProviderAccount};
use std::sync::atomic::{AtomicBool, Ordering};

/// Retains the original physical sources and one private journal handoff. The
/// host must retain this object and later explicitly drain its archive worker.
/// A receipt alone cannot reconstruct source authority.
pub struct NativeResourceAdoption {
    source: Arc<QuiescedSources>,
    authority: AdoptionAuthority,
    transfer: Arc<AdoptionTransfer>,
    binding: String,
    lane: Mutex<()>,
    custody: Arc<JournalCustody>,
    drain: Mutex<JournalDrain>,
}
pub(super) struct JournalCustody {
    original_lock: AtomicBool,
    journal: StdMutex<Option<PrivateManagementJournal>>,
}
impl JournalCustody {
    fn check(&self) -> bool {
        self.original_lock.load(Ordering::Acquire)
            || self.journal.try_lock().ok().is_some_and(|j| {
                j.as_ref()
                    .is_some_and(PrivateManagementJournal::writer_open)
            })
    }
}
pub(super) enum JournalDrain {
    Idle,
    Running(tokio::task::JoinHandle<Result<(), NativeError>>),
    Finished(Result<(), (u16, String)>),
}
fn store(error: hagency_store::Error) -> NativeError {
    super::super::management::store_error(error)
}
impl QuiescedSources {
    fn adoption_authority(
        self: &Arc<Self>,
    ) -> Result<(AdoptionAuthority, String, Arc<JournalCustody>), NativeError> {
        self.source_authority(false)
    }
    pub(super) fn empty_device_authority(
        self: &Arc<Self>,
    ) -> Result<(AdoptionAuthority, String, Arc<JournalCustody>), NativeError> {
        self.source_authority(true)
    }
    fn source_authority(
        self: &Arc<Self>,
        empty_device: bool,
    ) -> Result<(AdoptionAuthority, String, Arc<JournalCustody>), NativeError> {
        self.check()?;
        if !hagency_platform::legacy_writer::adoption_runtime_qualified_platform() {
            return Err(err("source_adoption_platform_unqualified"));
        }
        let mut claimed = self
            .adoption_custody
            .lock()
            .map_err(|_| err("source_adoption_custody_unknown"))?;
        if claimed.upgrade().is_some() {
            return Err(err("source_adoption_busy"));
        }
        self.files
            .lock()
            .map_err(|_| err("source_quiescence_unknown"))?
            .reacquire()
            .map_err(err)?;
        let identities = self
            .files
            .lock()
            .map_err(|_| err("source_quiescence_unknown"))?
            .identities();
        let binding = if empty_device {
            identities.empty_device_binding(&self.profile)
        } else {
            identities.binding(&self.profile)
        }.map_err(err)?;
        let gate = self.gate.clone();
        let profile = ProfileAuthority::new(
            self.profile.clone(),
            self.owner.0.expected_owner.clone(),
            self.gate.expires,
        )
        .and_then(|p| p.with_native_gate(move || gate.check().is_ok()))
        .map_err(store)?;
        let custody = Arc::new(JournalCustody {
            original_lock: AtomicBool::new(true),
            journal: StdMutex::new(None),
        });
        *claimed = Arc::downgrade(&custody);
        drop(claimed);
        let check = custody.clone();
        let retained = self.clone();
        let authority = AdoptionAuthority::new(
            self.registry.clone(),
            profile,
            self.gate.expires,
            move || {
                retained.gate.check().is_ok()
                    && retained.owner.0.host.check_state_lock().is_ok()
                    && retained.lease.check().is_ok()
                    && (if empty_device { identities.empty_device_identity_check().is_ok() } else { identities.adoption_identity_check().is_ok() })
                    && check.check()
            },
        )
        .map_err(store)?;
        Ok((authority, binding, custody))
    }
    /// Only the complete selected profile is eligible. Missing personal evidence
    /// and any unknown resource, runtime or command history stay blocked.
    pub async fn prepare_resource_adoption(
        self: &Arc<Self>,
        migration_id: String,
        provider: VerifiedProviderAccount,
        monthly_account_tokens: u64,
    ) -> Result<NativeResourceAdoption, NativeError> {
        self.prepare_resource_adoption_inner(migration_id, provider, monthly_account_tokens, None)
            .await
    }
    async fn prepare_resource_adoption_inner(
        self: &Arc<Self>,
        migration_id: String,
        provider: VerifiedProviderAccount,
        monthly_account_tokens: u64,
        expected: Option<&[(String, String)]>,
    ) -> Result<NativeResourceAdoption, NativeError> {
        super::super::key(&migration_id)?;
        if self.owner.0.adoption_recovery.get().is_some() {
            return Err(err("source_adoption_recovery_only"));
        }
        super::super::AdoptedManagementSelection::require_no_task_prepare(&self.owner)?;
        let (authority, binding, custody) = self.adoption_authority()?;
        let source = self.clone();
        let inventory = tokio::task::spawn_blocking(move || {
            let files = source
                .files
                .lock()
                .map_err(|_| "source_quiescence_unknown")?;
            files.inventory(&source.profile, &source.owner.0.expected_owner)
        })
        .await
        .map_err(|_| err("source_quiescence_unknown"))?
        .map_err(err)?;
        if let Some(expected) = expected
            && !inventory.matches_review(expected).map_err(store)?
        {
            return Err(err("source_adoption_review_changed"));
        }
        self.registry
            .prepare_resource_adoption(
                authority.clone(),
                migration_id.clone(),
                provider,
                inventory,
                binding.clone(),
                monthly_account_tokens,
            )
            .await
            .map_err(store)?;
        let adoption = self
            .adoption_with(authority, binding, migration_id, custody)
            .await?;
        adoption.initializer_selection().persist(&self.owner)?;
        Ok(adoption)
    }
    /// Exact saved plan only. Never creates replacement IDs or infers that an
    /// absent/partial canonical record means a previous source mutation failed.
    pub async fn resume_resource_adoption(
        self: &Arc<Self>,
        migration_id: String,
    ) -> Result<NativeResourceAdoption, NativeError> {
        super::super::key(&migration_id)?;
        if self
            .owner
            .0
            .adoption_recovery
            .get()
            .is_some_and(|id| id != &migration_id)
        {
            return Err(err("source_adoption_scope_changed"));
        }
        let (authority, binding, custody) = self.adoption_authority()?;
        self.adoption_with(authority, binding, migration_id, custody)
            .await
    }
    async fn adoption_with(
        self: &Arc<Self>,
        authority: AdoptionAuthority,
        binding: String,
        migration_id: String,
        custody: Arc<JournalCustody>,
    ) -> Result<NativeResourceAdoption, NativeError> {
        let transfer = self
            .registry
            .resource_adoption_transfer(authority.clone(), migration_id)
            .await
            .map_err(store)?;
        if transfer.source_binding() != binding {
            return Err(err("source_adoption_binding_changed"));
        }
        if transfer.includes_operator_tasks() {
            let record = task_prepare::saved(
                &self.owner,
                self.registry.clone(),
                transfer.migration_id(),
                &self.profile,
                &self.device_id,
            )?;
            task_prepare::matches_transfer(&record, &transfer)?;
        }
        self.check()?;
        Ok(NativeResourceAdoption {
            source: self.clone(),
            authority,
            transfer: Arc::new(transfer),
            binding,
            lane: Mutex::new(()),
            custody,
            drain: Mutex::new(JournalDrain::Idle),
        })
    }
}
impl NativeResourceAdoption {
    pub async fn status(&self) -> Result<AdoptionSummary, NativeError> {
        self.source.check()?;
        self.source
            .registry
            .resource_adoption(self.authority.clone(), self.transfer.migration_id().into())
            .await
            .map_err(store)
    }
    /// The feature is disabled by default. This performs no capacity grant,
    /// reservation, project/agent creation or provider inference.
    pub async fn advance(
        &self,
        provider: VerifiedProviderAccount,
    ) -> Result<AdoptionSummary, NativeError> {
        let _lane = self.lane.lock().await;
        if !matches!(*self.drain.lock().await, JournalDrain::Idle) {
            return Err(err("source_adoption_custody_closed"));
        }
        if self.status().await?.phase == "active" {
            return self.status().await;
        }
        // Require the original descriptor saved during preparation. Neither
        // resumed source custody nor a replacement device may reconstruct it.
        // Incomplete plans still fail active initializer checks.
        self.original_selection()?;
        let source = self.source.clone();
        let transfer = self.transfer.clone();
        let binding = self.binding.clone();
        let proof = tokio::task::spawn_blocking(move || {
            let mut files = source
                .files
                .lock()
                .map_err(|_| "source_quiescence_unknown")?;
            files.fence(
                &transfer,
                source.registry.id(),
                &binding,
                &source.gate,
                &source.lease,
            )
        })
        .await
        .map_err(|_| err("source_adoption_outcome_unknown"))?
        .map_err(err)?;
        self.source
            .registry
            .record_adoption_sources(self.authority.clone(), proof)
            .await
            .map_err(store)?;
        let source = self.source.clone();
        let transfer = self.transfer.clone();
        let binding = self.binding.clone();
        let proof = tokio::task::spawn_blocking(move || {
            let mut files = source
                .files
                .lock()
                .map_err(|_| "source_quiescence_unknown")?;
            files.fence(
                &transfer,
                source.registry.id(),
                &binding,
                &source.gate,
                &source.lease,
            )
        })
        .await
        .map_err(|_| err("source_adoption_outcome_unknown"))?
        .map_err(err)?;
        self.source
            .registry
            .import_resource_adoption(self.authority.clone(), proof)
            .await
            .map_err(store)?;
        let journal = self.custody.open_journal(
            &self.source, self.transfer.archive_binding().map_err(store)?,
        ).await?;
        let proof = self
            .transfer
            .verify_private_journal(&journal)
            .await
            .map_err(store)?;
        self.source
            .registry
            .record_adoption_custody(self.authority.clone(), proof)
            .await
            .map_err(store)?;
        let proof = self
            .transfer
            .verify_private_journal(&journal)
            .await
            .map_err(store)?;
        self.source
            .registry
            .activate_resource_adoption(self.authority.clone(), proof, provider)
            .await
            .map_err(store)
    }
    /// A completed migration may select the next native initializer only after
    /// its original archive writer drain has acknowledged. Drop this migration
    /// and explicitly shutdown/drop all retired owners before using the selection
    /// to reacquire the root (shutdown also releases the callback task).
    pub async fn finish_handoff(
        &self,
    ) -> Result<super::super::AdoptedManagementSelection, NativeError> {
        if self.status().await?.phase != "active" {
            return Err(err("source_adoption_not_active"));
        }
        self.drain_journal().await?;
        self.source.check()?;
        self.original_selection()
    }
    fn original_selection(&self) -> Result<super::super::AdoptedManagementSelection, NativeError> {
        super::super::AdoptedManagementSelection::recover_original(
            &self.source.owner,
            self.source.registry.clone(),
            self.transfer.migration_id(),
            &self.source.profile,
        )
    }
    fn initializer_selection(&self) -> super::super::AdoptedManagementSelection {
        super::super::AdoptedManagementSelection::completed(
            &self.source.owner,
            self.source.registry.clone(),
            self.transfer.migration_id().into(),
            self.source.profile.clone(),
            self.source.device_id.clone(),
        )
    }
    pub async fn drain_journal(&self) -> Result<(), NativeError> {
        let _lane = self.lane.lock().await;
        self.custody.drain_journal(&self.source, &self.drain).await
    }
}

impl NativeOwner {
    pub(in crate::console::native) async fn initialize_adopted_management(
        &self,
        registry: &OwnerResourceRegistry,
        migration_id: &str,
    ) -> Result<(), NativeError> {
        let device = self
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| err("source_quiescence_authority_changed"))?;
        verify_source(self, &device).await?;
        let profile = profile_identity(
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .map_err(|_| err("source_quiescence_owner_mismatch"))?;
        let expires = Instant::now() + Duration::from_secs(30);
        let weak = Arc::downgrade(&self.0);
        let authority = AdoptionAuthority::new(
            registry.clone(),
            ProfileAuthority::new(profile.clone(), device.owner_mxid().into(), expires)
                .map_err(store)?,
            expires,
            move || {
                weak.upgrade().is_some_and(|i| {
                    !i.configuration_stopped.load(Ordering::Acquire)
                        && i.host.check_state_lock().is_ok()
                })
            },
        )
        .map_err(store)?;
        if let Some(record) = super::super::AdoptedManagementSelection::saved_empty_prepare_for_active_open(
            self, registry.clone(), migration_id, &profile,
        )? {
            let transfer = Arc::new(registry.empty_device_transfer(authority, migration_id.into(), record.plan_digest.clone()).await.map_err(store)?);
            record.matches_transfer(&transfer)?;
            if transfer.summary().await.map_err(store)?.phase != "active" {
                return Err(err("source_adoption_not_active"));
            }
            let root = self.0.host.state_directory().to_owned();
            let original = transfer.clone();
            let journal = tokio::task::spawn_blocking(move || {
                files::Sources::capture(&root, &profile)?.open_empty_adopted_journal(&original, &profile)
            }).await.map_err(|_| err("source_quiescence_unknown"))?.map_err(err)?;
            let adopted = transfer.adopted_management(journal).await.map_err(store)?;
            self.0.management.install_adopted(adopted).await?;
            verify_source(self, &device).await?;
            self.0.host.check_state_lock().map_err(|_| err("source_root_changed"))?;
            return Ok(());
        }
        if registry
            .resource_adoption(authority.clone(), migration_id.into())
            .await
            .map_err(store)?
            .phase
            != "active"
        {
            return Err(err("source_adoption_not_active"));
        }
        let transfer = Arc::new(
            registry
                .resource_adoption_transfer(authority, migration_id.into())
                .await
                .map_err(store)?,
        );
        let root = self.0.host.state_directory().to_owned();
        let original = transfer.clone();
        let journal = tokio::task::spawn_blocking(move || {
            let sources = files::Sources::capture(&root, &profile)?;
            sources.open_adopted_journal(&original, &profile)
        })
        .await
        .map_err(|_| err("source_quiescence_unknown"))?
        .map_err(err)?;
        let adopted = transfer.adopted_management(journal).await.map_err(store)?;
        self.0.management.install_adopted(adopted).await?;
        verify_source(self, &device).await?;
        self.0
            .host
            .check_state_lock()
            .map_err(|_| err("source_root_changed"))?;
        Ok(())
    }
    pub(in crate::console::native) async fn validate_adoption_recovery(
        &self,
        registry: &OwnerResourceRegistry,
        migration_id: &str,
    ) -> Result<(), NativeError> {
        let device = self
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| err("source_quiescence_authority_changed"))?;
        verify_source(self, &device).await?;
        let profile = profile_identity(
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .map_err(|_| err("source_quiescence_owner_mismatch"))?;
        let expires = Instant::now() + Duration::from_secs(30);
        let weak = Arc::downgrade(&self.0);
        let authority = AdoptionAuthority::new(
            registry.clone(),
            ProfileAuthority::new(profile.clone(), device.owner_mxid().into(), expires)
                .map_err(store)?,
            expires,
            move || {
                weak.upgrade().is_some_and(|i| {
                    !i.configuration_stopped
                        .load(std::sync::atomic::Ordering::Acquire)
                })
            },
        )
        .map_err(store)?;
        if let Some(record) = super::super::AdoptedManagementSelection::saved_empty_prepare(
            self, registry.clone(), migration_id, &profile, device.device_id(),
        )? {
            self.validate_empty_initialization_recovery(registry, authority, migration_id, profile, record).await?;
            verify_source(self, &device).await?;
            self.0.host.check_state_lock().map_err(|_| err("source_root_changed"))?;
            return Ok(());
        }
        let transfer = registry
            .find_resource_adoption_transfer(authority.clone(), migration_id.into())
            .await
            .map_err(store)?;
        let expected_binding = if let Some(transfer) = transfer {
            if transfer.includes_operator_tasks() {
                // An active canonical plan permits same-profile inventory reads
                // on a replacement device. Incomplete migration still requires
                // the original device; this branch grants no resume or grant effect.
                let active = registry
                    .resource_adoption(authority, migration_id.into())
                    .await
                    .map_err(store)?
                    .phase
                    == "active";
                let record = if active {
                    let selected = super::super::AdoptedManagementSelection::recover_original(
                        self,
                        registry.clone(),
                        migration_id,
                        &profile,
                    )?;
                    selected
                        .task_preparation()
                        .cloned()
                        .ok_or_else(|| err("adopted_management_initializer_unavailable"))?
                } else {
                    task_prepare::saved(
                        self,
                        registry.clone(),
                        migration_id,
                        &profile,
                        device.device_id(),
                    )?
                };
                task_prepare::matches_transfer(&record, &transfer)?;
            }
            transfer.source_binding().to_owned()
        } else {
            task_prepare::saved(
                self,
                registry.clone(),
                migration_id,
                &profile,
                device.device_id(),
            )?
            .source_binding
        };
        let root = self.0.host.state_directory().to_owned();
        let binding = tokio::task::spawn_blocking(move || {
            files::Sources::capture(&root, &profile)?.binding(&profile)
        })
        .await
        .map_err(|_| err("source_quiescence_unknown"))?
        .map_err(err)?;
        if binding != expected_binding {
            return Err(err("source_adoption_binding_changed"));
        }
        verify_source(self, &device).await?;
        self.0
            .host
            .check_state_lock()
            .map_err(|_| err("source_root_changed"))?;
        Ok(())
    }
}
