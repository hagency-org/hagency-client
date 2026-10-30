//! Closed phase-A delta evidence; it is not a model execution capability.
use super::*;
pub const AGENT_BUDGET_CAPABILITY: &str = "hagency.managed_agents.budget.v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBudgetCommand {
    /// Exact resource-owner consent to Agent-specific additional capacity.
    /// The original pool grant stays immutable; machine/Seat limits still apply.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub owner_funded: bool,
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub top_up_id: String,
    pub top_up_request_digest: String,
    pub approval_command_id: String,
    pub approval_digest: String,
    pub approving_administrator_mxid: String,
    pub authority_revision: u64,
    pub assignment_id: String,
    pub assignment_generation: u64,
    pub reserve_command: ManagedAgentReserveCommand,
    pub reserve_command_digest: String,
    pub reserve_receipt: ManagedAgentReservationReceipt,
    pub reserve_receipt_digest: String,
    pub expected_budget_revision: u64,
    pub expected_budget_digest: String,
    pub expected_ceiling_tokens: u64,
    pub add_tokens: u64,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}
fn key(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}
pub(super) fn digest<T: Serialize>(value: &T) -> Result<String, Error> {
    Ok(canonical::digest(&serde_json::to_value(value)?)?)
}
impl AgentBudgetCommand {
    pub fn validate(&self, owner: &str) -> Result<(), Error> {
        for id in [
            &self.command_id,
            &self.top_up_id,
            &self.assignment_id,
            &self.approval_command_id,
        ] {
            key(id)?;
        }
        for value in [
            &self.top_up_request_digest,
            &self.approval_digest,
            &self.expected_budget_digest,
        ] {
            hash(value)?;
        }
        for n in [
            self.authority_revision,
            self.assignment_generation,
            self.expected_budget_revision,
            self.expected_ceiling_tokens,
            self.add_tokens,
            self.issued_at_ms,
            self.expires_at_ms,
        ] {
            exact(n)?;
        }
        self.reserve_command.validate(owner)?;
        local_user(&self.approving_administrator_mxid, owner)?;
        let c = &self.reserve_command;
        let r = &self.reserve_receipt;
        let child = r
            .agent_reservation_id
            .as_deref()
            .ok_or(Error::LocalAuthority)?;
        key(child)?;
        exact(r.ledger_revision)?;
        if self.version != if self.owner_funded { 2 } else { 1 }
            || self.owner_funded && c.pool_grant.is_none()
            || self.capability != AGENT_BUDGET_CAPABILITY
            || self.operation != "agent.budget.increase"
            || self.assignment_generation != 1
            || self.expected_budget_revision >= JSON_SAFE_MAX
            || self.expected_ceiling_tokens < c.requested_tokens
            || self
                .expected_ceiling_tokens
                .checked_add(self.add_tokens)
                .is_none_or(|v| v > JSON_SAFE_MAX)
            || self.expires_at_ms <= self.issued_at_ms
            || (c.pool_grant.is_none() && self.expires_at_ms > c.expires_at_ms)
            || self.reserve_command_digest != digest(c)?
            || self.reserve_receipt_digest != digest(r)?
            || c.pool_grant.as_ref().is_some_and(|g|
                self.approving_administrator_mxid != g.owner_mxid
                    || self.authority_revision != g.grant_revision)
            || r.version != 1
            || r.state != "reserved"
            || r.budget_revision != 1
            || r.command_id != c.command_id
            || r.command_digest != self.reserve_command_digest
            || r.request_id != c.request_id
            || r.agent_id != c.proposed_agent_id
            || r.allocation_id != c.allocation_id
            || r.definition_digest != c.definition_digest
            || r.period_key != c.period_key
            || r.allocated_tokens != c.requested_tokens
            || r.retained_tokens != c.requested_tokens
            || r.reason.is_some()
            || serde_json::to_vec(self)?.len() > 16 * 1024
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    pub(super) fn child(&self) -> Result<&str, Error> {
        self.reserve_receipt
            .agent_reservation_id
            .as_deref()
            .ok_or(Error::LocalAuthority)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBudgetReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub top_up_id: String,
    pub agent_reservation_id: String,
    pub assignment_id: String,
    pub original_reserve_receipt_digest: String,
    pub state: String,
    pub expected_budget_revision: u64,
    pub previous_budget_digest: String,
    pub budget_revision: u64,
    pub budget_digest: String,
    pub previous_ceiling_tokens: u64,
    pub applied_delta_tokens: u64,
    pub effective_ceiling_tokens: u64,
    pub ledger_revision: u64,
    pub period_key: String,
    pub reason: Option<String>,
}
pub fn initial_budget_digest(receipt: &ManagedAgentReservationReceipt) -> Result<String, Error> {
    if receipt.state != "reserved" || receipt.budget_revision != 1 {
        return Err(Error::LocalAuthority);
    }
    digest(
        &json!({"version":1,"originalReserveReceiptDigest":digest(receipt)?,"budgetRevision":1,"effectiveCeilingTokens":receipt.allocated_tokens}),
    )
}
pub fn next_budget_digest(
    command: &AgentBudgetCommand,
    ledger_revision: u64,
) -> Result<String, Error> {
    exact(ledger_revision)?;
    let next = command
        .expected_budget_revision
        .checked_add(1)
        .filter(|v| *v <= JSON_SAFE_MAX)
        .ok_or(Error::Capacity)?;
    let ceiling = command
        .expected_ceiling_tokens
        .checked_add(command.add_tokens)
        .filter(|v| *v <= JSON_SAFE_MAX)
        .ok_or(Error::Capacity)?;
    digest(
        &json!({"version":1,"previousBudgetDigest":command.expected_budget_digest,"commandDigest":digest(command)?,"budgetRevision":next,"effectiveCeilingTokens":ceiling,"ledgerRevision":ledger_revision}),
    )
}
impl AgentBudgetReceipt {
    pub fn validate_for(&self, c: &AgentBudgetCommand) -> Result<(), Error> {
        let applied = self.state == "applied";
        let delta = if applied { c.add_tokens } else { 0 };
        let revision = c
            .expected_budget_revision
            .checked_add(u64::from(applied))
            .filter(|v| *v <= JSON_SAFE_MAX)
            .ok_or(Error::Capacity)?;
        let ceiling = c
            .expected_ceiling_tokens
            .checked_add(delta)
            .filter(|v| *v <= JSON_SAFE_MAX)
            .ok_or(Error::Capacity)?;
        let head = if applied {
            next_budget_digest(c, self.ledger_revision)?
        } else {
            c.expected_budget_digest.clone()
        };
        exact(self.ledger_revision)?;
        if self.version != 1
            || self.command_id != c.command_id
            || self.command_digest != digest(c)?
            || self.top_up_id != c.top_up_id
            || self.agent_reservation_id != c.child()?
            || self.assignment_id != c.assignment_id
            || self.original_reserve_receipt_digest != c.reserve_receipt_digest
            || self.expected_budget_revision != c.expected_budget_revision
            || self.previous_budget_digest != c.expected_budget_digest
            || self.previous_ceiling_tokens != c.expected_ceiling_tokens
            || self.period_key != c.reserve_command.period_key
            || self.ledger_revision <= c.reserve_receipt.ledger_revision
            || !matches!(
                self.state.as_str(),
                "applied" | "refused" | "cancelled_before_apply"
            )
            || self.budget_revision != revision
            || self.applied_delta_tokens != delta
            || self.effective_ceiling_tokens != ceiling
            || self.budget_digest != head
            || (applied && self.reason.is_some())
            || (!applied
                && self
                    .reason
                    .as_ref()
                    .is_none_or(|s| s.is_empty() || s.len() > 80))
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBudgetDelivery {
    pub command: AgentBudgetCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: u64,
    pub application_allowed: bool,
    pub cancellation_requested: bool,
    pub recorded: Option<AgentBudgetReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBudgetResult {
    pub command_digest: String,
    pub token: String,
    pub receipt: AgentBudgetReceipt,
}
