//! Recorded cleanup observations, never proof of refunded or runnable capacity.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRemovalIndexItem {
    request_id: String,
    project_id: String,
    project_name: String,
    agent_name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRemovalPage {
    items: Vec<AgentRemovalIndexItem>,
    next_after: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRemovalProgress {
    request_id: String,
    project_id: String,
    project_name: String,
    proposed_agent_id: String,
    agent_name: String,
    request_revision: i64,
    admission_state: String,
    can_cancel: bool,
    can_revoke: bool,
    central_fenced: bool,
    native_fence: Option<AgentFenceProgress>,
    process_observation: Option<AgentProcessObservation>,
    matrix: Option<AgentMatrixCleanup>,
    usage: AgentCleanupUsage,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentFenceProgress {
    delivery_state: String,
    native_state: Option<String>,
    ledger_revision: Option<i64>,
    process_state: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentProcessObservation {
    kind: String,
    state: String,
    ledger_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentMatrixCleanup {
    state: String,
    cleanup_observed_at_ms: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentCleanupUsage {
    period_key: String,
    known_consumed_tokens: Option<i64>,
    retained_tokens_floor: Option<i64>,
    fence_usage_complete: Option<bool>,
    requires_reconciliation: bool,
}

use super::{ManagementCommand, NativeError, ServerOperation, failure};
use reqwest::Method;
use serde_json::{Value, json};
const MAX_EXACT: i64 = 9_007_199_254_740_991;
fn invalid() -> NativeError {
    failure(503, "management_agent_cleanup_invalid")
}
fn key(v: &str) -> bool {
    super::super::key(v).is_ok()
}
fn text(v: &str, max: usize) -> bool {
    !v.is_empty() && v.len() <= max && !v.chars().any(char::is_control)
}
fn number(v: i64) -> bool {
    (0..=MAX_EXACT).contains(&v)
}
fn positive(v: i64) -> bool {
    v > 0 && number(v)
}
fn period(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && matches!(
            &v[5..],
            "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10" | "11" | "12"
        )
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailReply {
    removal: AgentRemovalProgress,
    observed_at_ms: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListReply {
    requests: AgentRemovalPage,
    observed_at_ms: i64,
}
pub(super) fn prepare(command: &ManagementCommand) -> Result<ServerOperation, NativeError> {
    let (method, suffix, input) = match command {
        ManagementCommand::AgentRemoval { id } => {
            super::super::key(id)?;
            (Method::GET, format!("agent-requests/{id}/removal"), None)
        }
        ManagementCommand::AgentRemovals {
            project_id,
            after,
            limit,
        } => {
            for id in [project_id, after].into_iter().flatten() {
                super::super::key(id)?;
            }
            if limit.is_some_and(|n| !(1..=16).contains(&n)) {
                return Err(failure(400, "invalid_management_arguments"));
            }
            (
                Method::POST,
                "agent-removals/list".into(),
                Some(json!({"projectId":project_id,"after":after,"limit":limit.unwrap_or(16)})),
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
pub(super) fn project_reply(
    command: &ManagementCommand,
    value: Value,
) -> Result<Value, NativeError> {
    match command {
        ManagementCommand::AgentRemoval { id } => {
            let r: DetailReply = serde_json::from_value(value).map_err(|_| invalid())?;
            let d = &r.removal;
            let u = &d.usage;
            if !positive(r.observed_at_ms)
                || d.request_id != *id
                || !key(&d.project_id)
                || !key(&d.proposed_agent_id)
                || !text(&d.project_name, 160)
                || !text(&d.agent_name, 160)
                || !positive(d.request_revision)
                || !text(&d.admission_state, 80)
                || !period(&u.period_key)
                || u.known_consumed_tokens.is_some_and(|v| !number(v))
                || u.retained_tokens_floor.is_some_and(|v| !number(v))
                || matches!((u.known_consumed_tokens,u.retained_tokens_floor),(Some(n),Some(r)) if r<n)
                || d.native_fence.as_ref().is_some_and(|f| {
                    !text(&f.delivery_state, 80)
                        || f.native_state.as_deref().is_some_and(|s| !text(s, 80))
                        || f.process_state.as_deref().is_some_and(|s| !text(s, 80))
                        || f.ledger_revision.is_some_and(|n| !positive(n))
                })
                || d.process_observation.as_ref().is_some_and(|p| {
                    !text(&p.kind, 80) || !text(&p.state, 80) || !positive(p.ledger_revision)
                })
                || d.matrix.as_ref().is_some_and(|m| {
                    !text(&m.state, 80) || m.cleanup_observed_at_ms.is_some_and(|n| !positive(n))
                })
            {
                return Err(invalid());
            }
            Ok(
                json!({"protocol":"hagency-management-v1","removal":r.removal,"observedAtMs":r.observed_at_ms}),
            )
        }
        ManagementCommand::AgentRemovals {
            project_id,
            after,
            limit,
        } => {
            if value["requests"]["items"]
                .as_array()
                .is_none_or(|rows| rows.len() > usize::from(limit.unwrap_or(16)))
            {
                return Err(invalid());
            }
            let r: ListReply = serde_json::from_value(value).map_err(|_| invalid())?;
            if !positive(r.observed_at_ms) {
                return Err(invalid());
            }
            let mut previous = after.as_deref();
            for row in &r.requests.items {
                if !key(&row.request_id)
                    || !key(&row.project_id)
                    || !text(&row.project_name, 160)
                    || !text(&row.agent_name, 160)
                    || project_id.as_ref().is_some_and(|p| p != &row.project_id)
                    || previous.is_some_and(|p| row.request_id.as_str() <= p)
                {
                    return Err(invalid());
                }
                previous = Some(&row.request_id);
            }
            if r.requests.next_after.as_deref().is_some_and(|n| {
                !key(n)
                    || r.requests
                        .items
                        .last()
                        .is_none_or(|row| row.request_id != n)
            }) {
                return Err(invalid());
            }
            Ok(
                json!({"protocol":"hagency-management-v1","requests":r.requests,"observedAtMs":r.observed_at_ms}),
            )
        }
        _ => Ok(value),
    }
}
