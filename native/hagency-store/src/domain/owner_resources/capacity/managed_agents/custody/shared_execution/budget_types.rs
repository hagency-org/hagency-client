//! Closed v3 wire descriptions. No deserialized value creates native custody.
use super::*;
fn allocation_digest<T: Serialize>(value: &T) -> Result<String, Error> {
    Ok(canonical::digest(&serde_json::to_value(value)?)?)
}
fn valid_digest(value: &str) -> bool {
    hash(value).is_ok()
}
fn validate_predecessor(p: &SharedAttemptPredecessor) -> Result<(), Error> {
    identifier(&p.command_id, 128)?;
    exact(p.process_attempt)?;
    // V3 only: empty processDigest requires a verified not_started receipt.
    if !p.process_digest.is_empty() { hash(&p.process_digest)?; }
    for h in [
        &p.command_digest,
        &p.stopped_receipt_digest,
    ] {
        hash(h)?;
    }
    Ok(())
}
fn validate_observation(b: &SharedBudgetObservation) -> Result<(), Error> {
    exact(b.observed_ledger_revision)?;
    hash(&b.liability_digest)?;
    if [
        b.allocated_tokens,
        b.known_consumed_tokens,
        b.retained_tokens,
        b.remaining_tokens,
    ]
    .into_iter()
    .any(|n| n > JSON_SAFE_MAX)
        || b.retained_tokens < b.known_consumed_tokens
        || b.usage_complete && b.retained_tokens != b.known_consumed_tokens
        || b.remaining_tokens != b.allocated_tokens.saturating_sub(b.retained_tokens)
    {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}
pub const SHARED_BUDGET_CAPABILITY: &str = "hagency.managed_agents.execution.v3";
pub const SHARED_BUDGET_DISCOVERY: &str = "shared-budget-execution-v1";
fn invalid() -> Error {
    Error::LocalAuthority
}
fn positive(n: u64) -> bool {
    (1..=JSON_SAFE_MAX).contains(&n)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedExecutionBudget {
    pub version: u16,
    pub original_reserve_receipt_digest: String,
    pub budget_revision: u64,
    pub budget_digest: String,
    pub effective_ceiling_tokens: u64,
    pub applied_command_id: String,
    pub applied_command_digest: String,
    pub applied_receipt_digest: String,
}
impl SharedExecutionBudget {
    pub fn validate(&self) -> Result<(), Error> {
        identifier(&self.applied_command_id, 128)?;
        if self.version != 1
            || !(2..=JSON_SAFE_MAX).contains(&self.budget_revision)
            || !positive(self.effective_ceiling_tokens)
            || [
                &self.original_reserve_receipt_digest,
                &self.budget_digest,
                &self.applied_command_digest,
                &self.applied_receipt_digest,
            ]
            .iter()
            .any(|v| !valid_digest(v))
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetAttemptStartInput {
    /// Explicit owner opt-in, pinned in this successor start and command.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web_search: bool,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub tool_policy: Option<SharedAgentToolPolicy>,
    pub start_intent_id: String,
    pub assignment_id: String,
    pub predecessor: SharedAttemptPredecessor,
    pub execution_budget: SharedExecutionBudget,
    pub budget_observation: SharedBudgetObservation,
}
impl SharedBudgetAttemptStartInput {
    pub fn validate(&self) -> Result<(), Error> {
        if let Some(p)=&self.tool_policy {p.validate()?;}
        identifier(&self.start_intent_id, 128)?;
        identifier(&self.assignment_id, 128)?;
        validate_predecessor(&self.predecessor)?;
        self.execution_budget.validate()?;
        validate_observation(&self.budget_observation)?;
        if self.budget_observation.allocated_tokens
            != self.execution_budget.effective_ceiling_tokens
            || self.budget_observation.remaining_tokens == 0
            || serde_json::to_vec(self).map_or(true, |v| v.len() > 16 * 1024)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetAttemptCommand {
    /// Explicit owner opt-in, pinned in this successor start and command.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web_search: bool,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub tool_policy: Option<SharedAgentToolPolicy>,
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub assignment_id: String,
    pub assignment_generation: u64,
    pub agent_reservation_id: String,
    pub process_attempt: u64,
    pub original_runtime_command: SharedRuntimeCommand,
    pub original_runtime_digest: String,
    pub predecessor: SharedAttemptPredecessor,
    pub execution_budget: SharedExecutionBudget,
    pub budget_observation: SharedBudgetObservation,
    pub start_intent_id: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}
impl SharedBudgetAttemptCommand {
    pub fn validate(&self, server: &str) -> Result<(), Error> {
        if let Some(p)=&self.tool_policy {p.validate()?;}
        for id in [
            &self.command_id,
            &self.assignment_id,
            &self.agent_reservation_id,
            &self.start_intent_id,
        ] {
            identifier(id, 128)?;
        }
        self.original_runtime_command.validate(server)?;
        validate_predecessor(&self.predecessor)?;
        self.execution_budget.validate()?;
        validate_observation(&self.budget_observation)?;
        let original = &self.original_runtime_command;
        if self.version != 3
            || self.capability != SHARED_BUDGET_CAPABILITY
            || self.operation != "agent.execution.provision"
            || self.assignment_id != original.assignment_id
            || self.assignment_generation != original.assignment_generation
            || self.agent_reservation_id != original.agent_reservation_id
            || self.original_runtime_digest != allocation_digest(original)?
            || !(2..=JSON_SAFE_MAX).contains(&self.process_attempt)
            || self.predecessor.process_attempt != self.process_attempt - 1
            || self.command_id == self.predecessor.command_id
            || self.command_id == original.command_id
            || self.process_attempt == 2
                && (self.predecessor.command_id != original.command_id
                    || self.predecessor.command_digest != self.original_runtime_digest)
            || self.execution_budget.original_reserve_receipt_digest
                != original.reserve_receipt_digest
            || self.execution_budget.effective_ceiling_tokens
                <= original.reserve_command.requested_tokens
            || self.budget_observation.allocated_tokens
                != self.execution_budget.effective_ceiling_tokens
            || self.budget_observation.remaining_tokens == 0
            || !positive(self.issued_at_ms)
            || !positive(self.expires_at_ms)
            || self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms - self.issued_at_ms > 30_000
            || self.expires_at_ms > original.reserve_command.expires_at_ms
            || serde_json::to_vec(self).map_or(true, |v| v.len() > 32 * 1024)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetAttemptProcessReceipt {
    pub version: u16,
    pub observation_id: String,
    pub command_id: String,
    pub command_digest: String,
    pub assignment_id: String,
    pub agent_reservation_id: String,
    pub process_attempt: u64,
    pub original_runtime_digest: String,
    pub budget_revision: u64,
    pub budget_digest: String,
    pub ledger_revision: u64,
    pub process_digest: Option<String>,
    pub state: String,
    pub policy_digest: Option<String>,
}
impl SharedBudgetAttemptProcessReceipt {
    pub fn validate_for(&self, c: &SharedBudgetAttemptCommand) -> Result<(), Error> {
        identifier(&self.observation_id, 128)?;
        if self.version != 3
            || self.command_id != c.command_id
            || self.command_digest != allocation_digest(c)?
            || self.assignment_id != c.assignment_id
            || self.agent_reservation_id != c.agent_reservation_id
            || self.process_attempt != c.process_attempt
            || self.original_runtime_digest != c.original_runtime_digest
            || self.budget_revision != c.execution_budget.budget_revision
            || self.budget_digest != c.execution_budget.budget_digest
            || !positive(self.ledger_revision)
            || !matches!(self.state.as_str(), "qualified" | "stopped" | "unknown" | "not_started")
            || self
                .process_digest
                .as_deref()
                .is_some_and(|v| !valid_digest(v))
            || self
                .policy_digest
                .as_deref()
                .is_some_and(|v| !valid_digest(v))
            || self.state == "qualified"
                && (self.process_digest.is_none() || self.policy_digest.is_none())
            || self.state == "stopped" && self.process_digest.is_none()
            || self.state == "not_started" && self.process_digest.is_some()
            || self.state != "qualified" && self.policy_digest.is_some()
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetAttemptDelivery {
    pub command: SharedBudgetAttemptCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: u64,
    pub execution_allowed: bool,
    pub cleanup_allowed: bool,
    pub recorded: Option<SharedBudgetAttemptProcessReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetAttemptResultInput {
    pub command_digest: String,
    pub token: String,
    pub receipt: SharedBudgetAttemptProcessReceipt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedOriginalBudgetBlock {
    pub reason: String,
    pub payload_digest: String,
}
impl SharedOriginalBudgetBlock {
    pub fn validate(&self) -> Result<(), Error> {
        if !matches!(
            self.reason.as_str(),
            "budget_blocked" | "usage_unrepresentable" | "head_changed"
        ) || !valid_digest(&self.payload_digest)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetDemandStartInput {
    pub demand_digest: String,
    pub expected_control_revision: u64,
    pub original_block: Option<SharedOriginalBudgetBlock>,
    pub start: SharedBudgetAttemptStartInput,
}
impl SharedBudgetDemandStartInput {
    pub fn validate_for(&self, o: &SharedDemandOffer) -> Result<(), Error> {
        o.validate()?;
        self.start.validate()?;
        if let Some(b) = &self.original_block {
            b.validate()?;
            if b.reason != "budget_blocked" {
                return Err(invalid());
            }
        }
        if self.demand_digest != allocation_digest(o)?
            || self.expected_control_revision != o.control_revision
            || self.start.assignment_id != o.assignment_id
            || self.start.predecessor.command_id != o.predecessor_command_id
            || self.start.predecessor.command_digest != o.predecessor_command_digest
            || serde_json::to_vec(self).map_or(true, |v| v.len() > 16 * 1024)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetDemandStatus {
    pub offer: SharedDemandOffer,
    pub demand_digest: String,
    pub state: String,
    pub execution_budget: SharedExecutionBudget,
    pub original_block: Option<SharedOriginalBudgetBlock>,
    pub runtime_command_id: Option<String>,
    pub binding: Option<SharedDemandBinding>,
}
impl SharedBudgetDemandStatus {
    pub fn validate(&self) -> Result<(), Error> {
        self.execution_budget.validate()?;
        if self.offer.digest()? != self.demand_digest
            || !matches!(self.state.as_str(), "offered" | "start_committed" | "bound" | "cancelled" | "expired" | "head_changed" | "budget_blocked" | "usage_unrepresentable" | "blocked")
        { return Err(invalid()); }
        if let Some(block) = &self.original_block { block.validate()?; }
        if let Some(id) = &self.runtime_command_id { identifier(id, 128)?; }
        if (self.state == "bound") != self.binding.is_some()
            || self.state == "start_committed" && self.runtime_command_id.is_none()
        { return Err(invalid()); }
        if let Some(b) = &self.binding {
            if b.version != 1 || b.demand_id != self.offer.demand_id || b.demand_digest != self.demand_digest
                || b.assignment_id != self.offer.assignment_id || b.event_id != self.offer.event_id
                || b.received_at_ms != self.offer.received_at_ms || b.control_revision != self.offer.control_revision
                || self.runtime_command_id.as_ref() != Some(&b.runtime_command_id)
            { return Err(invalid()); }
            identifier(&b.work_id, 128)?; hash(&b.work_digest)?; hash(&b.runtime_command_digest)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetDemandPoll {
    pub demand: Option<SharedBudgetDemandStatus>,
    pub next_after: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetDemandBlockInput {
    pub demand_digest: String,
    pub expected_control_revision: u64,
    pub original_block: Option<SharedOriginalBudgetBlock>,
    pub execution_budget: SharedExecutionBudget,
    pub reason: String,
    pub budget: Option<SharedBudgetObservation>,
}
impl SharedBudgetDemandBlockInput {
    pub fn validate_for(&self, offer: &SharedDemandOffer) -> Result<(), Error> {
        self.execution_budget.validate()?;
        if self.demand_digest != offer.digest()? || self.expected_control_revision != offer.control_revision {
            return Err(invalid());
        }
        if let Some(block) = &self.original_block {
            block.validate()?;
            if block.reason != "budget_blocked" { return Err(invalid()); }
        }
        match (self.reason.as_str(), &self.budget) {
            ("budget_blocked", Some(b)) => {
                exact(b.observed_ledger_revision)?; hash(&b.liability_digest)?;
                if b.allocated_tokens != self.execution_budget.effective_ceiling_tokens || b.remaining_tokens != 0
                    || b.retained_tokens < b.allocated_tokens || b.retained_tokens < b.known_consumed_tokens
                    || b.retained_tokens > JSON_SAFE_MAX || b.usage_complete && b.retained_tokens != b.known_consumed_tokens
                { return Err(invalid()); }
            }
            ("usage_unrepresentable", None) => (),
            _ => return Err(invalid()),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetWorkRefusalInput {
    pub runtime_command_digest: String,
    pub work_digest: String,
    pub input_digest: String,
}
impl SharedBudgetWorkRefusalInput {
    pub fn validate(&self) -> Result<(), Error> {
        if [
            &self.runtime_command_digest,
            &self.work_digest,
            &self.input_digest,
        ]
        .iter()
        .any(|v| !valid_digest(v))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
/// Server evidence that this exact work was permanently fenced before any
/// accepted start. Native must separately prove its exact call was unissued.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetWorkRefusal {
    pub version: u16,
    pub refusal_id: String,
    pub work_id: String,
    pub work_digest: String,
    pub runtime_command_id: String,
    pub runtime_command_digest: String,
    pub input_digest: String,
    pub execution_budget: SharedExecutionBudget,
    pub current_budget_digest: String,
    pub state: String,
    pub reason: String,
    pub refused_at_ms: u64,
}
impl SharedBudgetWorkRefusal {
    pub fn validate_for(
        &self,
        c: &SharedBudgetAttemptCommand,
        w: &SharedWorkItem,
        input_digest: &str,
    ) -> Result<(), Error> {
        w.validate()?;
        self.execution_budget.validate()?;
        identifier(&self.refusal_id, 128)?;
        if self.version != 1
            || self.refusal_id != format!("sbr_{}", w.id)
            || self.work_id != w.id
            || self.work_digest != allocation_digest(w)?
            || w.assignment_id != c.assignment_id
            || w.runtime_command_digest != allocation_digest(c)?
            || self.runtime_command_id != c.command_id
            || self.runtime_command_digest != w.runtime_command_digest
            || self.input_digest != input_digest
            || !valid_digest(input_digest)
            || self.execution_budget != c.execution_budget
            || !valid_digest(&self.current_budget_digest)
            || self.current_budget_digest == self.execution_budget.budget_digest
            || self.state != "not_started"
            || self.reason != "budget_head_changed"
            || !positive(self.refused_at_ms)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetWorkRefusalRecord {
    pub refusal: SharedBudgetWorkRefusal,
    pub refusal_digest: String,
}

#[cfg(test)]
#[path = "budget_type_tests.rs"]
mod tests;
