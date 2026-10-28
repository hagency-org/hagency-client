//! Default-off native delta evidence. No provider launch or increased-budget call.
mod routes;
use super::allocation_transport::{AllocationTransport, Envelope, Operation, Scope};
use super::{NativeError, NativeManagement, failure};
use hagency_store::{management::Permission, owner_resources::budget::*, owner_resources::*};
use routes::Action;
pub(super) use routes::ServerOperation;
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
fn invalid() -> NativeError {
    failure(502, "invalid_agent_budget_response")
}
fn hash(s: &str) -> Result<(), NativeError> {
    if s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn digest(v: &impl Serialize) -> Result<String, NativeError> {
    hagency_core::canonical::digest(&serde_json::to_value(v).map_err(|_| invalid())?)
        .map_err(|_| invalid())
}
fn storage(e: hagency_store::Error) -> NativeError {
    super::management::store_error(e)
}
#[derive(Deserialize)]
struct Discovery {
    capabilities: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ack {
    receipt: AgentBudgetReceipt,
    observed_at_ms: u64,
}
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentBudgetProgress {
    Idle,
    HistoricalReceiptMissing {
        command_id: String,
    },
    Recorded {
        command_id: String,
        ledger_revision: u64,
        budget_revision: u64,
        disposition: String,
    },
}
pub struct AgentBudgetTransport {
    transport: AllocationTransport,
}
impl AgentBudgetTransport {
    pub fn new(manager: Arc<NativeManagement>) -> Self {
        Self {
            transport: AllocationTransport::new(manager),
        }
    }
    pub(super) fn restrict_to_host(mut self, gate: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.transport = self.transport.restrict_to_host(gate);
        self
    }
    pub async fn poll(
        &self,
        registry: &OwnerResourceRegistry,
        after: Option<String>,
    ) -> Result<AgentBudgetProgress, NativeError> {
        self.run(registry, Action::Poll { after }, None).await
    }
    pub async fn recover(
        &self,
        registry: &OwnerResourceRegistry,
        id: String,
        command_digest: String,
    ) -> Result<AgentBudgetProgress, NativeError> {
        self.run(
            registry,
            Action::Recover {
                id: id.clone(),
                digest: command_digest.clone(),
            },
            Some((id, command_digest)),
        )
        .await
    }
    async fn run(
        &self,
        registry: &OwnerResourceRegistry,
        action: Action,
        historical: Option<(String, String)>,
    ) -> Result<AgentBudgetProgress, NativeError> {
        let scope = self
            .transport
            .scope(Permission::ExecuteManagedAgents)
            .await?;
        let discovery: Discovery = self
            .transport
            .request(
                &scope,
                Operation::AgentBudget(ServerOperation::new(Action::Discovery)?),
                false,
            )
            .await?;
        if discovery.capabilities.len() > 64 || discovery.capabilities.iter().any(|c| c.len() > 128)
        {
            return Err(invalid());
        }
        if !discovery
            .capabilities
            .iter()
            .any(|c| c == "managed-agent-budget-delta-v1")
        {
            return Err(failure(409, "agent_budget_protocol_unavailable"));
        }
        let started = Instant::now();
        let reply: Envelope<AgentBudgetDelivery> = self
            .transport
            .request(
                &scope,
                Operation::AgentBudget(ServerOperation::new(action)?),
                false,
            )
            .await?;
        let Some(delivery) = reply.delivery else {
            if historical.is_some() {
                return Err(invalid());
            }
            return Ok(AgentBudgetProgress::Idle);
        };
        let until = deadline(
            &delivery,
            reply.observed_at_ms,
            started,
            &scope,
            historical.is_some(),
        )?;
        let c = &delivery.command;
        c.validate(scope.device.owner_mxid())
            .map_err(|_| invalid())?;
        if digest(c)? != delivery.command_digest
            || historical
                .as_ref()
                .is_some_and(|(id, d)| id != &c.command_id || d != &delivery.command_digest)
        {
            return Err(invalid());
        }
        let original = &c.reserve_command;
        if original.executor_device_id != scope.device.device_id()
            || original.executor_generation > scope.device.generation()
        {
            return Err(failure(409, "managed_agent_enrollment_changed"));
        }
        let grant = if let Some(grant) = &original.pool_grant { grant.clone() } else {
registry
            .managed_agent_project_history(
                scope.profile.clone(),
                scope.device.device_id().into(),
                original.allocation_id.clone(),
                original.allocation_receipt_digest.clone(),
            )
            .await
            .map_err(storage)?.grant_receipt
};
        self.transport.check_scope(&scope, false).await?;
        let cleanup = self
            .transport
            .enrollment(&scope, &grant, false)
            .await?;
        let saved = registry
            .recover_managed_budget(cleanup, c.clone())
            .await
            .map_err(storage)?;
        self.transport.check_scope(&scope, false).await?;
        let value = if let Some(saved) = saved {
            saved
        } else {
            if delivery.recorded.is_some() {
                return Err(failure(409, "native_agent_budget_receipt_missing"));
            }
            if !delivery.application_allowed && !delivery.cancellation_requested {
                return Ok(AgentBudgetProgress::HistoricalReceiptMissing {
                    command_id: c.command_id.clone(),
                });
            }
            let active = delivery.application_allowed && !delivery.cancellation_requested;
            if active && original.executor_generation != scope.device.generation() {
                return Err(failure(409, "managed_agent_enrollment_changed"));
            }
            let enrollment = self
                .transport
                .enrollment(&scope, &grant, active)
                .await?;
            self.transport.check_scope(&scope, false).await?;
            let proof = VerifiedAgentBudgetDelivery::new(
                enrollment,
                c.clone(),
                delivery.command_digest.clone(),
                delivery.application_allowed,
                delivery.cancellation_requested,
                until,
            )
            .map_err(storage)?;
            registry
                .apply_managed_budget(proof)
                .await
                .map_err(storage)?
        };
        self.transport.check_scope(&scope, true).await?;
        value.validate_for(c).map_err(|_| invalid())?;
        if let Some(recorded) = delivery.recorded {
            recorded.validate_for(c).map_err(|_| invalid())?;
            if digest(&recorded)? != digest(&value)? {
                return Err(failure(409, "agent_budget_receipt_conflict"));
            }
        } else {
            let ack: Ack = self
                .transport
                .upload(
                    &scope,
                    Operation::AgentBudget(ServerOperation::new(Action::Result {
                        id: c.command_id.clone(),
                        input: Box::new(AgentBudgetResult {
                            command_digest: delivery.command_digest,
                            token: delivery.token,
                            receipt: value.clone(),
                        }),
                    })?),
                    until,
                )
                .await?;
            if ack.observed_at_ms == 0
                || ack.observed_at_ms > hagency_core::JSON_SAFE_MAX
                || digest(&ack.receipt)? != digest(&value)?
            {
                return Err(failure(409, "agent_budget_acknowledgement_conflict"));
            }
        }
        Ok(AgentBudgetProgress::Recorded {
            command_id: c.command_id.clone(),
            ledger_revision: value.ledger_revision,
            budget_revision: value.budget_revision,
            disposition: value.state,
        })
    }
}
fn deadline(
    d: &AgentBudgetDelivery,
    observed: u64,
    started: Instant,
    scope: &Scope,
    historical: bool,
) -> Result<Instant, NativeError> {
    hash(&d.command_digest)?;
    hash(&d.token)?;
    if observed == 0
        || observed > hagency_core::JSON_SAFE_MAX
        || d.lease_until_ms > hagency_core::JSON_SAFE_MAX
        || (d.application_allowed
            && (historical
                || d.recorded.is_some()
                || d.cancellation_requested
                || d.lease_until_ms > d.command.expires_at_ms))
    {
        return Err(invalid());
    }
    let remaining = d
        .lease_until_ms
        .checked_sub(observed)
        .filter(|n| *n > 0 && *n <= 30000)
        .ok_or_else(invalid)?;
    let wall: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid())?
        .as_millis()
        .try_into()
        .map_err(|_| invalid())?;
    let wall_remaining = d
        .lease_until_ms
        .checked_sub(wall)
        .filter(|n| *n > 0)
        .ok_or_else(|| failure(409, "agent_budget_ticket_expired"))?;
    let until = (started + Duration::from_millis(remaining))
        .min(Instant::now() + Duration::from_millis(wall_remaining))
        .min(scope.deadline);
    if Instant::now() >= until {
        return Err(failure(409, "agent_budget_ticket_expired"));
    }
    Ok(until)
}
