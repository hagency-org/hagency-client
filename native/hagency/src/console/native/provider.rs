//! Native provider dispatch. Only trusted host code uses these operations;
//! mini apps receive separately filtered projections, never paths/auth URLs.
use super::{Command, NativeError, ProviderAction, failure, key};
use crate::console::owner_provider::Operation;

pub(super) fn operation(
    command: &Command,
) -> Result<Option<(Option<&str>, Operation)>, NativeError> {
    let (runtime, action) = match command {
        Command::Provider { action } => (None, action),
        Command::RuntimeProvider { runtime, action } => {
            if !matches!(runtime.as_str(), "claude" | "octos") {
                return Err(failure(400, "invalid_arguments"));
            }
            if !matches!(
                action,
                ProviderAction::Status
                    | ProviderAction::Models
                    | ProviderAction::UseLocal
                    | ProviderAction::Disconnect
            ) {
                return Err(failure(400, "local_provider_login_managed_locally"));
            }
            (Some(runtime.as_str()), action)
        }
        Command::PrepareAgentWorkspace { agent_id } => {
            key(agent_id)?;
            return Ok(Some((
                None,
                Operation::PrepareWorkspace {
                    agent: agent_id.clone(),
                },
            )));
        }
        _ => return Ok(None),
    };
    Ok(Some((
        runtime,
        match action {
            ProviderAction::UseLocal => Operation::UseLocal,
            ProviderAction::Disconnect => Operation::Disconnect,
            ProviderAction::Models => Operation::Models,
            ProviderAction::Status => Operation::Status,
            ProviderAction::Login => Operation::Login,
            ProviderAction::Cancel => Operation::Cancel,
            ProviderAction::Logout => Operation::Logout,
        },
    )))
}
