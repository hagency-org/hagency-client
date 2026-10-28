//! Human read projection of independently observed managed-agent stages.
//! Qualification is not model work. No private receipt or executor token leaves
//! this adapter, and a disabled policy is distinct from a failed network read.
use super::agents::AgentRequest;
use super::{ManagementCommand, NativeError, NativeManagement, failure};
use hagency_store::management::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStage<T> {
    pub supported: bool,
    pub observed_at_ms: Option<i64>,
    pub record: Option<T>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAssignmentStatus {
    pub id: String,
    pub request_id: String,
    pub agent_id: String,
    pub state: String,
    pub generation: i64,
    pub execution_available: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMembershipStatus {
    pub assignment_id: String,
    pub request_id: String,
    pub agent_id: String,
    pub puppet_mxid: String,
    pub room_id: String,
    pub state: String,
    pub verified_at_ms: Option<i64>,
    pub cleanup_observed_at_ms: Option<i64>,
    pub blocking_reason: Option<String>,
    pub work_available: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeStatus {
    pub assignment_id: String,
    pub state: String,
    pub process_state: String,
    pub provision_expires_at_ms: i64,
    pub work_available: bool,
    pub capacity_released: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExecutionStatus {
    pub assignment_id: String,
    pub state: String,
    pub policy: String,
    pub activity_expires_at_ms: Option<i64>,
    pub work_available: bool,
    pub capacity_released: bool,
}
impl AgentExecutionStatus {
    fn validate(&self) -> Result<(), NativeError> {
        super::super::key(&self.assignment_id).map_err(|_| invalid())?;
        if self.policy != "codex_text_v1"
            || !matches!(
                self.state.as_str(),
                "pending"
                    | "issued"
                    | "qualified"
                    | "stopped"
                    | "outcome_unknown"
                    | "cleanup_pending"
            )
            || self.capacity_released
        {
            return Err(invalid());
        }
        if let Some(expiry) = self.activity_expires_at_ms {
            timestamp(expiry)?;
        }
        if self.work_available
            && (self.state != "qualified" || self.activity_expires_at_ms.is_none())
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub protocol: String,
    pub request_observed_at_ms: i64,
    pub agent_request: AgentRequest,
    pub assignment: AgentStage<AgentAssignmentStatus>,
    pub membership: AgentStage<AgentMembershipStatus>,
    pub runtime: AgentStage<AgentRuntimeStatus>,
    /// Old cached projections omit this independent shared-process observation.
    #[serde(default)]
    pub execution: Option<AgentStage<AgentExecutionStatus>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestReply {
    agent_request: AgentRequest,
    observed_at_ms: i64,
}
fn invalid() -> NativeError {
    failure(503, "management_agent_status_invalid")
}
fn timestamp(value: i64) -> Result<i64, NativeError> {
    if !(1..=9_007_199_254_740_991).contains(&value) {
        return Err(invalid());
    }
    Ok(value)
}
fn stage<T: for<'de> Deserialize<'de>>(
    result: Result<Value, NativeError>,
    field: &str,
    disabled: &str,
) -> Result<AgentStage<T>, NativeError> {
    match result {
        Err(e) if e.status == 503 && e.code == disabled => Ok(AgentStage {
            supported: false,
            observed_at_ms: None,
            record: None,
        }),
        Err(e) => Err(e),
        Ok(value) => {
            let object = value.as_object().ok_or_else(invalid)?;
            let record = object.get(field).ok_or_else(invalid)?.clone();
            let at = timestamp(
                object
                    .get("observedAtMs")
                    .and_then(Value::as_i64)
                    .ok_or_else(invalid)?,
            )?;
            Ok(AgentStage {
                supported: true,
                observed_at_ms: Some(at),
                record: serde_json::from_value(record).map_err(|_| invalid())?,
            })
        }
    }
}
/// Drop future/unrelated fields at the fixed-route boundary as well as in the
/// composed status projection. Receipt objects are deliberately not modelled.
pub(super) fn project_reply(
    command: &ManagementCommand,
    value: Value,
) -> Result<Value, NativeError> {
    let observed_at_ms = value
        .get("observedAtMs")
        .and_then(Value::as_i64)
        .ok_or_else(invalid)?;
    let (field, record) = match command {
        ManagementCommand::AgentAssignment { .. } => (
            "assignment",
            json!(stage::<AgentAssignmentStatus>(Ok(value), "assignment", "")?.record),
        ),
        ManagementCommand::AgentMembership { .. } => (
            "membership",
            json!(stage::<AgentMembershipStatus>(Ok(value), "membership", "")?.record),
        ),
        ManagementCommand::AgentRuntime { .. } => (
            "runtime",
            json!(stage::<AgentRuntimeStatus>(Ok(value), "runtime", "")?.record),
        ),
        ManagementCommand::AgentExecution { .. } => {
            let execution = stage::<AgentExecutionStatus>(Ok(value), "execution", "")?;
            if let Some(record) = &execution.record {
                record.validate()?;
            }
            ("execution", json!(execution.record))
        }
        _ => return Ok(value),
    };
    Ok(json!({field:record,"observedAtMs":observed_at_ms}))
}
impl NativeManagement {
    pub async fn agent_status(&self, id: String) -> Result<AgentStatus, NativeError> {
        super::super::key(&id)?;
        let (scope, session) = self.authorized().await?;
        for permission in [Permission::ReadAgentRequests, Permission::ReadAgentStatus] {
            session
                .authorize(&scope, permission)
                .map_err(|_| failure(403, "management_permission_required"))?;
        }
        let original: RequestReply = serde_json::from_value(
            self.central(ManagementCommand::Agent { id: id.clone() })
                .await?,
        )
        .map_err(|_| invalid())?;
        if original.agent_request.id != id {
            return Err(invalid());
        }
        timestamp(original.observed_at_ms)?;
        let assignment = stage::<AgentAssignmentStatus>(
            self.central(ManagementCommand::AgentAssignment { id: id.clone() })
                .await,
            "assignment",
            "agent_executor_unavailable",
        )?;
        let membership = stage::<AgentMembershipStatus>(
            self.central(ManagementCommand::AgentMembership { id: id.clone() })
                .await,
            "membership",
            "agent_matrix_provisioning_unavailable",
        )?;
        let runtime = stage::<AgentRuntimeStatus>(
            self.central(ManagementCommand::AgentRuntime { id: id.clone() })
                .await,
            "runtime",
            "agent_runtime_provisioning_unavailable",
        )?;
        let execution = stage::<AgentExecutionStatus>(
            self.central(ManagementCommand::AgentExecution { id }).await,
            "execution",
            "shared_execution_unavailable",
        )?;
        // Stages can advance between reads. Compare only immutable request/agent
        // identities, not stage revisions or assignment snapshots across reads.
        for (request, agent) in assignment
            .record
            .as_ref()
            .map(|r| (&r.request_id, &r.agent_id))
            .into_iter()
            .chain(
                membership
                    .record
                    .as_ref()
                    .map(|r| (&r.request_id, &r.agent_id)),
            )
        {
            if request != &original.agent_request.id
                || agent != &original.agent_request.proposed_agent_id
            {
                return Err(invalid());
            }
        }
        let (current, current_session) = self.authorized().await?;
        if current != scope {
            return Err(failure(401, "management_scope_changed"));
        }
        for permission in [Permission::ReadAgentRequests, Permission::ReadAgentStatus] {
            current_session
                .authorize(&current, permission)
                .map_err(|_| failure(403, "management_permission_required"))?;
        }
        Ok(AgentStatus {
            protocol: "hagency-management-v1".into(),
            request_observed_at_ms: original.observed_at_ms,
            agent_request: original.agent_request,
            assignment,
            membership,
            runtime,
            execution: Some(execution),
        })
    }
}
