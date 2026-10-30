//! Closed mirror of the central schema-8 admission contract; not execution authority.
use super::*;

pub const AGENT_CAPABILITY: &str = "hagency.managed_agents.v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentDefinition {
    pub name: String,
    pub capability: String,
    pub description: String,
    pub tool_mode: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentReserveCommand {
    pub version: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_grant: Option<NativeGrantReceipt>,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub request_id: String,
    pub request_revision: u64,
    pub proposed_agent_id: String,
    pub manager_mxid: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    #[serde(default, skip_serializing_if = "zero_revision")]
    pub project_revision: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub setup_id: String,
    #[serde(default, skip_serializing_if = "zero_revision")]
    pub setup_binding_revision: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain_project_id: String,
    pub room_id: String,
    pub authority_revision: u64,
    pub decision_actor_mxid: String,
    pub decision_digest: String,
    pub allocation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub allocation_receipt_digest: String,
    pub grant_receipt_digest: String,
    pub enrollment_id: String,
    pub executor_device_id: String,
    pub executor_generation: u64,
    pub resource_id: String,
    pub resource_revision: String,
    pub definition: ManagedAgentDefinition,
    pub definition_digest: String,
    pub requested_tokens: u64,
    pub period_key: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentReservationReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub request_id: String,
    pub agent_id: String,
    pub allocation_id: String,
    pub agent_reservation_id: Option<String>,
    pub definition_digest: String,
    pub state: String,
    pub budget_revision: u64,
    pub ledger_revision: u64,
    pub allocated_tokens: u64,
    pub retained_tokens: u64,
    pub period_key: String,
    pub reason: Option<String>,
}

fn wire_key(s: &str) -> Result<(), Error> {
    if s.is_empty()
        || s.len() > 128
        || !s
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}
fn zero_revision(value: &u64) -> bool { *value == 0 }
impl ManagedAgentReserveCommand {
    pub fn validate(&self, owner: &str) -> Result<(), Error> {
        for value in [&self.command_id, &self.request_id, &self.proposed_agent_id,
            &self.allocation_id, &self.enrollment_id, &self.executor_device_id, &self.resource_id] { wire_key(value)?; }
        for value in [self.request_revision, self.authority_revision, self.executor_generation,
            self.requested_tokens, self.issued_at_ms, self.expires_at_ms] { exact(value)?; }
        for value in [&self.definition_digest, &self.decision_digest, &self.grant_receipt_digest, &self.resource_revision] { hash(value)?; }
        local_user(&self.manager_mxid, owner)?;
        local_user(&self.decision_actor_mxid, owner)?;
        match (self.version, &self.pool_grant) {
            (1, None) => {
                if self.capability != AGENT_CAPABILITY { return Err(Error::LocalAuthority); }
                for value in [&self.project_id, &self.setup_id, &self.domain_project_id] { wire_key(value)?; }
                for value in [self.project_revision, self.setup_binding_revision] { exact(value)?; }
                hash(&self.allocation_receipt_digest)?;
                if self.room_id.is_empty() { return Err(Error::LocalAuthority); }
            }
            (2, Some(grant)) => {
                if self.room_id.is_empty() || self.capability != "hagency.pool_agents.v1"
                    || !self.project_id.is_empty() || !self.setup_id.is_empty()
                    || !self.domain_project_id.is_empty() || !self.allocation_receipt_digest.is_empty()
                    || self.project_revision != 0 || self.setup_binding_revision != 0
                    || !matches!(grant.version, 2 | 3) || !grant.supported_shape()
                    || grant.admission_request_id.is_none()
                    || grant.capability != CAPABILITY || grant.owner_mxid != grant.coordinator_mxid
                    || self.decision_actor_mxid != grant.owner_mxid
                    || !grant.eligible_managers.contains(&self.manager_mxid)
                    || self.allocation_id != self.request_id
                    || self.enrollment_id != grant.enrollment_id
                    || self.executor_device_id != grant.executor_device_id
                    || self.executor_generation != grant.executor_generation
                    || self.resource_id != grant.resource_id || self.resource_revision != grant.resource_revision
                    || self.authority_revision != grant.grant_revision
                    || self.period_key != grant.period_key || self.requested_tokens > grant.allocated_tokens
                    || self.issued_at_ms < grant.period_start_ms
                    || self.expires_at_ms > grant.period_end_ms.min(grant.delegation_expires_at_ms)
                    || canonical::digest(&serde_json::to_value(grant)?)? != self.grant_receipt_digest
                { return Err(Error::LocalAuthority); }
                wire_key(grant.admission_request_id.as_deref().ok_or(Error::LocalAuthority)?)?;
            }
            _ => return Err(Error::LocalAuthority),
        }
        let d = &self.definition;
        let valid_period = self.period_key.len() == 7 && self.period_key.as_bytes()[4] == b'-'
            && self.period_key[..4].bytes().all(|c| c.is_ascii_digit())
            && self.period_key[5..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n));
        if self.operation != "agent.reserve" || self.expires_at_ms <= self.issued_at_ms || !valid_period
            || (!self.room_id.is_empty() && (!self.room_id.starts_with('!') || !self.room_id.contains(':')
                || self.room_id.len() > 255 || self.room_id.chars().any(|c| c.is_control() || c.is_whitespace())))
            || d.name.trim().is_empty() || d.name.len() > 160 || d.name.chars().any(char::is_control)
            || d.description.len() > 2000 || d.description.chars().any(|c| c.is_control() && c != '\n')
            || d.capability.is_empty() || d.capability.len() > 80
            || !d.capability.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-/".contains(&c))
            || d.tool_mode != "none" || canonical::digest(&serde_json::to_value(d)?)? != self.definition_digest
            || serde_json::to_vec(self)?.len() > 16 * 1024 { return Err(Error::LocalAuthority); }
        Ok(())
    }
}
