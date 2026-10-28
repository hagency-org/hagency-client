//! Closed schema17 B routes. Offers and bindings describe history, not launch authority.
use super::*;
use serde_json::{Value, json};
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Status {
    pub offer: SharedDemandOffer,
    pub demand_digest: String,
    pub state: String,
    pub runtime_command_id: Option<String>,
    pub binding: Option<SharedDemandBinding>,
}
impl Status {
    pub fn validate(&self) -> Result<(), NativeError> {
        if self.offer.digest().map_err(store)? != self.demand_digest
            || !matches!(
                self.state.as_str(),
                "offered"
                    | "start_committed"
                    | "bound"
                    | "cancelled"
                    | "expired"
                    | "head_changed"
                    | "budget_blocked"
                    | "usage_unrepresentable"
                    | "blocked"
            )
        {
            return Err(invalid());
        }
        if let Some(id) = &self.runtime_command_id {
            super::super::key(id)?;
        }
        if let Some(b) = &self.binding
            && (b.version != 1
                || b.demand_id != self.offer.demand_id
                || b.demand_digest != self.demand_digest
                || b.assignment_id != self.offer.assignment_id
                || b.event_id != self.offer.event_id
                || b.received_at_ms != self.offer.received_at_ms
                || b.control_revision != self.offer.control_revision
                || self.runtime_command_id.as_ref() != Some(&b.runtime_command_id))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PollReply {
    pub demand: Option<Status>,
    pub next_after: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoverReply {
    pub demand: Option<Status>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkBinding {
    pub offer: SharedDemandOffer,
    pub demand_digest: String,
    pub binding: SharedDemandBinding,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BindingReply {
    pub demand: Option<WorkBinding>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BlockInput {
    pub demand_digest: String,
    pub expected_control_revision: u64,
    pub reason: String,
    pub budget: Option<SharedBudgetObservation>,
}
pub(super) enum DemandAction {
    Discovery,
    Control {
        assignment: String,
    },
    Stop {
        assignment: String,
        input: SharedDemandStopInput,
    },
    Poll {
        after: Option<String>,
    },
    Recover {
        id: String,
        digest: String,
    },
    Binding {
        work: String,
        work_digest: String,
        runtime_digest: String,
    },
    Block {
        id: String,
        input: BlockInput,
    },
}
impl DemandAction {
    pub(super) fn validate(&self) -> Result<(), NativeError> {
        if !cfg!(feature = "shared-intake") {
            return Err(failure(409, "shared_intake_disabled"));
        }
        match self {
            Self::Discovery => {}
            Self::Control { assignment } => super::super::key(assignment)?,
            Self::Stop { assignment, input } => {
                super::super::key(assignment)?;
                super::super::key(&input.command_id)?;
                if input.expected_revision == 0
                    || input.expected_revision >= 9_007_199_254_740_991
                    || input.mode != "paused"
                {
                    return Err(invalid());
                }
            }
            Self::Poll { after } => {
                if let Some(id) = after {
                    super::super::key(id)?;
                }
            }
            Self::Recover { id, digest } => {
                super::super::key(id)?;
                hash(digest)?;
            }
            Self::Binding {
                work,
                work_digest,
                runtime_digest,
            } => {
                super::super::key(work)?;
                hash(work_digest)?;
                hash(runtime_digest)?;
            }
            Self::Block { id, input } => {
                super::super::key(id)?;
                hash(&input.demand_digest)?;
                if input.expected_control_revision == 0
                    || input.expected_control_revision > 9_007_199_254_740_991
                    || !matches!(
                        input.reason.as_str(),
                        "budget_blocked" | "usage_unrepresentable"
                    )
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
    pub(super) fn body(self) -> (String, Value) {
        match self {
            Self::Discovery => (String::new(), Value::Null),
            Self::Control { assignment } => {
                (format!("assignments/{assignment}/control"), Value::Null)
            }
            Self::Stop { assignment, input } => {
                (format!("assignments/{assignment}/control"), json!(input))
            }
            Self::Poll { after } => ("demands/poll".into(), json!({"after":after})),
            Self::Recover { id, digest } => (
                format!("demands/{id}/recover"),
                json!({"demandDigest":digest}),
            ),
            Self::Binding {
                work,
                work_digest,
                runtime_digest,
            } => (
                format!("work/{work}/demand"),
                json!({"workDigest":work_digest,"runtimeCommandDigest":runtime_digest}),
            ),
            Self::Block { id, input } => (format!("demands/{id}/block"), json!(input)),
        }
    }
}
impl From<DemandAction> for Action {
    fn from(v: DemandAction) -> Self {
        Self::Demand(Box::new(v))
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunControl {
    pub assignment_id: String,
    pub revision: u64,
    pub mode: String,
    pub running_since_ms: Option<u64>,
    pub updated_at_ms: u64,
    pub origin: String,
}
impl RunControl {
    pub fn validate(&self, id: &str) -> Result<(), NativeError> {
        if self.assignment_id != id
            || self.revision == 0
            || self.revision >= 9_007_199_254_740_991
            || !matches!(self.mode.as_str(), "manual" | "running" | "paused")
            || self.updated_at_ms == 0
            || self.updated_at_ms > 9_007_199_254_740_991
            || self
                .running_since_ms
                .is_some_and(|v| v == 0 || v > self.updated_at_ms)
            || self.origin.len() > 128
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlReply {
    pub control: RunControl,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StopReply {
    pub command_id: String,
    pub applied_revision: u64,
    pub control: RunControl,
}

// Discovery is extensible; only this bounded protocol-capability projection is
// consumed. It never substitutes for authenticated B route policy checks.
#[derive(Deserialize)]
pub(super) struct Discovery {
    pub capabilities: Vec<String>,
}
pub(super) async fn paired(b: &Bound) -> Result<bool, NativeError> {
    let d = b.demand_discovery().await?;
    if d.capabilities.len() > 64 || d.capabilities.iter().any(|c| c.len() > 128) {
        return Err(invalid());
    }
    Ok(d.capabilities
        .iter()
        .any(|c| c == "shared-durable-demand-v1"))
}
pub(super) async fn require_pair(b: &Bound) -> Result<(), NativeError> {
    if !paired(b).await? {
        return Err(failure(409, "shared_intake_protocol_unavailable"));
    }
    Ok(())
}
