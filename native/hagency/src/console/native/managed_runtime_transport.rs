//! Exact schema11 qualification/stop transport. Disabled by default, no mini-app
//! route. Historical responses can upload saved evidence but never mint custody.
use super::{
    NativeError, NativeManagement,
    allocation_transport::{AllocationTransport, Delivery, Envelope, Operation, lease},
    failure,
    managed_runtime::{LiveCheck, PreparedLaunch},
};
use crate::console::server_login::OwnerError;
use hagency_store::{management::Permission, owner_resources::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Instant};
fn invalid() -> NativeError {
    failure(502, "invalid_managed_runtime_response")
}
fn digest(v: &impl Serialize) -> Result<String, NativeError> {
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
        observation: Option<String>,
    },
    Result {
        id: String,
        digest: String,
        token: String,
        receipt: Box<ManagedRuntimeReceipt>,
    },
}
impl ServerOperation {
    fn new(action: Action) -> Result<Self, NativeError> {
        match &action {
            Action::Poll { after: Some(id) } => super::key(id)?,
            Action::Poll { after: None } => {}
            Action::Recover {
                id,
                digest,
                observation,
            } => {
                super::key(id)?;
                hash(digest)?;
                if let Some(o) = observation {
                    super::key(o)?;
                }
            }
            Action::Result {
                id, digest, token, ..
            } => {
                super::key(id)?;
                hash(digest)?;
                hash(token)?;
            }
        }
        Ok(Self(action))
    }
    pub(in crate::console::native) fn request(
        self,
    ) -> Result<(reqwest::Method, String, Option<Value>), OwnerError> {
        let (path, body) = match self.0 {
            Action::Poll { after } => ("poll".into(), json!({"after":after})),
            Action::Recover {
                id,
                digest,
                observation,
            } => (
                format!("{id}/recover"),
                json!({"commandDigest":digest,"observationId":observation}),
            ),
            Action::Result {
                id,
                digest,
                token,
                receipt,
            } => (
                format!("{id}/result"),
                json!({"commandDigest":digest,"token":token,"receipt":receipt}),
            ),
        };
        if serde_json::to_vec(&body).map_or(true, |b| b.len() > 16 * 1024) {
            return Err(OwnerError {
                status: 400,
                code: "invalid_managed_runtime_payload".into(),
            });
        }
        Ok((
            reqwest::Method::POST,
            format!("/hagency/api/v1/management/agent-runtime/{path}"),
            Some(body),
        ))
    }
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
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ManagedRuntimeProgress {
    Idle,
    HistoricalEvidenceMissing {
        command_id: String,
    },
    Recorded {
        command_id: String,
        observation_id: String,
        process_state: String,
    },
}
pub struct ManagedRuntimeTransport {
    enabled: bool,
    transport: Arc<AllocationTransport>,
    manager: Arc<NativeManagement>,
}
impl ManagedRuntimeTransport {
    pub fn new(manager: Arc<NativeManagement>) -> Self {
        Self {
            enabled: cfg!(feature = "managed-runtime"),
            transport: Arc::new(AllocationTransport::new(manager.clone())),
            manager,
        }
    }
    #[cfg(test)]
    pub(super) fn fixture(manager: Arc<NativeManagement>) -> Self {
        let mut result = Self::new(manager);
        result.enabled = true;
        result
    }
    pub async fn poll(
        &self,
        registry: &OwnerResourceRegistry,
        after: Option<String>,
    ) -> Result<ManagedRuntimeProgress, NativeError> {
        self.run(registry, Action::Poll { after }, None, None).await
    }
    pub async fn recover(
        &self,
        registry: &OwnerResourceRegistry,
        id: String,
        hash: String,
        observation: Option<String>,
    ) -> Result<ManagedRuntimeProgress, NativeError> {
        self.run(
            registry,
            Action::Recover {
                id: id.clone(),
                digest: hash.clone(),
                observation: observation.clone(),
            },
            Some((id, hash, observation)),
            None,
        )
        .await
    }
    #[cfg(test)]
    pub(super) async fn poll_fixture(
        &self,
        registry: &OwnerResourceRegistry,
        prepared: PreparedLaunch,
    ) -> Result<ManagedRuntimeProgress, NativeError> {
        self.run(registry, Action::Poll { after: None }, None, Some(prepared))
            .await
    }
    async fn run(
        &self,
        registry: &OwnerResourceRegistry,
        action: Action,
        historical: Option<(String, String, Option<String>)>,
        prepared: Option<PreparedLaunch>,
    ) -> Result<ManagedRuntimeProgress, NativeError> {
        if !self.enabled {
            return Err(failure(409, "managed_runtime_disabled"));
        }
        let scope = Arc::new(
            self.transport
                .scope(Permission::ExecuteManagedAgents)
                .await?,
        );
        let started = Instant::now();
        let reply: Envelope<Delivery<ManagedRuntimeCommand, ManagedRuntimeReceipt>> = self
            .transport
            .request(
                &scope,
                Operation::ManagedRuntime(ServerOperation::new(action)?),
                false,
            )
            .await?;
        let Some(mut d) = reply.delivery else {
            if historical.is_some() {
                return Err(invalid());
            }
            return Ok(ManagedRuntimeProgress::Idle);
        };
        // A fresh stop may carry an earlier unknown observation while allowing
        // the same retained process to be inspected again. No such exception
        // applies to provision or historical responses.
        if d.execution_allowed
            && matches!(d.command, ManagedRuntimeCommand::Stop(_))
            && d.recorded.as_ref().is_some_and(|r| r.state == "unknown")
        {
            d.recorded
                .as_ref()
                .unwrap()
                .validate_for(&d.command)
                .map_err(store)?;
            d.recorded = None;
        }
        let until = lease(
            &d,
            reply.observed_at_ms,
            started,
            &scope,
            historical.is_some(),
        )?;
        let c = &d.command;
        c.validate(scope.device.owner_mxid()).map_err(store)?;
        let original = c.original();
        if digest(c)? != d.command_digest
            || historical
                .as_ref()
                .is_some_and(|(id, h, _)| id != c.id() || h != &d.command_digest)
            || original.reserve_command.executor_device_id != scope.device.device_id()
            || original.reserve_command.executor_generation > scope.device.generation()
        {
            return Err(invalid());
        }
        let local = registry
            .recover_managed_runtime_receipt(
                scope.profile.clone(),
                scope.device.device_id().into(),
                c.id().into(),
                d.command_digest.clone(),
            )
            .await
            .map_err(store)?;
        self.transport.check_scope(&scope, false).await?;
        if let Some(recorded) = d.recorded {
            recorded.validate_for(c).map_err(store)?;
            if historical
                .as_ref()
                .and_then(|(_, _, o)| o.as_ref())
                .is_some_and(|id| id != &recorded.observation_id)
                || local
                    .as_ref()
                    .filter(|r| r.observation_id == recorded.observation_id)
                    .is_some_and(|r| digest(r).ok() != digest(&recorded).ok())
            {
                return Err(invalid());
            }
            return Ok(progress(recorded));
        }
        let local = local.filter(|r| {
            !(d.execution_allowed
                && matches!(c, ManagedRuntimeCommand::Stop(_))
                && r.state == "unknown")
        });
        let receipt = if let Some(local) = local {
            if historical
                .as_ref()
                .and_then(|(_, _, o)| o.as_ref())
                .is_some_and(|id| id != &local.observation_id)
            {
                return Err(invalid());
            }
            local
        } else if !d.execution_allowed {
            return Ok(ManagedRuntimeProgress::HistoricalEvidenceMissing {
                command_id: c.id().into(),
            });
        } else {
            let fresh = matches!(c, ManagedRuntimeCommand::Provision(_));
            if fresh && original.reserve_command.executor_generation != scope.device.generation() {
                return Err(failure(409, "managed_runtime_enrollment_changed"));
            }
            let grant = if let Some(grant) = &original.reserve_command.pool_grant { grant.clone() } else {
registry
                .managed_agent_project_history(
                    scope.profile.clone(),
                    scope.device.device_id().into(),
                    original.reserve_command.allocation_id.clone(),
                    original.reserve_command.allocation_receipt_digest.clone(),
                )
                .await
                .map_err(store)?.grant_receipt
};
            self.transport.check_scope(&scope, false).await?;
            let enrollment = self
                .transport
                .enrollment(&scope, &grant, fresh)
                .await?;
            self.transport.check_scope(&scope, false).await?;
            let proof = VerifiedManagedRuntimeDelivery::new(
                enrollment,
                c.clone(),
                d.command_digest.clone(),
                true,
                until,
            )
            .map_err(store)?;
            match c {
                ManagedRuntimeCommand::Provision(_) => {
                    let Some(proof) = registry
                        .prepare_managed_runtime_delivery(proof)
                        .await
                        .map_err(store)?
                    else {
                        return Ok(ManagedRuntimeProgress::HistoricalEvidenceMissing {
                            command_id: c.id().into(),
                        });
                    };
                    let transport = Arc::downgrade(&self.transport);
                    let live_scope = scope.clone();
                    let check: LiveCheck = Arc::new(move || {
                        let t = transport.upgrade();
                        let s = live_scope.clone();
                        Box::pin(async move {
                            t.ok_or_else(|| failure(409, "managed_runtime_closed"))?
                                .check_scope(&s, false)
                                .await
                        })
                    });
                    let prepared = match prepared {
                        Some(p) => p,
                        None => {
                            let root = self
                                .manager
                                .owner
                                .0
                                .host
                                .console
                                .0
                                .server_login
                                .state_directory()
                                .ok_or_else(|| {
                                    failure(409, "managed_runtime_launch_unavailable")
                                })?;
                            let resource=registry.resource(&scope.profile,original.reserve_command.resource_id.clone()).await.map_err(store)?;
                            if resource.revision!=original.reserve_command.resource_revision {return Err(failure(409,"managed_runtime_enrollment_changed"));}
                            PreparedLaunch::for_resource(&root,&scope.device,registry,&scope.profile,&resource,false).await?
                        }
                    };
                    let check=prepared.guard_metadata(check);
                    let prepared = self
                        .manager
                        .owner
                        .0
                        .managed_runtime
                        .prepare(prepared, until, check.clone())
                        .await?;
                    self.transport.check_scope(&scope, false).await?;
                    match registry.claim_managed_runtime(proof).await.map_err(store)? {
                        ManagedRuntimeClaim::History(_) => {
                            return Ok(ManagedRuntimeProgress::HistoricalEvidenceMissing {
                                command_id: c.id().into(),
                            });
                        }
                        ManagedRuntimeClaim::Issued(custody) => {
                            let started = self.manager.owner.0.managed_runtime.start(
                                registry.clone(),
                                custody,
                                prepared,
                                check,
                            )?;
                            let observed = started.qualification().await?;
                            observed
                                .receipt
                                .ok_or_else(|| failure(409, "managed_runtime_outcome_unknown"))?
                        }
                    }
                }
                ManagedRuntimeCommand::Stop(stop) => {
                    registry.fence_managed_runtime(proof).await.map_err(store)?;
                    let _ = self
                        .manager
                        .owner
                        .0
                        .managed_runtime
                        .stop(
                            &original.agent_reservation_id,
                            &stop.provision_command_digest,
                        )
                        .await;
                    self.transport.check_scope(&scope, false).await?;
                    let enrollment = self
                        .transport
                        .enrollment(&scope, &grant, false)
                        .await?;
                    registry
                        .collect_managed_runtime_stop(
                            VerifiedManagedRuntimeDelivery::new(
                                enrollment,
                                c.clone(),
                                d.command_digest.clone(),
                                true,
                                until,
                            )
                            .map_err(store)?,
                        )
                        .await
                        .map_err(store)?
                }
            }
        };
        receipt.validate_for(c).map_err(store)?;
        let receipt_hash = digest(&receipt)?;
        let ack: Ack = self
            .transport
            .upload(
                &scope,
                Operation::ManagedRuntime(ServerOperation::new(Action::Result {
                    id: c.id().into(),
                    digest: d.command_digest.clone(),
                    token: d.token,
                    receipt: Box::new(receipt.clone()),
                })?),
                until,
            )
            .await?;
        if ack.command_id != c.id()
            || ack.command_digest != d.command_digest
            || ack.receipt_digest != receipt_hash
            || ack.observed_at_ms == 0
            || ack.runtime.assignment_id != original.assignment_id
            || ack.runtime.command_id != original.command_id
            || ack.runtime.provision_expires_at_ms != original.expires_at_ms
            || ack.runtime.work_available
            || ack.runtime.capacity_released
        {
            return Err(failure(409, "managed_runtime_acknowledgement_conflict"));
        }
        self.transport.check_scope(&scope, true).await?;
        Ok(progress(receipt))
    }
}
fn progress(r: ManagedRuntimeReceipt) -> ManagedRuntimeProgress {
    ManagedRuntimeProgress::Recorded {
        command_id: r.command_id,
        observation_id: r.observation_id,
        process_state: r.process_state,
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ack {
    command_id: String,
    command_digest: String,
    receipt_digest: String,
    runtime: ManagedRuntimeReadiness,
    observed_at_ms: u64,
}
