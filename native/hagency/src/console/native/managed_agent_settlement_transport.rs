//! Post-fence ledger observations. This adapter has no provider execution path.
mod routes;
use super::allocation_transport::{AllocationTransport, Delivery, Envelope, Operation, lease};
use super::{NativeError, NativeManagement, failure};
use hagency_store::{management::Permission, owner_resources::*};
use routes::Action;
pub(super) use routes::ServerOperation;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Instant};

fn invalid() -> NativeError {
    failure(502, "invalid_agent_settlement_response")
}
fn digest(v: &impl Serialize) -> Result<String, NativeError> {
    hagency_core::canonical::digest(&serde_json::to_value(v).map_err(|_| invalid())?)
        .map_err(|_| invalid())
}
fn hash(value: &str) -> Result<(), NativeError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(invalid())
    }
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
    receipt: ManagedAgentSettlementReceipt,
    observed_at_ms: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultInput {
    command_digest: String,
    token: String,
    receipt: ManagedAgentSettlementReceipt,
}
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentSettlementProgress {
    Idle,
    HistoricalReceiptMissing {
        command_id: String,
    },
    Recorded {
        command_id: String,
        ledger_revision: u64,
        usage_complete: bool,
    },
}
pub struct AgentSettlementTransport {
    transport: AllocationTransport,
}
impl AgentSettlementTransport {
    pub fn new(manager: Arc<NativeManagement>) -> Self {
        Self {
            transport: AllocationTransport::new(manager),
        }
    }
    #[cfg(feature = "resource-adoption")]
    pub(super) fn restrict_to_host(mut self, gate: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.transport = self.transport.restrict_to_host(gate);
        self
    }
    pub async fn poll(
        &self,
        registry: &OwnerResourceRegistry,
        after: Option<String>,
    ) -> Result<AgentSettlementProgress, NativeError> {
        self.run(registry, Action::Poll { after }, None).await
    }
    pub async fn recover(
        &self,
        registry: &OwnerResourceRegistry,
        id: String,
        command_digest: String,
    ) -> Result<AgentSettlementProgress, NativeError> {
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
    ) -> Result<AgentSettlementProgress, NativeError> {
        let scope = self
            .transport
            .scope(Permission::ExecuteManagedAgents)
            .await?;
        let discovery: Discovery = self
            .transport
            .request(
                &scope,
                Operation::AgentSettlement(ServerOperation::new(Action::Discovery)?),
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
            .any(|c| c == "agent-settlement-v1")
        {
            return Err(failure(409, "agent_settlement_protocol_unavailable"));
        }
        let started = Instant::now();
        let reply: Envelope<
            Delivery<ManagedAgentSettlementCommand, ManagedAgentSettlementReceipt>,
        > = self
            .transport
            .request(
                &scope,
                Operation::AgentSettlement(ServerOperation::new(action)?),
                false,
            )
            .await?;
        let Some(delivery) = reply.delivery else {
            if historical.is_some() {
                return Err(invalid());
            }
            return Ok(AgentSettlementProgress::Idle);
        };
        let until = lease(
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
        let original = &c.fence_command.reserve_command;
        if original.executor_device_id != scope.device.device_id()
            || original.executor_generation > scope.device.generation()
        {
            return Err(failure(409, "managed_agent_enrollment_changed"));
        }
        let saved = registry
            .recover_managed_agent_settlement(
                scope.profile.clone(),
                scope.device.device_id().into(),
                c.clone(),
            )
            .await
            .map_err(storage)?;
        self.transport.check_scope(&scope, false).await?;
        let receipt = if let Some(saved) = saved {
            saved
        } else {
            if delivery.recorded.is_some() {
                return Err(failure(409, "native_agent_settlement_receipt_missing"));
            }
            if !delivery.execution_allowed {
                return Ok(AgentSettlementProgress::HistoricalReceiptMissing {
                    command_id: c.command_id.clone(),
                });
            }
            let project = if original.pool_grant.is_some() { None } else { Some(registry
                .managed_agent_project_history(
                    scope.profile.clone(),
                    scope.device.device_id().into(),
                    original.allocation_id.clone(),
                    original.allocation_receipt_digest.clone(),
                )
                .await
                .map_err(storage)?) };
            let grant = original.pool_grant.as_ref().or_else(|| project.as_ref().map(|p| &p.grant_receipt)).ok_or_else(invalid)?;
            self.transport.check_scope(&scope, false).await?;
            // Cleanup is still allowed after the grant expires. It cannot grant
            // fresh execution authority or revive a fenced reservation.
            let enrollment = self
                .transport
                .enrollment(&scope, grant, false)
                .await?;
            self.transport.check_scope(&scope, false).await?;
            let proof = if let Some(project) = project { VerifiedManagedAgentSettlement::new(
                enrollment, c.clone(), delivery.command_digest.clone(), project, true, until,
            ) } else { VerifiedManagedAgentSettlement::for_pool(
                enrollment, c.clone(), delivery.command_digest.clone(), true, until,
            ) }
            .map_err(storage)?;
            registry
                .settle_managed_agent(proof)
                .await
                .map_err(storage)?
        };
        self.transport.check_scope(&scope, true).await?;
        receipt.validate_for(c).map_err(|_| invalid())?;
        if let Some(recorded) = delivery.recorded {
            recorded.validate_for(c).map_err(|_| invalid())?;
            if digest(&recorded)? != digest(&receipt)? {
                return Err(failure(409, "agent_settlement_receipt_conflict"));
            }
        } else {
            let ack: Ack = self
                .transport
                .upload(
                    &scope,
                    Operation::AgentSettlement(ServerOperation::new(Action::Result {
                        id: c.command_id.clone(),
                        input: Box::new(ResultInput {
                            command_digest: delivery.command_digest,
                            token: delivery.token,
                            receipt: receipt.clone(),
                        }),
                    })?),
                    until,
                )
                .await?;
            if ack.observed_at_ms == 0
                || ack.observed_at_ms > hagency_core::JSON_SAFE_MAX
                || digest(&ack.receipt)? != digest(&receipt)?
            {
                return Err(failure(409, "agent_settlement_acknowledgement_conflict"));
            }
        }
        Ok(AgentSettlementProgress::Recorded {
            command_id: c.command_id.clone(),
            ledger_revision: receipt.ledger_revision,
            usage_complete: receipt.usage_complete,
        })
    }
}
