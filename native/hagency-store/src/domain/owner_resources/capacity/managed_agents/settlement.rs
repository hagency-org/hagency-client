//! Fresh observation after an immutable fence. This cannot issue work, increase
//! a budget, release a project/grant or rewrite the original cleanup receipt.
use super::*;

pub const AGENT_SETTLEMENT_CAPABILITY: &str = "hagency.managed_agents.settlement.v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentSettlementCommand {
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub fence_command: ManagedAgentFenceCommand,
    pub fence_receipt_digest: String,
    pub issued_at_ms: u64,
}
impl ManagedAgentSettlementCommand {
    pub fn validate(&self, owner: &str) -> Result<(), Error> {
        identifier(&self.command_id, 128)?;
        hash(&self.fence_receipt_digest)?;
        exact(self.issued_at_ms)?;
        self.fence_command.validate(owner)?;
        if self.version != 1
            || self.capability != AGENT_SETTLEMENT_CAPABILITY
            || self.operation != "agent.settle"
            || self.command_id == self.fence_command.command_id
            || self.command_id == self.fence_command.reserve_command.command_id
            || self.issued_at_ms < self.fence_command.issued_at_ms
            || serde_json::to_vec(self)?.len() > 20 * 1024
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentSettlementReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub fence_receipt_digest: String,
    pub reserve_command_digest: String,
    pub request_id: String,
    pub agent_id: String,
    pub agent_reservation_id: Option<String>,
    pub period_key: String,
    pub fence_revision: u64,
    pub ledger_revision: u64,
    pub state: String,
    pub process_state: String,
    pub known_consumed_tokens: u64,
    pub retained_tokens: u64,
    pub unresolved_tokens: u64,
    pub usage_complete: bool,
    pub budget_revision: Option<u64>,
    pub budget_digest: Option<String>,
    pub effective_ceiling_tokens: Option<u64>,
    pub liability_digest: String,
}
impl ManagedAgentSettlementReceipt {
    pub fn validate_for(&self, c: &ManagedAgentSettlementCommand) -> Result<(), Error> {
        let original = &c.fence_command.reserve_command;
        for n in [self.fence_revision, self.ledger_revision] {
            exact(n)?;
        }
        for n in [
            self.known_consumed_tokens,
            self.retained_tokens,
            self.unresolved_tokens,
        ] {
            if n > JSON_SAFE_MAX {
                return Err(Error::Capacity);
            }
        }
        hash(&self.liability_digest)?;
        let budget_valid = match (
            &self.agent_reservation_id,
            self.budget_revision,
            &self.budget_digest,
            self.effective_ceiling_tokens,
        ) {
            (Some(child), Some(revision), Some(digest), Some(ceiling)) => {
                identifier(child, 128)?;
                exact(revision)?;
                hash(digest)?;
                exact(ceiling)?;
                ceiling >= original.requested_tokens
            }
            (None, None, None, None) => {
                self.usage_complete
                    && self.retained_tokens == 0
                    && self.process_state == "not_started"
            }
            _ => false,
        };
        if self.version != 1
            || !budget_valid
            || self.command_id != c.command_id
            || self.command_digest != canonical::digest(&serde_json::to_value(c)?)?
            || self.fence_receipt_digest != c.fence_receipt_digest
            || self.reserve_command_digest != c.fence_command.reserve_command_digest
            || self.request_id != original.request_id
            || self.agent_id != original.proposed_agent_id
            || self.period_key != original.period_key
            || self.ledger_revision <= self.fence_revision
            || self.retained_tokens < self.known_consumed_tokens
            || self.unresolved_tokens != self.retained_tokens - self.known_consumed_tokens
            || !matches!(
                self.process_state.as_str(),
                "unknown" | "stopped" | "not_started"
            )
            || (self.process_state == "not_started" && self.known_consumed_tokens != 0)
            || if self.usage_complete {
                self.state != "settled"
                    || self.unresolved_tokens != 0
                    || self.process_state == "unknown"
            } else {
                self.state != "unresolved"
            }
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

/// A fresh SDK-authenticated, finite cleanup lease for the original owner and
/// device. Historical recovery cannot manufacture this authority.
pub struct VerifiedManagedAgentSettlement {
    enrollment: VerifiedManagementEnrollment,
    command: ManagedAgentSettlementCommand,
    digest: String,
    project: Option<AllocationCommand>,
    expires: Instant,
}
impl VerifiedManagedAgentSettlement {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        command: ManagedAgentSettlementCommand,
        digest: String,
        project: AllocationCommand,
        execution_allowed: bool,
        expires: Instant,
    ) -> Result<Self, Error> {
        enrollment.check()?;
        command.validate(&enrollment.profile.account)?;
        hash(&digest)?;
        if !execution_allowed
            || expires <= Instant::now()
            || expires > Instant::now() + Duration::from_secs(60)
            || canonical::digest(&serde_json::to_value(&command)?)? != digest
            || serde_json::to_vec(&project)?.len() > 32 * 1024
        {
            return Err(Error::LocalAuthority);
        }
        Ok(Self {
            enrollment,
            command,
            digest,
            project: Some(project),
            expires,
        })
    }
    pub fn for_pool(
        enrollment: VerifiedManagementEnrollment,
        command: ManagedAgentSettlementCommand,
        digest: String,
        execution_allowed: bool,
        expires: Instant,
    ) -> Result<Self, Error> {
        enrollment.check()?;
        command.validate(&enrollment.profile.account)?;
        if command.fence_command.reserve_command.pool_grant.is_none() || !execution_allowed
            || expires <= Instant::now() || expires > Instant::now() + Duration::from_secs(60)
            || canonical::digest(&serde_json::to_value(&command)?)? != digest {
            return Err(Error::LocalAuthority);
        }
        Ok(Self { enrollment, command, digest, project: None, expires })
    }
    fn check(&self, registry: &Inner) -> Result<(), Error> {
        check_profile(&self.enrollment.profile, registry)?;
        self.enrollment.check()?;
        if Instant::now() >= self.expires {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
fn check_profile(profile: &ProfileAuthority, registry: &Inner) -> Result<(), Error> {
    profile.check()?;
    if registry.closed.load(Ordering::Acquire) {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}
impl OwnerResourceRegistry {
    pub async fn settle_managed_agent(
        &self,
        delivery: VerifiedManagedAgentSettlement,
    ) -> Result<ManagedAgentSettlementReceipt, Error> {
        delivery.check(&self.0)?;
        let profile = delivery.enrollment.profile.clone();
        let expires = delivery.expires.min(delivery.enrollment.expires);
        let registry = self.0.clone();
        let result = self
            .0
            .store
            .call(64 * 1024, move |db| {
                db.owner_managed_agent_settlement(delivery, registry)
            })
            .await?;
        if check_profile(&profile, &self.0).is_err() || Instant::now() >= expires {
            return Err(Error::OutcomeUnknown);
        }
        Ok(result)
    }
    pub async fn recover_managed_agent_settlement(
        &self,
        profile: ProfileAuthority,
        device: String,
        command: ManagedAgentSettlementCommand,
    ) -> Result<Option<ManagedAgentSettlementReceipt>, Error> {
        check_profile(&profile, &self.0)?;
        identifier(&device, 128)?;
        command.validate(&profile.account)?;
        let original = &command.fence_command.reserve_command;
        if original.executor_device_id != device {
            return Err(Error::LocalAuthority);
        }
        let digest = canonical::digest(&serde_json::to_value(&command)?)?;
        let after = profile.clone();
        let registry = self.0.clone();
        let result = self
            .0
            .store
            .call(32 * 1024, move |db| {
                check_profile(&profile, &registry)?;
                let result: Option<ManagedAgentSettlementReceipt> = receipt(
                    &db.db,
                    &profile.digest,
                    &device,
                    &command.command_id,
                    &digest,
                )?;
                if let Some(r) = &result {
                    r.validate_for(&command)?;
                }
                check_profile(&profile, &registry)?;
                Ok(result)
            })
            .await?;
        check_profile(&after, &self.0)?;
        Ok(result)
    }
}
impl DomainRepository {
    fn owner_managed_agent_settlement(
        &mut self,
        d: VerifiedManagedAgentSettlement,
        registry: Arc<Inner>,
    ) -> Result<ManagedAgentSettlementReceipt, Error> {
        d.check(&registry)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let e = &d.enrollment;
        let c = &d.command;
        let f = &c.fence_command;
        let original = &f.reserve_command;
        if let Some(old) = receipt::<ManagedAgentSettlementReceipt>(
            &tx,
            &e.profile.digest,
            &e.executor_device_id,
            &c.command_id,
            &d.digest,
        )? {
            old.validate_for(c)?;
            d.check(&registry)?;
            return Ok(old);
        }
        fence::verify_original(&tx, e, original, d.project.as_ref())?;
        let fence_digest = canonical::digest(&serde_json::to_value(f)?)?;
        let first: ManagedAgentFenceReceipt = receipt(
            &tx,
            &e.profile.digest,
            &e.executor_device_id,
            &f.command_id,
            &fence_digest,
        )?
        .ok_or(Error::LocalAuthority)?;
        let encoded = canonical::encode(&serde_json::to_value(original)?)?;
        let tombstone: (String, String, String, String, u64) = tx.query_row(
            "SELECT original_command_digest,original_command,COALESCE(project_reservation_id,request_id),executor_device_id,fence_revision FROM owner_managed_fences WHERE profile=?1 AND agent_id=?2 AND request_id=?3 AND original_command_id=?4",
            params![e.profile.digest, original.proposed_agent_id, original.request_id, original.command_id],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
        )?;
        if canonical::digest(&serde_json::to_value(&first)?)? != c.fence_receipt_digest
            || first.command_digest != fence_digest
            || first.command_id != f.command_id
            || first.reserve_command_digest != f.reserve_command_digest
            || first.request_id != original.request_id
            || first.agent_id != original.proposed_agent_id
            || tombstone.0 != f.reserve_command_digest
            || tombstone.1 != encoded
            || tombstone.2 != original.allocation_id
            || tombstone.3 != e.executor_device_id
            || tombstone.4 != first.fence_revision
        {
            return Err(Error::Conflict);
        }
        let child: Option<(String, String, String, bool, String)> = tx.query_row(
            "SELECT id,original_command_digest,original_command,permanent_fence,state FROM owner_managed_agents WHERE profile=?1 AND (agent_id=?2 OR request_id=?3)",
            params![e.profile.digest,original.proposed_agent_id,original.request_id],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
        ).optional()?;
        let mut result = ManagedAgentSettlementReceipt {
            version: 1,
            command_id: c.command_id.clone(),
            command_digest: d.digest.clone(),
            fence_receipt_digest: c.fence_receipt_digest.clone(),
            reserve_command_digest: f.reserve_command_digest.clone(),
            request_id: original.request_id.clone(),
            agent_id: original.proposed_agent_id.clone(),
            agent_reservation_id: None,
            period_key: original.period_key.clone(),
            fence_revision: first.fence_revision,
            ledger_revision: 0,
            state: "settled".into(),
            process_state: "not_started".into(),
            known_consumed_tokens: 0,
            retained_tokens: 0,
            unresolved_tokens: 0,
            usage_complete: true,
            budget_revision: None,
            budget_digest: None,
            effective_ceiling_tokens: None,
            liability_digest: canonical::digest(
                &json!({"absentFenceDigest":c.fence_receipt_digest}),
            )?,
        };
        if let Some((id, digest, raw, fenced, state)) = child {
            if digest != f.reserve_command_digest
                || raw != encoded
                || !fenced
                || state != "fenced"
                || first.state != "fenced"
                || first.agent_reservation_id.as_deref() != Some(&id)
            {
                return Err(Error::Conflict);
            }
            let item = accounting::child(&tx, &id)?;
            let (budget_revision, budget_digest, ceiling) = budget::settlement_head(&tx, &id)?;
            result.agent_reservation_id = Some(id);
            result.state = if item.complete {
                "settled"
            } else {
                "unresolved"
            }
            .into();
            result.process_state = item.process_state.into();
            result.known_consumed_tokens = item.consumed;
            result.retained_tokens = item.retained;
            result.unresolved_tokens = item.retained.saturating_sub(item.consumed);
            result.usage_complete = item.complete;
            result.budget_revision = Some(budget_revision);
            result.effective_ceiling_tokens = Some(ceiling);
            result.liability_digest = canonical::digest(
                &json!({"liability":item.manifest,"processState":item.process_state,"budgetDigest":budget_digest,"fenceReceiptDigest":c.fence_receipt_digest}),
            )?;
            result.budget_digest = Some(budget_digest);
        } else if first.state != "absent_fenced" || first.agent_reservation_id.is_some() {
            return Err(Error::Conflict);
        }
        // One globally increasing native revision, saved atomically with the
        // exact new result. No usage, grant, budget or previous receipt update.
        result.ledger_revision = revision(&tx)?;
        if result.ledger_revision <= first.ledger_revision {
            return Err(Error::Conflict);
        }
        result.validate_for(c)?;
        save(
            &tx,
            &e.profile.digest,
            &e.executor_device_id,
            &c.command_id,
            &d.digest,
            &result,
        )?;
        d.check(&registry)?;
        tx.commit()?;
        d.check(&registry).map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
