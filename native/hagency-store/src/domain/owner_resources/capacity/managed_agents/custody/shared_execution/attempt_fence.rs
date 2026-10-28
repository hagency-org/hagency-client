//! A permanent local cancellation fence, never evidence that a process stopped.
use super::*;

/// Exact retained cancellation result. It grants no execution or refund right.
/// `claimed_revision` records whether one-shot custody existed when fenced;
/// even None does not become a network stopped-process receipt.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SharedAttemptFence {
    pub command_digest: String,
    pub fence_revision: u64,
    pub claimed_revision: Option<u64>,
}

pub(super) fn require_unfenced(db: &Connection, c: &SharedAttemptCommand) -> Result<(), Error> {
    let fenced: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_shared_successor_fences WHERE child_id=?1 AND (process_attempt=?2 OR command_id=?3 OR start_intent_id=?4 OR predecessor_id=?5))",
        params![c.agent_reservation_id,c.process_attempt,c.command_id,c.start_intent_id,c.predecessor.command_id],
        |r| r.get(0),
    )?;
    if fenced {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}

impl OwnerResourceRegistry {
    /// Same-installation native cleanup can permanently deny an exact pending
    /// or issued process attempt. A historical result never grants fresh issue
    /// authority. The adapter must separately stop any retained actual process.
    pub async fn fence_shared_successor(
        &self,
        authority: ManagedCleanupAuthority,
        command: SharedAttemptCommand,
    ) -> Result<SharedAttemptFence, Error> {
        authority.check(&self.0)?;
        command.validate(&authority.enrollment.profile.account)?;
        if authority.child != command.agent_reservation_id {
            return Err(Error::LocalAuthority);
        }
        let profile = authority.enrollment.profile.clone();
        let expires = authority.deadline();
        let registry = self.0.clone();
        let value = self.0.store.call(64 * 1024, move |db| {
            authority.check(&registry)?;
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let original = authority::cleanup(&tx, &authority)?;
            if original.digest != command.original_runtime_command.reserve_command_digest {
                return Err(Error::LocalAuthority);
            }
            let saved: Option<(String,String)> = tx.query_row(
                "SELECT command_digest,profile FROM owner_shared_runtime_lifetimes WHERE child_id=?1",
                [&command.agent_reservation_id], |r| Ok((r.get(0)?,r.get(1)?)),
            ).optional()?;
            if saved.as_ref().is_none_or(|(digest,profile)| digest != &command.original_runtime_digest || profile != &authority.enrollment.profile.digest) {
                return Err(Error::LocalAuthority);
            }
            let digest = canonical::digest(&serde_json::to_value(&command)?)?;
            let old: Option<(String,String,u64,Option<u64>)> = tx.query_row(
                "SELECT profile,command_digest,fence_revision,claimed_revision FROM owner_shared_successor_fences WHERE child_id=?1 AND process_attempt=?2",
                params![command.agent_reservation_id,command.process_attempt],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
            ).optional()?;
            if let Some((profile,old,revision,claimed)) = old {
                if profile != authority.enrollment.profile.digest || old != digest { return Err(Error::Conflict); }
                authority.check(&registry)?;
                return Ok(SharedAttemptFence{command_digest:digest,fence_revision:revision,claimed_revision:claimed});
            }
            let prior:Option<(String,u64)>=tx.query_row(
                "SELECT command_digest,issued_revision FROM owner_shared_successor_attempts WHERE child_id=?1 AND process_attempt=?2",
                params![command.agent_reservation_id,command.process_attempt],|r|Ok((r.get(0)?,r.get(1)?)),
            ).optional()?;
            if prior.as_ref().is_some_and(|(d,_)| d != &digest) { return Err(Error::Conflict); }
            let claimed = prior.map(|(_,revision)|revision);
            bounded_row(&tx,"owner_shared_successor_fences","command_id",&command.command_id,100_000)?;
            let revision = revision(&tx)?;
            tx.execute(
                "INSERT INTO owner_shared_successor_fences VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![command.command_id,command.agent_reservation_id,authority.enrollment.profile.digest,command.process_attempt,command.predecessor.command_id,command.start_intent_id,canonical::encode(&serde_json::to_value(&command)?)?,digest,revision,claimed],
            )?;
            authority.check(&registry)?;
            tx.commit()?;
            authority.check(&registry).map_err(|_|Error::OutcomeUnknown)?;
            Ok(SharedAttemptFence{command_digest:digest,fence_revision:revision,claimed_revision:claimed})
        }).await?;
        after(&profile, &self.0, expires)?;
        Ok(value)
    }
}
