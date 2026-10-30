//! A read-only reservation observation, never provider/runtime custody.
use super::*;
pub const AGENT_MATRIX_CAPABILITY: &str = "hagency.managed_agents.matrix.v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentMatrixChallenge {
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub assignment_id: String,
    pub assignment_generation: u64,
    pub agent_reservation_id: String,
    pub reserve_command: ManagedAgentReserveCommand,
    pub reserve_command_digest: String,
    pub reserve_receipt_digest: String,
    pub space_id: String,
    pub puppet_mxid: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}
impl ManagedAgentMatrixChallenge {
    pub fn validate(&self, owner: &str) -> Result<(), Error> {
        for id in [
            &self.command_id,
            &self.assignment_id,
            &self.agent_reservation_id,
        ] {
            identifier(id, 128)?;
        }
        self.reserve_command.validate(owner)?;
        hash(&self.reserve_receipt_digest)?;
        exact(self.issued_at_ms)?;
        exact(self.expires_at_ms)?;
        let server = owner.split_once(':').ok_or(Error::LocalAuthority)?.1;
        if self.version != 1
            || self.capability != AGENT_MATRIX_CAPABILITY
            || self.operation != "agent.matrix.check"
            || self.assignment_generation != 1
            || canonical::digest(&serde_json::to_value(&self.reserve_command)?)?
                != self.reserve_command_digest
            || if self.reserve_command.pool_grant.is_some() { !self.space_id.is_empty() || self.reserve_command.room_id.is_empty() } else { !self.space_id.starts_with('!') }
            || self.space_id.len() > 255
            || !self.puppet_mxid.starts_with("@_hagency_")
            || !self.puppet_mxid.ends_with(&format!(":{server}"))
            || self.puppet_mxid.len() > 255
            || self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms - self.issued_at_ms > 30_000
            || (self.reserve_command.pool_grant.is_none()
                && self.expires_at_ms > self.reserve_command.expires_at_ms)
            || serde_json::to_vec(self)?.len() > 16 * 1024
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentMatrixObservation {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub assignment_id: String,
    pub agent_reservation_id: String,
    pub reserve_command_digest: String,
    pub reserve_receipt_digest: String,
    pub observed_ledger_revision: u64,
    pub eligible: bool,
    pub reason: Option<String>,
    pub valid_until_ms: u64,
}
impl ManagedAgentMatrixObservation {
    pub fn validate_for(&self, c: &ManagedAgentMatrixChallenge) -> Result<(), Error> {
        exact(self.observed_ledger_revision)?;
        if self.version != 1
            || self.command_id != c.command_id
            || self.command_digest != canonical::digest(&serde_json::to_value(c)?)?
            || self.assignment_id != c.assignment_id
            || self.agent_reservation_id != c.agent_reservation_id
            || self.reserve_command_digest != c.reserve_command_digest
            || self.reserve_receipt_digest != c.reserve_receipt_digest
            || self.valid_until_ms != c.expires_at_ms
            || if self.eligible {
                self.reason.is_some()
            } else {
                self.reason.as_deref() != Some("native_reservation_unavailable")
            }
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
/// Native authenticated transport output. Never deserializable or model custody.
pub struct VerifiedManagedMatrixCheck {
    enrollment: VerifiedManagementEnrollment,
    command: ManagedAgentMatrixChallenge,
    digest: String,
    expires: Instant,
}
impl VerifiedManagedMatrixCheck {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        command: ManagedAgentMatrixChallenge,
        digest: String,
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
        {
            return Err(Error::LocalAuthority);
        }
        let value = Self {
            enrollment,
            command,
            digest,
            expires,
        };
        value.check()?;
        Ok(value)
    }
    fn check(&self) -> Result<(), Error> {
        self.enrollment.check()?;
        let now = graphs::now_ms()?;
        if Instant::now() >= self.expires
            || now < self.command.issued_at_ms
            || now >= self.command.expires_at_ms
            || self.enrollment.executor_generation
                != self.command.reserve_command.executor_generation
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
impl OwnerResourceRegistry {
    pub async fn check_managed_matrix_reservation(
        &self,
        p: VerifiedManagedMatrixCheck,
    ) -> Result<ManagedAgentMatrixObservation, Error> {
        p.check()?;
        let profile = p.enrollment.profile.clone();
        let deadline = p.expires.min(p.enrollment.expires);
        let registry = self.0.clone();
        let result = self
            .0
            .store
            .call(32 * 1024, move |db| {
                p.check()?;
                if registry.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Deferred)?;
                let e = &p.enrollment;
                let c = &p.command;
                let original = &c.reserve_command;
                // Recover immutable local evidence first. Central data cannot create
                // a missing child, provider binding, assignment pin or runtime claim.
                let saved: ManagedAgentReservationReceipt = receipt(
                    &tx,
                    &e.profile.digest,
                    &e.executor_device_id,
                    &original.command_id,
                    &c.reserve_command_digest,
                )?
                .ok_or(Error::LocalAuthority)?;
                if saved.state != "reserved"
                    || saved.agent_reservation_id.as_deref()
                        != Some(c.agent_reservation_id.as_str())
                    || canonical::digest(&serde_json::to_value(&saved)?)?
                        != c.reserve_receipt_digest
                {
                    return Err(Error::LocalAuthority);
                }
                let old = authority::original(&tx, e, &c.agent_reservation_id, true)?;
                if old.digest != c.reserve_command_digest
                    || canonical::digest(&serde_json::to_value(&old.command)?)?
                        != canonical::digest(&serde_json::to_value(original)?)?
                {
                    return Err(Error::LocalAuthority);
                }
                // A pre-existing runtime assignment pin must agree, but this
                // observation cannot create one or acquire runtime custody.
                let pin: Option<(String, u64, String, String)> = tx.query_row(
                    "SELECT assignment_id,assignment_generation,binding_digest,provider_digest FROM owner_managed_assignment_pins WHERE child_id=?1",
                    [&c.agent_reservation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))
                ).optional()?;
                if let Some((id, generation, binding, provider)) = pin {
                    let expected = canonical::digest(&json!({"child":c.agent_reservation_id,"assignmentId":c.assignment_id,"assignmentGeneration":c.assignment_generation,"commandDigest":old.digest,"providerDigest":old.provider}))?;
                    if id != c.assignment_id || generation != c.assignment_generation || binding != expected || provider != old.provider {
                        return Err(Error::Conflict);
                    }
                }
                let eligible = match authority::original(&tx, e, &c.agent_reservation_id, false) {
                    Ok(_) => true,
                    Err(Error::LocalAuthority) => false,
                    Err(e) => return Err(e),
                };
                let revision: u64 = tx.query_row(
                    "SELECT revision FROM owner_resource_identity WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )?;
                if revision < saved.ledger_revision {
                    return Err(Error::Conflict);
                }
                let result = ManagedAgentMatrixObservation {
                    version: 1,
                    command_id: c.command_id.clone(),
                    command_digest: p.digest.clone(),
                    assignment_id: c.assignment_id.clone(),
                    agent_reservation_id: c.agent_reservation_id.clone(),
                    reserve_command_digest: c.reserve_command_digest.clone(),
                    reserve_receipt_digest: c.reserve_receipt_digest.clone(),
                    observed_ledger_revision: revision,
                    eligible,
                    reason: (!eligible).then(|| "native_reservation_unavailable".into()),
                    valid_until_ms: c.expires_at_ms,
                };
                p.check()?;
                tx.commit()?;
                p.check()?;
                Ok(result)
            })
            .await?;
        if profile.check().is_err()
            || Instant::now() >= deadline
            || self.0.closed.load(Ordering::Acquire)
        {
            return Err(Error::LocalAuthority);
        }
        Ok(result)
    }
}
