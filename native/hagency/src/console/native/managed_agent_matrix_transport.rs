//! Native read-only reservation checks for server-owned Matrix membership.
//! No assignment/provision/call custody or provider operation is constructed.
use super::allocation_transport::{AllocationTransport, Delivery, Envelope, Operation, lease};
use super::{NativeError, NativeManagement, failure};
use crate::console::server_login::OwnerError;
use hagency_store::{management::Permission, owner_resources::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Instant};
fn invalid() -> NativeError {
    failure(502, "invalid_managed_matrix_response")
}
fn digest<T: Serialize>(v: &T) -> Result<String, NativeError> {
    hagency_core::canonical::digest(&serde_json::to_value(v).map_err(|_| invalid())?)
        .map_err(|_| invalid())
}
fn store(e: hagency_store::Error) -> NativeError {
    super::management::store_error(e)
}
pub(in crate::console::native) struct ServerOperation(Action);
enum Action {
    Poll {
        after: Option<String>,
    },
    Recover {
        id: String,
        digest: String,
    },
    Result {
        id: String,
        digest: String,
        token: String,
        observation: Box<ManagedAgentMatrixObservation>,
    },
}
impl ServerOperation {
    fn new(action: Action) -> Result<Self, NativeError> {
        match &action {
            Action::Poll { after: Some(id) } => super::key(id)?,
            Action::Poll { after: None } => {}
            Action::Recover { id, digest } | Action::Result { id, digest, .. } => {
                super::key(id)?;
                if digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(invalid());
                }
            }
        }
        Ok(Self(action))
    }
    pub(in crate::console::native) fn request(
        self,
    ) -> Result<(reqwest::Method, String, Option<Value>), OwnerError> {
        let (path, body) = match self.0 {
            Action::Poll { after } => ("poll".into(), json!({"after":after})),
            Action::Recover { id, digest } => {
                (format!("{id}/recover"), json!({"commandDigest":digest}))
            }
            Action::Result {
                id,
                digest,
                token,
                observation,
            } => (
                format!("{id}/result"),
                json!({"commandDigest":digest,"token":token,"observation":observation}),
            ),
        };
        if serde_json::to_vec(&body).map_or(true, |b| b.len() > 16 * 1024) {
            return Err(OwnerError {
                status: 400,
                code: "invalid_managed_matrix_payload".into(),
            });
        }
        Ok((
            reqwest::Method::POST,
            format!("/hagency/api/v1/management/agent-matrix/{path}"),
            Some(body),
        ))
    }
}
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ManagedMatrixProgress {
    Idle,
    HistoricalObservationMissing { command_id: String },
    Observed { command_id: String, eligible: bool },
}
pub struct ManagedAgentMatrixTransport {
    transport: AllocationTransport,
}
impl ManagedAgentMatrixTransport {
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
    ) -> Result<ManagedMatrixProgress, NativeError> {
        self.run(registry, Action::Poll { after }, None).await
    }
    pub async fn recover(
        &self,
        registry: &OwnerResourceRegistry,
        id: String,
        hash: String,
    ) -> Result<ManagedMatrixProgress, NativeError> {
        self.run(
            registry,
            Action::Recover {
                id: id.clone(),
                digest: hash.clone(),
            },
            Some((id, hash)),
        )
        .await
    }
    async fn run(
        &self,
        registry: &OwnerResourceRegistry,
        action: Action,
        historical: Option<(String, String)>,
    ) -> Result<ManagedMatrixProgress, NativeError> {
        let scope = self
            .transport
            .scope(Permission::ExecuteManagedAgents)
            .await?;
        let started = Instant::now();
        let reply: Envelope<Delivery<ManagedAgentMatrixChallenge, ManagedAgentMatrixObservation>> =
            self.transport
                .request(
                    &scope,
                    Operation::ManagedMatrix(ServerOperation::new(action)?),
                    false,
                )
                .await?;
        let Some(d) = reply.delivery else {
            if historical.is_some() {
                return Err(invalid());
            }
            return Ok(ManagedMatrixProgress::Idle);
        };
        let until = lease(
            &d,
            reply.observed_at_ms,
            started,
            &scope,
            historical.is_some(),
        )?;
        let c = &d.command;
        c.validate(scope.device.owner_mxid()).map_err(store)?;
        if digest(c)? != d.command_digest
            || historical
                .as_ref()
                .is_some_and(|(id, h)| id != &c.command_id || h != &d.command_digest)
            || c.reserve_command.executor_device_id != scope.device.device_id()
            || c.reserve_command.executor_generation > scope.device.generation()
        {
            return Err(invalid());
        }
        if let Some(recorded) = d.recorded {
            recorded.validate_for(c).map_err(store)?;
            self.transport.check_scope(&scope, false).await?;
            return Ok(ManagedMatrixProgress::Observed {
                command_id: c.command_id.clone(),
                eligible: recorded.eligible,
            });
        }
        if !d.execution_allowed {
            return Ok(ManagedMatrixProgress::HistoricalObservationMissing {
                command_id: c.command_id.clone(),
            });
        }
        if c.reserve_command.executor_generation != scope.device.generation() {
            return Err(failure(409, "managed_matrix_enrollment_changed"));
        }
        let grant = if let Some(grant) = &c.reserve_command.pool_grant { grant.clone() } else {
registry
            .managed_agent_project_history(
                scope.profile.clone(),
                scope.device.device_id().into(),
                c.reserve_command.allocation_id.clone(),
                c.reserve_command.allocation_receipt_digest.clone(),
            )
            .await
            .map_err(store)?.grant_receipt
};
        self.transport.check_scope(&scope, false).await?;
        let enrollment = self
            .transport
            .enrollment(&scope, &grant, true)
            .await?;
        self.transport.check_scope(&scope, false).await?;
        let observation = registry
            .check_managed_matrix_reservation(
                VerifiedManagedMatrixCheck::new(
                    enrollment,
                    c.clone(),
                    d.command_digest.clone(),
                    true,
                    until,
                )
                .map_err(store)?,
            )
            .await
            .map_err(store)?;
        self.transport.check_scope(&scope, false).await?;
        observation.validate_for(c).map_err(store)?;
        let hash = digest(&observation)?;
        let ack: Ack = self
            .transport
            .upload(
                &scope,
                Operation::ManagedMatrix(ServerOperation::new(Action::Result {
                    id: c.command_id.clone(),
                    digest: d.command_digest.clone(),
                    token: d.token,
                    observation: Box::new(observation.clone()),
                })?),
                until,
            )
            .await?;
        if ack.command_id != c.command_id
            || ack.command_digest != d.command_digest
            || ack.observation_digest != hash
            || ack.observed_at_ms == 0
            || ack.membership.assignment_id != c.assignment_id
            || ack.membership.request_id != c.reserve_command.request_id
            || ack.membership.agent_id != c.reserve_command.proposed_agent_id
            || ack.membership.puppet_mxid != c.puppet_mxid
            || ack.membership.room_id != c.reserve_command.room_id
            || ack.membership.work_available
            || ack.membership.runtime_state != "unsupported"
            || !matches!(
                ack.membership.state.as_str(),
                "waiting_native"
                    | "authorized"
                    | "matrix_verified"
                    | "membership_recheck_required"
                    | "fenced"
            )
        {
            return Err(failure(409, "managed_matrix_acknowledgement_conflict"));
        }
        self.transport.check_scope(&scope, true).await?;
        Ok(ManagedMatrixProgress::Observed {
            command_id: c.command_id.clone(),
            eligible: observation.eligible,
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Membership {
    assignment_id: String,
    request_id: String,
    agent_id: String,
    puppet_mxid: String,
    room_id: String,
    state: String,
    #[serde(rename = "verifiedAtMs")]
    _verified_at_ms: Option<u64>,
    #[serde(rename = "cleanupObservedAtMs")]
    _cleanup_observed_at_ms: Option<u64>,
    #[serde(rename = "blockingReason")]
    _blocking_reason: Option<String>,
    runtime_state: String,
    work_available: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ack {
    command_id: String,
    command_digest: String,
    observation_digest: String,
    membership: Membership,
    observed_at_ms: u64,
}
