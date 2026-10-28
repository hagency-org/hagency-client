//! Unadvertised native ledger transport. No provider, Matrix provisioning or
//! prompt path is reachable through this adapter.
mod routes;
mod wire;
use super::allocation_transport::{AllocationTransport, Delivery, Envelope, Operation, lease};
use super::{NativeError, NativeManagement, failure};
use hagency_store::{management::Permission, owner_resources::*};
use routes::AgentOperation;
pub(super) use routes::ServerOperation;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Instant};
use wire::*;
fn invalid() -> NativeError {
    failure(502, "invalid_managed_agent_response")
}
fn digest<T: Serialize>(v: &T) -> Result<String, NativeError> {
    hagency_core::canonical::digest(&serde_json::to_value(v).map_err(|_| invalid())?)
        .map_err(|_| invalid())
}
fn storage(e: hagency_store::Error) -> NativeError {
    super::management::store_error(e)
}
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ManagedAgentProgress {
    Idle,
    HistoricalReceiptMissing {
        command_id: String,
    },
    Recorded {
        command_id: String,
        ledger_revision: u64,
    },
}
pub struct ManagedAgentTransport {
    transport: AllocationTransport,
}
impl ManagedAgentTransport {
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
    ) -> Result<ManagedAgentProgress, NativeError> {
        self.run(registry, AgentOperation::Poll { after }, None)
            .await
    }
    pub async fn recover(
        &self,
        registry: &OwnerResourceRegistry,
        id: String,
        command_digest: String,
    ) -> Result<ManagedAgentProgress, NativeError> {
        self.run(
            registry,
            AgentOperation::Recover {
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
        operation: AgentOperation,
        historical: Option<(String, String)>,
    ) -> Result<ManagedAgentProgress, NativeError> {
        let scope = self
            .transport
            .scope(Permission::ExecuteManagedAgents)
            .await?;
        let started = Instant::now();
        let envelope: Envelope<Delivery<Command, Receipt>> = self
            .transport
            .request(
                &scope,
                Operation::ManagedAgent(ServerOperation::new(operation)?),
                false,
            )
            .await?;
        let Some(delivery) = envelope.delivery else {
            if historical.is_some() {
                return Err(invalid());
            }
            return Ok(ManagedAgentProgress::Idle);
        };
        let until = lease(
            &delivery,
            envelope.observed_at_ms,
            started,
            &scope,
            historical.is_some(),
        )?;
        let command = &delivery.command;
        command.validate(scope.device.owner_mxid())?;
        if digest(command)? != delivery.command_digest
            || historical
                .as_ref()
                .is_some_and(|(id, h)| id != command.id() || h != &delivery.command_digest)
        {
            return Err(invalid());
        }
        let original = command.reserve();
        if original.executor_device_id != scope.device.device_id()
            || original.executor_generation > scope.device.generation()
        {
            return Err(failure(409, "managed_agent_enrollment_changed"));
        }
        let old = match command {
            Command::Reserve(c) => registry
                .recover_managed_agent_reservation(
                    scope.profile.clone(),
                    scope.device.device_id().into(),
                    c.command_id.clone(),
                    delivery.command_digest.clone(),
                )
                .await
                .map_err(storage)?
                .map(Receipt::Reserve),
            Command::Fence(c) => registry
                .recover_managed_agent_fence(
                    scope.profile.clone(),
                    scope.device.device_id().into(),
                    c.command_id.clone(),
                    delivery.command_digest.clone(),
                )
                .await
                .map_err(storage)?
                .map(Receipt::Fence),
        };
        self.transport.check_scope(&scope, false).await?;
        let receipt = if let Some(old) = old {
            old
        } else {
            if delivery.recorded.is_some() {
                return Err(failure(409, "native_managed_agent_receipt_missing"));
            }
            if !delivery.execution_allowed {
                return Ok(ManagedAgentProgress::HistoricalReceiptMissing {
                    command_id: command.id().into(),
                });
            }
            let active = matches!(command, Command::Reserve(_));
            if active && original.executor_generation != scope.device.generation() {
                return Err(failure(409, "managed_agent_enrollment_changed"));
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
            let enrollment = self
                .transport
                .enrollment(&scope, grant, active)
                .await?;
            self.transport.check_scope(&scope, false).await?;
            match command {
                Command::Reserve(c) => Receipt::Reserve(
                    registry
                        .reserve_managed_agent(
                            if let Some(project) = project { VerifiedManagedAgentDelivery::new(
                                enrollment, c.clone(), delivery.command_digest.clone(), project, true, until,
                            ) } else { VerifiedManagedAgentDelivery::for_pool(
                                enrollment, c.clone(), delivery.command_digest.clone(), true, until,
                            ) }
                            .map_err(storage)?,
                        )
                        .await
                        .map_err(storage)?,
                ),
                Command::Fence(c) => Receipt::Fence(
                    registry
                        .apply_managed_agent_fence(
                            if let Some(project) = project { VerifiedManagedAgentFence::new(
                                enrollment, c.clone(), delivery.command_digest.clone(), project, true, until,
                            ) } else { VerifiedManagedAgentFence::for_pool(
                                enrollment, c.clone(), delivery.command_digest.clone(), true, until,
                            ) }
                            .map_err(storage)?,
                        )
                        .await
                        .map_err(storage)?,
                ),
            }
        };
        self.transport.check_scope(&scope, true).await?;
        receipt.validate(command)?;
        let receipt_digest = digest(&receipt)?;
        if let Some(saved) = delivery.recorded {
            if digest(&saved)? != receipt_digest {
                return Err(failure(409, "managed_agent_receipt_conflict"));
            }
        } else {
            let ack: Ack = self
                .transport
                .upload(
                    &scope,
                    Operation::ManagedAgent(ServerOperation::new(AgentOperation::Result {
                        id: command.id().into(),
                        digest: delivery.command_digest.clone(),
                        token: delivery.token,
                        receipt: Box::new(receipt.clone()),
                    })?),
                    until,
                )
                .await?;
            if ack.observed_at_ms == 0
                || ack.command_id != command.id()
                || ack.command_digest != delivery.command_digest
                || ack.receipt_digest != receipt_digest
            {
                return Err(failure(409, "managed_agent_acknowledgement_conflict"));
            }
            if let Some(a) = ack.assignment {
                if a.request_id != original.request_id
                    || a.agent_id != original.proposed_agent_id
                    || a.manager_mxid != original.manager_mxid
                    || a.executor_device_id != original.executor_device_id
                    || a.executor_generation != original.executor_generation
                    || a.generation != 1
                    || a.execution_available
                    || !matches!(a.state.as_str(), "pending_native" | "fenced")
                    || super::key(&a.agent_reservation_id).is_err()
                    || a.reserve_receipt_digest.len() != 64
                    || !a
                        .reserve_receipt_digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || super::key(&a.id).is_err()
                {
                    return Err(failure(409, "managed_agent_acknowledgement_conflict"));
                }
                if let Receipt::Reserve(r) = &receipt
                    && (r.state != "reserved"
                        || r.agent_reservation_id.as_ref() != Some(&a.agent_reservation_id)
                        || receipt_digest != a.reserve_receipt_digest)
                {
                    return Err(failure(409, "managed_agent_acknowledgement_conflict"));
                }
                if let Receipt::Fence(r) = &receipt
                    && (r.state != "fenced"
                        || a.state != "fenced"
                        || r.agent_reservation_id.as_ref() != Some(&a.agent_reservation_id))
                {
                    return Err(failure(409, "managed_agent_acknowledgement_conflict"));
                }
            }
        }
        self.transport.check_scope(&scope, true).await?;
        Ok(ManagedAgentProgress::Recorded {
            command_id: command.id().into(),
            ledger_revision: receipt.revision(),
        })
    }
}
