//! Native confirmation for original-resource adoption. No serializable consent,
//! source path, provider identity, or caller-selected migration identifier.
use super::*;
mod empty_setup;
pub use empty_setup::{DeviceSetupKind, DeviceSetupReview, EmptyDeviceActivationReview};
use hagency_store::{
    management::Permission,
    owner_resources::{
        OwnerResourceRegistry, ProfileAuthority, VerifiedProviderAccount,
        adoption::{AdoptionAuthority, AdoptionSummary, OperatorTaskTransferPreview},
    },
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceActivationResource {
    pub id: String,
    pub model: String,
    pub framework: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceActivationContext {
    pub kind: DeviceSetupKind,
    pub personal_database_present: Option<bool>,
    pub completed_private_records: Option<u32>,
    pub server: String,
    pub account: String,
    pub resources: Vec<ResourceActivationResource>,
    pub original_migration: Option<String>,
    pub original_monthly_tokens: Option<u64>,
    /// None is an original v1 scope; it never claims a populated task transfer.
    pub task_count: Option<u32>,
    pub comment_count: Option<u32>,
    /// None retains the original v1/v2 scope; Some(0) explicitly reviews empty history.
    pub alert_count: Option<u32>,
    pub alert_note_count: Option<u32>,
}
/// Only native widgets receive this handle. It binds the exact displayed source
/// revisions and ceiling, the authenticated owner, and its original device.
#[derive(Clone, PartialEq, Eq)]
enum ActivationHistory {
    Fresh(OperatorTaskTransferPreview),
    SavedV1,
    SavedV2,
}
pub struct ResourceActivationReview {
    owner: NativeOwner,
    context: ResourceActivationContext,
    revisions: Vec<(String, String)>,
    tokens: u64,
    expires: Instant,
    selection: Option<AdoptedManagementSelection>,
    device: String,
    history: ActivationHistory,
}
impl ResourceActivationReview {
    pub fn context(&self) -> &ResourceActivationContext {
        &self.context
    }
    pub fn monthly_tokens(&self) -> u64 {
        self.tokens
    }
    pub fn expired(&self) -> bool {
        Instant::now() >= self.expires
    }
    pub fn recovering(&self) -> bool {
        self.selection.is_some()
    }
}
#[cfg(any(test, feature = "test-support"))]
static FIXTURE_INSTALLATION: StdMutex<Option<std::path::PathBuf>> = StdMutex::new(None);
/// Explicit test binary startup only. Never accepts environment configuration.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn initialize_resource_activation_fixture(root: &Path) -> Result<(), NativeError> {
    OwnerResourceRegistry::ensure_fixture_startup()
        .map_err(|_| error("fixture_authority_already_open"))?;
    let mut saved = FIXTURE_INSTALLATION
        .lock()
        .map_err(|_| error("fixture_root_unavailable"))?;
    hagency_store::private::directory(root).map_err(|_| error("fixture_root_unavailable"))?;
    let root = root
        .canonicalize()
        .map_err(|_| error("fixture_root_unavailable"))?;
    if saved.as_ref().is_some_and(|old| old != &root) {
        return Err(error("fixture_root_changed"));
    }
    let authority = root.join("authority");
    hagency_store::private::directory(&authority).map_err(|_| error("fixture_root_unavailable"))?;
    hagency_platform::legacy_writer::test_support::initialize(&authority)
        .map_err(|_| error("fixture_authority_already_open"))?;
    *saved = Some(root);
    Ok(())
}
pub(super) fn activation_registry(
    existing: bool,
) -> Result<OwnerResourceRegistry, hagency_store::Error> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(root) = FIXTURE_INSTALLATION
        .lock()
        .map_err(|_| hagency_store::Error::Unavailable)?
        .clone()
    {
        let root = root.join("canonical");
        if existing {
            hagency_store::private::open(&root.join("domain.sqlite3"), false)?;
        }
        return OwnerResourceRegistry::isolated_for_tests(&root);
    }
    if existing {
        OwnerResourceRegistry::open_existing()
    } else {
        OwnerResourceRegistry::open()
    }
}
fn error(code: &str) -> NativeError {
    failure(409, code)
}
fn package_copy(p: &ManagementPackage) -> ManagementPackage {
    ManagementPackage {
        digest: p.digest.clone(),
        instance: p.instance,
        account_epoch: p.account_epoch,
        permissions: p.permissions.clone(),
    }
}
async fn source(owner: &NativeOwner) -> Result<crate::console::AuthorizedDevice, NativeError> {
    // Recovery renews SDK/device authentication only; normal management stays denied.
    owner.sync_matrix_source().await?;
    let cookie = owner.0.cookie.lock().await.clone();
    let raw = cookie
        .strip_prefix("hagency_console=")
        .ok_or_else(|| error("owner_authorization_required"))?;
    owner
        .0
        .host
        .console
        .0
        .server_login
        .check_matrix_source(raw)
        .await
        .map_err(|_| error("management_scope_changed"))?;
    let device = owner
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| error("owner_authorization_required"))?;
    if device.bearer().is_err()
        || device.origin() != owner.0.expected_origin
        || device.owner_mxid() != owner.0.expected_owner
        || owner
            .0
            .configuration_stopped
            .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err(error("management_scope_changed"));
    }
    owner
        .0
        .host
        .check_state_lock()
        .map_err(|_| error("source_root_changed"))?;
    Ok(device)
}
impl AdoptedManagementSelection {
    pub async fn activation_summary(
        &self,
        owner: &NativeOwner,
    ) -> Result<AdoptionSummary, NativeError> {
        if !self.matches_host(
            owner.0.host.state_directory(),
            &owner.0.expected_origin,
            &owner.0.expected_owner,
        ) {
            return Err(error("adopted_management_scope_changed"));
        }
        let device = source(owner).await?;
        let profile = crate::console::owned_agents::profile_identity(
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .map_err(|_| error("management_scope_changed"))?;
        if profile != self.profile {
            return Err(error("management_scope_changed"));
        }
        let expires = Instant::now() + Duration::from_secs(30);
        let weak = Arc::downgrade(&owner.0);
        let authority = AdoptionAuthority::new(
            self.registry.clone(),
            ProfileAuthority::new(profile, device.owner_mxid().into(), expires)
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
        let summary = if let Some(record) = self.task_preparation() {
            let transfer = self
                .registry
                .find_resource_adoption_transfer(authority.clone(), self.migration_id.clone())
                .await
                .map_err(management::store_error)?;
            if let Some(transfer) = transfer {
                if !transfer.includes_operator_tasks()
                    || transfer.plan_digest().map_err(management::store_error)?
                        != record.plan_digest
                    || transfer.inventory_digest() != record.inventory_digest
                    || transfer.source_binding() != record.source_binding
                {
                    return Err(error("source_adoption_review_changed"));
                }
                let summary = self
                    .registry
                    .resource_adoption(authority, self.migration_id.clone())
                    .await
                    .map_err(management::store_error)?;
                if let Some(old) = &record.review
                    && &super::adoption_handoff::TaskPrepareReview::from_summary(&summary)? != old
                {
                    return Err(error("source_adoption_review_changed"));
                }
                summary
            } else {
                self.original_task_prepare(device.device_id())?;
                let preview = match &record.review {
                    Some(preview) => preview.clone(),
                    None => {
                        owner
                            .saved_task_activation_preview(&self.profile, &device, record)
                            .await?
                    }
                };
                AdoptionSummary {
                    migration_id: self.migration_id.clone(),
                    phase: "prepare_saved",
                    resource_ids: preview.resources.iter().map(|r| r.0.clone()).collect(),
                    resource_revisions: preview.resources.iter().map(|r| r.1.clone()).collect(),
                    monthly_account_tokens: record.monthly_account_tokens,
                    introduces_shared_account_ceiling: false,
                    completed_private_records: 0,
                    transferred_task_count: Some(preview.tasks),
                    transferred_comment_count: Some(preview.comments),
                    transferred_alert_count: preview.alerts,
                    transferred_alert_note_count: preview.alert_notes,
                }
            }
        } else {
            self.registry
                .resource_adoption(authority, self.migration_id.clone())
                .await
                .map_err(management::store_error)?
        };
        let after = source(owner).await?;
        if after.device_id() != device.device_id() || after.generation() != device.generation() {
            return Err(error("management_scope_changed"));
        }
        Ok(summary)
    }
    /// Restart can expose only recovery for an incomplete immutable plan. Normal
    /// management remains denied until the canonical phase is actually active.
    pub async fn open_for_onboarding_with_matrix_guarded(
        &self,
        state: &Path,
        origin: &str,
        account: &str,
        source: Arc<dyn MatrixTokenSource>,
        guard: impl FnOnce(&NativeOwner) -> Result<(), NativeError> + Send,
    ) -> Result<NativeOwner, NativeError> {
        if !self.matches_host(state, origin, account) {
            return Err(error("adopted_management_scope_changed"));
        }
        let owner = NativeOwner::open_for_resource_adoption_with_matrix_guarded(
            state,
            origin,
            account,
            source,
            self.registry.clone(),
            self.migration_id.clone(),
            guard,
        )
        .await?;
        let result = async {
            if self.device_setup_phase(&owner).await? == "active" {
                owner
                    .initialize_adopted_management(&self.registry, &self.migration_id)
                    .await?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            let _ = owner.suspend().await;
            return Err(e);
        }
        Ok(owner)
    }
}
impl NativeOwner {
    async fn activation_snapshot(
        &self,
        package: &ManagementPackage,
        selection: Option<&AdoptedManagementSelection>,
    ) -> Result<
        (
            ResourceActivationContext,
            Vec<(String, String)>,
            String,
            ActivationHistory,
        ),
        NativeError,
    > {
        self.activation_snapshot_on_platform(
            package,
            selection,
            hagency_platform::legacy_writer::adoption_runtime_qualified_platform(),
        )
        .await
    }
    async fn activation_snapshot_on_platform(
        &self,
        package: &ManagementPackage,
        selection: Option<&AdoptedManagementSelection>,
        qualified: bool,
    ) -> Result<
        (
            ResourceActivationContext,
            Vec<(String, String)>,
            String,
            ActivationHistory,
        ),
        NativeError,
    > {
        if !qualified {
            return Err(error("source_adoption_platform_unqualified"));
        }
        if !package
            .permissions
            .contains(&Permission::PrepareSourceQuiescence)
            || !package.permissions.contains(&Permission::ReadResources)
        {
            return Err(error("source_quiescence_permission_required"));
        }
        let device = source(self).await?;
        let (
            resources,
            revisions,
            original_migration,
            original_monthly_tokens,
            task_count,
            comment_count,
            alert_count,
            alert_note_count,
            history,
        ) = if let Some(selected) = selection {
            let summary = selected.activation_summary(self).await?;
            let rows = summary
                .resource_ids
                .iter()
                .map(|id| ResourceActivationResource {
                    id: id.clone(),
                    model: String::new(),
                    framework: "codex".into(),
                })
                .collect();
            let revisions = summary
                .resource_ids
                .into_iter()
                .zip(summary.resource_revisions)
                .collect();
            (
                rows,
                revisions,
                Some(summary.migration_id),
                Some(summary.monthly_account_tokens),
                summary.transferred_task_count,
                summary.transferred_comment_count,
                summary.transferred_alert_count,
                summary.transferred_alert_note_count,
                if selected.task_preparation().is_some() {
                    ActivationHistory::SavedV2
                } else {
                    ActivationHistory::SavedV1
                },
            )
        } else {
            let manager = self.management(package_copy(package)).await?;
            let result = async {
                let page = manager.resources(ResourceQuery::default()).await?;
                let (context, session) = manager.authorized().await?;
                let preview = session
                    .resource_adoption_history_preview(&context)
                    .await
                    .map_err(management::store_error)?;
                manager.authorized().await?;
                Ok::<_, NativeError>((page, preview))
            }
            .await;
            manager.retire();
            let (page, preview) = result?;
            if page.next_after.is_some()
                || page.resources.is_empty()
                || page.resources.len() > 16
                || page.resources.iter().any(|r| r.framework != "codex")
            {
                return Err(error("source_adoption_inventory_unsupported"));
            }
            let ceiling = page.resources[0].budget.seat.quota.map(u64::from);
            if page
                .resources
                .iter()
                .any(|r| r.budget.seat.quota.map(u64::from) != ceiling)
            {
                return Err(error("source_adoption_inventory_unsupported"));
            }
            let revisions = page
                .resources
                .iter()
                .map(|r| (r.id.clone(), r.revision.clone()))
                .collect();
            let rows = page
                .resources
                .into_iter()
                .map(|r| ResourceActivationResource {
                    id: r.id,
                    model: r.model,
                    framework: r.framework,
                })
                .collect();
            (
                rows,
                revisions,
                None,
                ceiling,
                Some(preview.task_count()),
                Some(preview.comment_count()),
                preview.alert_count(),
                preview.alert_note_count(),
                ActivationHistory::Fresh(preview),
            )
        };
        let after = source(self).await?;
        if after.device_id() != device.device_id() || after.generation() != device.generation() {
            return Err(error("management_scope_changed"));
        }
        Ok((
            ResourceActivationContext {
                kind: DeviceSetupKind::ExistingResources,
                personal_database_present: None,
                completed_private_records: None,
                server: self.0.expected_origin.clone(),
                account: self.0.expected_owner.clone(),
                resources,
                original_migration,
                original_monthly_tokens,
                task_count,
                comment_count,
                alert_count,
                alert_note_count,
            },
            revisions,
            device.device_id().into(),
            history,
        ))
    }
    pub async fn resource_activation_context(
        &self,
        package: ManagementPackage,
        selection: Option<&AdoptedManagementSelection>,
    ) -> Result<ResourceActivationContext, NativeError> {
        self.activation_snapshot(&package, selection)
            .await
            .map(|v| v.0)
    }
    pub async fn review_resource_activation(
        &self,
        package: ManagementPackage,
        selection: Option<AdoptedManagementSelection>,
        monthly_tokens: u64,
    ) -> Result<ResourceActivationReview, NativeError> {
        if monthly_tokens == 0 || monthly_tokens > 9_007_199_254_740_991 {
            return Err(error("invalid_adoption_ceiling"));
        }
        let (context, revisions, device, history) = self
            .activation_snapshot(&package, selection.as_ref())
            .await?;
        if context
            .original_monthly_tokens
            .is_some_and(|n| n != monthly_tokens)
        {
            return Err(error("source_adoption_original_ceiling_required"));
        }
        if selection.as_ref().is_some_and(|s| s.device != device) {
            return Err(error("adopted_grant_device_reauthorization_required"));
        }
        Ok(ResourceActivationReview {
            owner: self.clone(),
            context,
            revisions,
            tokens: monthly_tokens,
            expires: Instant::now() + Duration::from_secs(120),
            selection,
            device,
            history,
        })
    }
    /// Called only after native confirmation. The outer host retains this task
    /// across screen closure/cancellation; every effect still uses finite gates.
    pub async fn confirm_resource_activation(
        &self,
        review: ResourceActivationReview,
        package: ManagementPackage,
        native_gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<AdoptedManagementSelection, NativeError> {
        if !Arc::ptr_eq(&self.0, &review.owner.0) || review.expired() || !native_gate() {
            return Err(error("source_adoption_review_expired"));
        }
        source(self).await?;
        let cookie = self.0.cookie.lock().await.clone();
        let raw_cookie = cookie
            .strip_prefix("hagency_console=")
            .ok_or_else(|| failure(401, "owner_authorization_required"))?;
        let (provider, deadline) =
            crate::console::owner_provider::identity::verify(&self.0.host.console, raw_cookie)
                .await
                .map_err(|e| failure(e.status, e.code))?;
        if Instant::now() >= deadline {
            return Err(error("provider_identity_unavailable"));
        }
        self.confirm_resource_activation_with(review, package, Arc::new(native_gate), provider)
            .await
    }
    #[cfg(test)]
    pub(in crate::console::native) async fn confirm_resource_activation_fixture(
        &self,
        review: ResourceActivationReview,
        package: ManagementPackage,
        provider: VerifiedProviderAccount,
    ) -> Result<AdoptedManagementSelection, NativeError> {
        self.confirm_resource_activation_with(review, package, Arc::new(|| true), provider)
            .await
    }
    #[cfg(test)]
    pub(in crate::console::native) async fn confirm_resource_activation_fixture_with_gate(
        &self,
        review: ResourceActivationReview,
        package: ManagementPackage,
        provider: VerifiedProviderAccount,
        gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<AdoptedManagementSelection, NativeError> {
        self.confirm_resource_activation_with(review, package, Arc::new(gate), provider)
            .await
    }
    async fn confirm_resource_activation_with(
        &self,
        review: ResourceActivationReview,
        package: ManagementPackage,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
        provider: VerifiedProviderAccount,
    ) -> Result<AdoptedManagementSelection, NativeError> {
        if !Arc::ptr_eq(&self.0, &review.owner.0) || review.expired() || !gate() {
            return Err(error("source_adoption_review_expired"));
        }
        let (_, revisions, device, history) = self
            .activation_snapshot(&package, review.selection.as_ref())
            .await?;
        if revisions != review.revisions || device != review.device || history != review.history {
            return Err(error("source_adoption_review_changed"));
        }
        let registry = match &review.selection {
            Some(s) => s.registry.clone(),
            None => tokio::task::spawn_blocking(|| activation_registry(false))
                .await
                .map_err(|_| error("source_registry_unavailable"))?
                .map_err(management::store_error)?,
        };
        let gate_copy = gate.clone();
        let request = SourceQuiescenceRequest::new(
            self,
            package,
            registry,
            Instant::now() + Duration::from_secs(55),
            move || gate_copy(),
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
        let sources = work.close_and_drain().await?;
        let transfer = if let Some(original) = &review.context.original_migration {
            match review.history {
                ActivationHistory::SavedV1 => {
                    sources.resume_resource_adoption(original.clone()).await?
                }
                ActivationHistory::SavedV2 => {
                    sources
                        .resume_resource_adoption_with_operator_tasks(
                            original.clone(),
                            provider.clone(),
                        )
                        .await?
                }
                ActivationHistory::Fresh(_) => return Err(error("source_adoption_review_changed")),
            }
        } else {
            let mut random = [0u8; 24];
            getrandom::fill(&mut random).map_err(|_| error("native_random_unavailable"))?;
            let id = random.iter().map(|b| format!("{b:02x}")).collect();
            let ActivationHistory::Fresh(preview) = &review.history else {
                return Err(error("source_adoption_review_changed"));
            };
            sources
                .prepare_reviewed_task_adoption(
                    id,
                    provider.clone(),
                    review.tokens,
                    &review.revisions,
                    preview,
                )
                .await?
        };
        let saved = transfer.status().await?;
        if saved
            .resource_ids
            .iter()
            .cloned()
            .zip(saved.resource_revisions.iter().cloned())
            .collect::<Vec<_>>()
            != review.revisions
            || saved.monthly_account_tokens != review.tokens
        {
            return Err(error("source_adoption_review_changed"));
        }
        transfer.advance(provider).await?;
        let selection = transfer.finish_handoff().await?;
        drop(transfer);
        drop(sources);
        drop(work);
        Ok(selection)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl NativeOwner {
    /// Synthetic test fixture bootstrap only. The authenticated state must be
    /// inside the explicit installation fixture root. Never overwrites, opens,
    /// recovers or changes an existing personal database.
    #[doc(hidden)]
    pub async fn seed_empty_personal_activation_fixture(&self) -> Result<bool, NativeError> {
        let root = FIXTURE_INSTALLATION
            .lock()
            .map_err(|_| error("fixture_root_unavailable"))?
            .clone()
            .ok_or_else(|| error("fixture_root_unavailable"))?;
        let state = self
            .0
            .host
            .state_directory()
            .canonicalize()
            .map_err(|_| error("fixture_root_unavailable"))?;
        if !state.starts_with(&root) || self.0.adoption_recovery.get().is_some() {
            return Err(error("fixture_profile_outside_root"));
        }
        let device = source(self).await?;
        let origin = device.origin().to_owned();
        let issuer = device.issuer().to_owned();
        let subject = device.subject().to_owned();
        let account = device.owner_mxid().to_owned();
        let result = tokio::task::spawn_blocking(move || {
            let directory = crate::console::owned_agents::journal_profile_directory(
                &state, &origin, &issuer, &subject, &account,
            )
            .map_err(|_| error("fixture_profile_unavailable"))?;
            let path = directory.join("agent-local.sqlite");
            if std::fs::symlink_metadata(&path).is_ok() {
                return Ok(false);
            }
            let profile = crate::console::owned_agents::profile_identity(
                &origin, &issuer, &subject, &account,
            )
            .map_err(|_| error("fixture_profile_unavailable"))?;
            // Create-new is required: a concurrent legitimate initializer may
            // have published a database since the absence check above.
            hagency_store::private::write_new(&path, b"")
                .map_err(|_| error("fixture_personal_create_failed"))?;
            drop(
                hagency_agent_local::Ledger::open_scoped(&path, &account, &profile)
                    .map_err(|_| error("fixture_personal_create_failed"))?,
            );
            Ok(true)
        })
        .await
        .map_err(|_| error("fixture_personal_create_failed"))??;
        source(self).await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::super::reconciliation_sources::tests::Fixture;
    use super::*;
    pub(super) fn package() -> ManagementPackage {
        ManagementPackage {
            digest: "a".repeat(64),
            instance: 1,
            account_epoch: 1,
            permissions: vec![
                Permission::ReadResources,
                Permission::ConfigureResources,
                Permission::PrepareSourceQuiescence,
            ],
        }
    }
    pub(super) fn provider() -> VerifiedProviderAccount {
        VerifiedProviderAccount::new(
            "codex",
            "fixture",
            "account",
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn unsupported_platform_status_refuses_before_source_auth_or_shutdown() {
        let f = Fixture::new().await;
        let (db, _, _) = f.domain();
        drop(db);
        assert_eq!(
            f.owner
                .activation_snapshot_on_platform(&package(), None, false)
                .await
                .err()
                .unwrap()
                .code,
            "source_adoption_platform_unqualified"
        );
        assert!(
            !f.owner
                .0
                .configuration_stopped
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(
            !f.owner
                .0
                .host
                .state_directory()
                .join("adopted-management-initializer-v1.json")
                .exists()
        );
        let manager = f.owner.management(package()).await.unwrap();
        assert!(
            !manager
                .resources(ResourceQuery::default())
                .await
                .unwrap()
                .resources
                .is_empty()
        );
        manager.retire();
        f.stop().await;
    }
    #[tokio::test]
    async fn expired_or_cancelled_native_review_cannot_retire_source_or_observe_provider() {
        let f = Fixture::new().await;
        let (mut db, _, r) = f.domain();
        db.put_seat(&hagency_core::project::Seat {
            id: r.seat_id.clone(),
            declaration: None,
        })
        .unwrap();
        drop(db);
        let mut review = f
            .owner
            .review_resource_activation(package(), None, 700)
            .await
            .unwrap();
        review.expires = Instant::now();
        assert_eq!(
            f.owner
                .confirm_resource_activation(review, package(), || true)
                .await
                .err()
                .unwrap()
                .code,
            "source_adoption_review_expired"
        );
        let review = f
            .owner
            .review_resource_activation(package(), None, 700)
            .await
            .unwrap();
        assert_eq!(
            f.owner
                .confirm_resource_activation(review, package(), || false)
                .await
                .err()
                .unwrap()
                .code,
            "source_adoption_review_expired"
        );
        assert!(
            !f.owner
                .0
                .configuration_stopped
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(
            !f.owner
                .0
                .host
                .state_directory()
                .join("adopted-management-initializer-v1.json")
                .exists()
        );
        f.stop().await;
    }
    #[tokio::test]
    async fn changed_real_resource_revision_refuses_before_drain_or_plan_creation() {
        let f = Fixture::new().await;
        let (mut db, _, r) = f.domain();
        db.put_seat(&hagency_core::project::Seat {
            id: r.seat_id.clone(),
            declaration: None,
        })
        .unwrap();
        drop(db);
        let review = f
            .owner
            .review_resource_activation(package(), None, 700)
            .await
            .unwrap();
        let manager = f.owner.management(package()).await.unwrap();
        let (context, session) = manager.authorized().await.unwrap();
        session
            .configure_resource(
                &context,
                hagency_store::management::ConfigureResource {
                    command_id: "changed_after_review".into(),
                    resource_id: r.id(),
                    expected_revision: hagency_store::resource_publication_revision(&r).unwrap(),
                    create: false,
                    profile: hagency_store::ProfileChange::Preserve {},
                    ceiling: hagency_store::CeilingChange::Monthly {
                        tokens: 4000u64.try_into().unwrap(),
                    },
                    engagement: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            f.owner
                .confirm_resource_activation_with(review, package(), Arc::new(|| true), provider())
                .await
                .err()
                .unwrap()
                .code,
            "source_adoption_review_changed"
        );
        assert!(
            !f.owner
                .0
                .configuration_stopped
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(
            !f.owner
                .0
                .host
                .state_directory()
                .join("adopted-management-initializer-v1.json")
                .exists()
        );
        assert_eq!(
            manager
                .resources(ResourceQuery::default())
                .await
                .unwrap()
                .resources[0]
                .budget
                .pool
                .ceiling
                .map(u64::from),
            Some(4000)
        );
        manager.retire();
        drop(session);
        drop(manager);
        f.stop().await;
    }
}

#[cfg(test)]
#[path = "resource_activation/task_history_tests.rs"]
mod task_history_tests;
