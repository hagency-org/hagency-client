//! Embedded resource management. No HTTP request, listener or browser assets.
use super::{NativeError, NativeOwner, failure};
use crate::console::owned_agents::profile_identity;
use hagency_store::{DomainRepository, DomainStore, management as service};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[cfg(test)]
#[path = "management_shutdown_tests.rs"]
mod shutdown_tests;

/// The Rinx host constructs this from its reviewed package lease, never from
/// Splash arguments. Each instance retains its exact permission set and epoch.
pub struct ManagementPackage {
    pub digest: String,
    pub instance: u64,
    pub account_epoch: u64,
    pub permissions: Vec<service::Permission>,
}

#[derive(Clone)]
enum ManagementStore {
    Legacy(DomainStore),
    #[cfg(feature = "resource-adoption")]
    Adopted(Box<hagency_store::owner_resources::adoption::AdoptedManagementStore>),
}
impl ManagementStore {
    async fn shutdown(&self) -> Result<(), hagency_store::Error> {
        match self {
            Self::Legacy(store) => store.shutdown().await,
            #[cfg(feature = "resource-adoption")]
            Self::Adopted(store) => store.shutdown().await,
        }
    }
    fn session(
        &self,
        context: service::HostContext,
        permissions: impl IntoIterator<Item = service::Permission>,
        expires: Instant,
    ) -> Result<service::Session, hagency_store::Error> {
        match self {
            Self::Legacy(store) => {
                service::Session::new(store.clone(), context, permissions, expires)
            }
            #[cfg(feature = "resource-adoption")]
            Self::Adopted(store) => {
                service::Session::new_adopted(store.as_ref().clone(), context, permissions, expires)
            }
        }
    }
}
#[derive(Default)]
pub(super) struct Registry {
    retired: AtomicBool,
    store: tokio::sync::Mutex<Option<ManagementStore>>,
    drain: tokio::sync::Mutex<Drain>,
    sessions: Mutex<Vec<service::WeakManagementAuthority>>,
    pub(super) mutations: tokio::sync::Mutex<()>,
}

#[derive(Default)]
enum Drain {
    #[default]
    Idle,
    Running(tokio::task::JoinHandle<Result<(), DrainFailure>>),
    Finished(Result<(), DrainFailure>),
}

#[derive(Clone, Copy)]
enum DrainFailure {
    Unknown,
    Unavailable,
}

impl Registry {
    pub(super) async fn has_store(&self) -> bool {
        self.ensure_live().is_ok() && self.store.lock().await.is_some()
    }
    #[cfg(feature = "resource-adoption")]
    pub(super) async fn install_adopted(
        &self,
        store: hagency_store::owner_resources::adoption::AdoptedManagementStore,
    ) -> Result<(), NativeError> {
        let mut current = self.store.lock().await;
        self.ensure_live()?;
        if current.is_some() || !store.is_live() {
            return Err(failure(409, "management_backing_changed"));
        }
        *current = Some(ManagementStore::Adopted(Box::new(store)));
        Ok(())
    }
    #[cfg(feature = "resource-adoption")]
    pub(super) async fn adopted_ready(&self) -> bool {
        self.ensure_live().is_ok()
            && matches!(self.store.lock().await.as_ref(),Some(ManagementStore::Adopted(store)) if store.is_live())
    }
    #[cfg(feature = "resource-adoption")]
    pub(super) async fn matches_adopted(
        &self,
        registry: &hagency_store::owner_resources::OwnerResourceRegistry,
        profile: &str,
        account: &str,
    ) -> bool {
        let current = self.store.lock().await;
        self.ensure_live().is_ok()
            && matches!(current.as_ref(),Some(ManagementStore::Adopted(store)) if store.is_live() && store.matches_registry(registry) && store.matches_profile(profile,account))
    }
    fn ensure_live(&self) -> Result<(), NativeError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(failure(401, "management_access_required"));
        }
        Ok(())
    }
    pub(super) fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        if let Ok(mut sessions) = self.sessions.lock() {
            for session in sessions.drain(..) {
                session.retire();
            }
        }
    }
    #[cfg(test)]
    pub(super) fn live_scope_count(&self) -> usize {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.is_live())
            .count()
    }
    fn remember(&self, session: service::Session) -> Result<(), NativeError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| failure(503, "management_unavailable"))?;
        if self.retired.load(Ordering::Acquire) {
            session.retire();
            return Err(failure(401, "owner_authorization_required"));
        }
        sessions.retain(service::WeakManagementAuthority::is_live);
        if sessions.len() >= 16 {
            session.retire();
            return Err(failure(503, "management_capacity"));
        }
        sessions.push(session.authority().downgrade());
        Ok(())
    }
    /// Retain one original shutdown attempt across cancelled and concurrent
    /// callers. A positive result is the domain worker's acknowledgement after
    /// repository drop, not proof that every runtime/provider writer stopped.
    /// Migration must separately drain those writers and reacquire source locks.
    /// A timed-out/closed attempt remains failed even if the channel later closes.
    pub(super) async fn drain(&self) -> Result<(), NativeError> {
        self.retire();
        let mut drain = self.drain.lock().await;
        if matches!(*drain, Drain::Idle) {
            let store = self.store.lock().await;
            *drain = match store.as_ref() {
                None => Drain::Finished(Ok(())),
                Some(store) => {
                    let store = store.clone();
                    // Keep the task in the registry before the next await. A
                    // dropped caller cannot lose its acknowledgement receiver.
                    Drain::Running(tokio::spawn(async move {
                        store.shutdown().await.map_err(|error| match error {
                            hagency_store::Error::OutcomeUnknown => DrainFailure::Unknown,
                            _ => DrainFailure::Unavailable,
                        })
                    }))
                }
            };
        }
        if let Drain::Running(task) = &mut *drain {
            let result = task.await.unwrap_or(Err(DrainFailure::Unknown));
            *drain = Drain::Finished(result);
        }
        let Drain::Finished(result) = *drain else {
            unreachable!("shutdown task retained until completion")
        };
        drop(drain);
        if result.is_ok() {
            self.store.lock().await.take();
        }
        result.map_err(|error| match error {
            DrainFailure::Unknown => failure(409, "management_drain_outcome_unknown"),
            DrainFailure::Unavailable => failure(503, "management_drain_unavailable"),
        })
    }
}

#[derive(Default)]
struct PackageSessions {
    current: Option<(service::HostContext, service::Session)>,
    issued: Vec<service::WeakManagementAuthority>,
}
impl PackageSessions {
    fn retire(&mut self) {
        if let Some((_, current)) = self.current.take() {
            current.retire();
        }
        for authority in self.issued.drain(..) {
            authority.retire();
        }
    }
}

pub struct NativeManagement {
    pub(super) owner: NativeOwner,
    package: ManagementPackage,
    store: ManagementStore,
    profile: String,
    retired: AtomicBool,
    // Session renewal is serialized but resource reads/writes remain on the
    // bounded domain worker. The UI thread never opens or queries SQLite.
    session: Mutex<PackageSessions>,
}
impl Drop for NativeManagement {
    fn drop(&mut self) {
        self.retired.store(true, Ordering::Release);
        if let Ok(current) = self.session.get_mut() {
            current.retire();
        }
    }
}

impl NativeOwner {
    pub async fn management(
        &self,
        package: ManagementPackage,
    ) -> Result<Arc<NativeManagement>, NativeError> {
        self.0.management.ensure_live()?;
        self.sync_management_matrix().await?;
        let device = self
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        device
            .bearer()
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        if device.origin() != self.0.expected_origin || device.owner_mxid() != self.0.expected_owner
        {
            return Err(failure(401, "matrix_account_mismatch"));
        }
        let profile = profile_identity(
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .map_err(|_| failure(401, "management_profile_mismatch"))?;
        // Validate package context before any filesystem operation.
        service::HostContext::new(
            device.owner_mxid().into(),
            package.account_epoch,
            package.digest.clone(),
            package.instance,
            device.device_id().into(),
            device.generation(),
        )
        .map_err(store_error)?;
        let store = {
            let mut current = self.0.management.store.lock().await;
            self.0.management.ensure_live()?;
            if current.is_none() {
                self.require_regular_mode()?;
                let root = self
                    .0
                    .host
                    .console
                    .0
                    .server_login
                    .state_directory()
                    .ok_or(failure(503, "management_state_unavailable"))?;
                let origin = device.origin().to_owned();
                let issuer = device.issuer().to_owned();
                let subject = device.subject().to_owned();
                let owner = device.owner_mxid().to_owned();
                let repository = tokio::task::spawn_blocking(move || {
                    // Derive the same authenticated profile key as the owner
                    // engine. Resource reads must not initialize the unrelated
                    // personal-agent SQLite file or race its startup worker.
                    let profile = profile_identity(&origin, &issuer, &subject, &owner)
                        .map_err(|_| failure(401, "management_profile_mismatch"))?;
                    let owners = root.join("owned-agent-owners");
                    hagency_store::private::directory(&owners).map_err(store_error)?;
                    let directory = owners.join(format!("management_{profile}"));
                    DomainRepository::open_managed(&directory, &profile).map_err(store_error)
                })
                .await
                .map_err(|_| failure(503, "management_state_unavailable"))??;
                self.0.management.ensure_live()?;
                device
                    .bearer()
                    .map_err(|_| failure(401, "owner_authorization_required"))?;
                *current = Some(ManagementStore::Legacy(
                    DomainStore::start(repository, 16).map_err(store_error)?,
                ));
            }
            let store = current.as_ref().unwrap();
            #[cfg(feature = "resource-adoption")]
            if let ManagementStore::Adopted(adopted) = store
                && !adopted.matches_profile(&profile, device.owner_mxid())
            {
                return Err(failure(401, "management_profile_mismatch"));
            }
            store.clone()
        };
        let manager = Arc::new(NativeManagement {
            owner: self.clone(),
            package,
            store,
            profile,
            retired: AtomicBool::new(false),
            session: Default::default(),
        });
        manager.authorized().await?;
        Ok(manager)
    }
}

impl NativeManagement {
    pub(super) fn account_enrollment_ready(&self)->bool {
        #[cfg(feature="resource-adoption")]
        {matches!(&self.store,ManagementStore::Adopted(_))}
        #[cfg(not(feature="resource-adoption"))]
        {false}
    }


    /// Acquire off the async/UI thread, then carry the lease into the actual
    /// queued resource command. A cancelled reply must not release writer
    /// exclusion while SQLite can still commit the original command.
    pub(super) async fn legacy_resource_lease(
        &self,
        context: &service::HostContext,
        session: &service::Session,
        permission: service::Permission,
    ) -> Result<hagency_store::installation_authority::LegacyResourceLease, NativeError> {
        session
            .authorize(context, permission)
            .map_err(store_error)?;
        let authority = session.authority();
        let original = context.clone();
        #[cfg(test)]
        let root = self
            .owner
            .0
            .host
            .console
            .0
            .server_login
            .state_directory()
            .ok_or(failure(503, "management_state_unavailable"))?
            .join("test-installation-authority");
        let lease = tokio::task::spawn_blocking(move || {
            let expires = Instant::now() + Duration::from_secs(60);
            let gate = move || authority.authorize(&original, permission).is_ok();
            #[cfg(not(test))]
            let lease =
                hagency_store::installation_authority::LegacyResourceLease::acquire(expires, gate);
            #[cfg(test)]
            let lease =
                hagency_store::installation_authority::LegacyResourceLease::isolated_for_tests(
                    &root, expires, gate,
                );
            lease.map_err(store_error)
        })
        .await
        .map_err(|_| failure(503, "management_unavailable"))??;
        let (current, _) = self.authorized().await?;
        if current != *context {
            return Err(failure(401, "management_scope_changed"));
        }
        lease.check().map_err(store_error)?;
        Ok(lease)
    }
    /// Irreversible package or owner retirement. Expired renewable sessions are
    /// not retirement, and a false result does not replace current authorization.
    pub fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
            || self.owner.0.management.retired.load(Ordering::Acquire)
    }
    pub(super) async fn authorized(
        &self,
    ) -> Result<(service::HostContext, service::Session), NativeError> {
        if self.is_retired() {
            return Err(failure(401, "management_access_required"));
        }
        self.owner.sync_management_matrix().await?;
        // Renewal may reuse a cached device grant; each management boundary
        // still requires the SDK's current account/token source.
        let cookie = self.owner.0.cookie.lock().await.clone();
        let raw = cookie
            .strip_prefix("hagency_console=")
            .ok_or_else(|| failure(401, "owner_authorization_required"))?;
        self.owner
            .0
            .host
            .console
            .0
            .server_login
            .check_matrix_source(raw)
            .await
            .map_err(|code| {
                failure(
                    if code.ends_with("unavailable") {
                        503
                    } else {
                        401
                    },
                    code,
                )
            })?;
        if self.is_retired() {
            return Err(failure(401, "management_access_required"));
        }
        let device = self
            .owner
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        device
            .bearer()
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        if device.origin() != self.owner.0.expected_origin
            || device.owner_mxid() != self.owner.0.expected_owner
        {
            return Err(failure(401, "matrix_account_mismatch"));
        }
        if profile_identity(
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .map_err(|_| failure(401, "management_profile_mismatch"))?
            != self.profile
        {
            return Err(failure(401, "management_profile_mismatch"));
        }
        let context = service::HostContext::new(
            device.owner_mxid().into(),
            self.package.account_epoch,
            self.package.digest.clone(),
            self.package.instance,
            device.device_id().into(),
            device.generation(),
        )
        .map_err(store_error)?;
        let mut current = self
            .session
            .lock()
            .map_err(|_| failure(503, "management_unavailable"))?;
        if self.retired.load(Ordering::Acquire) {
            return Err(failure(401, "management_access_required"));
        }
        if current
            .current
            .as_ref()
            .is_some_and(|(old, _)| *old != context)
        {
            current.retire();
        }
        current
            .issued
            .retain(service::WeakManagementAuthority::is_live);
        let expires = device
            .valid_until()
            .min(Instant::now() + Duration::from_secs(60));
        if let Some((_, session)) = &current.current
            && session.is_live()
            && session.valid_until() == expires
        {
            return Ok((context, session.clone()));
        }
        // A fresh operation uses the current authenticated deadline. Drop only
        // the cache's strong reference; queued/in-flight copies keep their own
        // unchanged deadline and remain revocable through the weak handles.
        current.current.take();
        let session = self
            .store
            .session(
                context.clone(),
                self.package.permissions.iter().copied(),
                expires,
            )
            .map_err(store_error)?;
        self.owner.0.management.remember(session.clone())?;
        current.issued.push(session.authority().downgrade());
        current.current = Some((context.clone(), session.clone()));
        Ok((context, session))
    }
    pub(super) async fn finish<T>(
        &self,
        original: &service::HostContext,
        result: Result<T, hagency_store::Error>,
        mutation: bool,
    ) -> Result<T, NativeError> {
        let (current, _) = self.authorized().await.map_err(|error| {
            if mutation && result.is_ok() {
                failure(409, "management_outcome_unknown")
            } else {
                error
            }
        })?;
        if current != *original {
            return Err(if mutation && result.is_ok() {
                failure(409, "management_outcome_unknown")
            } else {
                failure(401, "management_scope_changed")
            });
        }
        result.map_err(store_error)
    }
    pub async fn resources(
        &self,
        query: service::ResourceQuery,
    ) -> Result<service::ResourcePage, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.resources(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn account_inventory(&self, query: service::AccountInventoryQuery) -> Result<service::AccountInventory, NativeError> {
        query.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        let result = session.account_inventory(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn resource_detail(
        &self,
        query: service::ResourceDetailQuery,
    ) -> Result<service::ResourceDetail, NativeError> {
        query.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        let result = session.resource_detail(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn configure_resource(
        &self,
        input: service::ConfigureResource,
    ) -> Result<hagency_store::ResourceConfigurationResult, NativeError> {
        self.owner.require_regular_mode()?;
        let intent = service::Intent::new(
            service::IntentOperation::ConfigureResource,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, self.configure_resource_now(input))
            .await
    }
    async fn configure_resource_now(
        &self,
        input: service::ConfigureResource,
    ) -> Result<hagency_store::ResourceConfigurationResult, NativeError> {
        let (context, session) = self.authorized().await?;
        let lease = self
            .legacy_resource_lease(&context, &session, service::Permission::ConfigureResources)
            .await?;
        let result = session
            .configure_resource_with_legacy_lease(&context, input, lease)
            .await;
        self.finish(&context, result, true).await
    }
    pub async fn publish_resource(
        &self,
        input: service::PublishResource,
    ) -> Result<hagency_store::ResourcePublicationResult, NativeError> {
        if !self.account_enrollment_ready() {
            self.owner.require_regular_mode()?;
        }
        input.validate().map_err(store_error)?;
        let intent = service::Intent::new(
            service::IntentOperation::PublishResource,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, self.publish_resource_now(input))
            .await
    }
    async fn publish_resource_now(
        &self,
        input: service::PublishResource,
    ) -> Result<hagency_store::ResourcePublicationResult, NativeError> {
        let (context, session) = self.authorized().await?;
        if self.account_enrollment_ready() {
            let result = session.publish_resource(&context, input).await;
            return self.finish(&context, result, true).await;
        }
        let lease = self
            .legacy_resource_lease(&context, &session, service::Permission::PublishResources)
            .await?;
        let result = session
            .publish_resource_with_legacy_lease(&context, input, lease)
            .await;
        self.finish(&context, result, true).await
    }
    pub fn retire(&self) {
        // Package closure is local and must not log the human out of Matrix.
        // Marking the package permanently retired is handled below as well as
        // fencing its current session, so a later queued call cannot renew it.
        self.retired.store(true, Ordering::Release);
        if let Ok(mut current) = self.session.lock() {
            current.retire();
        }
    }
}

/// Closed codes for a refused monthly-maximum amendment; nothing was written.
pub(super) fn ceiling_refusal_code(
    reason: hagency_store::management::CeilingAmendmentRefusal,
) -> &'static str {
    use hagency_store::management::CeilingAmendmentRefusal as Refusal;
    match reason {
        Refusal::NotAvailable => "management_ceiling_amendment_not_available",
        Refusal::ClearRefused => "management_ceiling_clear_refused",
        Refusal::Unchanged => "management_ceiling_unchanged",
        Refusal::BelowCommitments => "management_ceiling_below_commitments",
        Refusal::UsageUnsettled => "management_ceiling_usage_unsettled",
    }
}

pub(super) fn store_error(error: hagency_store::Error) -> NativeError {
    use hagency_store::Error;
    let (status, code) = match error {
        Error::LocalAuthority => (401, "management_access_required"),
        Error::Invalid(_) => (400, "invalid_management_arguments"),
        Error::NotFound => (404, "management_not_found"),
        Error::Conflict => (409, "management_revision_conflict"),
        Error::WorkspaceInspectionRequired => (409, "management_workspace_inspection_required"),
        Error::WorkspaceActive => (409, "management_workspace_active"),
        Error::StoppedNotResolvable => (409, "management_stopped_not_resolvable"),
        Error::StoppedReviewExpired => (409, "management_stopped_review_expired"),
        Error::OperatorTaskRevisionChanged => (409, "management_task_revision_changed"),
        Error::ResourceNameRevisionChanged => (409, "management_resource_name_changed"),
        Error::ResourceEditRevisionChanged => (409, "management_resource_edit_changed"),
        Error::ResourceConfigurationSourceChanged => (409, "management_resource_source_changed"),
        Error::ResourceEditNotAvailable => (409, "management_resource_edit_not_available"),
        Error::CeilingAmendmentRefused(reason) => (409, ceiling_refusal_code(reason)),
        Error::State => (409, "management_resource_in_use"),
        Error::ManagementChangeCancelled => (409, "management_change_cancelled"),
        Error::Unqualified => (409, "management_model_unqualified"),
        Error::Capacity | Error::Busy => (503, "management_capacity"),
        Error::OutcomeUnknown => (409, "management_outcome_unknown"),
        _ => (503, "management_unavailable"),
    };
    failure(status, code)
}

#[cfg(test)]
#[path = "management_scope_tests.rs"]
mod scope_tests;

#[cfg(feature = "shared-intake")]
impl NativeManagement {
    pub(super) async fn owner_stop_transport(
        &self,
    ) -> Result<
        (
            super::shared_execution::SharedExecutionTransport,
            hagency_store::owner_resources::OwnerResourceRegistry,
        ),
        NativeError,
    > {
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, service::Permission::ChangeAgentRunControl)
            .map_err(|_| failure(403, "management_permission_required"))?;
        let deadline = session.valid_until();
        let gate: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
            session
                .authorize(&context, service::Permission::ChangeAgentRunControl)
                .is_ok()
        });
        let device = self
            .owner
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        let profile = hagency_store::owner_resources::ProfileAuthority::new(
            self.profile.clone(),
            device.owner_mxid().into(),
            deadline,
        )
        .map_err(store_error)?
        .with_native_gate({
            let gate = gate.clone();
            move || gate()
        })
        .map_err(store_error)?;
        #[cfg(feature = "resource-adoption")]
        let registry = match &self.store {
            ManagementStore::Adopted(s) => s
                .native_owner_cleanup_registry(&profile)
                .map_err(store_error)?,
            _ => return Err(failure(409, "owner_stop_local_custody_required")),
        };
        #[cfg(not(feature = "resource-adoption"))]
        {
            let _ = profile;
            return Err(failure(409, "owner_stop_local_custody_required"));
        }
        #[cfg(feature = "resource-adoption")]
        {
            self.owner
                .adopted_management_selection(registry.clone())
                .await?
                .check_owner_stop(&self.owner)
                .await?;
            let manager = self
                .owner
                .management(ManagementPackage {
                    digest: self.package.digest.clone(),
                    instance: self.package.instance,
                    account_epoch: self.package.account_epoch,
                    permissions: vec![service::Permission::ChangeAgentRunControl],
                })
                .await?;
            if !gate() {
                manager.retire();
                return Err(failure(401, "management_scope_changed"));
            }
            Ok((
                super::shared_execution::SharedExecutionTransport::owner_stop_only(manager, gate),
                registry,
            ))
        }
    }
}
