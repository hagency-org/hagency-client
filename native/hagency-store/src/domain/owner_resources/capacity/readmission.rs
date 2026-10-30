//! Readmission consumes remaining capacity without rewriting old liabilities.
use super::*;

pub(super) fn closed_grants(
    db: &rusqlite::Connection,
    profile: &ProfileAuthority,
    device: &str,
    contribution: &str,
    admission: Option<&str>,
    enrollment: Option<&str>,
) -> Result<Vec<String>, Error> {
    let mut query = db.prepare(
        "SELECT id,receipt,active FROM owner_capacity_grants WHERE profile=?1 ORDER BY id LIMIT 2049",
    )?;
    let mut rows = query.query([&profile.digest])?;
    let mut result = Vec::new();
    for index in 0..=2048 {
        let Some(row) = rows.next()? else { return Ok(result) };
        profile.check()?;
        if index == 2048 { return Err(Error::Capacity) }
        let id: String = row.get(0)?;
        let raw: String = row.get(1)?;
        if raw.len() > 12 * 1024 { return Err(Error::Capacity) }
        let grant: NativeGrantReceipt = serde_json::from_str(&raw)?;
        if grant.owner_mxid != profile.account || grant.grant_id != id {
            return Err(Error::LocalAuthority);
        }
        if grant.contribution_id != contribution { continue }
        // Chain history without its own closure: a successor that never held
        // capacity, or a grant superseded within its month by an amendment
        // whose chain was then closed. Their identity checks still apply.
        if grant.executor_device_id == device && grant.capability == CAPABILITY
            && grant.supported_shape()
            && (super::succession::void(db, &id)? || super::succession::superseded_into_closed_head(db, &id)?)
            && enrollment != Some(grant.enrollment_id.as_str())
            && !admission.is_some_and(|value| grant.admission_request_id.as_deref() == Some(value))
        { result.push(id); continue }
        if row.get::<_, bool>(2)? || grant.executor_device_id != device
            || grant.capability != CAPABILITY
            || enrollment == Some(grant.enrollment_id.as_str())
            || !grant.supported_shape()
            || admission.is_some_and(|id| grant.admission_request_id.as_deref() == Some(id))
        { return Err(Error::Conflict) }
        checked_closure(db, profile, device, &grant, false)?.ok_or(Error::Conflict)?;
        result.push(id);
    }
    Err(Error::Capacity)
}

pub(super) fn checked_closure(
    db: &rusqlite::Connection, profile: &ProfileAuthority, device: &str,
    grant: &NativeGrantReceipt, active: bool,
) -> Result<Option<NativeGrantSettlementReceipt>, Error> {
    if grant.owner_mxid != profile.account || grant.executor_device_id != device
        || grant.capability != CAPABILITY
        || !grant.supported_shape()
    { return Err(Error::LocalAuthority) }
    let saved: Option<(u64, u64, String, String)> = db.query_row(
        "SELECT fence_revision,retained_tokens,receipt,receipt_digest FROM owner_parent_settlements WHERE grant_id=?1",
        [&grant.grant_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).optional()?;
    // A committed settlement remains descriptive history even if an old active
    // bit is restored. Issuance/reoffer keep their independent active-bit guard;
    // an inconsistent bit cannot erase the original receipt or undo its fence.
    if active && saved.is_none() { return Ok(None) }
    let (fence, retained, bytes, digest) = saved.ok_or(Error::Conflict)?;
    if bytes.len() > 16 * 1024 { return Err(Error::Capacity) }
    let settled: NativeGrantSettlementReceipt = serde_json::from_str(&bytes)?;
    if settled.version != 1 || settled.grant_id != grant.grant_id
        || settled.enrollment_id != grant.enrollment_id || settled.period_key != grant.period_key
        || settled.grant_receipt_digest != canonical::digest(&serde_json::to_value(grant)?)?
        || settled.fence_revision != fence || fence <= grant.ledger_revision
        || settled.ledger_revision < fence || settled.retained_tokens != retained
        || !matches!(settled.state.as_str(), "closed" | "retained")
        || canonical::digest(&serde_json::to_value(&settled)?)? != digest
    { return Err(Error::Conflict) }
    // A row/active bit alone is not a committed settlement. Require the
    // original writer's immutable command result as well.
    let committed: Option<NativeGrantSettlementReceipt> = receipt(db, &profile.digest,
        device, &settled.command_id, &settled.command_digest)?;
    if committed.as_ref().map(serde_json::to_value).transpose()? != Some(serde_json::to_value(&settled)?) {
        return Err(Error::Conflict);
    }
    if settled.known_consumed_tokens.checked_add(settled.unresolved_tokens) != Some(settled.retained_tokens)
        || settled.retained_tokens > hagency_core::JSON_SAFE_MAX
        || (settled.state == "closed") != (settled.retained_tokens == 0)
    { return Err(Error::Conflict) }
    Ok(Some(settled))
}

impl OwnerResourceRegistry {
    /// Canonical descriptive state for an already recovered original receipt.
    /// Active means no closure is recorded; missing/inconsistent history is an
    /// error, never a claim that the grant closed or its capacity was refunded.
    pub async fn capacity_grant_closure(
        &self, profile: ProfileAuthority, device: String, grant: NativeGrantReceipt,
    ) -> Result<Option<NativeGrantSettlementReceipt>, Error> {
        profile.check()?;
        identifier(&device, 128)?;
        identifier(&grant.grant_id, 128)?;
        let caller = profile.clone();
        let fence = self.0.clone();
        let result = self.0.store.call(16 * 1024, move |db| {
            profile.check()?;
            if fence.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority) }
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let row: Option<(String, String, bool)> = tx.query_row(
                "SELECT profile,receipt,active FROM owner_capacity_grants WHERE id=?1",
                [&grant.grant_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).optional()?;
            let (owner, raw, active) = row.ok_or(Error::Conflict)?;
            if raw.len() > 12 * 1024 { return Err(Error::Capacity) }
            if owner != profile.digest
                || canonical::digest(&serde_json::from_str::<serde_json::Value>(&raw)?)?
                    != canonical::digest(&serde_json::to_value(&grant)?)?
            { return Err(Error::LocalAuthority) }
            let result = checked_closure(&tx, &profile, &device, &grant, active)?;
            profile.check()?;
            if fence.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority) }
            tx.commit()?;
            Ok(result)
        }).await?;
        caller.check()?;
        if self.0.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority) }
        Ok(result)
    }

    /// Descriptive closure evidence only. New consent repeats this check in
    /// its atomic writer and retains all old receipts and settlement amounts.
    pub async fn closed_prior_admission_grants(
        &self, profile: ProfileAuthority, device: String, contribution: String, admission: String,
    ) -> Result<Vec<String>, Error> {
        self.closed_contribution_grants_for(profile, device, contribution, Some(admission)).await
    }

    /// Before a new admission exists, require every previous grant for this
    /// contribution to have a committed fence and settlement. Retained amounts
    /// remain charged; this read does not release or replace any grant.
    pub async fn closed_contribution_grants(
        &self, profile: ProfileAuthority, device: String, contribution: String,
    ) -> Result<Vec<String>, Error> {
        self.closed_contribution_grants_for(profile, device, contribution, None).await
    }

    async fn closed_contribution_grants_for(
        &self, profile: ProfileAuthority, device: String, contribution: String,
        admission: Option<String>,
    ) -> Result<Vec<String>, Error> {
        profile.check()?;
        for id in [&device, &contribution] { identifier(id, 128)?; }
        if let Some(id) = &admission { identifier(id, 128)?; }
        let caller = profile.clone();
        let fence = self.0.clone();
        let result = self.0.store.call(4096, move |db| {
            profile.check()?;
            if fence.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority) }
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let result = closed_grants(&tx, &profile, &device, &contribution, admission.as_deref(), None)?;
            profile.check()?;
            if fence.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority) }
            tx.commit()?;
            Ok(result)
        }).await?;
        caller.check()?;
        if self.0.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority) }
        Ok(result)
    }
}
