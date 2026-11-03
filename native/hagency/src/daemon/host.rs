//! Process-level entry points: run the daemon, enroll an owner, probe a
//! running daemon, roll back a handover and sign out. Used by the `hagency
//! daemon` CLI and by Hagency Desktop's startup/enrollment integration.
pub use super::enrollment_rpc::EnrollmentControl;
use super::{
    DaemonHome, HomeError,
    auth::{AuthError, SdkCustody},
    control::ControlStore,
    dispatch::{Controller, Runtime},
    enrollment::{Enrollment, EnrollmentError, Handover},
    ipc::{DaemonClient, IpcError, Server, new_request_id},
    protocol::{Command, Hello, Request},
    runtime::OwnerRuntime,
};
use crate::native_owner::NativeOwner;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error(transparent)]
    Home(#[from] HomeError),
    #[error("{0}")]
    Code(&'static str),
    #[error(transparent)]
    Ipc(#[from] IpcError),
}

impl From<EnrollmentError> for HostError {
    fn from(e: EnrollmentError) -> Self {
        Self::Code(e.code())
    }
}
impl From<AuthError> for HostError {
    fn from(e: AuthError) -> Self {
        Self::Code(e.code())
    }
}

/// `~/Library/Application Support/HagencyOwnerDaemon` on macOS, else
/// `$XDG_STATE_HOME/hagency-owner-daemon` or `~/.local/state/...`.
pub fn default_home() -> Result<PathBuf, HostError> {
    crate::service::default_daemon_dir().map_err(|_| HostError::Code("daemon_home_unavailable"))
}

/// Run until `shutdown` fires. Holds the daemon home lock for the whole
/// process; a second instance fails with `AlreadyRunning`. On shutdown the
/// hosting intent is preserved and the owner is suspended, not signed out.
pub async fn run(
    home: &Path,
    shutdown: hagency_matrix::CancellationToken,
) -> Result<(), HostError> {
    let home = DaemonHome::open(home)?;
    let store = ControlStore::open(&home.control()).map_err(|e| HostError::Code(e.code))?;
    let runtime = Arc::new(OwnerRuntime::new(home.path().into()).map_err(HostError::Code)?);
    let controller = Controller::new(store, runtime);
    controller
        .runtime()
        .process_revocation()
        .await
        .map_err(HostError::Code)?;
    controller.boot().await;
    let mut server = Server::start(home.path(), controller.clone())?;
    tracing::info!("Hagency owner daemon is serving its private control socket");
    let observer = {
        let controller = controller.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let _ = controller.runtime().process_revocation().await;
                controller.observe_transition();
            }
        })
    };
    shutdown.cancelled().await;
    observer.abort();
    controller.begin_shutdown();
    // Keep the private home lock, original task/native handles, and read-only
    // status socket while a graceful drain needs recovery. A failed drain is
    // never permission to release custody to another process.
    loop {
        let result=match controller.runtime().drain_enrollment(false).await {
            Ok(())=>controller.shutdown().await,
            Err(code)=>Err(code),
        };
        match result {
            Ok(())=>break,
            Err(code)=>{
                tracing::warn!(code,"Daemon shutdown is waiting for the original drain");
                controller.observe_transition();
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    server.stop().await;
    drop(home);
    Ok(())
}

/// Result of a probe. Desktop must not start its own executor for an owner
/// root while a daemon reports that owner.
#[derive(Debug)]
pub enum Presence {
    NotRunning,
    Running(Hello),
    /// The socket exists but is not private or the peer is another user.
    Unsafe,
}

pub async fn probe(home: &Path) -> Presence {
    let mut client = match DaemonClient::connect(home).await {
        Ok(client) => client,
        Err(IpcError::Unsafe | IpcError::PeerRejected) => return Presence::Unsafe,
        Err(_) => return Presence::NotRunning,
    };
    let request = Request::new(new_request_id(), None, Command::Hello);
    match client
        .call(&request)
        .await
        .ok()
        .and_then(|reply| reply.hello().ok())
    {
        Some(hello) => Presence::Running(hello),
        None => Presence::NotRunning,
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EnrollReport {
    pub owner: String,
    pub owner_root: PathBuf,
    /// The owner root was opened and authorized during enrollment, which
    /// completes native device registration under the daemon's SDK client.
    pub registered: bool,
    /// Static code when registration must be retried later.
    pub pending: Option<String>,
}

/// Explicit enrollment. Performs a fresh browser sign-in for the daemon's own
/// SDK device, records the journal, then opens the owner root in-process so
/// the one-time explicit sign-in permission is consumed by native device
/// registration immediately. Requires that no daemon runs on this home.
///
/// `adopt_root` names Hagency Desktop's existing owner root (handover);
/// Desktop must already have drained and released it. Without it a fresh
/// root is created under the daemon home.
pub async fn enroll<F, Fut>(
    home: &Path,
    origin: &str,
    expected_owner: Option<&str>,
    display_name: Option<String>,
    adopt_root: Option<&Path>,
    open_url: F,
) -> Result<EnrollReport, HostError>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), ()>>,
{
    enroll_controlled(
        home,
        origin,
        expected_owner,
        display_name,
        adopt_root,
        Arc::new(EnrollmentControl::default()),
        open_url,
    )
    .await
}

pub async fn enroll_controlled<F, Fut>(
    home: &Path,
    origin: &str,
    expected_owner: Option<&str>,
    display_name: Option<String>,
    adopt_root: Option<&Path>,
    control: Arc<EnrollmentControl>,
    open_url: F,
) -> Result<EnrollReport, HostError>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), ()>>,
{
    match probe(home).await {
        Presence::Running(_) => {
            return tokio::select! {
                result=super::enrollment_rpc::remote_enroll(home,origin,expected_owner,display_name,adopt_root,open_url)=>result,
                _=control.cancelled()=>Err(HostError::Code("daemon_enrollment_detached")),
            };
        }
        Presence::Unsafe => return Err(HostError::Code("daemon_socket_unsafe")),
        Presence::NotRunning => {}
    }
    let held = DaemonHome::open(home)?;
    enroll_locked_controlled(
        held.path(),
        origin,
        expected_owner,
        display_name,
        adopt_root,
        control,
        open_url,
    )
    .await
}

pub(super) async fn enroll_locked<F, Fut>(
    home: &Path,
    origin: &str,
    expected_owner: Option<&str>,
    display_name: Option<String>,
    adopt_root: Option<&Path>,
    open_url: F,
) -> Result<EnrollReport, HostError>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), ()>>,
{
    enroll_locked_controlled(
        home,
        origin,
        expected_owner,
        display_name,
        adopt_root,
        Arc::new(EnrollmentControl::default()),
        open_url,
    )
    .await
}

pub(super) async fn enroll_locked_controlled<F, Fut>(
    home: &Path,
    origin: &str,
    expected_owner: Option<&str>,
    display_name: Option<String>,
    adopt_root: Option<&Path>,
    control: Arc<EnrollmentControl>,
    open_url: F,
) -> Result<EnrollReport, HostError>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), ()>>,
{
    if !control.accepting() {
        return Err(HostError::Code("daemon_enrollment_cancelled"));
    }
    if super::revocation::load(home)
        .map_err(HostError::Code)?
        .is_some()
    {
        return Err(HostError::Code("daemon_logout_pending"));
    }
    if let Some(existing) = Enrollment::load(home)?
        && expected_owner.is_some_and(|o| o != existing.owner)
    {
        return Err(EnrollmentError::OtherOwner.into());
    }
    let public_client = match adopt_root {
        Some(root) => {
            let owner = expected_owner.ok_or(HostError::Code("daemon_handover_owner_required"))?;
            Some(
                NativeOwner::matrix_resume_client_id(root, origin, owner)
                    .map_err(|error| HostError::Code(registration_code(&error.code)))?,
            )
        }
        None => None,
    };
    if let Some(root) = adopt_root {
        let owner = expected_owner.ok_or(HostError::Code("daemon_handover_owner_required"))?;
        Enrollment::stage_handover(home, origin, owner, display_name.clone(), root)?;
    }
    // Pin prospective custody before OAuth when the Desktop has supplied its
    // authenticated owner. This makes a concurrent explicit logout durable
    // even before the browser has returned its independent daemon grant.
    if adopt_root.is_none() {
        if let Some(owner) = expected_owner {
            Enrollment::enroll_fresh(home, origin, owner, display_name.clone())?;
        }
    }
    let directory = home.join(super::MATRIX);
    let custody = tokio::select! {
        biased;
        _=control.cancelled()=>return Err(HostError::Code("daemon_enrollment_cancelled")),
        result=SdkCustody::reauthenticate(&directory,origin,expected_owner,public_client.as_deref(),open_url)=>result?,
    };
    let owner = custody.owner().to_owned();
    let mut enrollment = match adopt_root {
        Some(root) => Enrollment::stage_handover(home, origin, &owner, display_name, root)?,
        None => Enrollment::enroll_fresh(home, origin, &owner, display_name)?,
    };
    if matches!(enrollment.handover, Handover::Released { .. }) {
        // This explicit enrollment is the new handover attempt; an ordinary
        // daemon restart can never undo a Released rollback.
        enrollment = Enrollment::restage(home)?;
    }
    if super::revocation::load(home)
        .map_err(HostError::Code)?
        .is_some()
    {
        custody.retire();
        return Err(HostError::Code("daemon_logout_pending"));
    }
    control.accept_native().map_err(HostError::Code)?;
    control.set_custody(custody.clone());
    let opened = open_for_registration(&enrollment, &custody, control.clone()).await;
    let report = match opened {
        Ok(native) => {
            if super::revocation::load(home)
                .map_err(HostError::Code)?
                .is_some()
            {
                control.drain(true).await.map_err(HostError::Code)?;
                return Err(HostError::Code("daemon_logout_pending"));
            }
            let _ = native.identity().await;
            control.drain(false).await.map_err(HostError::Code)?;
            if matches!(enrollment.handover, Handover::Fresh) {
                Enrollment::mark_active(home)?;
            }
            EnrollReport {
                owner,
                owner_root: enrollment.owner_root,
                registered: true,
                pending: None,
            }
        }
        Err(code) => EnrollReport {
            owner,
            owner_root: enrollment.owner_root,
            registered: false,
            pending: Some(code.into()),
        },
    };
    control.drain(false).await.map_err(HostError::Code)?;
    custody.retire();
    Ok(report)
}

async fn open_for_registration(
    enrollment: &Enrollment,
    custody: &Arc<SdkCustody>,
    control: Arc<EnrollmentControl>,
) -> Result<NativeOwner, &'static str> {
    let root = &enrollment.owner_root;
    #[cfg(feature = "resource-adoption")]
    {
        use crate::native_owner::AdoptedManagementSelection;
        let selection = if root.exists() {
            AdoptedManagementSelection::load_for_host(root, &enrollment.origin, &enrollment.owner)
                .await
                .map_err(|_| "adopted_management_initializer_unavailable")?
        } else {
            None
        };
        if let Some(selection) = selection {
            return selection
                .open_for_onboarding_with_matrix_guarded(
                    root,
                    &enrollment.origin,
                    &enrollment.owner,
                    custody.tokens(),
                    move |owner| {
                        control.register(owner);
                        Ok(())
                    },
                )
                .await
                .map_err(|e| registration_code(&e.code));
        }
    }
    NativeOwner::open_with_matrix_guarded(
        root,
        &enrollment.origin,
        &enrollment.owner,
        custody.tokens(),
        move |owner| {
            control.register(owner);
            Ok(())
        },
    )
    .await
    .map_err(|e| registration_code(&e.code))
}

fn registration_code(code: &str) -> &'static str {
    match code {
        "native_owner_state_unavailable" => "owner_root_busy_or_invalid",
        "matrix_account_mismatch" => "matrix_account_mismatch",
        "native_resume_missing" => "native_resume_missing",
        "native_resume_revoked" => "native_resume_revoked",
        "native_device_registration_unknown" => "native_device_registration_unknown",
        "adopted_management_scope_changed" => "adopted_management_scope_changed",
        "server_unavailable" | "matrix_authorization_unavailable" => "server_unavailable",
        _ => "native_registration_failed",
    }
}

/// Handover rollback. Requires the daemon to be stopped (this takes the
/// home lock). Afterwards the daemon will not open the root again until it
/// is restaged, so Desktop may resume hosting it.
pub fn release(home: &Path) -> Result<Enrollment, HostError> {
    let home = DaemonHome::open(home)?;
    Ok(Enrollment::release(home.path())?)
}

/// Explicit owner sign-out while the daemon is stopped. Durable native
/// revocation precedes the drain; the SDK grant is revoked last.
pub async fn sign_out(home: &Path) -> Result<(), HostError> {
    if let Presence::Running(_) = probe(home).await {
        let enrollment = Enrollment::load(home)?.ok_or(EnrollmentError::Missing)?;
        super::revocation::request(home, &enrollment.origin, &enrollment.owner)
            .map_err(HostError::Code)?;
        let client = super::bridge::ControllerClient::new(
            home.into(),
            enrollment.owner,
            enrollment.origin,
            1,
        );
        let result = client
            .sign_out()
            .await
            .map_err(|_| HostError::Code("daemon_logout_pending"));
        client.retire();
        return result;
    }
    let home = DaemonHome::open(home)?;
    let runtime = OwnerRuntime::new(home.path().into()).map_err(HostError::Code)?;
    if runtime.owner().is_none() {
        return Err(EnrollmentError::Missing.into());
    }
    runtime.sign_out().await.map_err(HostError::Code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short_home() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("hh")
            .tempdir_in("/tmp")
            .unwrap()
    }

    #[tokio::test]
    async fn run_serves_an_unenrolled_daemon_and_detaching_controllers_change_nothing() {
        let temp = short_home();
        let home = temp.path().join("d");
        let shutdown = hagency_matrix::CancellationToken::new();
        let task = {
            let (home, shutdown) = (home.clone(), shutdown.clone());
            tokio::spawn(async move { run(&home, shutdown).await })
        };
        let mut hello = None;
        for _ in 0..200 {
            if let Presence::Running(h) = probe(&home).await {
                hello = Some(h);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let hello = hello.expect("daemon never answered");
        assert_eq!(hello.owner_id, None);
        assert_eq!(
            hello.hosting,
            hagency_metering::dashboard::HostingState::Unconfigured
        );
        // A second daemon on the same home is refused while the first runs.
        assert!(matches!(
            run(&home, hagency_matrix::CancellationToken::new()).await,
            Err(HostError::Home(HomeError::AlreadyRunning))
        ));
        // Enrollment and rollback also require the home lock.
        assert!(matches!(
            release(&home),
            Err(HostError::Home(HomeError::AlreadyRunning))
        ));
        // Controllers attach and detach freely.
        for _ in 0..3 {
            let mut client = DaemonClient::connect(&home).await.unwrap();
            let reply = client
                .call(&Request::new(
                    new_request_id(),
                    None,
                    Command::Snapshot {
                        period: None,
                        usage_filter: None,
                    },
                ))
                .await
                .unwrap();
            assert_eq!(
                reply.snapshot().unwrap().hosting.state,
                hagency_metering::dashboard::HostingState::Unconfigured
            );
        }
        assert!(matches!(probe(&home).await, Presence::Running(_)));
        shutdown.cancel();
        task.await.unwrap().unwrap();
        assert!(matches!(probe(&home).await, Presence::NotRunning));
        assert!(!home.join(super::super::ipc::SOCKET).exists());
    }
}
