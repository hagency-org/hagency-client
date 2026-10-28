//! Native owner management reuses the authenticated client service in process.
pub use crate::console::native::*;
pub use hagency_agent_local::remote_approval;

/// Desktop entry point preserving the original Matrix authorization boundary.
pub async fn open_with_matrix(
    state: &std::path::Path,
    origin: &str,
    owner: &str,
    source: std::sync::Arc<dyn MatrixTokenSource>,
) -> Result<NativeOwner, NativeError> {
    crate::console::native::NativeOwner::open_with_matrix(state, origin, owner, source).await
}

/// Private process-custody entry. The guardian validates its inherited handles;
/// this does not launch a provider or accept owner commands from argv.
pub fn run_guardian_if_requested() -> Option<std::io::Result<()>> {
    #[cfg(unix)]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("guardian")) {
        return Some(hagency_platform::run_guardian());
    }
    None
}
