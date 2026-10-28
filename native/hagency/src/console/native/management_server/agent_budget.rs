//! Closed human top-up requests and accounting observations. No executor receipt or authority.
use super::{ManagementCommand, NativeError, NativeManagement, ServerOperation, failure};
use hagency_store::management::{IntentOperation, Permission};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
const MAX_EXACT: i64 = 9_007_199_254_740_991;
const PROTOCOL_FEATURE: &str = "managed-agent-budget-delta-v1";
fn invalid() -> NativeError {
    failure(503, "management_agent_budget_invalid")
}
fn key(s: &str) -> bool {
    super::super::key(s).is_ok()
}
fn positive(n: i64) -> bool {
    (1..=MAX_EXACT).contains(&n)
}
fn number(n: i64) -> bool {
    (0..=MAX_EXACT).contains(&n)
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn text(s: &str, max: usize, newline: bool) -> bool {
    !s.trim().is_empty()
        && s.len() <= max
        && !s.chars().any(|c| c.is_control() && !(newline && c == '\n'))
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Budget {
    #[serde(default)]
    can_owner_fund: bool,
    #[serde(default)]
    can_owner_fund_direct: bool,
    request_id: String,
    request_revision: i64,
    assignment_generation: i64,
    assignment_id: String,
    agent_reservation_id: String,
    original_ceiling_tokens: i64,
    applied_delta_tokens: i64,
    effective_ceiling_tokens: i64,
    budget_revision: i64,
    budget_digest: String,
    known_consumed_tokens: i64,
    conservative_commitment_tokens: i64,
    native_retained_tokens: Option<i64>,
    native_remaining_tokens: Option<i64>,
    usage_complete: Option<bool>,
    execution_supported: bool,
    can_request: bool,
    blocking_reason: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TopUp {
    #[serde(default)]
    owner_funded: bool,
    id: String,
    authority_revision: i64,
    request_id: String,
    assignment_id: String,
    add_tokens: i64,
    purpose: String,
    expected_budget_revision: i64,
    expected_budget_digest: String,
    expected_ceiling_tokens: i64,
    revision: i64,
    state: String,
    native_state: String,
    executor_command_id: Option<String>,
    created_at_ms: i64,
    expires_at_ms: i64,
    can_decide: bool,
    can_cancel: bool,
}
impl TopUp {
    fn validate(&self) -> Result<(), NativeError> {
        if ![&self.id, &self.request_id, &self.assignment_id]
            .into_iter()
            .all(|s| key(s))
            || ![
                self.authority_revision,
                self.add_tokens,
                self.expected_budget_revision,
                self.expected_ceiling_tokens,
                self.revision,
                self.created_at_ms,
                self.expires_at_ms,
            ]
            .into_iter()
            .all(positive)
            || self.expires_at_ms <= self.created_at_ms
            || !digest(&self.expected_budget_digest)
            || !text(&self.purpose, 2048, true)
            || !text(&self.state, 80, false)
            || !text(&self.native_state, 80, false)
            || self.executor_command_id.as_deref().is_some_and(|s| !key(s))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ack {
    command_id: String,
    operation: String,
    top_up_id: String,
    applied_revision: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    receipt: Ack,
    top_up: TopUp,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    items: Vec<TopUp>,
    next_after: Option<String>,
}
pub(in crate::console::native) fn is_operation(op: IntentOperation) -> bool {
    matches!(
        op,
        IntentOperation::AgentTopUpRequest
            | IntentOperation::AgentTopUpDecide
            | IntentOperation::AgentTopUpCancel
    )
}
pub(super) fn operation(op: IntentOperation) -> Option<&'static str> {
    match op {
        IntentOperation::AgentTopUpRequest => Some("agent.topup.request"),
        IntentOperation::AgentTopUpDecide => Some("agent.topup.decide"),
        IntentOperation::AgentTopUpCancel => Some("agent.topup.cancel"),
        _ => None,
    }
}
pub(super) fn prepare(c: &ManagementCommand) -> Result<ServerOperation, NativeError> {
    let (method, suffix, input) = match c {
        ManagementCommand::AgentBudget { id } => {
            super::super::key(id)?;
            (Method::GET, format!("agent-requests/{id}/budget"), None)
        }
        ManagementCommand::AgentTopUp { id } => {
            super::super::key(id)?;
            (Method::GET, format!("agent-topups/{id}"), None)
        }
        ManagementCommand::AgentTopUps { id, after, limit } => {
            super::super::key(id)?;
            if let Some(s) = after {
                super::super::key(s)?;
            }
            if limit.is_some_and(|n| !(1..=16).contains(&n)) {
                return Err(failure(400, "invalid_management_arguments"));
            }
            (
                Method::POST,
                format!("agent-requests/{id}/topups/query"),
                Some(json!({"after":after,"limit":limit.unwrap_or(16)})),
            )
        }
        ManagementCommand::AgentTopUpRequest {
            owner_funded,
            approve_immediately,
            command_id,
            request_id,
            expected_request_revision,
            expected_assignment_generation,
            expected_budget_revision,
            expected_budget_digest,
            add_tokens,
            purpose,
        } => {
            super::super::key(command_id)?;
            super::super::key(request_id)?;
            if ![
                *expected_request_revision,
                *expected_assignment_generation,
                *expected_budget_revision,
                *add_tokens,
            ]
            .into_iter()
            .all(positive)
                || (*approve_immediately && !*owner_funded)
                || !digest(expected_budget_digest)
                || !text(purpose, 2048, true)
            {
                return Err(failure(400, "invalid_agent_budget"));
            }
            let mut input = json!({"commandId":command_id,"expectedRequestRevision":expected_request_revision,"expectedAssignmentGeneration":expected_assignment_generation,"expectedBudgetRevision":expected_budget_revision,"expectedBudgetDigest":expected_budget_digest,"addTokens":add_tokens,"purpose":purpose});
            if *owner_funded { input["ownerFunded"] = json!(true); }
            if *approve_immediately { input["approveImmediately"] = json!(true); }
            (
                Method::POST,
                format!("agent-requests/{request_id}/topups"),
                Some(input),
            )
        }
        ManagementCommand::AgentTopUpDecide {
            command_id,
            top_up_id,
            expected_revision,
            expected_authority_revision,
            approve,
        } => {
            super::super::key(command_id)?;
            super::super::key(top_up_id)?;
            if !positive(*expected_revision) || !positive(*expected_authority_revision) {
                return Err(failure(400, "invalid_management_revision"));
            }
            (
                Method::POST,
                format!("agent-topups/{top_up_id}/decide"),
                Some(
                    json!({"commandId":command_id,"expectedRevision":expected_revision,"expectedAuthorityRevision":expected_authority_revision,"decision":if *approve {"approve"}else{"reject"}}),
                ),
            )
        }
        ManagementCommand::AgentTopUpCancel {
            command_id,
            top_up_id,
            expected_revision,
        } => {
            super::super::key(command_id)?;
            super::super::key(top_up_id)?;
            if !positive(*expected_revision) {
                return Err(failure(400, "invalid_management_revision"));
            }
            (
                Method::POST,
                format!("agent-topups/{top_up_id}/cancel"),
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
fn checked_reply(
    value: Value,
    id: &str,
    op: IntentOperation,
    target: Option<&str>,
    request: Option<&str>,
) -> Result<Reply, NativeError> {
    let r: Reply = serde_json::from_value(value).map_err(|_| invalid())?;
    r.top_up.validate()?;
    if r.receipt.command_id != id
        || Some(r.receipt.operation.as_str()) != operation(op)
        || r.receipt.top_up_id != r.top_up.id
        || !positive(r.receipt.applied_revision)
        || r.top_up.revision < r.receipt.applied_revision
        || target.is_some_and(|s| s != r.top_up.id)
        || request.is_some_and(|s| s != r.top_up.request_id)
    {
        return Err(invalid());
    }
    Ok(r)
}
pub(super) fn project_reply(c: &ManagementCommand, value: Value) -> Result<Value, NativeError> {
    let relevant = matches!(
        c,
        ManagementCommand::AgentBudget { .. }
            | ManagementCommand::AgentTopUp { .. }
            | ManagementCommand::AgentTopUps { .. }
            | ManagementCommand::AgentTopUpRequest { .. }
            | ManagementCommand::AgentTopUpDecide { .. }
            | ManagementCommand::AgentTopUpCancel { .. }
    ) || matches!(c,ManagementCommand::Recover{original_operation,..} if is_operation(*original_operation));
    if !relevant {
        return Ok(value);
    }
    let at = value["observedAtMs"]
        .as_i64()
        .filter(|v| positive(*v))
        .ok_or_else(invalid)?;
    let mut out = match c {
        ManagementCommand::AgentBudget { id } => {
            let b: Budget =
                serde_json::from_value(value["budget"].clone()).map_err(|_| invalid())?;
            if b.request_id != *id
                || !key(&b.assignment_id)
                || !key(&b.agent_reservation_id)
                || ![
                    b.request_revision,
                    b.assignment_generation,
                    b.original_ceiling_tokens,
                    b.effective_ceiling_tokens,
                    b.budget_revision,
                ]
                .into_iter()
                .all(positive)
                || ![
                    b.applied_delta_tokens,
                    b.known_consumed_tokens,
                    b.conservative_commitment_tokens,
                ]
                .into_iter()
                .all(number)
                || b.original_ceiling_tokens
                    .checked_add(b.applied_delta_tokens)
                    != Some(b.effective_ceiling_tokens)
                || !digest(&b.budget_digest)
                || b.native_retained_tokens.is_some()
                || b.native_remaining_tokens.is_some()
                || b.usage_complete.is_some()
                || b.execution_supported
                || b.blocking_reason
                    .as_deref()
                    .is_some_and(|s| !text(s, 128, false))
            {
                return Err(invalid());
            }
            json!({"budget":b})
        }
        ManagementCommand::AgentTopUp { id } => {
            let t: TopUp = serde_json::from_value(value["topUp"].clone()).map_err(|_| invalid())?;
            t.validate()?;
            if t.id != *id {
                return Err(invalid());
            }
            json!({"topUp":t})
        }
        ManagementCommand::AgentTopUps { id, after, limit } => {
            if value["topUps"]["items"]
                .as_array()
                .is_none_or(|v| v.len() > usize::from(limit.unwrap_or(16)))
            {
                return Err(invalid());
            }
            let p: Page = serde_json::from_value(value["topUps"].clone()).map_err(|_| invalid())?;
            let mut prev = after.as_deref();
            for t in &p.items {
                t.validate()?;
                if t.request_id != *id || prev.is_some_and(|s| s >= t.id.as_str()) {
                    return Err(invalid());
                }
                prev = Some(&t.id);
            }
            if p.next_after
                .as_deref()
                .is_some_and(|s| !key(s) || p.items.last().is_none_or(|t| s != t.id))
            {
                return Err(invalid());
            }
            json!({"topUps":p})
        }
        ManagementCommand::AgentTopUpRequest {
            command_id,
            request_id,
            ..
        } => json!(checked_reply(
            value,
            command_id,
            IntentOperation::AgentTopUpRequest,
            None,
            Some(request_id)
        )?),
        ManagementCommand::AgentTopUpDecide {
            command_id,
            top_up_id,
            ..
        } => json!(checked_reply(
            value,
            command_id,
            IntentOperation::AgentTopUpDecide,
            Some(top_up_id),
            None
        )?),
        ManagementCommand::AgentTopUpCancel {
            command_id,
            top_up_id,
            ..
        } => json!(checked_reply(
            value,
            command_id,
            IntentOperation::AgentTopUpCancel,
            Some(top_up_id),
            None
        )?),
        ManagementCommand::Recover {
            command_id,
            original_operation,
        } => match value["recorded"].as_bool() {
            Some(false) if value.get("result") == Some(&Value::Null) => {
                json!({"recorded":false,"result":null})
            }
            Some(true) => {
                let r = checked_reply(
                    value["result"].clone(),
                    command_id,
                    *original_operation,
                    None,
                    None,
                )?;
                json!({"recorded":true,"receipt":r.receipt,"topUp":r.top_up})
            }
            _ => return Err(invalid()),
        },
        _ => unreachable!(),
    };
    out["protocol"] = json!("hagency-management-v1");
    out["observedAtMs"] = json!(at);
    Ok(out)
}
// Only exact first-attempt pre-effect rejections qualify. Authentication,
// transport, malformed replies and ambiguous command conflicts retain custody.
pub(in crate::console::native) fn known_failure(e: &NativeError) -> bool {
    matches!(
        (e.status, e.code.as_str()),
        (
            400,
            "invalid_identifier"
                | "invalid_management_arguments"
                | "invalid_management_payload"
                | "invalid_agent_budget"
                | "invalid_management_revision"
        ) | (
            401,
            "project_manager_required" | "assigned_project_administrator_required" | "pool_resource_owner_required"
        ) | (
            409,
            "management_revision_changed"
                | "agent_topup_pending"
                | "agent_topup_capacity"
                | "agent_topup_closed"
                | "agent_budget_changed"
                | "agent_budget_exceeds_project"
                | "agent_budget_exceeds_pool"
        ) | (503, "agent_budget_issuance_unavailable")
    )
}
impl NativeManagement {
    /// Discovery only determines whether this read protocol exists. Mutations
    /// still need current server role, policy, exact revisions and saved command.
    pub async fn agent_budget(&self, id: String) -> Result<Value, NativeError> {
        super::super::key(&id)?;
        let (scope, session) = self.authorized().await?;
        session
            .authorize(&scope, Permission::ReadAgentBudget)
            .map_err(|_| failure(403, "management_permission_required"))?;
        // This is a management read. An adopted owner can use its canonical
        // management store while the old configuration surface stays fenced.
        // authorized() above has already refreshed and checked that authority.
        let console = &self.owner.0.host.console;
        let device = console.authorized_device().await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        let pin = super::DeviceIdentity::from(&device);
        let metadata = crate::server_admission::discover(&pin.origin)
            .await
            .map_err(|e| failure(503, e.code()))?;
        let (current, session) = self.authorized().await?;
        let current_device = console.authorized_device().await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        let current_pin = super::DeviceIdentity::from(&current_device);
        if current != scope || current_pin != pin {
            return Err(failure(401, "management_scope_changed"));
        }
        session
            .authorize(&current, Permission::ReadAgentBudget)
            .map_err(|_| failure(403, "management_permission_required"))?;
        if !metadata.capabilities.iter().any(|s| s == PROTOCOL_FEATURE) {
            return Ok(
                json!({"protocol":"hagency-management-v1","supported":false,"budget":null,"observedAtMs":null}),
            );
        }
        let mut v = self.central(ManagementCommand::AgentBudget { id }).await?;
        v["supported"] = json!(true);
        Ok(v)
    }
}
#[cfg(test)]
#[path = "agent_budget_tests.rs"]
mod tests;
