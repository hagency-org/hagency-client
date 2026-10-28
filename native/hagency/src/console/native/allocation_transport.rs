//! Native-only allocation transport. No mini-app route advertises this module.
//! Existing owner enrollment and ledger reconciliation remain activation gates.
mod executor;
mod publication;
pub(super) use publication::PublicationObservation;
mod routes;
mod wire;
use super::{NativeError, NativeManagement, failure};
use crate::console::{
    AuthorizedDevice, device_execution::DeviceIdentity, server_login::OwnerOperation,
};
use hagency_store::{
    management::{HostContext, Permission, Session},
    owner_resources::*,
};
pub(super) use routes::Operation;
pub(in crate::console) use routes::ServerOperation;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
pub(super) use wire::ConnectionReply;
pub use wire::ResourceGrantView;
use wire::*;
pub(super) use wire::{Delivery, Envelope};

fn invalid() -> NativeError {
    failure(502, "invalid_allocation_response")
}
fn key(value: &str) -> Result<(), NativeError> {
    super::key(value)
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
fn positive(value: u64) -> Result<(), NativeError> {
    if (1..=hagency_core::JSON_SAFE_MAX).contains(&value) {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn digest(value: &impl Serialize) -> Result<String, NativeError> {
    hagency_core::canonical::digest(&serde_json::to_value(value).map_err(|_| invalid())?)
        .map_err(|_| invalid())
}
fn storage(error: hagency_store::Error) -> NativeError {
    super::management::store_error(error)
}

pub(super) struct Scope {
    context: HostContext,
    session: Session,
    pub(super) device: AuthorizedDevice,
    pub(super) profile: ProfileAuthority,
    permission: Permission,
    pub(super) deadline: Instant,
    host_gate: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}
impl Scope {
    fn check(&self) -> Result<(), NativeError> {
        if self.host_gate.as_ref().is_some_and(|gate| !gate()) {
            return Err(failure(401, "management_scope_changed"));
        }
        self.device
            .bearer()
            .map_err(|_| failure(401, "management_scope_changed"))?;
        self.session
            .authorize(&self.context, self.permission)
            .map_err(storage)?;
        if Instant::now() >= self.deadline {
            return Err(failure(401, "management_scope_changed"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AllocationProgress {
    Idle,
    HistoricalReceiptMissing {
        command_id: String,
    },
    Recorded {
        command_id: String,
        ledger_revision: u64,
    },
}

pub struct AllocationTransport {
    require_unused_publication: bool,
    manager: Arc<NativeManagement>,
    host_gate: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}
impl AllocationTransport {
    pub fn new(manager: Arc<NativeManagement>) -> Self {
        Self {
            manager,
            host_gate: None,
            require_unused_publication: false,
        }
    }

    pub(super) fn require_complete_publication_history(mut self) -> Self {
        self.require_unused_publication = true;
        self
    }
    /// Restriction only; the native manager/device/SDK and ledger gates remain.
    pub(super) fn restrict_to_host(mut self, gate: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        let prior = self.host_gate.take();
        self.host_gate = Some(Arc::new(move || {
            prior.as_ref().is_none_or(|p| p()) && gate()
        }));
        self
    }

    pub(super) async fn scope(&self, permission: Permission) -> Result<Scope, NativeError> {
        if self.host_gate.as_ref().is_some_and(|gate| !gate()) {
            return Err(failure(401, "management_scope_changed"));
        }
        let (context, session) = self.manager.authorized().await?;
        session.authorize(&context, permission).map_err(storage)?;
        let device = self
            .manager
            .owner
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        device
            .bearer()
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        let profile_digest = crate::console::owned_agents::profile_identity(
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .map_err(|_| failure(401, "management_profile_mismatch"))?;
        let deadline = device
            .valid_until()
            .min(session.valid_until())
            .min(Instant::now() + Duration::from_secs(60));
        let gate_device = device.clone();
        let gate_session = session.clone();
        let gate_context = context.clone();
        let host_gate = self.host_gate.clone();
        let profile = ProfileAuthority::new(profile_digest, device.owner_mxid().into(), deadline)
            .map_err(storage)?
            .with_native_gate(move || {
                host_gate.as_ref().is_none_or(|gate| gate())
                    && gate_device.bearer().is_ok()
                    && gate_session.authorize(&gate_context, permission).is_ok()
            })
            .map_err(storage)?;
        let result = Scope {
            context,
            session,
            device,
            profile,
            permission,
            deadline,
            host_gate: self.host_gate.clone(),
        };
        self.check_scope(&result, false).await?;
        Ok(result)
    }
    pub(super) async fn check_scope(
        &self,
        scope: &Scope,
        uncertain: bool,
    ) -> Result<(), NativeError> {
        let result = async {
            scope.check()?;
            // Device renewal may reuse its unexpired server grant. Allocation
            // still requires a fresh SDK-owned account check at every boundary.
            let cookie = self.manager.owner.0.cookie.lock().await.clone();
            scope.check()?;
            let raw = cookie
                .strip_prefix("hagency_console=")
                .ok_or_else(|| failure(401, "owner_authorization_required"))?;
            self.manager
                .owner
                .0
                .host
                .console
                .0
                .server_login
                .check_matrix_source(raw)
                .await
                .map_err(|code| {
                    failure(
                        if code.ends_with("unavailable") {
                            503
                        } else {
                            401
                        },
                        code,
                    )
                })?;
            scope.check()?;
            let (current, session) = self.manager.authorized().await?;
            scope.check()?;
            if current != scope.context {
                return Err(failure(401, "management_scope_changed"));
            }
            session
                .authorize(&current, scope.permission)
                .map_err(storage)?;
            let device = self
                .manager
                .owner
                .0
                .host
                .console
                .authorized_device()
                .await
                .map_err(|_| failure(401, "owner_authorization_required"))?;
            if DeviceIdentity::from(&device) != DeviceIdentity::from(&scope.device) {
                return Err(failure(401, "management_scope_changed"));
            }
            scope.check()
        }
        .await;
        result.map_err(|error| {
            if uncertain {
                failure(409, "management_outcome_unknown")
            } else {
                error
            }
        })
    }
    pub(super) async fn request<T: serde::de::DeserializeOwned>(
        &self,
        scope: &Scope,
        operation: Operation,
        uncertain: bool,
    ) -> Result<T, NativeError> {
        self.check_scope(scope, uncertain).await?;
        let response_limit = if matches!(&operation, Operation::SharedExecution(_)) { 128 * 1024 } else { 32 * 1024 };
        let operation = ServerOperation::new(operation)?;
        let reply = self
            .manager
            .owner
            .0
            .host
            .console
            .owner_api_pinned(
                &DeviceIdentity::from(&scope.device),
                OwnerOperation::Allocation(operation),
            )
            .await;
        self.check_scope(scope, uncertain).await?;
        let reply = reply.map_err(|e| failure(e.status, &e.code))?;
        if reply.owner != scope.device.owner_mxid()
            || reply.origin != scope.device.origin()
            || reply.issuer != scope.device.issuer()
            || reply.subject != scope.device.subject()
        {
            return Err(failure(401, "management_scope_changed"));
        }
        wire::decode_bounded(reply.value, response_limit)
    }
    async fn original_grant(
        &self,
        scope: &Scope,
        registry: &OwnerResourceRegistry,
        id: &str,
    ) -> Result<NativeGrantReceipt, NativeError> {
        key(id)?;
        let grant = registry
            .recover_capacity_grant(
                scope.profile.clone(),
                scope.device.device_id().into(),
                id.into(),
            )
            .await
            .map_err(storage)?
            .ok_or_else(|| failure(409, "native_grant_receipt_required"))?;
        self.check_scope(scope, false).await?;
        Ok(grant)
    }
    pub(super) async fn upload<T: serde::de::DeserializeOwned>(
        &self,
        scope: &Scope,
        operation: Operation,
        until: Instant,
    ) -> Result<T, NativeError> {
        if Instant::now() >= until {
            return Err(failure(409, "allocation_ticket_expired"));
        }
        tokio::time::timeout_at(
            tokio::time::Instant::from_std(until),
            self.request(scope, operation, true),
        )
        .await
        .map_err(|_| failure(409, "allocation_ticket_expired"))?
    }
    pub(super) async fn enrollment(
        &self,
        scope: &Scope,
        grant: &NativeGrantReceipt,
        active: bool,
    ) -> Result<VerifiedManagementEnrollment, NativeError> {
        Ok(self.enrollment_observation(scope, grant, active).await?.0)
    }
    async fn enrollment_observation(
        &self,
        scope: &Scope,
        grant: &NativeGrantReceipt,
        active: bool,
    ) -> Result<(VerifiedManagementEnrollment, u64, u64), NativeError> {
        let reply: ConnectionReply = self
            .request(
                scope,
                Operation::Contribution(grant.contribution_id.clone()),
                false,
            )
            .await?;
        positive(reply.observed_at_ms)?;
        let c = reply.admission.contribution;
        if c.id != grant.contribution_id
            || c.owner_mxid != scope.device.owner_mxid()
            || c.executor_device_id != scope.device.device_id()
            || c.executor_generation == 0
            || c.executor_generation > scope.device.generation()
            || c.resource_id != grant.resource_id
            || c.revision == 0
            || reply.admission.request.contribution_id != c.id
            || (active
                && (c.state != "admitted"
                    || c.resource_revision != grant.resource_revision
                    || (c.coordinator_mxid != grant.coordinator_mxid && c.coordinator_mxid != grant.owner_mxid)
                    || c.executor_generation != grant.executor_generation
                    || c.executor_generation != scope.device.generation()
                    || grant.admission_request_id.as_ref().is_some_and(|id| id != &c.request_id)))
        {
            return Err(failure(409, "allocation_enrollment_changed"));
        }
        hash(&c.resource_revision)?;
        let revision = c.revision;
        let mut proof = VerifiedManagementEnrollment::new(
            scope.profile.clone(),
            grant.enrollment_id.clone(),
            c.id,
            c.resource_id,
            c.resource_revision,
            // Keep the original grant binding for recovery after ownership of
            // coordination is merged; the current owner was checked above.
            grant.coordinator_mxid.clone(),
            c.executor_device_id,
            scope.device.generation(),
            scope.deadline,
        )
        .map_err(storage)?;
        match (grant.version, grant.admission_request_id.as_ref()) {
            (1, None) => {},
            (2, Some(request)) => { proof = proof.with_admission_request(request.clone()).map_err(storage)?; },
            _ => return Err(failure(409, "allocation_enrollment_changed")),
        }
        Ok((proof, revision, reply.observed_at_ms))
    }
}

/// A lease starts at the request's monotonic send boundary, never at receipt.
/// Network time can shorten it; a slow response cannot extend central authority.
pub(super) fn lease<C, R>(
    delivery: &Delivery<C, R>,
    observed: u64,
    started: Instant,
    scope: &Scope,
    historical: bool,
) -> Result<Instant, NativeError> {
    positive(observed)?;
    hash(&delivery.command_digest)?;
    hash(&delivery.token)?;
    if historical && delivery.execution_allowed
        || delivery.recorded.is_some() && delivery.execution_allowed
    {
        return Err(invalid());
    }
    let remaining = delivery
        .lease_until_ms
        .checked_sub(observed)
        .filter(|v| *v > 0 && *v <= 30_000)
        .ok_or_else(invalid)?;
    let deadline = (started + Duration::from_millis(remaining)).min(scope.deadline);
    if Instant::now() >= deadline {
        return Err(failure(409, "allocation_ticket_expired"));
    }
    Ok(deadline)
}

#[cfg(test)]
mod tests;
