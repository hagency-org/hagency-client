//! Trusted initialization choice after completed original-source adoption.
//! Not a serialized authority, registry opener, or mini-app input.
use super::*;
use hagency_store::owner_resources::OwnerResourceRegistry;
use serde::{Deserialize, Serialize};
use std::io::Read;

mod task_prepare;
pub(super) use task_prepare::{TaskAdoptionPrepare, TaskPrepareReview};
mod empty_prepare;
pub(super) use empty_prepare::EmptyDevicePrepare;
mod allocation_executor;
pub use allocation_executor::AdoptedAllocationExecutor;
mod shared_executor;
pub use shared_executor::AdoptedSharedExecutor;

const INITIALIZER_FILE: &str = "adopted-management-initializer-v1.json";
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Descriptor {
    version: u8,
    registry_id: String,
    migration_id: String,
    origin: String,
    owner: String,
    profile: String,
    original_device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_prepare: Option<TaskAdoptionPrepare>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    empty_prepare: Option<EmptyDevicePrepare>,
}
fn descriptor_error() -> NativeError {
    failure(409, "adopted_management_initializer_unavailable")
}
fn read_descriptor(state: &Path) -> Result<Option<Descriptor>, NativeError> {
    let path = state.join(INITIALIZER_FILE);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(descriptor_error()),
        Ok(_) => {}
    }
    let file = hagency_store::private::open(&path, false).map_err(|_| descriptor_error())?;
    let mut bytes = Vec::new();
    file.take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| descriptor_error())?;
    if bytes.len() > 16384 {
        return Err(descriptor_error());
    }
    let value: Descriptor = serde_json::from_slice(&bytes).map_err(|_| descriptor_error())?;
    if bytes.len() > if value.version == 1 { 4096 } else { 16384 } {
        return Err(descriptor_error());
    }
    if !matches!(
        (value.version, value.task_prepare.is_some(), value.empty_prepare.is_some()),
        (1, false, false) | (2, true, false) | (3, false, true)
    ) || value.profile.len() != 64
        || !value.profile.bytes().all(|b| b.is_ascii_hexdigit())
        || value.registry_id.is_empty()
        || value.registry_id.len() > 128
        || value.original_device.is_empty()
        || value.original_device.len() > 128
        || key(&value.migration_id).is_err()
    {
        return Err(descriptor_error());
    }
    if let Some(prepare) = &value.task_prepare {
        prepare.validate()?;
    }
    if let Some(prepare) = &value.empty_prepare {
        prepare.validate()?;
    }
    Ok(Some(value))
}

/// Retains only original initialization identity, not the retired source owner
/// or its physical locks. Every open reauthenticates the SDK and revalidates the
/// active canonical plan and original source archive before returning a manager.
#[derive(Clone)]
pub struct AdoptedManagementSelection {
    pub(super) registry: OwnerResourceRegistry,
    pub(super) migration_id: String,
    state: std::path::PathBuf,
    origin: String,
    owner: String,
    pub(super) profile: String,
    pub(super) device: String,
    task_prepare: Option<TaskAdoptionPrepare>,
    empty_prepare: Option<EmptyDevicePrepare>,
}
impl AdoptedManagementSelection {
    pub(super) fn recover_original(
        owner: &NativeOwner,
        registry: OwnerResourceRegistry,
        migration_id: &str,
        profile: &str,
    ) -> Result<Self, NativeError> {
        let descriptor =
            read_descriptor(owner.0.host.state_directory())?.ok_or_else(descriptor_error)?;
        if descriptor.migration_id != migration_id || descriptor.profile != profile {
            return Err(descriptor_error());
        }
        Self::loaded(
            owner.0.host.state_directory(),
            &owner.0.expected_origin,
            &owner.0.expected_owner,
            registry,
            descriptor,
        )
    }
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            version: if self.empty_prepare.is_some() { 3 } else if self.task_prepare.is_some() { 2 } else { 1 },
            task_prepare: self.task_prepare.clone(),
            empty_prepare: self.empty_prepare.clone(),
            registry_id: self.registry.id().into(),
            migration_id: self.migration_id.clone(),
            origin: self.origin.clone(),
            owner: self.owner.clone(),
            profile: self.profile.clone(),
            original_device: self.device.clone(),
        }
    }
    /// Called under original owner-root custody: after v1 preparation or before
    /// the exact v2 commit. Create-once descriptive selection, never authority.
    pub(super) fn persist(&self, owner: &NativeOwner) -> Result<(), NativeError> {
        owner
            .0
            .host
            .check_state_lock()
            .map_err(|_| descriptor_error())?;
        let descriptor = self.descriptor();
        if let Some(old) = read_descriptor(&self.state)? {
            return if old == descriptor {
                Ok(())
            } else {
                Err(descriptor_error())
            };
        }
        let bytes = serde_json::to_vec(&descriptor).map_err(|_| descriptor_error())?;
        if bytes.len() > if descriptor.version == 1 { 4096 } else { 16384 } {
            return Err(descriptor_error());
        }
        if self.task_prepare.is_some() || self.empty_prepare.is_some() {
            task_prepare::persist_new(&self.state, &bytes)
        } else {
            hagency_store::private::write_new(&self.state.join(INITIALIZER_FILE), &bytes)
                .map_err(|_| descriptor_error())
        }
    }
    fn loaded(
        state: &Path,
        origin: &str,
        owner: &str,
        registry: OwnerResourceRegistry,
        descriptor: Descriptor,
    ) -> Result<Self, NativeError> {
        if descriptor.origin != origin
            || descriptor.owner != owner
            || descriptor.registry_id != registry.id()
        {
            return Err(descriptor_error());
        }
        Ok(Self {
            registry,
            migration_id: descriptor.migration_id,
            state: state.canonicalize().map_err(|_| descriptor_error())?,
            origin: origin.into(),
            owner: owner.into(),
            profile: descriptor.profile,
            device: descriptor.original_device,
            task_prepare: descriptor.task_prepare,
            empty_prepare: descriptor.empty_prepare,
        })
    }
    /// Trusted Desktop restart path. Only the OS-owner canonical registry can be
    /// selected; no descriptor field can choose a database path. The returned
    /// choice still requires fresh SDK authorization and exact active-plan checks.
    pub async fn load_for_host(
        state: &Path,
        origin: &str,
        owner: &str,
    ) -> Result<Option<Self>, NativeError> {
        let state = state.to_owned();
        let origin = origin.to_owned();
        let owner = owner.to_owned();
        tokio::task::spawn_blocking(move || {
            let Some(descriptor) = read_descriptor(&state)? else {
                return Ok(None);
            };
            let registry = super::resource_activation::activation_registry(true)
                .map_err(|_| descriptor_error())?;
            Self::loaded(&state, &origin, &owner, registry, descriptor).map(Some)
        })
        .await
        .map_err(|_| descriptor_error())?
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn load_for_isolated_tests(
        state: &Path,
        origin: &str,
        owner: &str,
        registry: OwnerResourceRegistry,
    ) -> Result<Option<Self>, NativeError> {
        read_descriptor(state)?
            .map(|d| Self::loaded(state, origin, owner, registry, d))
            .transpose()
    }
    pub(super) fn completed(
        owner: &NativeOwner,
        registry: OwnerResourceRegistry,
        migration_id: String,
        profile: String,
        device: String,
    ) -> Self {
        Self {
            registry,
            migration_id,
            state: owner.0.host.state_directory().into(),
            origin: owner.0.expected_origin.clone(),
            owner: owner.0.expected_owner.clone(),
            profile,
            device,
            task_prepare: None,
            empty_prepare: None,
        }
    }
    /// Identity comparison for a trusted host's existing account slot. No path
    /// or private identity is returned to a script.
    pub fn matches_host(&self, state: &Path, origin: &str, owner: &str) -> bool {
        state.canonicalize().is_ok_and(|path| self.state == path)
            && self.origin == origin
            && self.owner == owner
    }
    async fn check_owner(&self, owner: &NativeOwner) -> Result<(), NativeError> {
        if !self.matches_host(
            owner.0.host.state_directory(),
            &owner.0.expected_origin,
            &owner.0.expected_owner,
        ) || owner.0.adoption_recovery.get() != Some(&self.migration_id)
        {
            return Err(failure(401, "adopted_management_scope_changed"));
        }
        owner
            .0
            .host
            .check_state_lock()
            .map_err(|_| failure(409, "source_root_changed"))?;
        owner.sync_management_matrix().await?;
        let cookie = owner.0.cookie.lock().await.clone();
        let raw = cookie
            .strip_prefix("hagency_console=")
            .ok_or_else(|| failure(401, "owner_authorization_required"))?;
        owner
            .0
            .host
            .console
            .0
            .server_login
            .check_matrix_source(raw)
            .await
            .map_err(|_| failure(401, "adopted_management_scope_changed"))?;
        let device = owner
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        if device.bearer().is_err()
            || crate::console::owned_agents::profile_identity(
                device.origin(),
                device.issuer(),
                device.subject(),
                device.owner_mxid(),
            )
            .map_err(|_| failure(401, "management_profile_mismatch"))?
                != self.profile
            || !owner
                .0
                .management
                .matches_adopted(&self.registry, &self.profile, &self.owner)
                .await
        {
            return Err(failure(401, "adopted_management_scope_changed"));
        }
        Ok(())
    }
    pub async fn open_with_matrix_guarded(
        &self,
        state: &Path,
        origin: &str,
        owner: &str,
        source: Arc<dyn MatrixTokenSource>,
        guard: impl FnOnce(&NativeOwner) -> Result<(), NativeError> + Send,
    ) -> Result<NativeOwner, NativeError> {
        if !self.matches_host(state, origin, owner) {
            return Err(failure(401, "adopted_management_scope_changed"));
        }
        let native = NativeOwner::open_adopted_management_with_matrix_guarded(
            state,
            origin,
            owner,
            source,
            self.registry.clone(),
            self.migration_id.clone(),
            guard,
        )
        .await?;
        if let Err(error) = self.check_owner(&native).await {
            let _ = native.suspend().await;
            return Err(error);
        }
        Ok(native)
    }
    /// Native review gets its own manager. Script service permissions never
    /// acquire ConsentGrants by sharing the ordinary Resources manager.
    pub async fn grant_access(
        &self,
        owner: &NativeOwner,
        mut package: ManagementPackage,
    ) -> Result<OwnerGrantAccess, NativeError> {
        self.check_owner(owner).await?;
        let device = owner
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        if device.device_id() != self.device {
            return Err(failure(
                409,
                "adopted_grant_device_reauthorization_required",
            ));
        }
        package.permissions = vec![
            ManagementPermission::ReadResources,
            ManagementPermission::ReadProviders,
            ManagementPermission::ConnectProviders,
            ManagementPermission::ConsentGrants,
            ManagementPermission::ReadIntents,
            ManagementPermission::PublishGrants,
        ];
        let manager = owner.management(package).await?;
        let result = OwnerGrantIntents::new(manager.clone())
            .bind_registry(self.registry.clone())
            .await;
        if let Err(error) = self.check_owner(owner).await {
            manager.retire();
            return Err(error);
        }
        if result.is_err() {
            manager.retire();
        }
        result
    }
}
impl NativeOwner {
    /// Recovers a descriptive initialization choice from an already authenticated
    /// adopted owner. It cannot open a legacy source or activate an incomplete plan.
    pub async fn adopted_management_selection(
        &self,
        registry: OwnerResourceRegistry,
    ) -> Result<AdoptedManagementSelection, NativeError> {
        let id = self
            .0
            .adoption_recovery
            .get()
            .cloned()
            .ok_or_else(|| failure(409, "owner_source_adoption_required"))?;
        self.validate_adoption_recovery(&registry, &id).await?;
        if let Some(descriptor) = read_descriptor(self.0.host.state_directory())? {
            let selection = AdoptedManagementSelection::loaded(
                self.0.host.state_directory(),
                &self.0.expected_origin,
                &self.0.expected_owner,
                registry,
                descriptor,
            )?;
            selection.check_owner(self).await?;
            return Ok(selection);
        }
        Err(descriptor_error())
    }
}

#[cfg(feature = "shared-intake")]
impl AdoptedManagementSelection {
    pub(super) async fn check_owner_stop(&self, owner: &NativeOwner) -> Result<(), NativeError> {
        self.check_executor_owner(owner).await
    }
}
