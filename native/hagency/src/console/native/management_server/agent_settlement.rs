//! Bounded human settlement projection. No executor ticket or execution authority.
use super::{failure, ManagementCommand, NativeError, ServerOperation};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
const MAX: i64 = 9_007_199_254_740_991;
fn invalid() -> NativeError {
    failure(503, "management_agent_settlement_invalid")
}
fn positive(n: i64) -> bool {
    (1..=MAX).contains(&n)
}
fn amount(n: i64) -> bool {
    (0..=MAX).contains(&n)
}
fn key(s: &str) -> bool {
    super::super::key(s).is_ok()
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(in crate::console::native) fn known_failure(e: &NativeError) -> bool {
    // Authorization/transport errors cannot prove that an earlier POST failed.
    (e.status == 400
        && matches!(
            e.code.as_str(),
            "invalid_management_arguments" | "invalid_management_revision"
        ))
        || (e.status == 409
            && matches!(
                e.code.as_str(),
                "agent_settlement_changed" | "agent_settlement_not_ready"
            ))
}
pub(super) fn prepare(c: &ManagementCommand) -> Result<ServerOperation, NativeError> {
    let (id, input) = match c {
        ManagementCommand::AgentSettlement { id } => (id, None),
        ManagementCommand::AgentSettlementRequest {
            command_id,
            request_id,
            expected_request_revision,
            expected_fence_receipt_digest,
        } => {
            if !key(command_id)
                || !positive(*expected_request_revision)
                || !digest(expected_fence_receipt_digest)
            {
                return Err(failure(400, "invalid_management_arguments"));
            }
            (
                request_id,
                Some(
                    json!({"commandId":command_id,"expectedRequestRevision":expected_request_revision,"expectedFenceReceiptDigest":expected_fence_receipt_digest}),
                ),
            )
        }
        _ => return Err(failure(400, "invalid_management_operation")),
    };
    super::super::key(id)?;
    Ok(ServerOperation {
        method: if input.is_some() {
            Method::POST
        } else {
            Method::GET
        },
        suffix: format!("agent-requests/{id}/settlement"),
        input,
    })
}
// Deliberately project only descriptive fields; original reserve payloads,
// executor tokens and any future extra fields never enter the script VM.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    request_id: String,
    command_id: String,
    fence_receipt_digest: String,
    ledger_revision: i64,
    state: String,
    process_state: String,
    known_consumed_tokens: i64,
    retained_tokens: i64,
    unresolved_tokens: i64,
    usage_complete: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    request_id: String,
    request_revision: i64,
    fence_receipt_digest: Option<String>,
    pending_command_id: Option<String>,
    latest_receipt: Option<Receipt>,
    settlement_current: bool,
    can_request: bool,
    blocking_reason: Option<String>,
    known_consumed_tokens: Option<i64>,
    retained_tokens_floor: Option<i64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailReply {
    settlement: Progress,
    observed_at_ms: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Requested {
    request_id: String,
    command_id: String,
    command_digest: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestedReply {
    requested: Requested,
    observed_at_ms: i64,
}
pub(super) fn project_reply(c: &ManagementCommand, value: Value) -> Result<Value, NativeError> {
    match c {
        ManagementCommand::AgentSettlement { id } => {
            let r: DetailReply = serde_json::from_value(value).map_err(|_| invalid())?;
            let s = &r.settlement;
            if !positive(r.observed_at_ms)
                || s.request_id != *id
                || !positive(s.request_revision)
                || s.fence_receipt_digest
                    .as_deref()
                    .is_some_and(|v| !digest(v))
                || s.pending_command_id.as_deref().is_some_and(|v| !key(v))
                || s.blocking_reason.as_deref().is_some_and(|v| {
                    v.is_empty() || v.len() > 80 || v.chars().any(char::is_control)
                })
                || s.known_consumed_tokens.is_some_and(|v| !amount(v))
                || s.retained_tokens_floor.is_some_and(|v| !amount(v))
                || matches!((s.known_consumed_tokens,s.retained_tokens_floor), (Some(n),Some(f)) if f<n)
                || s.can_request
                    && (s.fence_receipt_digest.is_none()
                        || s.pending_command_id.is_some()
                        || s.settlement_current
                        || s.blocking_reason.is_some())
                || s.settlement_current
                    && s.latest_receipt.as_ref().is_none_or(|v| !v.usage_complete)
            {
                return Err(invalid());
            }
            if let Some(p) = &s.latest_receipt
                && (p.request_id != *id
                    || !key(&p.command_id)
                    || !positive(p.ledger_revision)
                    || s.fence_receipt_digest.as_deref() != Some(p.fence_receipt_digest.as_str())
                    || !amount(p.known_consumed_tokens)
                    || !amount(p.retained_tokens)
                    || !amount(p.unresolved_tokens)
                    || p.retained_tokens < p.known_consumed_tokens
                    || p.unresolved_tokens != p.retained_tokens - p.known_consumed_tokens
                    || !matches!(
                        p.process_state.as_str(),
                        "unknown" | "stopped" | "not_started"
                    )
                    || if p.usage_complete {
                        p.state != "settled"
                            || p.unresolved_tokens != 0
                            || p.process_state == "unknown"
                    } else {
                        p.state != "unresolved"
                    })
            {
                return Err(invalid());
            }
            Ok(
                json!({"protocol":"hagency-management-v1","settlement":r.settlement,"observedAtMs":r.observed_at_ms}),
            )
        }
        ManagementCommand::AgentSettlementRequest { request_id, .. } => {
            let r: RequestedReply = serde_json::from_value(value).map_err(|_| invalid())?;
            if !positive(r.observed_at_ms)
                || r.requested.request_id != *request_id
                || !key(&r.requested.command_id)
                || !digest(&r.requested.command_digest)
            {
                return Err(invalid());
            }
            Ok(
                json!({"protocol":"hagency-management-v1","requested":r.requested,"observedAtMs":r.observed_at_ms}),
            )
        }
        _ => Ok(value),
    }
}
