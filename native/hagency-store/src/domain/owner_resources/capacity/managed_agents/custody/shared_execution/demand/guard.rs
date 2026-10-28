//! Restriction-only checks remain active in builds with intake disabled.
use super::*;

pub(in super::super) fn require_explicit_start(db: &Connection, id: &str) -> Result<(), Error> {
    let linked: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_shared_demand_decisions WHERE start_intent_id=?1) OR EXISTS(SELECT 1 FROM owner_shared_budget_demand_decisions WHERE start_intent_id=?1)",
        [id],
        |r| r.get(0),
    )?;
    if linked {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}

pub(in super::super) fn check_attempt_control(
    db: &Connection,
    profile: &str,
    command: &SharedAttemptCommand,
) -> Result<(), Error> {
    let id: Option<String> = db
        .query_row(
            "SELECT demand_id FROM owner_shared_demand_decisions WHERE start_intent_id=?1",
            [&command.start_intent_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        return require_no_owner_stop(db, profile, &command.agent_reservation_id);
    };
    let saved = journal::load(db, profile, &command.agent_reservation_id, &id)?
        .ok_or(Error::LocalAuthority)?;
    let input = saved.input.ok_or(Error::Conflict)?;
    let original = SharedAttemptStartIntent {
        input: input.start,
        original: command.original_runtime_command.clone(),
    };
    original.matches_command(command)?;
    journal::control(
        db,
        profile,
        &command.agent_reservation_id,
        saved.offer.control_revision,
    )
}

/// An explicit Stop permanently restricts old unversioned/manual process
/// authority. Only a demand-linked attempt can prove a later exact Run revision.
pub(in super::super) fn require_no_owner_stop(
    db: &Connection,
    profile: &str,
    child: &str,
) -> Result<(), Error> {
    let stopped: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_shared_stop_intents WHERE child_id=?1 AND profile=?2)",
        params![child, profile],
        |r| r.get(0),
    )?;
    if stopped {
        Err(Error::LocalAuthority)
    } else {
        Ok(())
    }
}

/// Descriptive control pin on newly issued one-shot custody, used only to scope
/// cancellation. It is derived inside the same canonical claim transaction.
pub(in super::super) fn attempt_control_revision(
    db: &Connection,
    profile: &str,
    c: &SharedAttemptCommand,
) -> Result<Option<u64>, Error> {
    check_attempt_control(db, profile, c)?;
    let id: Option<String> = db
        .query_row(
            "SELECT demand_id FROM owner_shared_demand_decisions WHERE start_intent_id=?1",
            [&c.start_intent_id],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| {
        journal::load(db, profile, &c.agent_reservation_id, &id)?
            .map(|s| s.offer.control_revision)
            .ok_or(Error::Conflict)
    })
    .transpose()
}
