//! Monthly re-grant on the same admission. Earlier receipts stay immutable
//! history; a new grant only needs the month they covered to have ended.
use super::*;

/// Every earlier grant of this contribution must be history before its
/// admission is granted for `period`:
/// - a grant of this same admission whose UTC month ended at or before `now`,
///   issued under this same enrollment, or
/// - a grant of another admission with a committed canonical closure, as
///   readmission requires.
///
/// No earlier grant may cover `period`, so a second grant in one month is
/// refused. At least one earlier grant of this admission must exist: a first
/// grant on an admission uses the readmission check instead.
#[allow(clippy::too_many_arguments)] // Each value is native-owned and rechecked here.
pub(super) fn prior_periods_ended(
    db: &rusqlite::Connection,
    profile: &ProfileAuthority,
    device: &str,
    contribution: &str,
    admission: &str,
    enrollment: Option<&str>,
    period: &OwnerGrantPeriod,
    now: u64,
) -> Result<Vec<String>, Error> {
    period.require_current(now)?;
    let mut query = db.prepare(
        "SELECT id,receipt,active FROM owner_capacity_grants WHERE profile=?1 ORDER BY id LIMIT 2049",
    )?;
    let mut rows = query.query([&profile.digest])?;
    let mut result = Vec::new();
    let mut same_admission = false;
    for index in 0..=2048 {
        let Some(row) = rows.next()? else {
            return if same_admission {
                Ok(result)
            } else {
                Err(Error::Conflict)
            };
        };
        profile.check()?;
        if index == 2048 {
            return Err(Error::Capacity);
        }
        let id: String = row.get(0)?;
        let raw: String = row.get(1)?;
        if raw.len() > 12 * 1024 {
            return Err(Error::Capacity);
        }
        let grant: NativeGrantReceipt = serde_json::from_str(&raw)?;
        if grant.owner_mxid != profile.account || grant.grant_id != id {
            return Err(Error::LocalAuthority);
        }
        if grant.contribution_id != contribution {
            continue;
        }
        // A rejected successor or a cancelled renewal never held capacity; it
        // is history whatever month it named.
        if grant.executor_device_id == device
            && grant.capability == CAPABILITY
            && grant.supported_shape()
            && grant.admission_request_id.as_deref() == Some(admission)
            && super::succession::void(db, &id)?
        {
            result.push(id);
            continue;
        }
        if grant.executor_device_id != device
            || grant.capability != CAPABILITY
            || !grant.supported_shape()
            || grant.period_key == period.period_key
            || grant.period_end_ms > period.period_start_ms
        {
            return Err(Error::Conflict);
        }
        if grant.admission_request_id.as_deref() == Some(admission) {
            if grant.period_end_ms > now || enrollment.is_some_and(|e| e != grant.enrollment_id) {
                return Err(Error::Conflict);
            }
            same_admission = true;
        } else {
            if row.get::<_, bool>(2)? || enrollment == Some(grant.enrollment_id.as_str()) {
                return Err(Error::Conflict);
            }
            super::readmission::checked_closure(db, profile, device, &grant, false)?
                .ok_or(Error::Conflict)?;
        }
        result.push(id);
    }
    Err(Error::Capacity)
}

impl OwnerResourceRegistry {
    /// Descriptive evidence only: the earlier grants of this contribution that
    /// are history for a new month on `admission`. New consent repeats this
    /// check in its atomic writer; no grant, hold or settlement changes here.
    pub async fn regrant_prior_grants(
        &self,
        profile: ProfileAuthority,
        device: String,
        contribution: String,
        admission: String,
    ) -> Result<Vec<String>, Error> {
        profile.check()?;
        for id in [&device, &contribution, &admission] {
            identifier(id, 128)?;
        }
        let caller = profile.clone();
        let fence = self.0.clone();
        let result = self
            .0
            .store
            .call(4096, move |db| {
                profile.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Deferred)?;
                let now = graphs::now_ms()?;
                let period = OwnerGrantPeriod::at(now)?;
                let result = prior_periods_ended(
                    &tx,
                    &profile,
                    &device,
                    &contribution,
                    &admission,
                    None,
                    &period,
                    now,
                )?;
                profile.check()?;
                if fence.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                tx.commit()?;
                Ok(result)
            })
            .await?;
        caller.check()?;
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::LocalAuthority);
        }
        Ok(result)
    }
}
