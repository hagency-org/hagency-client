//! Permanent v2 non-issuance evidence. The fence row is immutable, records
//! whether a claim existed when it was written, and blocks every later claim,
//! so an unclaimed fence proves the attempt never started. Its not_started
//! receipt is derived from that fence alone: identical bytes on every replay,
//! no process identity, no released hold and no new issue authority.
use super::*;

fn receipt(c: &SharedAttemptCommand, digest: &str, revision: u64) -> Result<SharedAttemptProcessReceipt, Error> {
    let id = canonical::digest(&json!({"command":c.command_id,"digest":digest,"state":"not_started"}))?;
    Ok(SharedAttemptProcessReceipt {
        version: 2,
        observation_id: format!("sanu_{}", &id[..48]),
        command_id: c.command_id.clone(),
        command_digest: digest.to_owned(),
        assignment_id: c.assignment_id.clone(),
        agent_reservation_id: c.agent_reservation_id.clone(),
        process_attempt: c.process_attempt,
        original_runtime_digest: c.original_runtime_digest.clone(),
        ledger_revision: revision,
        process_digest: None,
        state: "not_started".into(),
        policy_digest: None,
        reason: None,
    })
}

/// None while no unclaimed fence exists for this exact command; claimed
/// custody is never relabelled as never started.
pub(super) fn load(
    db: &Connection,
    profile: &str,
    c: &SharedAttemptCommand,
) -> Result<Option<SharedAttemptProcessReceipt>, Error> {
    let row: Option<(String, String, String, String, u64, Option<u64>)> = db.query_row(
        "SELECT profile,child_id,command_payload,command_digest,fence_revision,claimed_revision FROM owner_shared_successor_fences WHERE command_id=?1",
        [&c.command_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
    ).optional()?;
    let Some((saved_profile, child, payload, digest, fence, claimed)) = row else { return Ok(None) };
    if saved_profile != profile
        || child != c.agent_reservation_id
        || canonical::digest(&serde_json::from_str::<serde_json::Value>(&payload)?)? != digest
        || canonical::digest(&serde_json::to_value(c)?)? != digest
    {
        return Err(Error::Conflict);
    }
    let actual: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_shared_successor_attempts WHERE id=?1 OR (child_id=?2 AND process_attempt=?3))",
        params![c.command_id, c.agent_reservation_id, c.process_attempt], |r| r.get(0),
    )?;
    if claimed.is_some() || actual {
        return Ok(None);
    }
    let value = receipt(c, &digest, fence)?;
    value.validate_for(c)?;
    Ok(Some(value))
}

/// The highest never-started attempt after the last actual process. The next
/// attempt names it as predecessor, with an empty process digest.
pub(super) enum UnstartedHead {
    V2(SharedAttemptCommand, SharedAttemptProcessReceipt),
    #[cfg(feature = "shared-budget-execution")]
    V3(SharedBudgetAttemptCommand, SharedBudgetAttemptProcessReceipt),
}
// Some accessors serve only the v3 budget snapshot.
#[cfg_attr(not(feature = "shared-budget-execution"), allow(dead_code))]
impl UnstartedHead {
    pub(super) fn command_id(&self) -> &str {
        match self {
            Self::V2(c, _) => &c.command_id,
            #[cfg(feature = "shared-budget-execution")]
            Self::V3(c, _) => &c.command_id,
        }
    }
    pub(super) fn process_attempt(&self) -> u64 {
        match self {
            Self::V2(c, _) => c.process_attempt,
            #[cfg(feature = "shared-budget-execution")]
            Self::V3(c, _) => c.process_attempt,
        }
    }
    pub(super) fn command_digest(&self) -> Result<String, Error> {
        Ok(match self {
            Self::V2(c, _) => canonical::digest(&serde_json::to_value(c)?)?,
            #[cfg(feature = "shared-budget-execution")]
            Self::V3(c, _) => canonical::digest(&serde_json::to_value(c)?)?,
        })
    }
    pub(super) fn receipt_digest(&self) -> Result<String, Error> {
        Ok(match self {
            Self::V2(_, r) => canonical::digest(&serde_json::to_value(r)?)?,
            #[cfg(feature = "shared-budget-execution")]
            Self::V3(_, r) => canonical::digest(&serde_json::to_value(r)?)?,
        })
    }
    pub(super) fn ledger_revision(&self) -> u64 {
        match self {
            Self::V2(_, r) => r.ledger_revision,
            #[cfg(feature = "shared-budget-execution")]
            Self::V3(_, r) => r.ledger_revision,
        }
    }
}

/// A v2 head is any unclaimed fence; a v3 head needs its recorded receipt.
pub(super) fn head(
    db: &Connection,
    profile: &str,
    original: &SharedRuntimeCommand,
) -> Result<Option<UnstartedHead>, Error> {
    let payload: Option<String> = db.query_row(
        "SELECT f.command_payload FROM owner_shared_successor_fences f WHERE f.child_id=?1 AND f.process_attempt>coalesce((SELECT max(process_attempt) FROM owner_shared_successor_attempts WHERE child_id=?1),1) AND ((json_extract(f.command_payload,'$.version')=2 AND f.claimed_revision IS NULL) OR EXISTS(SELECT 1 FROM owner_shared_unstarted_attempts u WHERE u.command_id=f.command_id)) ORDER BY f.process_attempt DESC LIMIT 1",
        [&original.agent_reservation_id], |r| r.get(0),
    ).optional()?;
    let Some(payload) = payload else { return Ok(None) };
    let value: serde_json::Value = serde_json::from_str(&payload)?;
    let original_digest = canonical::digest(&serde_json::to_value(original)?)?;
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(2) => {
            let c: SharedAttemptCommand = serde_json::from_value(value)?;
            c.validate(&original.reserve_command.manager_mxid)?;
            if c.agent_reservation_id != original.agent_reservation_id || c.original_runtime_digest != original_digest {
                return Err(Error::Conflict);
            }
            let r = load(db, profile, &c)?.ok_or(Error::Conflict)?;
            Ok(Some(UnstartedHead::V2(c, r)))
        }
        #[cfg(feature = "shared-budget-execution")]
        Some(3) => {
            let c: SharedBudgetAttemptCommand = serde_json::from_value(value)?;
            c.validate(&original.reserve_command.manager_mxid)?;
            if c.agent_reservation_id != original.agent_reservation_id || c.original_runtime_digest != original_digest {
                return Err(Error::Conflict);
            }
            let r = budget_unstarted::load(db, profile, &c)?.ok_or(Error::Conflict)?;
            Ok(Some(UnstartedHead::V3(c, r)))
        }
        // Builds without v3 never treated its rows as a head.
        #[cfg(not(feature = "shared-budget-execution"))]
        Some(3) => Ok(None),
        _ => Err(Error::LocalAuthority),
    }
}
