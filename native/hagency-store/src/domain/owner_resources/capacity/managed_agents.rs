//! Unwired managed-agent children in the canonical owner ledger. No Matrix
//! identity, provider call or legacy registration is created here.
use super::*;
mod accounting;
mod custody;
pub use custody::{budget, shared_execution};
pub use custody::{
    AGENT_MATRIX_CAPABILITY, AGENT_RUNTIME_CAPABILITY, ManagedAgentMatrixChallenge,
    ManagedAgentMatrixObservation, ManagedAssignmentAuthority, ManagedCallAuthority,
    ManagedCallClaim, ManagedCallCustody, ManagedCallHistory, ManagedCleanupAuthority,
    ManagedObservationReceipt, ManagedProvisionClaim, ManagedProvisionCustody,
    ManagedProvisionHistory, ManagedProvisionObservation, ManagedRuntimeAcknowledgement,
    ManagedRuntimeClaim, ManagedRuntimeCommand, ManagedRuntimeCustody, ManagedRuntimeDelivery,
    ManagedRuntimeEvidenceSink, ManagedRuntimeHistory, ManagedRuntimeProvisionCommand,
    ManagedRuntimeReadiness, ManagedRuntimeReceipt, ManagedRuntimeRecordedProcess,
    ManagedRuntimeRecoverInput, ManagedRuntimeResultInput, ManagedRuntimeStopCommand,
    ManagedUsageObservation, VerifiedManagedMatrixCheck, VerifiedManagedRuntimeDelivery,
};
mod fence;
mod settlement;
pub use settlement::{
    AGENT_SETTLEMENT_CAPABILITY, ManagedAgentSettlementCommand, ManagedAgentSettlementReceipt,
    VerifiedManagedAgentSettlement,
};
mod reserve;
mod types;
pub(in crate::domain) use accounting::unsettled_resource;
#[cfg(feature = "resource-adoption")]
pub(in crate::domain) use accounting::unsettled_exact_resource;
pub(super) use accounting::{
    Liability, MANAGED, accounted_floor, accounted_pool_liability, grant_floor,
    period_floor, project_liability, release_project,
};
pub(in crate::domain) use accounting::unsettled_resource_except;
pub use fence::{
    AGENT_FENCE_CAPABILITY, ManagedAgentFenceCommand, ManagedAgentFenceReceipt,
    VerifiedManagedAgentFence,
};
pub(super) use reserve::save_project_authority;
pub use types::{
    AGENT_CAPABILITY, ManagedAgentDefinition, ManagedAgentReservationReceipt,
    ManagedAgentReserveCommand,
};

impl DomainRepository {
    #[cfg(test)]
    pub(crate) fn management_web_managed_usage(&self, profile: &str) -> Result<crate::management::WebManagedUsage, Error> {
        self.management_web_managed_usage_page(profile,"",256)
    }
    #[cfg(any(test, feature = "resource-adoption"))]
    pub(crate) fn management_web_managed_usage_page(&self, profile: &str, after: &str, limit: usize) -> Result<crate::management::WebManagedUsage, Error> {
        if !(1..=256).contains(&limit) { return Err(Error::Capacity); }
        let mut statement = self.db.prepare("SELECT a.id,a.original_command,a.allocated_tokens FROM owner_managed_agents a JOIN owner_project_reservations p ON p.id=a.project_reservation_id JOIN owner_capacity_grants g ON g.id=p.grant_id WHERE a.profile=?1 AND g.profile=?1 AND a.request_id>?2 AND EXISTS(SELECT 1 FROM owner_resource_grants r WHERE r.resource_id=g.resource_id AND r.profile=?1 AND r.active=1) ORDER BY a.request_id LIMIT ?3")?;
        let rows = statement.query_map(params![profile,after,limit+1],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u64>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
        drop(statement);
        let complete = rows.len()<=limit;
        let mut items = Vec::new();
        for (id, raw, original) in rows.into_iter().take(limit) {
            let command: ManagedAgentReserveCommand = serde_json::from_str(&raw)?;
            let usage = accounting::child(&self.db,&id)?;
            let call_count = self.db.query_row("SELECT COUNT(*) FROM owner_managed_calls WHERE child_id=?1",[&id],|r|r.get(0))?;
            let state = self.db.query_row("SELECT state FROM owner_managed_agents WHERE id=?1",[&id],|r|r.get(0))?;
            items.push(crate::management::WebManagedUsageRow {request_id:command.request_id,agent_id:command.proposed_agent_id,name:command.definition.name,capability:command.definition.capability,manager_mxid:command.manager_mxid,state,project_id:command.project_id,allocation_id:command.allocation_id,resource_id:command.resource_id,period_key:command.period_key,allocated_tokens:custody::budget::effective_ceiling(&self.db,&id,original)?,retained_tokens:usage.retained,known_consumed_tokens:usage.consumed,usage_complete:usage.complete,call_count});
        }
        Ok(crate::management::WebManagedUsage { supported:true,complete,items })
    }
    /// Human readback of an existing immutable managed child. Only the retained
    /// profile/resource grant can join this to a provider-account alias.
    #[cfg(any(test, feature = "resource-adoption"))]
    pub(crate) fn management_web_managed_link(&self, profile: &str, request: &str) -> Result<Option<crate::management::WebWorkforceLink>, Error> {
        let row: Option<(String, String, u64)> = self.db.query_row("SELECT a.id,a.original_command,a.allocated_tokens FROM owner_managed_agents a JOIN owner_project_reservations p ON p.id=a.project_reservation_id JOIN owner_capacity_grants g ON g.id=p.grant_id WHERE a.profile=?1 AND a.request_id=?2 AND g.profile=?1 AND EXISTS(SELECT 1 FROM owner_resource_grants r WHERE r.resource_id=g.resource_id AND r.profile=?1 AND r.active=1)", params![profile,request], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((id, raw, original)) = row else { return Ok(None); };
        let command: ManagedAgentReserveCommand = serde_json::from_str(&raw)?;
        let resource = self.resource_configuration(&command.resource_id)?;
        let account: Option<String> = self.db.query_row("SELECT n.account_id FROM owner_resource_namespaces n JOIN management_account_profiles p ON p.account_id=n.account_id WHERE n.resource_id=?1 AND p.profile=?2",params![command.resource_id,profile],|r|r.get(0)).optional()?;
        let account_id = if let Some(account) = account { Some(account) } else {
            let bound: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM owner_resource_accounts WHERE seat_id=?1)",[&resource.seat_id],|r|r.get(0))?;
            if bound { Some(format!("binding_{}", canonical::digest(&json!({"version":1,"profile":profile,"seat":resource.seat_id}))?)) } else { None }
        };
        let usage = accounting::child(&self.db, &id)?;
        Ok(Some(crate::management::WebWorkforceLink {
            request_id: command.request_id, agent_id: command.proposed_agent_id,
            project_id: command.project_id, allocation_id: command.allocation_id,
            resource_id: command.resource_id, account_id,
            model_revision_matches: crate::resource_publication_revision(&resource)? == command.resource_revision,
            model: resource.model, reasoning: resource.reasoning, period_key: command.period_key,
            allocated_tokens: custody::budget::effective_ceiling(&self.db, &id, original)?,
            retained_tokens: usage.retained, known_consumed_tokens: usage.consumed,
            usage_complete: usage.complete, process_state: usage.process_state.into(),
        }))
    }
}

/// Authenticated native transport owns construction. Supporting project data
/// must match the original command already applied in this exact owner ledger.
/// Historical recovery and script JSON cannot supply an executable lease.
pub struct VerifiedManagedAgentDelivery {
    enrollment: VerifiedManagementEnrollment,
    command: ManagedAgentReserveCommand,
    digest: String,
    project_command: Option<AllocationCommand>,
    lease_expires: Instant,
}
impl VerifiedManagedAgentDelivery {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        command: ManagedAgentReserveCommand,
        digest: String,
        project_command: AllocationCommand,
        execution_allowed: bool,
        lease_expires: Instant,
    ) -> Result<Self, Error> {
        enrollment.check()?;
        hash(&digest)?;
        if !execution_allowed
            || lease_expires <= Instant::now()
            || lease_expires > Instant::now() + Duration::from_secs(60)
            || serde_json::to_vec(&command)?.len() > 16 * 1024
            || serde_json::to_vec(&project_command)?.len() > 32 * 1024
        {
            return Err(Error::LocalAuthority);
        }
        if canonical::digest(&serde_json::to_value(&command)?)? != digest {
            return Err(Error::Conflict);
        }
        Ok(Self {
            enrollment,
            command,
            digest,
            project_command: Some(project_command),
            lease_expires,
        })
    }
    pub fn for_pool(
        enrollment: VerifiedManagementEnrollment,
        command: ManagedAgentReserveCommand,
        digest: String,
        execution_allowed: bool,
        lease_expires: Instant,
    ) -> Result<Self, Error> {
        enrollment.check()?;
        command.validate(&enrollment.profile.account)?;
        if command.version != 2 || command.pool_grant.is_none() || !execution_allowed
            || lease_expires <= Instant::now()
            || lease_expires > Instant::now() + Duration::from_secs(60)
            || canonical::digest(&serde_json::to_value(&command)?)? != digest {
            return Err(Error::LocalAuthority);
        }
        Ok(Self { enrollment, command, digest, project_command: None, lease_expires })
    }
    fn check(&self, registry: &Inner) -> Result<(), Error> {
        self.enrollment.check()?;
        if registry.closed.load(Ordering::Acquire) || Instant::now() >= self.lease_expires {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
impl OwnerResourceRegistry {
    pub async fn reserve_managed_agent(
        &self,
        delivery: VerifiedManagedAgentDelivery,
    ) -> Result<ManagedAgentReservationReceipt, Error> {
        delivery.check(&self.0)?;
        let fence = self.0.clone();
        let profile = delivery.enrollment.profile.clone();
        let expires = delivery.lease_expires.min(delivery.enrollment.expires);
        let result = self
            .0
            .store
            .call(64 * 1024, move |db| {
                db.owner_managed_agent_reserve(delivery, fence)
            })
            .await?;
        if profile.check().is_err()
            || self.0.closed.load(Ordering::Acquire)
            || Instant::now() >= expires
        {
            return Err(Error::OutcomeUnknown);
        }
        Ok(result)
    }

    /// Exact original result only. No allocation, provision or execution grant.
    pub async fn recover_managed_agent_reservation(
        &self,
        profile: ProfileAuthority,
        executor_device_id: String,
        command_id: String,
        command_digest: String,
    ) -> Result<Option<ManagedAgentReservationReceipt>, Error> {
        profile.check()?;
        identifier(&executor_device_id, 128)?;
        identifier(&command_id, 128)?;
        hash(&command_digest)?;
        let fence = self.0.clone();
        let caller = profile.clone();
        let value = self
            .0
            .store
            .call(8192, move |db| {
                profile.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                let r = receipt(
                    &db.db,
                    &profile.digest,
                    &executor_device_id,
                    &command_id,
                    &command_digest,
                )?;
                profile.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                Ok(r)
            })
            .await?;
        caller.check()?;
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::LocalAuthority);
        }
        Ok(value)
    }
}
