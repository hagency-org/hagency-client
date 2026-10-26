//! Reports and acknowledgements are local to the authenticated owner/device.
use super::{NativeError, NativeOwner, UsageQuery, failure, ledger};
use crate::console::{device_execution::DeviceIdentity, owned_agents};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UsageAction {
    Report { query: UsageQuery },
    Acknowledge { query: UsageQuery, id: String },
}
pub(super) async fn execute(
    native: &NativeOwner,
    action: UsageAction,
) -> Result<Value, NativeError> {
    let query = match &action {
        UsageAction::Report { query } | UsageAction::Acknowledge { query, .. } => query,
    };
    query.validate().map_err(local_error)?;
    let device = native
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    let identity = native.identity().await?;
    let pin = DeviceIdentity::from(&device);
    if pin.origin != identity.origin
        || pin.issuer != identity.issuer
        || pin.subject != identity.subject
        || pin.owner != identity.owner
    {
        return Err(failure(401, "owner_authorization_required"));
    }
    device
        .bearer()
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    let profile = owned_agents::profile_identity(
        &identity.origin,
        &identity.issuer,
        &identity.subject,
        &identity.owner,
    )
    .map_err(|_| failure(403, "owner_scope_required"))?;
    let mut ledger = ledger::local_ledger(native, &identity, &profile)?;
    let query = match action {
        UsageAction::Report { query } => query,
        UsageAction::Acknowledge { query, id } => {
            ledger
                .acknowledge_usage_alert(&identity.owner, &query.agent, &id, now()?)
                .map_err(local_error)?;
            query
        }
    };
    let report = ledger
        .usage_report(&identity.owner, &query, now()?)
        .map_err(local_error)?;
    device
        .bearer()
        .map_err(|_| failure(401, "owner_authorization_required"))?;
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
    native.check_matrix_source().await?;
    serde_json::to_value(report).map_err(|_| failure(503, "usage_report_unavailable"))
}
fn now() -> Result<i64, NativeError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|t| t.as_secs().try_into().ok())
        .ok_or_else(|| failure(503, "usage_report_unavailable"))
}
fn local_error(error: hagency_agent_local::Error) -> NativeError {
    match error {
        hagency_agent_local::Error::Invalid(_) => failure(400, "invalid_usage_query"),
        hagency_agent_local::Error::Unauthorized => failure(403, "owner_scope_required"),
        _ => failure(503, "usage_report_unavailable"),
    }
}
