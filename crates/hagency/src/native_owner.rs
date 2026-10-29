//! Native owner management reuses the authenticated client service in process.
pub use crate::console::native::*;
pub use hagency_agent_local::remote_approval;

/// Validate an Agent's explicit MCP configuration without starting a process,
/// opening a connection, or importing the owner's ambient provider settings.
pub fn validate_mcp_servers(servers: &serde_json::Value) -> Result<(), String> {
    let programming: hagency_agent_local::codex::Programming =
        serde_json::from_value(serde_json::json!({"enabled": true, "mcp_servers": servers}))
            .map_err(|error| error.to_string())?;
    programming.validate().map_err(|error| error.to_string())
}

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
