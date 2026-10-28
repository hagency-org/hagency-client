//! Exact central qualification command custody. No provider or work invocation.
use super::*;
mod claim;
mod observed;
pub use observed::{ManagedRuntimeEvidenceSink, ManagedRuntimeRecordedProcess};
mod types;
pub use claim::{ManagedRuntimeClaim, ManagedRuntimeCustody, ManagedRuntimeHistory};
pub use types::*;

/// Authenticated transport output, never deserializable or constructible without
/// the native profile/enrollment proof. Recovery responses cannot mint custody.
pub struct VerifiedManagedRuntimeDelivery {
    enrollment: VerifiedManagementEnrollment,
    command: ManagedRuntimeCommand,
    digest: String,
    expires: Instant,
}
impl VerifiedManagedRuntimeDelivery {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        command: ManagedRuntimeCommand,
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
            || serde_json::to_vec(&command)?.len() > 16 * 1024
        {
            return Err(Error::LocalAuthority);
        }
        let expires = if matches!(command, ManagedRuntimeCommand::Provision(_)) {
            let remaining = command
                .original()
                .expires_at_ms
                .saturating_sub(graphs::now_ms()?);
            expires.min(Instant::now() + Duration::from_millis(remaining))
        } else {
            expires
        };
        let result = Self {
            enrollment,
            command,
            digest,
            expires,
        };
        result.check()?;
        Ok(result)
    }
    fn check(&self) -> Result<(), Error> {
        self.enrollment.check()?;
        if Instant::now() >= self.expires {
            return Err(Error::LocalAuthority);
        }
        let original = self.command.original();
        if matches!(self.command, ManagedRuntimeCommand::Provision(_)) {
            let now = graphs::now_ms()?;
            if now < original.issued_at_ms
                || now >= original.expires_at_ms
                || self.enrollment.executor_generation
                    != original.reserve_command.executor_generation
            {
                return Err(Error::LocalAuthority);
            }
        } else if self.enrollment.executor_generation < original.reserve_command.executor_generation
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    fn deadline(&self) -> Instant {
        self.expires.min(self.enrollment.expires)
    }
}

/// Checks immutable native evidence before considering any fresh authority. A
/// central receipt cannot initialize a missing child or provider account.
fn original(
    db: &Connection,
    p: &VerifiedManagedRuntimeDelivery,
) -> Result<(Binding, String), Error> {
    let e = &p.enrollment;
    let c = p.command.original();
    let saved: ManagedAgentReservationReceipt = receipt(
        db,
        &e.profile.digest,
        &e.executor_device_id,
        &c.reserve_command.command_id,
        &c.reserve_command_digest,
    )?
    .ok_or(Error::LocalAuthority)?;
    if saved.state != "reserved"
        || saved.agent_reservation_id.as_deref() != Some(c.agent_reservation_id.as_str())
        || canonical::digest(&serde_json::to_value(&saved)?)? != c.reserve_receipt_digest
    {
        return Err(Error::LocalAuthority);
    }
    let b = authority::original(db, e, &c.agent_reservation_id, true)?;
    if b.digest != c.reserve_command_digest
        || canonical::digest(&serde_json::to_value(&b.command)?)?
            != canonical::digest(&serde_json::to_value(&c.reserve_command)?)?
    {
        return Err(Error::Conflict);
    }
    let binding = canonical::digest(
        &json!({"child":c.agent_reservation_id,"assignmentId":c.assignment_id,"assignmentGeneration":c.assignment_generation,"commandDigest":b.digest,"providerDigest":b.provider}),
    )?;
    let pin: Option<(String,u64,String,String)> = db.query_row("SELECT assignment_id,assignment_generation,binding_digest,provider_digest FROM owner_managed_assignment_pins WHERE child_id=?1",[&c.agent_reservation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    if pin.is_some_and(|(a, g, h, provider)| {
        a != c.assignment_id
            || g != c.assignment_generation
            || h != binding
            || provider != b.provider
    }) {
        return Err(Error::Conflict);
    }
    Ok((b, binding))
}
