//! Proof-only selected-source shutdown and lock custody. No source fencing,
//! import, sentinel creation, capacity assignment or mini-app route.
#[cfg(feature = "resource-adoption")]
mod adoption;
#[cfg(feature = "resource-adoption")]
mod empty_device;
#[cfg(feature = "resource-adoption")]
pub(in crate::console::native) use empty_device::EmptyDeviceObservation;
#[cfg(feature = "resource-adoption")]
pub use empty_device::{EmptyDeviceReview, EmptyDeviceSource, NativeEmptyDeviceInitialization};
mod files;
#[cfg(feature = "resource-adoption")]
mod graph_history;
#[cfg(feature = "resource-adoption")]
pub use graph_history::{NativeOperatorGraphInventory, OperatorGraphInventoryReview};
#[cfg(feature = "resource-adoption")]
pub use crate::operator_graphs::history::OperatorGraphHistorySummary;
#[cfg(feature = "resource-adoption")]
pub use adoption::NativeResourceAdoption;
#[cfg(test)]
mod tests;
use super::{ManagementPackage, NativeError, NativeOwner, failure};
use crate::console::owned_agents::profile_identity;
use crate::console::{AuthorizedDevice, device_execution::DeviceIdentity};
use hagency_store::{
    installation_authority::AdoptionExclusiveLease,
    management::{HostContext, Permission},
    owner_resources::OwnerResourceRegistry,
};
use serde::Serialize;
use std::{
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

struct Gate {
    expires: Instant,
    native: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl Gate {
    fn check(&self) -> Result<(), &'static str> {
        if Instant::now() >= self.expires || !(self.native)() {
            Err("source_quiescence_authority_changed")
        } else {
            Ok(())
        }
    }
}

/// Native host confirmation must bind SDK account/epoch, package and installation.
/// Its live gate is independent of the source Session intentionally retired here.
pub struct SourceQuiescenceRequest {
    owner: NativeOwner,
    package: ManagementPackage,
    registry: OwnerResourceRegistry,
    gate: Arc<Gate>,
    #[cfg(test)]
    pub(in crate::console::native) lease_root: Option<std::path::PathBuf>,
}
impl SourceQuiescenceRequest {
    pub fn new(
        owner: &NativeOwner,
        package: ManagementPackage,
        registry: OwnerResourceRegistry,
        expires: Instant,
        native_gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, NativeError> {
        if expires > Instant::now() + Duration::from_secs(60)
            || !package
                .permissions
                .contains(&Permission::PrepareSourceQuiescence)
        {
            return Err(failure(403, "source_quiescence_permission_required"));
        }
        let gate = Arc::new(Gate {
            expires,
            native: Arc::new(native_gate),
        });
        gate.check().map_err(err)?;
        Ok(Self {
            owner: owner.clone(),
            package,
            registry,
            gate,
            #[cfg(test)]
            lease_root: None,
        })
    }
}
fn err(code: &'static str) -> NativeError {
    failure(409, code)
}

struct Scope {
    owner: NativeOwner,
    #[cfg(feature = "resource-adoption")]
    profile: String,
    device: AuthorizedDevice,
    registry: OwnerResourceRegistry,
    gate: Arc<Gate>,
    sources: StdMutex<Option<files::Sources>>,
    #[cfg(feature = "resource-adoption")]
    graph_archive_allowed: bool,
    #[cfg(test)]
    lease_root: Option<std::path::PathBuf>,
}
async fn verify_source(
    owner: &NativeOwner,
    expected: &AuthorizedDevice,
) -> Result<(), NativeError> {
    let cookie = owner.0.cookie.lock().await.clone();
    let raw = cookie
        .strip_prefix("hagency_console=")
        .ok_or(err("source_quiescence_authority_changed"))?;
    owner
        .0
        .host
        .console
        .0
        .server_login
        .check_matrix_source(raw)
        .await
        .map_err(|_| err("source_quiescence_authority_changed"))?;
    let current = owner
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| err("source_quiescence_authority_changed"))?;
    if DeviceIdentity::from(&current) != DeviceIdentity::from(expected)
        || expected.bearer().is_err()
        || current.bearer().is_err()
    {
        return Err(err("source_quiescence_authority_changed"));
    }
    Ok(())
}
enum Phase {
    Ready,
    Running(tokio::task::JoinHandle<Result<Arc<QuiescedSources>, &'static str>>),
    Finished(Result<Arc<QuiescedSources>, &'static str>),
}
/// Retains one drain attempt across cancelled/concurrent callers. No constructor
/// accepts paths, externally asserted exit results or a serialized proof.
pub struct SourceQuiescence {
    scope: Arc<Scope>,
    phase: Mutex<Phase>,
}
/// Exact handles, not a bool asserting that all legacy sources were discovered.
pub struct QuiescedSources {
    owner: NativeOwner,
    #[cfg(feature = "resource-adoption")]
    profile: String,
    #[cfg(feature = "resource-adoption")]
    device_id: String,
    registry: OwnerResourceRegistry,
    gate: Arc<Gate>,
    lease: AdoptionExclusiveLease,
    files: StdMutex<files::LockedSources>,
    #[cfg(feature = "resource-adoption")]
    adoption_custody: StdMutex<std::sync::Weak<adoption::JournalCustody>>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceQuiescenceStatus {
    pub scope: &'static str,
    pub external_roots_complete: bool,
    pub adoption: &'static str,
    pub personal_database_present: bool,
    pub blockers: Vec<&'static str>,
}
impl QuiescedSources {
    pub fn check(&self) -> Result<(), NativeError> {
        self.gate.check().map_err(err)?;
        self.owner
            .0
            .host
            .check_state_lock()
            .map_err(|_| err("source_root_changed"))?;
        self.lease
            .check()
            .map_err(|_| err("source_installation_lease_changed"))?;
        if self.lease.registry_id() != self.registry.id() {
            return Err(err("source_registry_changed"));
        }
        self.files
            .lock()
            .map_err(|_| err("source_quiescence_unknown"))?
            .check()
            .map_err(err)?;
        self.gate.check().map_err(err)
    }
    pub fn status(&self) -> Result<SourceQuiescenceStatus, NativeError> {
        self.check()?;
        let personal = self
            .files
            .lock()
            .map_err(|_| err("source_quiescence_unknown"))?
            .personal_present();
        let mut blockers = vec![
            "installation_writer_coverage_not_complete",
            "source_inventory_adoption_not_assessed",
            "external_roots_not_complete",
        ];
        if !personal {
            blockers.push("personal_history_not_present");
        }
        Ok(SourceQuiescenceStatus {
            scope: "current_profile_selected_root",
            external_roots_complete: false,
            adoption: "not_enabled",
            personal_database_present: personal,
            blockers,
        })
    }
}
impl NativeOwner {
    pub async fn prepare_source_quiescence(
        &self,
        request: SourceQuiescenceRequest,
    ) -> Result<SourceQuiescence, NativeError> {
        if !Arc::ptr_eq(&self.0, &request.owner.0) {
            return Err(err("source_quiescence_owner_mismatch"));
        }
        request.gate.check().map_err(err)?;
        self.0
            .host
            .check_state_lock()
            .map_err(|_| err("source_root_changed"))?;
        let work = async {
            let cookie = self.0.cookie.lock().await.clone();
            let raw = cookie
                .strip_prefix("hagency_console=")
                .ok_or(err("source_quiescence_authority_changed"))?;
            self.0
                .host
                .console
                .0
                .server_login
                .check_matrix_source(raw)
                .await
                .map_err(|_| err("source_quiescence_authority_changed"))?;
            let device = self
                .0
                .host
                .console
                .authorized_device()
                .await
                .map_err(|_| err("source_quiescence_authority_changed"))?;
            device
                .bearer()
                .map_err(|_| err("source_quiescence_authority_changed"))?;
            if device.origin() != self.0.expected_origin
                || device.owner_mxid() != self.0.expected_owner
            {
                return Err(err("source_quiescence_owner_mismatch"));
            }
            HostContext::new(
                device.owner_mxid().into(),
                request.package.account_epoch,
                request.package.digest,
                request.package.instance,
                device.device_id().into(),
                device.generation(),
            )
            .map_err(|_| err("source_quiescence_permission_required"))?;
            let profile = profile_identity(
                device.origin(),
                device.issuer(),
                device.subject(),
                device.owner_mxid(),
            )
            .map_err(|_| err("source_quiescence_owner_mismatch"))?;
            let root = self.0.host.state_directory().to_owned();
            let captured_profile = profile.clone();
            let sources = tokio::task::spawn_blocking(move || {
                files::Sources::capture(&root, &captured_profile)
            })
            .await
            .map_err(|_| err("source_quiescence_unknown"))?
            .map_err(err)?;
            request.gate.check().map_err(err)?;
            verify_source(self, &device).await?;
            request.gate.check().map_err(err)?;
            Ok(SourceQuiescence {
                scope: Arc::new(Scope {
                    owner: self.clone(),
                    #[cfg(feature = "resource-adoption")]
                    profile,
                    device,
                    registry: request.registry,
                    gate: request.gate.clone(),
                    sources: StdMutex::new(Some(sources)),
                    #[cfg(feature = "resource-adoption")]
                    graph_archive_allowed: request.package.permissions.contains(&Permission::ArchiveOperatorGraphHistory),
                    #[cfg(test)]
                    lease_root: request.lease_root,
                }),
                phase: Mutex::new(Phase::Ready),
            })
        };
        tokio::time::timeout_at(request.gate.expires.into(), work)
            .await
            .map_err(|_| err("source_quiescence_authority_changed"))?
    }
}
impl SourceQuiescence {
    pub async fn close_and_drain(&self) -> Result<Arc<QuiescedSources>, NativeError> {
        self.scope.gate.check().map_err(err)?;
        let mut phase = self.phase.lock().await;
        if matches!(*phase, Phase::Ready) {
            let scope = self.scope.clone();
            tokio::time::timeout_at(
                scope.gate.expires.into(),
                verify_source(&scope.owner, &scope.device),
            )
            .await
            .map_err(|_| err("source_quiescence_authority_changed"))??;
            scope.gate.check().map_err(err)?;
            scope
                .owner
                .0
                .host
                .console
                .0
                .owned_runtime
                .close_for_migration();
            scope
                .owner
                .0
                .host
                .console
                .0
                .owner_provider
                .close_for_migration();
            scope.owner.retire();
            *phase = Phase::Running(tokio::spawn(async move {
                tokio::time::timeout_at(scope.gate.expires.into(), drain(scope.clone()))
                    .await
                    .unwrap_or(Err("source_quiescence_timeout"))
            }));
        }
        if let Phase::Running(task) = &mut *phase {
            let result = task.await.unwrap_or(Err("source_quiescence_unknown"));
            *phase = Phase::Finished(result);
        }
        let Phase::Finished(result) = &*phase else {
            return Err(err("source_quiescence_unknown"));
        };
        let result = result.clone().map_err(err)?;
        result.check()?;
        Ok(result)
    }
}
async fn drain(scope: Arc<Scope>) -> Result<Arc<QuiescedSources>, &'static str> {
    // Native calls already admitted before retirement must leave before lock
    // acquisition. Each manager also closes its own constructor gate.
    let _calls = scope.owner.0.calls.lock().await;
    let _mutations = scope.owner.0.management.mutations.lock().await;
    scope
        .owner
        .0
        .configuration_custody
        .drain()
        .await
        .map_err(|_| "source_configuration_drain_unknown")?;
    scope.owner.0.account_logins.drain().await.map_err(|_|"source_account_login_drain_unknown")?;
    scope.owner.0.shared_execution.drain().await.map_err(|_|"source_shared_execution_drain_unknown")?;
    scope
        .owner
        .0
        .managed_runtime
        .drain()
        .await
        .map_err(|_| "source_managed_runtime_drain_unknown")?;
    let console = &scope.owner.0.host.console;
    let (runtime, provider, domain) = tokio::join!(
        console.0.owned_runtime.drain_for_migration(),
        console.0.owner_provider.drain_for_migration(),
        scope.owner.0.management.drain()
    );
    runtime.map_err(|_| "source_runtime_drain_unknown")?;
    provider.map_err(|_| "source_provider_drain_unknown")?;
    domain.map_err(|_| "source_domain_drain_unknown")?;
    scope.gate.check()?;
    scope
        .owner
        .0
        .host
        .check_state_lock()
        .map_err(|_| "source_root_changed")?;
    let gate = scope.gate.clone();
    let check = gate.clone();
    let registry = scope.registry.id().to_owned();
    let sources = scope
        .sources
        .lock()
        .map_err(|_| "source_quiescence_unknown")?
        .take()
        .ok_or("source_quiescence_unknown")?;
    #[cfg(test)]
    let lease_root = scope.lease_root.clone();
    let (lease, files) = tokio::task::spawn_blocking(move || {
        #[cfg(not(test))]
        let lease =
            AdoptionExclusiveLease::acquire(registry, gate.expires, move || check.check().is_ok())
                .map_err(|_| "source_installation_busy")?;
        #[cfg(test)]
        let lease = AdoptionExclusiveLease::isolated_for_tests(
            &lease_root.ok_or("isolated_lease_root_required")?,
            registry,
            gate.expires,
            move || check.check().is_ok(),
        )
        .map_err(|_| "source_installation_busy")?;
        let files = sources.lock(gate)?;
        Ok::<_, &'static str>((lease, files))
    })
    .await
    .map_err(|_| "source_quiescence_unknown")??;
    scope.gate.check()?;
    Ok(Arc::new(QuiescedSources {
        owner: scope.owner.clone(),
        #[cfg(feature = "resource-adoption")]
        profile: scope.profile.clone(),
        #[cfg(feature = "resource-adoption")]
        device_id: scope.device.device_id().into(),
        registry: scope.registry.clone(),
        gate: scope.gate.clone(),
        lease,
        files: StdMutex::new(files),
        #[cfg(feature = "resource-adoption")]
        adoption_custody: StdMutex::new(std::sync::Weak::new()),
    }))
}
