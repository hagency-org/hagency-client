//! Additive, unadvertised parent cleanup. A durable fence precedes any refund.
//! No execution API exists for reservation_only_v1. A future executor must
//! atomically change that mode before any work; unknown modes retain the whole
//! parent and cannot use the zero-consumption child release path.
use super::*;
use sha2::{Digest, Sha256};

pub const PARENT_CAPABILITY: &str = "hagency.parent_settlement.v1";
pub(super) const RESERVATION_ONLY: &str = "reservation_only_v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantClosureCommand {
    pub version: u16,
    pub command_id: String,
    pub operation: String,
    pub grant_id: String,
    pub grant_receipt_digest: String,
    pub previous_settlement_digest: Option<String>,
    pub reason: String,
    pub requested_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeGrantSettlementReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub grant_id: String,
    pub grant_receipt_digest: String,
    pub enrollment_id: String,
    pub period_key: String,
    pub fence_revision: u64,
    pub ledger_revision: u64,
    pub state: String,
    pub retained_tokens: u64,
    pub known_consumed_tokens: u64,
    pub unresolved_tokens: u64,
    pub liability_count: u64,
    pub liability_digest: String,
}

/// Construct only from a current authenticated native delivery for this exact
/// command. Read-only recovery responses never constitute execution authority.
pub struct VerifiedParentDelivery {
    enrollment: VerifiedManagementEnrollment,
    command: GrantClosureCommand,
    digest: String,
    lease_expires: Instant,
}
impl VerifiedParentDelivery {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        command: GrantClosureCommand,
        digest: String,
        execution_allowed: bool,
        lease_expires: Instant,
    ) -> Result<Self, Error> {
        enrollment.check()?;
        validate(&command)?;
        hash(&digest)?;
        let value = serde_json::to_value(&command)?;
        if !execution_allowed
            || lease_expires <= Instant::now()
            || lease_expires > Instant::now() + Duration::from_secs(60)
            || serde_json::to_vec(&value)?.len() > 4096
        {
            return Err(Error::LocalAuthority);
        }
        if canonical::digest(&value)? != digest {
            return Err(Error::Conflict);
        }
        Ok(Self {
            enrollment,
            command,
            digest,
            lease_expires,
        })
    }
    fn check(&self, registry: &Inner) -> Result<(), Error> {
        self.enrollment.check()?;
        if registry.closed.load(Ordering::Acquire) || Instant::now() >= self.lease_expires {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}

fn validate(c: &GrantClosureCommand) -> Result<(), Error> {
    identifier(&c.command_id, 128)?;
    identifier(&c.grant_id, 128)?;
    hash(&c.grant_receipt_digest)?;
    exact(c.requested_at_ms)?;
    let valid = match c.operation.as_str() {
        "grant.close" => {
            c.previous_settlement_digest.is_none()
                && matches!(
                    c.reason.as_str(),
                    "owner_withdrawal" | "administrator_withdrawal" | "contribution_revoked"
                )
        }
        "grant.settle" => {
            hash(
                c.previous_settlement_digest
                    .as_deref()
                    .ok_or(Error::Conflict)?,
            )?;
            c.reason == "settlement_progress"
        }
        _ => false,
    };
    if c.version != 1 || !valid {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}

impl OwnerResourceRegistry {
    pub async fn apply_parent_settlement(
        &self,
        delivery: VerifiedParentDelivery,
    ) -> Result<NativeGrantSettlementReceipt, Error> {
        delivery.check(&self.0)?;
        let profile = delivery.enrollment.profile.clone();
        let fence = self.0.clone();
        let result = self
            .0
            .store
            .call(8192, move |db| db.owner_parent_settlement(delivery, fence))
            .await?;
        if profile.check().is_err() || self.0.closed.load(Ordering::Acquire) {
            return Err(Error::OutcomeUnknown);
        }
        Ok(result)
    }

    /// Return the exact historical receipt, even if a later settlement exists.
    /// This method never creates a fence, runs a command or refunds capacity.
    pub async fn recover_parent_settlement(
        &self,
        profile: ProfileAuthority,
        executor_device_id: String,
        command_id: String,
        command_digest: String,
    ) -> Result<Option<NativeGrantSettlementReceipt>, Error> {
        profile.check()?;
        identifier(&executor_device_id, 128)?;
        identifier(&command_id, 128)?;
        hash(&command_digest)?;
        let caller = profile.clone();
        let fence = self.0.clone();
        let result = self
            .0
            .store
            .call(4096, move |db| {
                check_read(&profile, &fence)?;
                let result = receipt(
                    &db.db,
                    &profile.digest,
                    &executor_device_id,
                    &command_id,
                    &command_digest,
                )?;
                check_read(&profile, &fence)?;
                Ok(result)
            })
            .await?;
        check_read(&caller, &self.0)?;
        Ok(result)
    }

    /// The latest complete settlement is separate from exact command recovery.
    pub async fn parent_settlement(
        &self,
        profile: ProfileAuthority,
        executor_device_id: String,
        grant_id: String,
    ) -> Result<Option<NativeGrantSettlementReceipt>, Error> {
        profile.check()?;
        identifier(&executor_device_id, 128)?;
        identifier(&grant_id, 128)?;
        let caller = profile.clone();
        let fence = self.0.clone();
        let result = self.0.store.call(4096, move |db| {
            check_read(&profile, &fence)?;
            let row: Option<(String, String, Option<String>)> = db.db.query_row(
                "SELECT g.profile,g.receipt,p.receipt FROM owner_capacity_grants g LEFT JOIN owner_parent_settlements p ON p.grant_id=g.id WHERE g.id=?1",
                [&grant_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))
            ).optional()?;
            let result = if let Some((owner, grant, settlement)) = row {
                let grant: NativeGrantReceipt = serde_json::from_str(&grant)?;
                if owner != profile.digest || grant.owner_mxid != profile.account || grant.executor_device_id != executor_device_id {
                    return Err(Error::LocalAuthority);
                }
                settlement.map(|v| serde_json::from_str(&v)).transpose()?
            } else { None };
            check_read(&profile, &fence)?;
            Ok(result)
        }).await?;
        check_read(&caller, &self.0)?;
        Ok(result)
    }
}
fn check_read(profile: &ProfileAuthority, fence: &Inner) -> Result<(), Error> {
    profile.check()?;
    if fence.closed.load(Ordering::Acquire) {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}

impl DomainRepository {
    fn owner_parent_settlement(
        &mut self,
        delivery: VerifiedParentDelivery,
        fence: Arc<Inner>,
    ) -> Result<NativeGrantSettlementReceipt, Error> {
        delivery.check(&fence)?;
        let e = &delivery.enrollment;
        let c = &delivery.command;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(old) = receipt(
            &tx,
            &e.profile.digest,
            &e.executor_device_id,
            &c.command_id,
            &delivery.digest,
        )? {
            delivery.check(&fence)?;
            return Ok(old);
        }
        let row: Option<(String, String, String)> = tx
            .query_row(
                "SELECT profile,receipt,accounting_mode FROM owner_capacity_grants WHERE id=?1",
                [&c.grant_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (profile, grant, mode) = row.ok_or(Error::LocalAuthority)?;
        let grant: NativeGrantReceipt = serde_json::from_str(&grant)?;
        if profile != e.profile.digest
            || !e.matches_cleanup(&grant)
            || e.executor_generation < grant.executor_generation
            || canonical::digest(&serde_json::to_value(&grant)?)? != c.grant_receipt_digest
            || c.grant_id != grant.grant_id
        {
            return Err(Error::LocalAuthority);
        }
        let old: Option<(u64, String, String)> = tx.query_row(
            "SELECT fence_revision,receipt,receipt_digest FROM owner_parent_settlements WHERE grant_id=?1", [&c.grant_id],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))
        ).optional()?;
        let previous: Option<NativeGrantSettlementReceipt> = old
            .as_ref()
            .map(|(_, v, _)| serde_json::from_str(v))
            .transpose()?;
        match (c.operation.as_str(), &old) {
            ("grant.close", None) => {}
            ("grant.settle", Some((_, _, digest)))
                if c.previous_settlement_digest.as_ref() == Some(digest) => {}
            _ => return Err(Error::Conflict),
        }
        // Every distinct command owns a new global revision. No child receipt
        // is fabricated here, and the permanent fence is never advanced/reset.
        let ledger_revision = revision(&tx)?;
        let fence_revision = old.as_ref().map_or(ledger_revision, |(r, _, _)| *r);
        if fence_revision <= grant.ledger_revision {
            return Err(Error::Conflict);
        }
        let liability = liabilities(&tx, &grant, &mode, previous.as_ref())?;
        let result = NativeGrantSettlementReceipt {
            version: 1,
            command_id: c.command_id.clone(),
            command_digest: delivery.digest.clone(),
            grant_id: c.grant_id.clone(),
            grant_receipt_digest: c.grant_receipt_digest.clone(),
            enrollment_id: grant.enrollment_id.clone(),
            period_key: grant.period_key.clone(),
            fence_revision,
            ledger_revision,
            state: if liability.unresolved == 0 {
                "closed"
            } else {
                "retained"
            }
            .into(),
            retained_tokens: liability.retained,
            known_consumed_tokens: liability.consumed,
            unresolved_tokens: liability.unresolved,
            liability_count: liability.count,
            liability_digest: canonical::digest(&liability.manifest)?,
        };
        tx.execute(
            "UPDATE owner_capacity_grants SET active=0 WHERE id=?1",
            [&c.grant_id],
        )?;
        tx.execute(
            "INSERT INTO owner_parent_settlements(grant_id,fence_revision,retained_tokens,receipt,receipt_digest,manifest) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(grant_id) DO UPDATE SET retained_tokens=excluded.retained_tokens,receipt=excluded.receipt,receipt_digest=excluded.receipt_digest,manifest=excluded.manifest",
            params![c.grant_id,fence_revision,result.retained_tokens,serde_json::to_string(&result)?,canonical::digest(&serde_json::to_value(&result)?)?,serde_json::to_string(&liability.manifest)?]
        )?;
        save(
            &tx,
            &e.profile.digest,
            &e.executor_device_id,
            &c.command_id,
            &delivery.digest,
            &result,
        )?;
        delivery.check(&fence)?;
        tx.commit()?;
        delivery.check(&fence).map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}

struct Liabilities {
    retained: u64,
    consumed: u64,
    unresolved: u64,
    count: u64,
    manifest: serde_json::Value,
}
fn liabilities(
    db: &Connection,
    grant: &NativeGrantReceipt,
    mode: &str,
    previous: Option<&NativeGrantSettlementReceipt>,
) -> Result<Liabilities, Error> {
    // Stream a bounded, canonical child manifest. Its SHA256 commits every
    // ordered row, including tombstones, without a large in-memory JSON array.
    let mut digest = Sha256::new();
    digest.update(b"[");
    let mut query = db.prepare("SELECT id,state,requested_tokens,retained_tokens,binding_digest,accounting_mode FROM owner_project_reservations WHERE grant_id=?1 ORDER BY id")?;
    let mut rows = query.query([&grant.grant_id])?;
    let mut child_count = 0_u64;
    let mut count = 0_u64;
    let mut held = 0_u64;
    let mut known_consumed = 0_u64;
    let mut unknown = !matches!(mode, RESERVATION_ONLY | managed_agents::MANAGED);
    while let Some(row) = rows.next()? {
        if child_count >= 100_000 {
            return Err(Error::Capacity);
        }
        let id: String = row.get(0)?;
        let state: String = row.get(1)?;
        let requested: u64 = row.get(2)?;
        let retained: u64 = row.get(3)?;
        let binding: String = row.get(4)?;
        let child_mode: String = row.get(5)?;
        let managed = if child_mode == managed_agents::MANAGED {
            Some(managed_agents::project_liability(db, &id)?)
        } else {
            None
        };
        if child_count > 0 {
            digest.update(b",");
        }
        let mut entry = json!({"id":id,"state":state,"requestedTokens":requested,"retainedTokens":retained,"bindingDigest":binding,"accountingMode":child_mode});
        if let Some(value) = &managed {
            entry["managedLiabilityDigest"] = json!(value.digest);
        }
        digest.update(canonical::encode(&entry)?.as_bytes());
        child_count += 1;
        unknown |= managed.as_ref().map_or(
            child_mode != RESERVATION_ONLY
                || (state == "reserved" && retained != requested)
                || (state == "released" && retained != 0),
            |value| !value.complete,
        );
        let unresolved = managed.as_ref().map_or(retained, |value| value.unresolved);
        known_consumed = known_consumed
            .checked_add(managed.as_ref().map_or(0, |value| value.consumed))
            .filter(|n| *n <= hagency_core::JSON_SAFE_MAX)
            .ok_or(Error::Capacity)?;
        held = held
            .checked_add(unresolved)
            .filter(|n| *n <= hagency_core::JSON_SAFE_MAX)
            .ok_or(Error::Capacity)?;
        if managed.as_ref().map_or(retained, |value| value.retained) > 0 {
            count += 1;
        }
    }
    let (direct,direct_count)=managed_agents::pool_liability(db,&grant.grant_id)?;
    if direct_count>0 {
        if child_count>0 {digest.update(b",");}
        digest.update(canonical::encode(&json!({"poolAgentsDigest":direct.digest,"count":direct_count}))?.as_bytes());
        child_count=child_count.checked_add(direct_count).ok_or(Error::Capacity)?;
        known_consumed=known_consumed.checked_add(direct.consumed).filter(|n|*n<=hagency_core::JSON_SAFE_MAX).ok_or(Error::Capacity)?;
        held=held.checked_add(direct.unresolved).filter(|n|*n<=hagency_core::JSON_SAFE_MAX).ok_or(Error::Capacity)?;
        unknown|=!direct.complete;
        if direct.retained>0 {count+=direct_count;}
    }
    digest.update(b"]");
    let consumed = known_consumed.max(previous.map_or(0, |p| p.known_consumed_tokens));
    let mut unresolved = held;
    if unknown {
        // An old/future accounting mode cannot establish that missing children
        // mean unused capacity. Keep at least the entire original parent hold
        // and every previously reported liability; UTC expiry changes nothing.
        unresolved = unresolved
            .max(grant.allocated_tokens)
            .max(previous.map_or(0, |p| p.unresolved_tokens));
        count += 1;
    }
    if consumed > 0 {
        count += 1;
    }
    let retained = consumed
        .checked_add(unresolved)
        .filter(|n| *n <= hagency_core::JSON_SAFE_MAX)
        .ok_or(Error::Capacity)?;
    let manifest = json!({"version":1,"grantId":grant.grant_id,"accountingMode":mode,"childCount":child_count,"childrenDigest":format!("{:x}",digest.finalize()),"unknownAccounting":unknown,"knownConsumedTokens":consumed,"unresolvedTokens":unresolved,"liabilityCount":count});
    Ok(Liabilities {
        retained,
        consumed,
        unresolved,
        count,
        manifest,
    })
}
