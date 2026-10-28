//! Closed direct-pool contract. Only the selected resource owner decides.
use super::agents::AgentDefinition;
use super::{ManagementCommand, NativeError, ServerOperation, failure};
use hagency_store::management::IntentOperation;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PoolAgentInput {
    pub room_id: String,
    pub expected_pool_revision: i64,
    pub expected_grant_revision: i64,
    pub definition: AgentDefinition,
    pub requested_tokens: i64,
}
impl PoolAgentInput {
    pub fn validate(&self) -> Result<(), NativeError> {
        self.definition.validate()?;
        if !self.room_id.starts_with('!')
            || !self.room_id.contains(':')
            || self.room_id.len() > 255
            || self
                .room_id
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(failure(400, "invalid_pool_agent_room"));
        }
        for n in [
            self.expected_pool_revision,
            self.expected_grant_revision,
            self.requested_tokens,
        ] {
            positive(n)?;
        }
        Ok(())
    }
}
fn positive(n: i64) -> Result<(), NativeError> {
    if (1..=9_007_199_254_740_991).contains(&n) {
        Ok(())
    } else {
        Err(failure(400, "invalid_management_revision"))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pool {
    id: String,
    label: String,
    revision: i64,
    owner_mxid: String,
    runtime: String,
    capabilities: Vec<String>,
    grant_revision: Option<i64>,
    published_tokens: Option<i64>,
    native_available_tokens: Option<i64>,
    can_request: bool,
    blocking_reason: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    id: String,
    pool_id: String,
    pool_label: String,
    resource_owner_mxid: String,
    manager_mxid: String,
    agent_id: String,
    room_id: String,
    definition: AgentDefinition,
    requested_tokens: i64,
    period_key: String,
    assignment_id: Option<String>,
    membership_state: Option<String>,
    runtime_state: Option<String>,
    reservation_state: Option<String>,
    reservation_reason: Option<String>,
    state: String,
    revision: i64,
    created_at_ms: i64,
    expires_at_ms: i64,
    can_decide: bool,
    can_cancel: bool,
    can_stop_supply: bool,
    blocking_reason: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    protocol: String,
    observed_at_ms: i64,
    items: Vec<T>,
    next_after: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    request: Request,
    observed_at_ms: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recovery {
    recorded: bool,
    request: Option<Request>,
    observed_at_ms: i64,
}
pub(in crate::console::native) fn is_operation(op: IntentOperation) -> bool {
    matches!(
        op,
        IntentOperation::PoolAgentRequest
            | IntentOperation::PoolAgentDecide
            | IntentOperation::PoolAgentStop
    )
}
pub(super) fn prepare(command: &ManagementCommand) -> Result<ServerOperation, NativeError> {
    let key = super::super::key;
    let (method, suffix, input) = match command {
        ManagementCommand::Pools { after, capability } => {
            if let Some(id) = after {
                key(id)?;
            }
            (
                Method::POST,
                "pools/query".into(),
                Some(json!({"after":after,"capability":capability})),
            )
        }
        ManagementCommand::PoolAgents {
            after,
            pool_id,
            needs_my_action,
        } => {
            for id in [after, pool_id].into_iter().flatten() {
                key(id)?;
            }
            (
                Method::POST,
                "pool-agent-requests/query".into(),
                Some(json!({"after":after,"poolId":pool_id,"needsMyAction":needs_my_action})),
            )
        }
        ManagementCommand::PoolAgent { id } => {
            key(id)?;
            (Method::GET, format!("pool-agent-requests/{id}"), None)
        }
        ManagementCommand::PoolAgentRequest {
            command_id,
            pool_id,
            request,
        } => {
            key(command_id)?;
            key(pool_id)?;
            request.validate()?;
            let mut body = serde_json::to_value(request)
                .map_err(|_| failure(400, "invalid_management_arguments"))?;
            body["commandId"] = json!(command_id);
            (Method::POST, format!("pools/{pool_id}/agents"), Some(body))
        }
        ManagementCommand::PoolAgentDecide {
            command_id,
            request_id,
            expected_revision,
            approve,
        } => {
            key(command_id)?;
            key(request_id)?;
            positive(*expected_revision)?;
            (
                Method::POST,
                format!("pool-agent-requests/{request_id}/decide"),
                Some(
                    json!({"commandId":command_id,"expectedRevision":expected_revision,"decision":if *approve{"approve"}else{"reject"}}),
                ),
            )
        }
        ManagementCommand::PoolAgentStop {
            command_id,
            request_id,
            expected_revision,
        } => {
            key(command_id)?;
            key(request_id)?;
            positive(*expected_revision)?;
            (
                Method::POST,
                format!("pool-agent-requests/{request_id}/stop"),
                Some(json!({"commandId":command_id,"expectedRevision":expected_revision})),
            )
        }
        _ => return Err(failure(400, "invalid_management_operation")),
    };
    Ok(ServerOperation {
        method,
        suffix,
        input,
    })
}
fn typed<T: serde::de::DeserializeOwned + Serialize>(v: Value) -> Result<Value, NativeError> {
    serde_json::from_value::<T>(v)
        .and_then(serde_json::to_value)
        .map_err(|_| failure(503, "management_pool_reply_invalid"))
}
pub(in crate::console::native) fn project_reply(
    command: &ManagementCommand,
    v: Value,
) -> Result<Value, NativeError> {
    if matches!(
        command,
        ManagementCommand::Pools { .. } | ManagementCommand::PoolAgents { .. }
    ) && v["items"].as_array().is_none_or(|a| a.len() > 16)
    {
        return Err(failure(503, "management_pool_reply_invalid"));
    }
    match command {
        ManagementCommand::Pools { .. } => typed::<Page<Pool>>(v),
        ManagementCommand::PoolAgents { .. } => typed::<Page<Request>>(v),
        ManagementCommand::PoolAgent { .. }
        | ManagementCommand::PoolAgentRequest { .. }
        | ManagementCommand::PoolAgentDecide { .. }
        | ManagementCommand::PoolAgentStop { .. } => typed::<Reply>(v),
        ManagementCommand::Recover {
            original_operation, ..
        } if is_operation(*original_operation) => typed::<Recovery>(v),
        _ => Ok(v),
    }
}
pub(in crate::console::native) fn known_failure(e: &NativeError) -> bool {
    matches!(
        e.code.as_str(),
        "invalid_pool_agent_room"
            | "invalid_managed_agent_definition"
            | "invalid_management_revision"
            | "management_revision_changed"
            | "pool_capacity_unavailable"
            | "pool_not_shared_with_account"
            | "pool_agent_resource_mismatch"
            | "pool_resource_owner_required"
            | "pool_agent_request_closed"
            | "pool_agent_authority_changed"
            | "pool_agent_request_capacity"
            | "management_receipt_capacity"
    )
}
