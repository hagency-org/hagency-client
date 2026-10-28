//! Bounded observation of existing source files. No normal ledger opener,
//! startup recovery, source write, migration, or allocation authority.
mod filesystem;
#[cfg(test)]
pub(super) mod tests;

use super::{ManagementPackage, NativeError, NativeOwner, failure};
use crate::console::{owned_agents::profile_identity, server_login::AuthorizedDevice};
use hagency_agent_local::reconciliation::PersonalLedgerAudit;
use hagency_store::management::{HostContext, Permission, ProfileLedgerAudit};
use serde::Serialize;
use std::{
    path::{Component, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

const MAX_ROOTS: usize = 8;
const MAX_RESULT_BYTES: usize = 128 * 1024;
static PENDING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// Construct only in the trusted host from its reviewed package, live account
/// epoch and explicit root selections. No script can deserialize this request.
/// Selecting a path does not establish that it is an owned or complete source.
pub struct SourceInspectionRequest {
    owner: NativeOwner,
    package: ManagementPackage,
    additional_roots: Vec<PathBuf>,
    expires: Instant,
    native_gate: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl SourceInspectionRequest {
    pub fn new(
        owner: &NativeOwner,
        package: ManagementPackage,
        additional_roots: Vec<PathBuf>,
        expires: Instant,
        native_gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, NativeError> {
        let now = Instant::now();
        if expires <= now
            || expires > now + Duration::from_secs(30)
            || !package.permissions.contains(&Permission::ReadResources)
            || additional_roots.len() >= MAX_ROOTS
            || additional_roots.iter().any(|p| {
                !p.is_absolute()
                    || p.as_os_str().len() > 4096
                    || p.components()
                        .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
            })
            || !native_gate()
            || owner.0.configuration_stopped.load(Ordering::Acquire)
        {
            return Err(failure(403, "source_inspection_permission_required"));
        }
        Ok(Self {
            owner: owner.clone(),
            package,
            additional_roots,
            expires,
            native_gate: Arc::new(native_gate),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceObservationStatus {
    Inspected,
    NotPresent,
    Busy,
    Unsupported,
    Unreadable,
    Changed,
    ObservationLimit,
    RecoveryRequired,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceObservation<T> {
    pub status: SourceObservationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<T>,
}
impl<T> SourceObservation<T> {
    fn status(status: SourceObservationStatus) -> Self {
        Self {
            status,
            evidence: None,
        }
    }
    fn inspected(evidence: T) -> Self {
        Self {
            status: SourceObservationStatus::Inspected,
            evidence: Some(evidence),
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRootObservation {
    /// Zero is the current native owner root; remaining indices follow the
    /// host's explicit selections. Paths and other profile IDs never leave.
    pub selection: u32,
    pub status: SourceObservationStatus,
    pub profile_metadata: SourceObservationStatus,
    pub profile_directories_observed: u32,
    pub other_profile_directories: u32,
    pub unrecognized_entries: u32,
    pub domain: SourceObservation<ProfileLedgerAudit>,
    pub personal: SourceObservation<PersonalLedgerAudit>,
}
impl SourceRootObservation {
    fn unavailable(selection: u32, status: SourceObservationStatus) -> Self {
        Self {
            selection,
            status,
            profile_metadata: status,
            profile_directories_observed: 0,
            other_profile_directories: 0,
            unrecognized_entries: 0,
            domain: SourceObservation::status(status),
            personal: SourceObservation::status(status),
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCensus {
    pub version: u16,
    pub scope: &'static str,
    pub adoption: &'static str,
    pub external_roots_complete: bool,
    pub roots: Vec<SourceRootObservation>,
    pub blockers: Vec<&'static str>,
}

struct Scope {
    owner: NativeOwner,
    device: AuthorizedDevice,
    profile: String,
    expires: Instant,
    native_gate: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl Scope {
    fn check(&self) -> Result<(), NativeError> {
        if Instant::now() >= self.expires
            || self.owner.0.configuration_stopped.load(Ordering::Acquire)
            || !(self.native_gate)()
            || self.device.bearer().is_err()
        {
            return Err(failure(401, "source_inspection_authority_changed"));
        }
        Ok(())
    }
    async fn check_current(&self) -> Result<(), NativeError> {
        self.check()?;
        // Observation deliberately does not call sync_matrix/renew_matrix.
        // The original device and SDK epoch must still be current, or the host
        // must obtain fresh authorization separately before another audit.
        let cookie = self.owner.0.cookie.lock().await.clone();
        let raw = cookie
            .strip_prefix("hagency_console=")
            .ok_or(failure(401, "source_inspection_authority_changed"))?;
        self.owner
            .0
            .host
            .console
            .0
            .server_login
            .check_matrix_source(raw)
            .await
            .map_err(|_| failure(401, "source_inspection_authority_changed"))?;
        let current = self
            .owner
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "source_inspection_authority_changed"))?;
        if current.origin() != self.device.origin()
            || current.issuer() != self.device.issuer()
            || current.subject() != self.device.subject()
            || current.owner_mxid() != self.device.owner_mxid()
            || current.device_id() != self.device.device_id()
            || current.generation() != self.device.generation()
            || current.bearer().is_err()
        {
            return Err(failure(401, "source_inspection_authority_changed"));
        }
        self.check()
    }
    async fn current_with_deadline(&self) -> Result<(), NativeError> {
        tokio::time::timeout_at(self.expires.into(), self.check_current())
            .await
            .map_err(|_| failure(401, "source_inspection_authority_changed"))?
    }
}
impl NativeOwner {
    /// Observe the exact authenticated profile in already-existing files.
    /// This does not create NativeManagement, run ledger recovery, renew a login,
    /// expose another profile, or certify that all historical sources were found.
    /// The SDK source retains its own token-refresh policy during epoch checks.
    pub async fn inspect_reconciliation_sources(
        &self,
        request: SourceInspectionRequest,
    ) -> Result<SourceCensus, NativeError> {
        let permit = PENDING
            .try_acquire()
            .map_err(|_| failure(503, "source_inspection_busy"))?;
        if !Arc::ptr_eq(&request.owner.0, &self.0)
            || Instant::now() >= request.expires
            || !(request.native_gate)()
            || self.0.configuration_stopped.load(Ordering::Acquire)
        {
            return Err(failure(401, "source_inspection_authority_changed"));
        }
        let device = tokio::time::timeout_at(
            request.expires.into(),
            self.0.host.console.authorized_device(),
        )
        .await
        .map_err(|_| failure(401, "source_inspection_authority_changed"))?
        .map_err(|_| failure(401, "source_inspection_authority_changed"))?;
        if device.origin() != self.0.expected_origin || device.owner_mxid() != self.0.expected_owner
        {
            return Err(failure(401, "source_inspection_authority_changed"));
        }
        HostContext::new(
            device.owner_mxid().into(),
            request.package.account_epoch,
            request.package.digest,
            request.package.instance,
            device.device_id().into(),
            device.generation(),
        )
        .map_err(|_| failure(403, "source_inspection_permission_required"))?;
        let profile = profile_identity(
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .map_err(|_| failure(401, "source_inspection_authority_changed"))?;
        let scope = Arc::new(Scope {
            owner: self.clone(),
            expires: request.expires.min(device.valid_until()),
            device,
            profile,
            native_gate: request.native_gate,
        });
        scope.current_with_deadline().await?;
        let mut roots = vec![self.0.host.state_directory().to_owned()];
        roots.extend(request.additional_roots);
        let job_scope = scope.clone();
        // A canceled caller does not release the work bound until the finite
        // read-only job actually exits. Its retained owner also holds root lock.
        let job = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            filesystem::inspect(job_scope, roots)
        });
        let result = tokio::time::timeout_at(scope.expires.into(), job)
            .await
            .map_err(|_| failure(401, "source_inspection_authority_changed"))?
            .map_err(|_| failure(503, "source_inspection_unavailable"))??;
        scope.current_with_deadline().await?;
        if serde_json::to_vec(&result)
            .map_err(|_| failure(503, "source_inspection_unavailable"))?
            .len()
            > MAX_RESULT_BYTES
        {
            return Err(failure(503, "source_inspection_limit"));
        }
        Ok(result)
    }
}
