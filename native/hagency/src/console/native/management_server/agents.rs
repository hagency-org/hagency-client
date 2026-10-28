//! Human project-agent admission only. No executor, provider or room authority.
use super::{ManagementCommand, NativeError, ServerOperation, failure};
use hagency_store::management::IntentOperation;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const AGENT_ADMISSION_FEATURE: &str = "agent-admission-v1";
const MAX_EXACT: i64 = 9_007_199_254_740_991;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDefinition {
    pub name: String,
    pub capability: String,
    pub description: String,
    pub tool_mode: String,
}
impl AgentDefinition {
    pub fn validate(&self) -> Result<(), NativeError> {
        if self.name.trim().is_empty()
            || self.name.len() > 160
            || self.name.chars().any(char::is_control)
            || self.description.len() > 2000
            || self
                .description
                .chars()
                .any(|c| c.is_control() && c != '\n')
            || self.capability.is_empty()
            || self.capability.len() > 80
            || !self
                .capability
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-/".contains(&b))
            || self.tool_mode != "none"
        {
            return Err(failure(400, "invalid_managed_agent_definition"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRequestInput {
    pub expected_setup_revision: i64,
    pub expected_setup_binding_revision: i64,
    pub expected_authority_revision: i64,
    pub expected_allocation_revision: i64,
    pub definition: AgentDefinition,
    pub requested_tokens: i64,
}
impl AgentRequestInput {
    pub fn validate(&self) -> Result<(), NativeError> {
        self.definition.validate()?;
        for value in [
            self.expected_setup_revision,
            self.expected_setup_binding_revision,
            self.expected_authority_revision,
            self.expected_allocation_revision,
            self.requested_tokens,
        ] {
            positive(value)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRequestContext {
    pub setup_id: String,
    pub setup_revision: i64,
    pub setup_binding_revision: i64,
    pub authority_revision: i64,
    pub allocation_id: String,
    pub allocation_revision: i64,
    pub allocation_receipt_digest: String,
    pub requested_token_limit: i64,
    pub native_available_tokens: Option<i64>,
    pub period_key: String,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
    pub capabilities: Vec<String>,
    pub tool_modes: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRequestReadiness {
    pub protocol: String,
    pub observed_at_ms: i64,
    pub project_id: String,
    pub project_name: String,
    pub manager_mxid: String,
    pub administrator_mxid: Option<String>,
    pub can_view_requests: bool,
    pub can_request: bool,
    pub blocking_reason: Option<String>,
    pub request_context: Option<AgentRequestContext>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRequest {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub project_name: Option<String>,
    pub proposed_agent_id: String,
    pub manager_mxid: String,
    pub administrator_mxid: String,
    pub authority_revision: i64,
    pub definition: AgentDefinition,
    pub definition_digest: String,
    pub requested_tokens: i64,
    pub period_key: String,
    pub state: String,
    pub revision: i64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub can_decide: bool,
    pub can_approve: bool,
    pub can_reject: bool,
    pub can_cancel: bool,
    pub blocking_reason: Option<String>,
    pub execution_available: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    protocol: String,
    observed_at_ms: i64,
    items: Vec<AgentRequest>,
    next_after: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    agent_request: AgentRequest,
    observed_at_ms: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recovery {
    recorded: bool,
    agent_request: Option<AgentRequest>,
    observed_at_ms: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Readiness {
    readiness: AgentRequestReadiness,
    observed_at_ms: i64,
}
fn positive(value: i64) -> Result<(), NativeError> {
    if !(1..=MAX_EXACT).contains(&value) {
        return Err(failure(400, "invalid_management_revision"));
    }
    Ok(())
}
pub(in crate::console::native) fn is_operation(op: IntentOperation) -> bool {
    matches!(
        op,
        IntentOperation::AgentRequest | IntentOperation::AgentDecide | IntentOperation::AgentCancel | IntentOperation::AgentRevoke
    )
}
pub(super) fn prepare(command: &ManagementCommand) -> Result<ServerOperation, NativeError> {
    let key = super::super::key;
    let (method, suffix, input) = match command {
        ManagementCommand::AgentRemoval { .. } | ManagementCommand::AgentRemovals { .. } => return super::agent_cleanup::prepare(command),
        ManagementCommand::AgentReadiness { project_id } => {
            key(project_id)?;
            (
                Method::GET,
                format!("projects/{project_id}/agent-readiness"),
                None,
            )
        }
        ManagementCommand::AgentRequests {
            project_id,
            after,
            needs_my_action,
        } => {
            for value in [project_id, after].into_iter().flatten() {
                key(value)?;
            }
            (
                Method::POST,
                "agent-requests/list".into(),
                Some(json!({"projectId":project_id,"after":after,"needsMyAction":needs_my_action})),
            )
        }
        ManagementCommand::Agent { id } => {
            key(id)?;
            (Method::GET, format!("agent-requests/{id}"), None)
        }
        ManagementCommand::AgentAssignment { id }
        | ManagementCommand::AgentMembership { id }
        | ManagementCommand::AgentRuntime { id }
        | ManagementCommand::AgentExecution { id } => {
            key(id)?;
            let stage = match command {
                ManagementCommand::AgentAssignment { .. } => "assignment",
                ManagementCommand::AgentMembership { .. } => "matrix",
                ManagementCommand::AgentExecution { .. } => "execution",
                _ => "runtime",
            };
            (Method::GET, format!("agent-requests/{id}/{stage}"), None)
        }
        ManagementCommand::AgentWork { .. } | ManagementCommand::AgentWorkList { .. } => return super::agent_work::prepare(command),
        ManagementCommand::AgentRequest {
            command_id,
            project_id,
            request,
        } => {
            key(command_id)?;
            key(project_id)?;
            request.validate()?;
            let mut body = serde_json::to_value(request)
                .map_err(|_| failure(400, "invalid_management_arguments"))?;
            body["commandId"] = json!(command_id);
            (
                Method::POST,
                format!("projects/{project_id}/agent-requests"),
                Some(body),
            )
        }
        ManagementCommand::AgentDecide {
            command_id,
            request_id,
            expected_revision,
            expected_authority_revision,
            approve,
        } => {
            key(command_id)?;
            key(request_id)?;
            positive(*expected_revision)?;
            positive(*expected_authority_revision)?;
            (
                Method::POST,
                format!("agent-requests/{request_id}/decide"),
                Some(
                    json!({"commandId":command_id,"expectedRevision":expected_revision,"expectedAuthorityRevision":expected_authority_revision,"decision":if *approve{"approve"}else{"reject"}}),
                ),
            )
        }
        ManagementCommand::AgentCancel { command_id, request_id, expected_revision }
        | ManagementCommand::AgentRevoke { command_id, request_id, expected_revision } => {
            key(command_id)?;
            key(request_id)?;
            positive(*expected_revision)?;
            (
                Method::POST,
                format!("agent-requests/{request_id}/{}", if matches!(command,ManagementCommand::AgentRevoke{..}) {"revoke"} else {"cancel"}),
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
fn typed<T: for<'a> Deserialize<'a> + Serialize>(value: Value) -> Result<Value, NativeError> {
    let decoded: T = serde_json::from_value(value)
        .map_err(|_| failure(503, "management_agent_reply_invalid"))?;
    serde_json::to_value(decoded).map_err(|_| failure(503, "management_agent_reply_invalid"))
}
pub(super) fn project_reply(
    command: &ManagementCommand,
    value: Value,
) -> Result<Value, NativeError> {
    match command {
        ManagementCommand::AgentWork { .. } | ManagementCommand::AgentWorkList { .. } => super::agent_work::project_reply(command, value),
        ManagementCommand::AgentRemoval { .. } | ManagementCommand::AgentRemovals { .. } => super::agent_cleanup::project_reply(command,value),
        ManagementCommand::AgentAssignment { .. }
        | ManagementCommand::AgentMembership { .. }
        | ManagementCommand::AgentRuntime { .. }
        | ManagementCommand::AgentExecution { .. } => {
            super::agent_status::project_reply(command, value)
        }
        ManagementCommand::AgentReadiness { .. } => typed::<Readiness>(value),
        ManagementCommand::AgentRequests { .. } => {
            if value["items"]
                .as_array()
                .is_none_or(|items| items.len() > 16)
            {
                return Err(failure(503, "management_agent_reply_invalid"));
            }
            typed::<Page>(value)
        }
        ManagementCommand::Agent { .. }
        | ManagementCommand::AgentRequest { .. }
        | ManagementCommand::AgentDecide { .. }
        | ManagementCommand::AgentCancel { .. }
        | ManagementCommand::AgentRevoke { .. } => typed::<Reply>(value),
        ManagementCommand::Recover {
            original_operation, ..
        } if is_operation(*original_operation) => typed::<Recovery>(value),
        _ => Ok(value),
    }
}
/// Only explicit pre-effect codes are definitive, and only on the first POST.
/// HTTP/auth/encoding/transport status alone cannot establish non-application.
pub(in crate::console::native) fn known_failure(op: IntentOperation, error: &NativeError) -> bool {
    if op == IntentOperation::AgentRevoke {
        return matches!(error.code.as_str(), "invalid_arguments" | "invalid_management_arguments" | "invalid_identifier" | "invalid_management_payload" | "invalid_management_revision" | "management_revision_changed" | "management_receipt_capacity" | "assigned_project_administrator_required" | "agent_request_closed");
    }
    let common = matches!(
        error.code.as_str(),
        "invalid_management_arguments"
            | "invalid_management_key"
            | "invalid_management_revision"
            | "management_revision_changed"
            | "management_receipt_capacity"
    );
    common
        || match op {
            IntentOperation::AgentRequest => matches!(
                error.code.as_str(),
                "invalid_managed_agent_definition"
                    | "project_manager_required"
                    | "project_administrator_required"
                    | "matrix_setup_required"
                    | "matrix_verification_required"
                    | "project_setup_changed"
                    | "project_administrator_changed"
                    | "live_reservation_required"
                    | "project_disabled"
                    | "agent_offer_unavailable"
                    | "agent_request_capacity"
                    | "project_matrix_not_ready"
                    | "project_membership_pending"
                    | "project_private_room_required"
                    | "project_matrix_marker_mismatch"
                    | "project_matrix_proof_missing"
                    | "room_administration_denied"
            ),
            IntentOperation::AgentDecide => matches!(
                error.code.as_str(),
                "assigned_project_administrator_required"
                    | "agent_request_closed"
                    | "agent_request_binding_changed"
                    | "project_administrator_required"
                    | "matrix_setup_required"
                    | "matrix_verification_required"
                    | "project_setup_changed"
                    | "project_administrator_changed"
                    | "live_reservation_required"
                    | "project_disabled"
                    | "project_matrix_not_ready"
                    | "project_membership_pending"
                    | "project_private_room_required"
                    | "project_matrix_marker_mismatch"
                    | "project_matrix_proof_missing"
                    | "room_administration_denied"
            ),
            IntentOperation::AgentCancel => matches!(
                error.code.as_str(),
                "project_manager_required" | "agent_request_closed"
            ),
            IntentOperation::AgentRevoke => matches!(error.code.as_str(), "assigned_project_administrator_required" | "agent_request_closed"),
            _ => false,
        }
}
