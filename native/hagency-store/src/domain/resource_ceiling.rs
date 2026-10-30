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
    match latest_amendment(db, &resource.id())? {
        Some((_, tokens)) => Ok(Some(monthly(tokens)?)),
        None => Ok(resource.ceiling.clone()),
    }
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
