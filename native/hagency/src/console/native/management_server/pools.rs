//! Closed direct-pool contract. Only the selected resource owner decides.
use super::agents::AgentDefinition;
use hagency_core::recurring_quota::{Calendar, RenewalMode};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewal_mode: Option<RenewalMode>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renewal_mode: Option<RenewalMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    per_request_tokens: Option<i64>,
    native_available_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    calendar: Option<Calendar>,
    can_request: bool,
    blocking_reason: Option<String>,
    /// End of the governing grant's UTC month (pool succession, phase 1b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_period_end_ms: Option<i64>,
    /// Next month's scheduled grant of this pool, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renewal: Option<Renewal>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Renewal {
    period_key: String,
    period_start_ms: i64,
    published_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renewal_mode: Option<RenewalMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    per_request_tokens: Option<i64>,
    grant_revision: i64,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    calendar: Option<Calendar>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renewal_mode: Option<RenewalMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_period: Option<CurrentPeriod>,
    #[serde(default)]
    can_renew: bool,
    #[serde(default)]
    can_reallocate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_grant_revision: Option<i64>,
    /// Recovery never silently restores removed or unverified room bindings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rooms_requiring_reinvite: Option<u64>,
    can_decide: bool,
    can_cancel: bool,
    can_stop_supply: bool,
    blocking_reason: Option<String>,
    /// When a reserved Agent's authority ends: the end of its pool's
    /// contiguous capacity chain, or its reserve expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authority_until_ms: Option<i64>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurrentPeriod {
    key: String,
    start_ms: u64,
    end_ms: u64,
    tokens_per_period: u64,
    consumed_tokens: u64,
    remaining_tokens: Option<u64>,
    state: String,
    next_reset_ms: u64,
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
            | IntentOperation::PoolAgentReallocate
            | IntentOperation::PoolAgentRenew
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
        ManagementCommand::PoolAgentReallocate {command_id,request_id,expected_revision,expected_grant_revision,renewal_mode} => {
            key(command_id)?; key(request_id)?; positive(*expected_revision)?; positive(*expected_grant_revision)?;
            (Method::POST, format!("pool-agent-requests/{request_id}/reallocate"), Some(json!({"commandId":command_id,"expectedRevision":expected_revision,"expectedGrantRevision":expected_grant_revision,"renewalMode":renewal_mode})))
        }
        ManagementCommand::PoolAgentRenew {command_id,request_id,expected_revision,expected_period_key} => {
            key(command_id)?; key(request_id)?; positive(*expected_revision)?;
            if expected_period_key.is_empty() || expected_period_key.len() > 32 || !expected_period_key.bytes().all(|b| b.is_ascii_digit() || b == b'-') { return Err(failure(400, "invalid_calendar_period")); }
            (Method::POST, format!("pool-agents/{request_id}/renew"), Some(json!({"commandId":command_id,"expectedRevision":expected_revision,"expectedPeriodKey":expected_period_key})))
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
        | ManagementCommand::PoolAgentStop { .. }
        | ManagementCommand::PoolAgentRenew { .. }
        | ManagementCommand::PoolAgentReallocate { .. } => typed::<Reply>(v),
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
            | "invalid_calendar_period"
            | "pool_agent_renewal_not_due"
            | "pool_agent_period_changed"
            | "pool_agent_not_recurring"
            | "agent_reallocation_unavailable"
            | "agent_generation_overflow"
            | "agent_period_allowance_fixed"
            | "agent_not_recurring"
            | "agent_period_changed"
            | "agent_period_already_authorized"
            | "agent_period_terms_changed"
            | "agent_allocation_fenced"
            | "agent_assignment_unavailable"
            | "pool_agent_renewal_mode_required"
            | "pool_renewal_policy_changed"
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

#[cfg(test)]
mod recurring_transport_tests {
    use super::*;
    #[test]
    fn room_reinvitation_count_survives_typed_projection_without_inventing_legacy_counts() {
        let request=json!({
            "id":"request_1","poolId":"pool_1","poolLabel":"Pool",
            "resourceOwnerMxid":"@owner:test","managerMxid":"@manager:test",
            "agentId":"agent_1","roomId":"!room:test",
            "definition":{"name":"Helper","capability":"coding","description":"","toolMode":"none"},
            "requestedTokens":1000,"periodKey":"2026-11-02",
            "state":"approved_waiting_executor","revision":4,
            "createdAtMs":1793606400000u64,"expiresAtMs":1794211200000u64,
            "canDecide":false,"canCancel":true,"canStopSupply":true
        });
        let command=ManagementCommand::PoolAgent{id:"request_1".into()};
        for count in [None,Some(0u64),Some(2u64)] {
            let mut reply=json!({"request":request.clone(),"observedAtMs":1793606400000u64});
            if let Some(count)=count {reply["request"]["roomsRequiringReinvite"]=json!(count);}
            let projected=project_reply(&command,reply).unwrap();
            assert_eq!(projected["request"].get("roomsRequiringReinvite").and_then(Value::as_u64),count);
            if count.is_none() {assert!(projected["request"].get("roomsRequiringReinvite").is_none());}
        }
        let mut invalid=json!({"request":request,"observedAtMs":1793606400000u64});
        invalid["request"]["roomsRequiringReinvite"]=json!(-1);
        assert!(project_reply(&command,invalid).is_err());
    }
    #[test]
    fn legacy_request_stays_byte_compatible_and_recurring_mode_round_trips() {
        let old=json!({"roomId":"!room:test","expectedPoolRevision":2,"expectedGrantRevision":1,"definition":{"name":"Helper","capability":"coding","description":"","toolMode":"none"},"requestedTokens":1000});
        let input:PoolAgentInput=serde_json::from_value(old.clone()).unwrap();
        input.validate().unwrap();
        assert_eq!(serde_json::to_value(input).unwrap(),old);
        let mut recurring=old;recurring["renewalMode"]=json!("owner");
        let input:PoolAgentInput=serde_json::from_value(recurring.clone()).unwrap();
        assert_eq!(input.renewal_mode,Some(RenewalMode::Owner));
        assert_eq!(serde_json::to_value(input).unwrap(),recurring);
    }
    #[test]
    fn manual_renewal_keeps_period_revision_and_recovery_identity() {
        let command=ManagementCommand::PoolAgentRenew{command_id:"command_renew".into(),request_id:"request_1".into(),expected_revision:4,expected_period_key:"2026-11-02".into()};
        let operation=prepare(&command).unwrap();
        assert_eq!(operation.suffix,"pool-agents/request_1/renew");
        assert_eq!(operation.input.unwrap(),json!({"commandId":"command_renew","expectedRevision":4,"expectedPeriodKey":"2026-11-02"}));
        assert!(is_operation(IntentOperation::PoolAgentRenew));
        assert_eq!(serde_json::to_value(IntentOperation::PoolAgentRenew).unwrap(),json!("pool_agent_renew"));
    }
}

#[cfg(test)]
mod split_pool_limits_tests {
    use super::*;
    #[test]
    fn pool_transport_keeps_total_and_per_agent_limit_independent() {
        let value = serde_json::json!({"id":"pool","label":"Terra","revision":1,
            "ownerMxid":"@owner:test","runtime":"codex","capabilities":["coding"],
            "grantRevision":1,"publishedTokens":20_000_000,"perRequestTokens":5_000_000,
            "nativeAvailableTokens":15_000_000,"canRequest":true,"blockingReason":null,
            "renewal":{"periodKey":"2026-12","periodStartMs":1796112000000i64,
                "publishedTokens":20_000_000,"perRequestTokens":5_000_000,"grantRevision":1}});
        let pool: Pool = serde_json::from_value(value).unwrap();
        let forwarded = serde_json::to_value(pool).unwrap();
        assert_eq!(forwarded["publishedTokens"],20_000_000);
        assert_eq!(forwarded["perRequestTokens"],5_000_000);
        assert_eq!(forwarded["renewal"]["perRequestTokens"],5_000_000);
    }
}
