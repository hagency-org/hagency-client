//! Account limits are explicit owner declarations. Older enrollment copied the
//! first pool's maximum into its provider seat without a separate account choice.
//! Ignore that inferred limit; retain original rows and immutable grant receipts.
use super::*;

pub(crate) fn declaration(
    db: &Connection,
    seat_id: &str,
) -> Result<Option<allocation::Declaration>, Error> {
    let raw: Option<String> = db
        .query_row("SELECT config FROM seats WHERE id=?1", [seat_id], |r| r.get(0))
        .optional()?;
    let stored = raw.map(|s| serde_json::from_str::<Seat>(&s)).transpose()?;
    let Some(declaration) = stored.and_then(|s| s.declaration) else { return Ok(None) };
    let enrolled: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_resource_accounts WHERE seat_id=?1 AND identity_kind='verified_provider')",
        [seat_id], |r| r.get(0),
    )?;
    // Resource migration has a separate, explicit shared-account confirmation.
    // Empty-device initialization has no targetSeat and creates no account limit.
    let explicit: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_resource_adoptions WHERE phase>=2 AND json_extract(plan,'$.targetSeat.id')=?1 AND json_extract(plan,'$.targetSeat.declaration.quotaTokens') IS NOT NULL)",
        [seat_id], |r| r.get(0),
    )?;
    Ok((!enrolled || explicit).then_some(declaration))
}

pub(crate) fn shared(db: &Connection, seat_id: &str) -> Result<bool, Error> {
    Ok(declaration(db, seat_id)?.is_some_and(|d| d.quota_tokens.is_some()))
}
