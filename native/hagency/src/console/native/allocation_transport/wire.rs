// Complete closed wire projections are parsed even when a field is not used as
// authority. Keeping those fields prevents permissive Value-shaped replies.
#![allow(dead_code)]
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceGrantView {
    pub id: String,
    pub receipt: NativeGrantReceipt,
    pub receipt_digest: String,
    pub state: String,
    pub revision: u64,
    pub observed_at_ms: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct Publication {
    pub command_id: String,
    pub expected_contribution_revision: u64,
    pub receipt: NativeGrantReceipt,
}
/// `grants/{predecessor}/successors` body: a v3 receipt with the exact
/// contribution and predecessor revisions the owner reviewed.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct SuccessorPublication {
    pub command_id: String,
    pub expected_contribution_revision: u64,
    pub expected_predecessor_revision: u64,
    pub receipt: NativeGrantReceipt,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GrantReply {
    pub grant: ResourceGrantView,
    pub observed_at_ms: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Published {
    pub grant: ResourceGrantView,
    pub observed_at_ms: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PublicationRecovery {
    pub recorded: bool,
    pub grant: Option<ResourceGrantView>,
    pub observed_at_ms: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct Envelope<D> {
    pub delivery: Option<D>,
    pub observed_at_ms: u64,
}
// No Debug/Serialize: reporting tickets must not enter UI results or logs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct Delivery<C, R> {
    pub command: C,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: u64,
    pub execution_allowed: bool,
    pub recorded: Option<R>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct ConnectionReply {
    pub admission: Admission,
    pub observed_at_ms: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct Admission {
    pub contribution: Contribution,
    pub request: Request,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct Contribution {
    pub id: String,
    pub owner_mxid: String,
    pub executor_device_id: String,
    pub executor_generation: u64,
    pub resource_id: String,
    pub resource_revision: String,
    pub label: String,
    pub runtime: String,
    pub capabilities: BTreeSet<String>,
    pub coordinator_mxid: String,
    pub state: String,
    pub revision: u64,
    pub request_id: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub verification: Option<Verification>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct Verification {
    pub status: String,
    pub verified_at_ms: u64,
    pub fresh_until_ms: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct Request {
    pub id: String,
    pub kind: String,
    pub contribution_id: String,
    pub project_id: Option<String>,
    pub allocation_id: Option<String>,
    pub project_name: Option<String>,
    pub resource_label: Option<String>,
    pub requester_mxid: String,
    pub approver_mxid: String,
    pub state: String,
    pub revision: u64,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub decided_at_ms: Option<u64>,
    pub can_decide: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AllocationView {
    pub id: String,
    pub project_id: String,
    pub project_revision: u64,
    pub grant_id: String,
    pub requested_tokens: u64,
    pub state: String,
    pub revision: u64,
    pub request_id: String,
    pub requester_mxid: String,
    pub coordinator_mxid: String,
    pub period_key: String,
    pub expires_at_ms: u64,
    pub can_decide: bool,
    pub can_release: bool,
    pub blocking_reason: Option<String>,
    pub result: Option<NativeAllocationReceipt>,
    pub balance: Option<NativeAllocationReceipt>,
    #[serde(default)]
    pub observed_usage: Option<AllocationObservedUsage>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AllocationObservedUsage {
    pub known_consumed_tokens: u64,
    pub retained_tokens_floor: u64,
    pub requires_reconciliation: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AllocationAck {
    pub command_id: String,
    pub command_digest: String,
    pub receipt_digest: String,
    pub allocation: AllocationView,
    pub observed_at_ms: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SettlementView {
    pub grant_id: String,
    pub grant_revision: u64,
    pub revision: u64,
    pub state: String,
    pub original_granted_tokens: u64,
    pub held_tokens: u64,
    pub known_consumed_tokens: Option<u64>,
    pub unresolved_tokens: Option<u64>,
    pub known_unused_released_tokens: u64,
    pub overrun_tokens: u64,
    pub unreconciled_child_tokens: u64,
    pub can_close: bool,
    pub can_settle: bool,
    pub receipt: Option<NativeGrantSettlementReceipt>,
    pub receipt_digest: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ParentAck {
    pub command_id: String,
    pub command_digest: String,
    pub receipt_digest: String,
    pub settlement: SettlementView,
    pub observed_at_ms: u64,
}

pub(super) fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, NativeError> {
    decode_bounded(value, 32 * 1024)
}
pub(super) fn decode_bounded<T: serde::de::DeserializeOwned>(value: Value, max_bytes: usize) -> Result<T, NativeError> {
    fn exact(value: &Value) -> bool {
        match value {
            Value::Number(n) => n.as_u64().is_some_and(|n| n <= hagency_core::JSON_SAFE_MAX),
            Value::Array(a) => a.iter().all(exact),
            Value::Object(o) => o.values().all(exact),
            _ => true,
        }
    }
    if !exact(&value) || serde_json::to_vec(&value).map_err(|_| invalid())?.len() > max_bytes {
        return Err(invalid());
    }
    serde_json::from_value(value).map_err(|_| invalid())
}
