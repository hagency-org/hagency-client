//! Human decisions within an owner-delegated grant. No executor authority.
use super::{ManagementCommand, NativeError, ServerOperation, failure};
use hagency_store::management::IntentOperation;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_EXACT: i64 = 9_007_199_254_740_991;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Balance {
    version: u16,
    command_id: String,
    allocation_id: String,
    state: String,
    ledger_revision: i64,
    reserved_tokens: i64,
    retained_tokens: i64,
    period_key: String,
    reason: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    known_consumed_tokens: i64,
    retained_tokens_floor: i64,
    requires_reconciliation: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Allocation {
    id: String,
    project_id: String,
    project_revision: i64,
    grant_id: String,
    requested_tokens: i64,
    state: String,
    revision: i64,
    request_id: String,
    requester_mxid: String,
    coordinator_mxid: String,
    period_key: String,
    expires_at_ms: i64,
    can_decide: bool,
    can_release: bool,
    blocking_reason: Option<String>,
    result: Option<Balance>,
    balance: Option<Balance>,
    #[serde(default)]
    observed_usage: Option<Usage>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    id: String,
    kind: String,
    contribution_id: String,
    project_id: Option<String>,
    allocation_id: Option<String>,
    requester_mxid: String,
    approver_mxid: String,
    state: String,
    revision: i64,
    expires_at_ms: i64,
    can_decide: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Grant {
    id: String,
    revision: i64,
    state: String,
    owner_mxid: String,
    coordinator_mxid: String,
    total_tokens: i64,
    period_key: String,
    period_start_ms: i64,
    period_end_ms: i64,
    delegation_expires_at_ms: i64,
    native_available_tokens: Option<i64>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Detail {
    allocation: Allocation,
    request: Request,
    project_name: String,
    resource_label: String,
    grant: Grant,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailReply {
    detail: Detail,
    observed_at_ms: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    items: Vec<Detail>,
    next_after: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageReply {
    allocations: Page,
    observed_at_ms: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    allocation: Allocation,
    observed_at_ms: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recovery {
    recorded: bool,
    allocation: Option<Allocation>,
    observed_at_ms: i64,
}
fn invalid() -> NativeError {
    failure(503, "management_allocation_reply_invalid")
}
fn key(s: &str) -> bool {
    super::super::key(s).is_ok()
}
fn text(s: &str, n: usize) -> bool {
    !s.is_empty() && s.len() <= n && !s.chars().any(char::is_control)
}
fn positive(n: i64) -> bool {
    (1..=MAX_EXACT).contains(&n)
}
fn mxid(s: &str) -> bool {
    text(s, 255) && s.starts_with('@') && s.contains(':')
}
fn period(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && matches!(
            &s[5..],
            "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10" | "11" | "12"
        )
}
impl Allocation {
    fn validate(&self) -> Result<(), NativeError> {
        if ![&self.id, &self.project_id, &self.grant_id, &self.request_id]
            .into_iter()
            .all(|s| key(s))
            || ![
                self.project_revision,
                self.requested_tokens,
                self.revision,
                self.expires_at_ms,
            ]
            .into_iter()
            .all(positive)
            || !mxid(&self.requester_mxid)
            || !mxid(&self.coordinator_mxid)
            || !text(&self.state, 80)
            || !period(&self.period_key)
            || self.blocking_reason.as_ref().is_some_and(|s| !text(s, 128))
        {
            return Err(invalid());
        }
        for b in [&self.result, &self.balance].into_iter().flatten() {
            if b.version != 1
                || !key(&b.command_id)
                || b.allocation_id != self.id
                || !positive(b.ledger_revision)
                || !text(&b.state, 80)
                || b.period_key != self.period_key
                || !(0..=MAX_EXACT).contains(&b.reserved_tokens)
                || !(0..=MAX_EXACT).contains(&b.retained_tokens)
                || b.reason.as_ref().is_some_and(|s| !text(s, 128))
            {
                return Err(invalid());
            }
        }
        if self.observed_usage.as_ref().is_some_and(|u| {
            !(0..=MAX_EXACT).contains(&u.known_consumed_tokens)
                || !(u.known_consumed_tokens..=MAX_EXACT).contains(&u.retained_tokens_floor)
        }) {
            return Err(invalid());
        }
        Ok(())
    }
}
impl Detail {
    fn validate(&self) -> Result<(), NativeError> {
        self.allocation.validate()?;
        let a = &self.allocation;
        let r = &self.request;
        let g = &self.grant;
        if r.id!=a.request_id || r.kind!="project_allocation" || !key(&r.contribution_id)
            || r.project_id.as_ref()!=Some(&a.project_id) || r.allocation_id.as_ref()!=Some(&a.id)
            || r.requester_mxid!=a.requester_mxid || r.approver_mxid!=a.coordinator_mxid
            || !positive(r.revision) || !positive(r.expires_at_ms) || !text(&r.state,80)
            || !text(&self.project_name,160) || !text(&self.resource_label,160)
            || g.id!=a.grant_id || !positive(g.revision) || !text(&g.state,80)
            || !mxid(&g.owner_mxid) || g.coordinator_mxid!=a.coordinator_mxid
            || !positive(g.total_tokens) || g.period_key!=a.period_key
            || !positive(g.period_start_ms) || !positive(g.period_end_ms) || g.period_end_ms<=g.period_start_ms
            || !positive(g.delegation_expires_at_ms)
            // This read contract has no physical-headroom observation yet.
            || g.native_available_tokens.is_some()
        {
            return Err(invalid());
        }
        Ok(())
    }
}
pub(in crate::console::native) fn is_operation(op: IntentOperation) -> bool {
    matches!(
        op,
        IntentOperation::AllocationDecide | IntentOperation::AllocationRelease
    )
}
pub(super) fn prepare(c: &ManagementCommand) -> Result<ServerOperation, NativeError> {
    let key = super::super::key;
    let (method, suffix, input) = match c {
        ManagementCommand::Allocations {
            project_id,
            after,
            limit,
        } => {
            key(project_id)?;
            if let Some(id) = after {
                key(id)?;
            }
            if limit.is_some_and(|n| !(1..=16).contains(&n)) {
                return Err(failure(400, "invalid_management_arguments"));
            }
            (
                Method::POST,
                format!("projects/{project_id}/allocations/query"),
                Some(json!({"after":after,"limit":limit.unwrap_or(16)})),
            )
        }
        ManagementCommand::Allocation { id } => {
            key(id)?;
            (Method::GET, format!("allocations/{id}"), None)
        }
        ManagementCommand::AllocationDecide {
            command_id,
            request_id,
            expected_revision,
            approve,
        } => {
            key(command_id)?;
            key(request_id)?;
            if !positive(*expected_revision) {
                return Err(failure(400, "invalid_management_revision"));
            }
            (
                Method::POST,
                format!("allocation-requests/{request_id}/decide"),
                Some(
                    json!({"commandId":command_id,"expectedRevision":expected_revision,"decision":if *approve{"approve"}else{"reject"}}),
                ),
            )
        }
        ManagementCommand::AllocationRelease {
            command_id,
            allocation_id,
            expected_revision,
        } => {
            key(command_id)?;
            key(allocation_id)?;
            if !positive(*expected_revision) {
                return Err(failure(400, "invalid_management_revision"));
            }
            (
                Method::POST,
                format!("allocations/{allocation_id}/release"),
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
pub(super) fn project_reply(c: &ManagementCommand, value: Value) -> Result<Value, NativeError> {
    match c {
        ManagementCommand::Allocations {
            project_id,
            after,
            limit,
        } => {
            if value["allocations"]["items"]
                .as_array()
                .is_none_or(|v| v.len() > usize::from(limit.unwrap_or(16)))
            {
                return Err(invalid());
            }
            let r: PageReply = serde_json::from_value(value).map_err(|_| invalid())?;
            if !positive(r.observed_at_ms) {
                return Err(invalid());
            }
            let mut previous = after.as_deref();
            for d in &r.allocations.items {
                d.validate()?;
                if d.allocation.project_id != *project_id
                    || previous.is_some_and(|p| d.allocation.id.as_str() <= p)
                {
                    return Err(invalid());
                }
                previous = Some(&d.allocation.id);
            }
            if r.allocations.next_after.as_deref().is_some_and(|id| {
                !key(id)
                    || r.allocations
                        .items
                        .last()
                        .is_none_or(|d| d.allocation.id != id)
            }) {
                return Err(invalid());
            }
            serde_json::to_value(r).map_err(|_| invalid())
        }
        ManagementCommand::Allocation { id } => {
            let r: DetailReply = serde_json::from_value(value).map_err(|_| invalid())?;
            r.detail.validate()?;
            if !positive(r.observed_at_ms) || r.detail.allocation.id != *id {
                return Err(invalid());
            }
            serde_json::to_value(r).map_err(|_| invalid())
        }
        ManagementCommand::AllocationDecide { request_id, .. }
        | ManagementCommand::AllocationRelease {
            allocation_id: request_id,
            ..
        } => {
            let r: Reply = serde_json::from_value(value).map_err(|_| invalid())?;
            r.allocation.validate()?;
            let expected = if matches!(c, ManagementCommand::AllocationDecide { .. }) {
                &r.allocation.request_id
            } else {
                &r.allocation.id
            };
            if !positive(r.observed_at_ms) || expected != request_id {
                return Err(invalid());
            }
            serde_json::to_value(r).map_err(|_| invalid())
        }
        ManagementCommand::Recover {
            original_operation, ..
        } if is_operation(*original_operation) => {
            let r: Recovery = serde_json::from_value(value).map_err(|_| invalid())?;
            if !positive(r.observed_at_ms) || r.recorded != r.allocation.is_some() {
                return Err(invalid());
            }
            if let Some(a) = &r.allocation {
                a.validate()?;
            }
            serde_json::to_value(r).map_err(|_| invalid())
        }
        _ => Ok(value),
    }
}
/// Only explicit first-POST pre-effect failures close a saved human command.
pub(in crate::console::native) fn known_failure(op: IntentOperation, e: &NativeError) -> bool {
    matches!(
        e.code.as_str(),
        "invalid_management_arguments"
            | "invalid_arguments"
            | "invalid_identifier"
            | "invalid_management_payload"
            | "invalid_management_revision"
            | "management_revision_changed"
            | "management_receipt_capacity"
    ) || match op {
        IntentOperation::AllocationDecide => matches!(
            e.code.as_str(),
            "allocation_coordinator_required"
                | "management_action_closed"
                | "allocation_exceeds_grant"
        ),
        IntentOperation::AllocationRelease => e.code == "allocation_release_forbidden",
        _ => false,
    }
}
