//! Private owner history; a historical grant cannot re-enter the live queue.
use super::{NativeError, NativeOwner, failure, ledger};
use crate::console::{device_execution::DeviceIdentity, owned_agents};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HistoryAction {
    Approvals {
        agent: String,
        cursor: Option<String>,
    },
    Approval {
        agent: String,
        id: String,
    },
}
pub(super) async fn execute(
    native: &NativeOwner,
    action: HistoryAction,
) -> Result<Value, NativeError> {
    let identity = native.identity().await?;
    let device = native
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    let pin = DeviceIdentity::from(&device);
    if pin.owner != identity.owner
        || pin.subject != identity.subject
        || pin.issuer != identity.issuer
        || pin.origin != identity.origin
    {
        return Err(failure(401, "owner_authorization_required"));
    }
    let profile = owned_agents::profile_identity(
        &identity.origin,
        &identity.issuer,
        &identity.subject,
        &identity.owner,
    )
    .map_err(|_| failure(403, "owner_scope_required"))?;
    let agent = match &action {
        HistoryAction::Approvals { agent, .. } | HistoryAction::Approval { agent, .. } => agent,
    };
    super::key(agent)?;
    let active = native
        .0
        .host
        .console
        .0
        .owned_runtime
        .agent_busy(&device, agent)
        .await;
    device
        .bearer()
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    let mut ledger = ledger::local_ledger(native, &identity, &profile)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|t| t.as_secs().try_into().ok())
        .ok_or_else(|| failure(503, "local_state_unavailable"))?;
    ledger
        .close_approval_audits(&identity.owner, agent, !active, now)
        .map_err(local_error)?;
    let result = match action {
        HistoryAction::Approvals { agent, cursor } => {
            json!({"agentId":agent,"approvals":ledger.approval_history(&identity.owner,&agent,cursor.as_deref()).map_err(local_error)?,"runtimeActive":active})
        }
        HistoryAction::Approval { agent, id } => {
            json!({"agentId":agent,"approval":ledger.approval_record(&identity.owner,&agent,&id).map_err(local_error)?,"runtimeActive":active})
        }
    };
    let fresh = native
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    if DeviceIdentity::from(&fresh) != pin || native.identity().await? != identity {
        return Err(failure(401, "owner_authorization_required"));
    }
    Ok(result)
}
fn local_error(error: hagency_agent_local::Error) -> NativeError {
    match error {
        hagency_agent_local::Error::Unauthorized => failure(403, "owner_scope_required"),
        hagency_agent_local::Error::Invalid(_) => failure(400, "invalid_history_query"),
        _ => failure(503, "owner_history_unavailable"),
    }
}
