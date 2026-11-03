//! The single reader of a resource's monthly maximum.
//!
//! A resource that was offered, published or retains commitments keeps its
//! definition byte-identical: contributions, grant receipts, reserve commands
//! and Agent custody pin its revision. Its owner changes the maximum through
//! append-only native amendments instead (`owner_resource_ceiling_amendments`).
//! The latest amendment is the effective maximum; without one it is the
//! definition's own ceiling.
//!
//! Every capacity, display and copy path reads the maximum here. The raw
//! `Resource::ceiling` field stays the definition: it feeds the revision
//! digest and the writers that build a definition, and nothing else. The
//! `raw_ceiling_readers_are_audited` test keeps that true.
use super::*;
use hagency_core::allocation::Ceiling;
use hagency_core::recurring_quota::Calendar;

/// The latest amendment of a resource as `(sequence, tokens)`, if any.
pub(crate) fn latest_amendment(
    db: &Connection,
    resource_id: &str,
) -> Result<Option<(u64, u64)>, Error> {
    Ok(db
        .query_row(
            "SELECT sequence,tokens FROM owner_resource_ceiling_amendments WHERE resource_id=?1 ORDER BY sequence DESC LIMIT 1",
            [resource_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// The resource's effective monthly maximum. Never derive a revision from it:
/// the revision covers the definition, which an amendment leaves unchanged.
pub(crate) fn effective_ceiling(
    db: &Connection,
    resource: &Resource,
) -> Result<Option<Ceiling>, Error> {
    let mut ceiling = match latest_amendment(db, &resource.id())? {
        Some((_, tokens)) => Some(monthly(tokens)?),
        None => resource.ceiling.clone(),
    };
    if let (Some(value), Some(calendar)) = (&mut ceiling, calendar(db, &resource.id())?) {
        value.period = serde_json::from_value(serde_json::to_value(calendar.cadence)?)?;
    }
    Ok(ceiling)
}

/// The effective maximum's tokens. Whether a maximum exists never changes
/// with an amendment: one needs a monthly maximum and can never clear it.
pub(crate) fn effective_ceiling_tokens(
    db: &Connection,
    resource: &Resource,
) -> Result<Option<u64>, Error> {
    Ok(effective_ceiling(db, resource)?
        .and_then(|c| c.tokens)
        .map(u64::from))
}

/// A catalogue projection whose maximum is the effective one.
pub(crate) fn catalog_entry(
    db: &Connection,
    resource: &Resource,
) -> Result<CatalogResource, Error> {
    let mut entry = resource.catalog();
    entry.ceiling = effective_ceiling(db, resource)?;
    Ok(entry)
}

pub(crate) fn monthly(tokens: u64) -> Result<Ceiling, Error> {
    Ok(serde_json::from_value(
        json!({"tokens": tokens, "period": "monthly"}),
    )?)
}

#[cfg(test)]
#[path = "resource_ceiling_tests.rs"]
mod tests;

/// A trusted publication binds the resource calendar once; cadence changes
/// require a separate resource so existing calls cannot acquire a new period.
pub(crate) fn calendar(db: &Connection, resource: &str) -> Result<Option<hagency_core::recurring_quota::Calendar>, Error> {
    let raw: Option<String> = db.query_row("SELECT calendar FROM owner_resource_calendars WHERE resource_id=?1", [resource], |r| r.get(0)).optional()?;
    raw.map(|raw| {
        let calendar: hagency_core::recurring_quota::Calendar = serde_json::from_str(&raw)?;
        calendar.validate().map_err(|_| Error::LocalAuthority)?;
        Ok(calendar)
    }).transpose()
}

/// Legacy shared account limits use UTC windows. Resource-local settled spend
/// cannot safely be released against that limit at a different boundary. Check
/// every bound sibling, including history published before the owner set a cap.
/// An absent effective cap imposes no calendar restriction.
pub(crate) fn shared_account_calendars_match(
    db: &Connection,
    seat_id: &str,
    declaration: Option<&allocation::Declaration>,
    selected: Option<&Calendar>,
) -> Result<bool, Error> {
    let Some(declaration) = declaration.filter(|value| value.quota_tokens.is_some()) else {
        return Ok(true);
    };
    let period = serde_json::to_value(&declaration.period)?;
    let matches = |calendar: &Calendar| -> Result<bool, Error> {
        // A zone with today's UTC offset may change at its next DST transition.
        // Only the canonical fixed UTC calendar is supported with an account cap.
        Ok(calendar.time_zone == "UTC" && serde_json::to_value(calendar.cadence)? == period)
    };
    if let Some(selected) = selected {
        if !matches(selected)? {
            return Ok(false);
        }
    }
    let mut query = db.prepare(
        "SELECT c.calendar FROM owner_resource_calendars c JOIN resources r ON r.id=c.resource_id WHERE json_extract(r.config,'$.seatId')=?1",
    )?;
    let rows = query.query_map([seat_id], |row| row.get::<_, String>(0))?;
    for raw in rows {
        let calendar: Calendar = serde_json::from_str(&raw?)?;
        calendar.validate().map_err(|_| Error::LocalAuthority)?;
        if !matches(&calendar)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn bind_calendar(db: &Connection, resource: &str, profile: &str, grant: &str,
    selected: Option<&hagency_core::recurring_quota::Calendar>, now: u64) -> Result<(), Error> {
    if let Some(selected) = selected {
        selected.validate().map_err(|_| Error::LocalAuthority)?;
    }
    let definition = read_resource(db, resource)?;
    let declaration = account_ceiling::declaration(db, &definition.seat_id)?;
    if !shared_account_calendars_match(db, &definition.seat_id, declaration.as_ref(), selected)? {
        return Err(Error::NoCeiling);
    }
    let existing = calendar(db, resource)?;
    if let Some(existing) = existing {
        return if Some(&existing) == selected { Ok(()) } else { Err(Error::LocalAuthority) };
    }
    let Some(selected) = selected else { return Ok(()); };
    // Legacy engagements own UTC usage buckets and cannot be silently rebased.
    // Old closed grants retain their liability independently in the ledger.
    let occupied: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM engagements WHERE resource_id=?1 AND state IN ('reserved','active')) OR EXISTS(SELECT 1 FROM owner_capacity_grants WHERE resource_id=?1 AND active=1 AND CAST(json_extract(receipt,'$.periodEndMs') AS INTEGER)>?2)", params![resource,now], |r| r.get(0))?;
    if occupied { return Err(Error::Conflict); }
    db.execute("INSERT INTO owner_resource_calendars(resource_id,profile,calendar,source_grant_id,created_at_ms) VALUES(?1,?2,?3,?4,?5)", params![resource,profile,serde_json::to_string(selected)?,grant,now])?;
    Ok(())
}

#[cfg(test)]
#[path = "resource_calendar_account_tests.rs"]
mod calendar_account_tests;
