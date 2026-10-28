//! Provider-free first-use setup alongside the original resource migration.
//! Only native widgets hold the sealed review; scripts receive descriptive state.
use super::*;
use crate::console::native::source_quiescence::EmptyDeviceObservation;
use hagency_store::owner_resources::adoption::EmptyDeviceInitializationSummary;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSetupKind {
    ExistingResources,
    EmptyDevice,
}
pub enum DeviceSetupReview {
    Resources(ResourceActivationReview),
    Empty(EmptyDeviceActivationReview),
}
impl DeviceSetupReview {
    pub fn context(&self) -> &ResourceActivationContext {
        match self {
            Self::Resources(r) => r.context(),
            Self::Empty(r) => &r.context,
        }
    }
    pub fn monthly_tokens(&self) -> Option<u64> {
        match self {
            Self::Resources(r) => Some(r.monthly_tokens()),
            Self::Empty(_) => None,
        }
    }
    pub fn expired(&self) -> bool {
        match self {
            Self::Resources(r) => r.expired(),
            Self::Empty(r) => Instant::now() >= r.expires,
        }
    }
    pub fn recovering(&self) -> bool {
        match self {
            Self::Resources(r) => r.recovering(),
            Self::Empty(r) => r.selection.is_some(),
        }
    }
}
pub struct EmptyDeviceActivationReview {
    owner: NativeOwner,
    context: ResourceActivationContext,
    device: String,
    expires: Instant,
    observation: Option<EmptyDeviceObservation>,
    selection: Option<AdoptedManagementSelection>,
}
fn empty_context(
    owner: &NativeOwner,
    id: Option<String>,
    personal: bool,
    records: u32,
) -> ResourceActivationContext {
    ResourceActivationContext {
        kind: DeviceSetupKind::EmptyDevice,
        server: owner.0.expected_origin.clone(),
        account: owner.0.expected_owner.clone(),
        resources: Vec::new(),
        original_migration: id,
        original_monthly_tokens: None,
        task_count: None,
        comment_count: None,
        alert_count: None,
        alert_note_count: None,
        personal_database_present: Some(personal),
        completed_private_records: Some(records),
    }
}
fn require_package(package: &ManagementPackage) -> Result<(), NativeError> {
    if !hagency_platform::legacy_writer::adoption_runtime_qualified_platform() {
        return Err(error("source_adoption_platform_unqualified"));
    }
    if !package.permissions.contains(&Permission::ReadResources)
        || !package
            .permissions
            .contains(&Permission::PrepareSourceQuiescence)
    {
        return Err(error("source_quiescence_permission_required"));
    }
    Ok(())
}
fn profile(device: &crate::console::AuthorizedDevice) -> Result<String, NativeError> {
    crate::console::owned_agents::profile_identity(
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .map_err(|_| error("management_scope_changed"))
}
async fn same_device(
    owner: &NativeOwner,
    device: &crate::console::AuthorizedDevice,
) -> Result<(), NativeError> {
    let after = source(owner).await?;
    if after.device_id() != device.device_id() || after.generation() != device.generation() {
        return Err(error("management_scope_changed"));
    }
    Ok(())
}
impl AdoptedManagementSelection {
    /// Common phase projection for desktop reopen. The old AdoptionSummary
    /// remains a resource contract and never receives a fake empty quota.
    pub async fn device_setup_phase(
        &self,
        owner: &NativeOwner,
    ) -> Result<&'static str, NativeError> {
        if self.empty_preparation().is_some() {
            Ok(self.empty_setup_summary(owner).await?.phase)
        } else {
            Ok(self.activation_summary(owner).await?.phase)
        }
    }
    async fn empty_setup_summary(
        &self,
        owner: &NativeOwner,
    ) -> Result<EmptyDeviceInitializationSummary, NativeError> {
        if !self.matches_host(
            owner.0.host.state_directory(),
            &owner.0.expected_origin,
            &owner.0.expected_owner,
        ) {
            return Err(error("adopted_management_scope_changed"));
        }
        let device = source(owner).await?;
        let current_profile = profile(&device)?;
        if current_profile != self.profile {
            return Err(error("management_scope_changed"));
        }
        let record = self
            .empty_preparation()
            .ok_or_else(|| error("source_adoption_review_changed"))?;
        let expires = Instant::now() + Duration::from_secs(30);
        let weak = Arc::downgrade(&owner.0);
        let authority = AdoptionAuthority::new(
            self.registry.clone(),
            ProfileAuthority::new(current_profile.clone(), device.owner_mxid().into(), expires)
                .map_err(management::store_error)?,
            expires,
            move || {
                weak.upgrade().is_some_and(|o| {
                    !o.configuration_stopped
                        .load(std::sync::atomic::Ordering::Acquire)
                        && o.host.check_state_lock().is_ok()
                })
            },
        )
        .map_err(management::store_error)?;
        let summary = if let Some(transfer) = self
            .registry
            .find_empty_device_transfer(
                authority.clone(),
                self.migration_id.clone(),
                record.plan_digest.clone(),
            )
            .await
            .map_err(management::store_error)?
        {
            record.matches_transfer(&transfer)?;
            transfer.summary().await.map_err(management::store_error)?
        } else {
            self.original_empty_prepare(device.device_id())?;
            owner
                .validate_empty_initialization_recovery(
                    &self.registry,
                    authority,
                    &self.migration_id,
                    current_profile,
                    record.clone(),
                )
                .await?;
            EmptyDeviceInitializationSummary {
                initialization_id: self.migration_id.clone(),
                phase: "prepare_saved",
                personal_database_present: record.personal_database_present,
                completed_private_records: record.completed_private_records,
            }
        };
        if summary.phase != "active" {
            self.original_empty_prepare(device.device_id())?;
        }
        same_device(owner, &device).await?;
        Ok(summary)
    }
}
impl NativeOwner {
    /// A filtered zero-resource page only selects further inspection. Complete
    /// native source inventory must independently establish the empty state.
    pub async fn device_setup_context(
        &self,
        package: ManagementPackage,
        selection: Option<&AdoptedManagementSelection>,
    ) -> Result<ResourceActivationContext, NativeError> {
        require_package(&package)?;
        if let Some(selected) = selection {
            if selected.empty_preparation().is_some() {
                let summary = selected.empty_setup_summary(self).await?;
                return Ok(empty_context(
                    self,
                    Some(summary.initialization_id),
                    summary.personal_database_present,
                    summary.completed_private_records,
                ));
            }
            return self.resource_activation_context(package, selection).await;
        }
        let device = source(self).await?;
        let manager = self.management(package_copy(&package)).await?;
        let page = manager.resources(ResourceQuery::default()).await;
        manager.retire();
        let page = page?;
        if !page.resources.is_empty() || page.next_after.is_some() {
            return self.resource_activation_context(package, None).await;
        }
        let observed = self.observe_empty_device(profile(&device)?).await?;
        same_device(self, &device).await?;
        Ok(empty_context(
            self,
            None,
            observed.personal_present(),
            observed.completed_private_records(),
        ))
    }
    pub async fn review_device_setup(
        &self,
        package: ManagementPackage,
        selection: Option<AdoptedManagementSelection>,
        monthly_tokens: Option<u64>,
    ) -> Result<DeviceSetupReview, NativeError> {
        let context = self
            .device_setup_context(package_copy(&package), selection.as_ref())
            .await?;
        if context.kind == DeviceSetupKind::ExistingResources {
            return self
                .review_resource_activation(
                    package,
                    selection,
                    monthly_tokens.ok_or_else(|| error("invalid_adoption_ceiling"))?,
                )
                .await
                .map(DeviceSetupReview::Resources);
        }
        if monthly_tokens.is_some() {
            return Err(error("empty_device_setup_has_no_quota"));
        }
        let device = source(self).await?;
        let (context, observation) = if let Some(selected) = &selection {
            selected.original_empty_prepare(device.device_id())?;
            let summary = selected.empty_setup_summary(self).await?;
            if summary.phase == "active" {
                return Err(error("empty_device_already_active"));
            }
            (
                empty_context(
                    self,
                    Some(summary.initialization_id),
                    summary.personal_database_present,
                    summary.completed_private_records,
                ),
                None,
            )
        } else {
            let observed = self.observe_empty_device(profile(&device)?).await?;
            (
                empty_context(
                    self,
                    None,
                    observed.personal_present(),
                    observed.completed_private_records(),
                ),
                Some(observed),
            )
        };
        same_device(self, &device).await?;
        Ok(DeviceSetupReview::Empty(EmptyDeviceActivationReview {
            owner: self.clone(),
            context,
            device: device.device_id().into(),
            expires: Instant::now() + Duration::from_secs(120),
            observation,
            selection,
        }))
    }
    pub async fn confirm_device_setup(
        &self,
        review: DeviceSetupReview,
        package: ManagementPackage,
        native_gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<AdoptedManagementSelection, NativeError> {
        match review {
            DeviceSetupReview::Resources(review) => {
                self.confirm_resource_activation(review, package, native_gate)
                    .await
            }
            DeviceSetupReview::Empty(review) => {
                self.confirm_empty_device_setup(review, package, Arc::new(native_gate))
                    .await
            }
        }
    }
    async fn confirm_empty_device_setup(
        &self,
        review: EmptyDeviceActivationReview,
        package: ManagementPackage,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<AdoptedManagementSelection, NativeError> {
        require_package(&package)?;
        if !Arc::ptr_eq(&self.0, &review.owner.0) || Instant::now() >= review.expires || !gate() {
            return Err(error("source_adoption_review_expired"));
        }
        let device = source(self).await?;
        if device.device_id() != review.device {
            return Err(error("source_adoption_review_changed"));
        }
        let registry = match &review.selection {
            Some(selected) => {
                selected.original_empty_prepare(device.device_id())?;
                selected.empty_setup_summary(self).await?;
                selected.registry.clone()
            }
            None => tokio::task::spawn_blocking(|| activation_registry(false))
                .await
                .map_err(|_| error("source_registry_unavailable"))?
                .map_err(management::store_error)?,
        };
        let live = gate.clone();
        let request = SourceQuiescenceRequest::new(
            self,
            package,
            registry,
            Instant::now() + Duration::from_secs(55),
            move || live(),
        )?;
        #[cfg(test)]
        let request = {
            let mut request = request;
            request.lease_root = Some(
                hagency_platform::legacy_writer::test_support::configured_root()
                    .map_err(|_| error("source_installation_busy"))?,
            );
            request
        };
        let work = self.prepare_source_quiescence(request).await?;
        let fresh = if let Some(observed) = &review.observation {
            let current = work.review_empty_device().await?;
            current.matches_observation(observed)?;
            Some(current)
        } else {
            None
        };
        if !gate() || Instant::now() >= review.expires {
            return Err(error("source_adoption_review_expired"));
        }
        let sources = work.close_and_drain().await?;
        let initialization = if let Some(current) = &fresh {
            let mut random = [0u8; 24];
            getrandom::fill(&mut random).map_err(|_| error("native_random_unavailable"))?;
            let id = random.iter().map(|b| format!("{b:02x}")).collect();
            sources
                .prepare_empty_device_initialization(id, current)
                .await?
        } else {
            sources
                .resume_empty_device_initialization(
                    review
                        .context
                        .original_migration
                        .ok_or_else(|| error("source_adoption_review_changed"))?,
                )
                .await?
        };
        let advanced = initialization.advance().await;
        if let Err(error) = advanced {
            // Explicitly close a possibly opened archive worker before returning
            // to the retained desktop task/recovery path. Never reset its plan.
            initialization.drain_journal().await?;
            return Err(error);
        }
        let selected = initialization.finish_handoff().await?;
        drop(initialization);
        drop(sources);
        drop(work);
        Ok(selected)
    }
}

#[cfg(test)]
mod tests;
