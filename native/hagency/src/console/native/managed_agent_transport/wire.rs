use super::*;
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Command {
    Reserve(ManagedAgentReserveCommand),
    Fence(ManagedAgentFenceCommand),
}
impl Command {
    pub fn id(&self) -> &str {
        match self {
            Self::Reserve(c) => &c.command_id,
            Self::Fence(c) => &c.command_id,
        }
    }
    pub fn reserve(&self) -> &ManagedAgentReserveCommand {
        match self {
            Self::Reserve(c) => c,
            Self::Fence(c) => &c.reserve_command,
        }
    }
    pub fn validate(&self, owner: &str) -> Result<(), NativeError> {
        match self {
            Self::Reserve(c) => c.validate(owner),
            Self::Fence(c) => c.validate(owner),
        }
        .map_err(|_| invalid())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Receipt {
    Reserve(ManagedAgentReservationReceipt),
    Fence(ManagedAgentFenceReceipt),
}
impl Receipt {
    pub fn revision(&self) -> u64 {
        match self {
            Self::Reserve(r) => r.ledger_revision,
            Self::Fence(r) => r.ledger_revision,
        }
    }
    pub fn validate(&self, c: &Command) -> Result<(), NativeError> {
        let hash = digest(c)?;
        let valid = match (self, c) {
            (Self::Reserve(r), Command::Reserve(c)) => {
                r.version == 1
                    && r.command_id == c.command_id
                    && r.command_digest == hash
                    && r.request_id == c.request_id
                    && r.agent_id == c.proposed_agent_id
                    && r.allocation_id == c.allocation_id
                    && r.definition_digest == c.definition_digest
                    && r.period_key == c.period_key
                    && match r.state.as_str() {
                        "reserved" => {
                            r.agent_reservation_id.is_some()
                                && r.budget_revision > 0
                                && r.allocated_tokens == c.requested_tokens
                                && r.retained_tokens == c.requested_tokens
                                && r.reason.is_none()
                        }
                        "refused" => {
                            r.agent_reservation_id.is_none()
                                && r.budget_revision == 0
                                && r.allocated_tokens == 0
                                && r.retained_tokens == 0
                                && r.reason.as_deref().is_some_and(|s| {
                                    matches!(
                                        s,
                                        "insufficient_capacity"
                                            | "authority_changed"
                                            | "invalid_request"
                                            | "project_unavailable"
                                            | "resource_unavailable"
                                            | "request_conflict"
                                            | "unsettled_usage"
                                    )
                                })
                        }
                        _ => false,
                    }
            }
            (Self::Fence(r), Command::Fence(c)) => {
                r.version == 1
                    && r.command_id == c.command_id
                    && r.command_digest == hash
                    && r.reserve_command_digest == c.reserve_command_digest
                    && r.request_id == c.reserve_command.request_id
                    && r.agent_id == c.reserve_command.proposed_agent_id
                    && r.fence_revision > 0
                    && r.ledger_revision >= r.fence_revision
                    && r.retained_tokens
                        .is_none_or(|n| n >= r.known_consumed_tokens)
                    && match r.state.as_str() {
                        "absent_fenced" => {
                            r.agent_reservation_id.is_none()
                                && r.retained_tokens == Some(0)
                                && r.known_consumed_tokens == 0
                                && r.usage_complete
                                && r.process_state == "not_started"
                        }
                        "fenced" => {
                            r.agent_reservation_id.is_some()
                                && matches!(
                                    r.process_state.as_str(),
                                    "not_started" | "stopped" | "unknown"
                                )
                                && (!r.usage_complete
                                    || r.process_state != "unknown" && r.retained_tokens.is_some())
                        }
                        _ => false,
                    }
            }
            _ => false,
        };
        if !valid || self.revision() == 0 || self.revision() > hagency_core::JSON_SAFE_MAX {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Assignment {
    pub id: String,
    pub request_id: String,
    pub agent_id: String,
    pub manager_mxid: String,
    pub executor_device_id: String,
    pub executor_generation: u64,
    pub agent_reservation_id: String,
    pub reserve_receipt_digest: String,
    pub state: String,
    pub generation: u64,
    pub execution_available: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Ack {
    pub command_id: String,
    pub command_digest: String,
    pub receipt_digest: String,
    pub assignment: Option<Assignment>,
    pub observed_at_ms: u64,
}
