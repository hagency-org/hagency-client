use super::*;
use crate::management::{AccountInventory, AccountInventoryQuery, AccountSummary, MAX_PAGE, PROTOCOL};
/// The scope and database are provided by the original worker, never by input.
impl DomainRepository {
    pub(crate) fn management_account_inventory(
        &self,
        profile: Option<&str>,
        query: AccountInventoryQuery,
    ) -> Result<AccountInventory, Error> {
        let db = self;
        let at = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| Error::Unavailable)?
                .as_millis(),
        )
        .map_err(|_| Error::Unavailable)?;
        let transaction = db.db.unchecked_transaction()?;
        let mut rows = Vec::new();
        let identity_scope = match profile {
            Some(value) => value.to_owned(),
            None => db.db.query_row("SELECT profile_digest FROM management_identity WHERE singleton=1", [], |r| r.get::<_, String>(0))?,
        };
        // Only hide bindings represented by a namespace this profile owns.
        // A resource grant does not grant access to somebody else's login.
        let unrepresented = "NOT EXISTS(SELECT 1 FROM owner_resource_namespaces n JOIN management_account_profiles p ON p.account_id=n.account_id WHERE n.resource_id=r.id AND p.profile=g.profile) AND NOT EXISTS(SELECT 1 FROM resource_accounts a JOIN management_account_profiles p ON p.account_id=a.account_id WHERE a.preset_id=r.preset_id AND p.profile=g.profile)";
        // Private backing keys stay in this function; neither seat nor provider
        // identity is returned to Splash. Canonical IDs differ across profiles.
        if let Some(profile) = profile {
            let seats = db.db.prepare("SELECT a.seat_id FROM owner_resource_accounts a WHERE EXISTS(SELECT 1 FROM resources r JOIN owner_resource_grants g ON g.resource_id=r.id WHERE json_extract(r.config,'$.seatId')=a.seat_id AND g.profile=?1 AND g.active=1) ORDER BY a.seat_id LIMIT 101")?
            .query_map([profile], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            if seats.len() > 100 {
                return Err(Error::Capacity);
            }
            for seat in seats {
                let id = format!(
                    "binding_{}",
                    hagency_core::canonical::digest(
                        &serde_json::json!({"version":1,"profile":profile,"seat":seat})
                    )?
                );
                let (count, runtime): (u64, String) = db.db.query_row(&format!("SELECT COUNT(*),CASE WHEN COUNT(DISTINCT json_extract(r.config,'$.framework'))=1 THEN MIN(json_extract(r.config,'$.framework')) ELSE 'mixed' END FROM resources r JOIN owner_resource_grants g ON g.resource_id=r.id WHERE json_extract(r.config,'$.seatId')=?1 AND g.profile=?2 AND g.active=1 AND {unrepresented}"), params![seat,profile], |r| Ok((r.get(0)?, r.get(1)?)))?;
                if count == 0 && query.id.as_deref() != Some(id.as_str()) { continue; }
                let identity_key = db.management_recorded_account_identity(&identity_scope, Some(profile), "resource_binding", &seat)?;
                rows.push((
                    AccountSummary {
                        id,
                        kind: "resource_binding",
                        runtime,
                        state: "linked".into(),
                        ordinal: None,
                        revision: None,
                        profile: None,
                        readiness: "unknown",
                        identity_key,
                        resource_count: count,
                    },
                    seat,
                ));
            }
        } else {
            // An older private owner database predates managed namespaces.
            // Retain its real seat grouping without turning a stored binding
            // into provider-login proof or exposing the underlying seat key.
            // Managed namespace members already have a row below, so exclude
            // them here instead of counting their declared capacity twice.
            let owner: String = db.db.query_row("SELECT profile_digest FROM management_identity WHERE singleton=1", [], |r| r.get(0))?;
            let seats = db.db.prepare("SELECT json_extract(r.config,'$.seatId'),COUNT(*),CASE WHEN COUNT(DISTINCT json_extract(r.config,'$.framework'))=1 THEN MIN(json_extract(r.config,'$.framework')) ELSE 'mixed' END FROM resources r WHERE NOT EXISTS(SELECT 1 FROM resource_accounts a WHERE a.preset_id=r.preset_id) AND NOT EXISTS(SELECT 1 FROM owner_resource_namespaces n WHERE n.resource_id=r.id) GROUP BY json_extract(r.config,'$.seatId') ORDER BY 1 LIMIT 101")?
                .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,u64>(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
            if seats.len() > 100 { return Err(Error::Capacity); }
            for (seat, count, runtime) in seats {
                let id = format!("binding_{}",hagency_core::canonical::digest(&serde_json::json!({"version":1,"profile":owner,"seat":seat}))?);
                let identity_key = db.management_recorded_account_identity(&identity_scope, profile, "resource_binding", &seat)?;
                rows.push((AccountSummary { id,kind:"resource_binding",runtime,state:"linked".into(),ordinal:None,revision:None,profile:None,readiness:"unknown",identity_key,resource_count:count },seat));
            }
        }
        {
            for choice in db.account_choices()? {
                if let Some(profile) = profile {
                    let owned: bool = db.db.query_row("SELECT EXISTS(SELECT 1 FROM management_account_profiles WHERE account_id=?1 AND profile=?2)", params![choice.id,profile], |r|r.get(0))?;
                    if !owned { continue; }
                }
                let ready = db.account_readiness(&choice.id, at)?;
                let readiness = match (&choice.state, ready.mode) {
                    (crate::AccountState::Active, crate::AccountReadinessMode::Subscription) => {
                        "subscription"
                    }
                    (crate::AccountState::Active, crate::AccountReadinessMode::ApiKey) => "api_key",
                    _ => "unknown",
                };
                let count = db.db.query_row("SELECT COUNT(*) FROM resources r WHERE (EXISTS(SELECT 1 FROM resource_accounts a WHERE a.preset_id=r.preset_id AND a.account_id=?1) OR EXISTS(SELECT 1 FROM owner_resource_namespaces n WHERE n.resource_id=r.id AND n.account_id=?1)) AND (?2 IS NULL OR EXISTS(SELECT 1 FROM owner_resource_grants g WHERE g.resource_id=r.id AND g.profile=?2 AND g.active=1))", params![choice.id,profile], |r| r.get(0))?;
                let state = match choice.state {
                    crate::AccountState::Preparing if db.account_preparation_live(&choice.id) => "preparing",
                    crate::AccountState::Preparing => "uncertain",
                    crate::AccountState::Active => "active",
                    crate::AccountState::Uncertain => "uncertain",
                    crate::AccountState::Retired => "retired",
                };
                let identity_key = db.management_recorded_account_identity(&identity_scope, profile, "managed_namespace", &choice.id)?;
                let key = choice.id.clone();
                rows.push((
                    AccountSummary {
                        id: choice.id,
                        kind: "managed_namespace",
                        runtime: "codex".into(),
                        state: state.into(),
                        ordinal: Some(choice.ordinal),
                        revision: Some(choice.revision),
                        profile: Some(choice.profile),
                        readiness,
                        identity_key,
                        resource_count: count,
                    },
                    key,
                ));
            }
        }
        rows.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        let mut page = AccountInventory {
            protocol: PROTOCOL,
            observed_at_ms: at,
            accounts: vec![],
            account: None,
            resources: vec![],
            next_after: None,
        };
        if let Some(id) = query.id {
            let (account, key) = rows
                .into_iter()
                .find(|(a, _)| a.id == id)
                .ok_or(Error::NotFound)?;
            let after = query.after.unwrap_or_default();
            let ids = if account.kind == "resource_binding" {
                if let Some(profile) = profile {
                    db.db.prepare(&format!("SELECT r.id FROM resources r JOIN owner_resource_grants g ON g.resource_id=r.id WHERE json_extract(r.config,'$.seatId')=?1 AND g.profile=?2 AND g.active=1 AND {unrepresented} AND r.id>?3 ORDER BY r.id LIMIT 17"))?
                    .query_map(params![key,profile,after], |r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?
                } else {
                    db.db.prepare("SELECT r.id FROM resources r WHERE json_extract(r.config,'$.seatId')=?1 AND r.id>?2 AND NOT EXISTS(SELECT 1 FROM resource_accounts a WHERE a.preset_id=r.preset_id) AND NOT EXISTS(SELECT 1 FROM owner_resource_namespaces n WHERE n.resource_id=r.id) ORDER BY r.id LIMIT 17")?
                    .query_map(params![key,after], |r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?
                }
            } else {
                db.db.prepare("SELECT r.id FROM resources r WHERE (EXISTS(SELECT 1 FROM resource_accounts a WHERE a.preset_id=r.preset_id AND a.account_id=?1) OR EXISTS(SELECT 1 FROM owner_resource_namespaces n WHERE n.resource_id=r.id AND n.account_id=?1)) AND r.id>?2 AND (?3 IS NULL OR EXISTS(SELECT 1 FROM owner_resource_grants g WHERE g.resource_id=r.id AND g.profile=?3 AND g.active=1)) ORDER BY r.id LIMIT 17")?
                .query_map(params![key,after,profile], |r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?
            };
            if ids.len() > MAX_PAGE {
                page.next_after = ids.get(MAX_PAGE - 1).cloned();
            }
            for id in ids.into_iter().take(MAX_PAGE) {
                page.resources.push(db.management_resource(&id, at)?);
            }
            page.account = Some(account);
        } else {
            let after = query.after.unwrap_or_default();
            let mut iter = rows.into_iter().filter(|(a, _)| a.id > after);
            page.accounts = iter.by_ref().take(MAX_PAGE).map(|(a, _)| a).collect();
            if iter.next().is_some() {
                page.next_after = page.accounts.last().map(|a| a.id.clone());
            }
        }
        if serde_json::to_vec(&page)?.len() > 64 * 1024 {
            return Err(Error::Capacity);
        }
        transaction.commit()?;
        Ok(page)
    }
}


impl DomainRepository {
    /// Display grouping only: never refreshes authentication or grants authority.
    /// All visible associated resources must agree on a verified provider identity.
    fn management_recorded_account_identity(
        &self, scope: &str, profile: Option<&str>, kind: &str, key: &str,
    ) -> Result<Option<String>, Error> {
        let identities = self.db.prepare("SELECT DISTINCT a.provider_digest FROM resources r LEFT JOIN owner_resource_accounts a ON a.seat_id=json_extract(r.config,'$.seatId') AND a.identity_kind='verified_provider' WHERE ((?1='resource_binding' AND json_extract(r.config,'$.seatId')=?2) OR (?1='managed_namespace' AND (EXISTS(SELECT 1 FROM resource_accounts n WHERE n.preset_id=r.preset_id AND n.account_id=?2) OR EXISTS(SELECT 1 FROM owner_resource_namespaces n WHERE n.resource_id=r.id AND n.account_id=?2)))) AND (?3 IS NULL OR EXISTS(SELECT 1 FROM owner_resource_grants g WHERE g.resource_id=r.id AND g.profile=?3 AND g.active=1)) LIMIT 2")?
            .query_map(params![kind, key, profile], |r| r.get::<_, Option<String>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if let [Some(identity)] = identities.as_slice() {
            Ok(Some(format!("identity_{}", hagency_core::canonical::digest(
                &serde_json::json!({"version":1,"profile":scope,"provider":identity})
            )?)))
        } else { Ok(None) }
    }
}

#[cfg(test)]
mod tests;

/// The receipt stores the original identity, never a reusable materialization grant.
#[derive(serde::Serialize, serde::Deserialize)]
struct ReservedAccount { account_id: String }
impl DomainRepository {
    pub(crate) fn management_prepare_account(
        &mut self,
        profile: &str,
        receipt: crate::management::CommandReceipt,
        check: impl Fn() -> Result<(), Error>,
    ) -> Result<crate::management::AccountPreparation, Error> {
        check()?;
        let id = if let Some(original) = receipt.lookup::<ReservedAccount>(&self.db)? {
            let owned: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM management_account_profiles WHERE account_id=?1 AND profile=?2)",params![original.account_id,profile],|r|r.get(0))?;
            if !owned { return Err(Error::LocalAuthority); }
            original.account_id
        } else {
            let reserved = self.reserve_account_checked(crate::ACCOUNT_PROFILE, |tx, choice| {
                check()?;
                tx.execute("INSERT INTO management_account_profiles VALUES(?1,?2)", params![choice.id,profile])?;
                receipt.record(tx, &ReservedAccount { account_id: choice.id.clone() })?;
                check()
            })?;
            // Exactly one attempt, in the original worker. Replay only observes.
            // Any IO/authority/deadline failure keeps the original identity visible.
            let _outcome = self.materialize_account_checked(&reserved.id, &check);
            reserved.id
        };
        check().map_err(|_| Error::OutcomeUnknown)?;
        let choice = self.account_choices()?.into_iter().find(|a| a.id == id).ok_or(Error::NotFound)?;
        let state = match choice.state {
            crate::AccountState::Active => "active",
            crate::AccountState::Retired => "retired",
            _ => "uncertain",
        };
        Ok(crate::management::AccountPreparation { account_id: id, state: state.into() })
    }
}

impl DomainRepository {
    pub(crate) fn management_retire_account(
        &mut self,
        profile: &str,
        input: &crate::management::RetireAccount,
        receipt: crate::management::CommandReceipt,
        check: impl Fn() -> Result<(), Error>,
    ) -> Result<crate::management::AccountRetirement, Error> {
        input.validate()?; check()?;
        let owned = |db: &rusqlite::Connection| -> Result<(), Error> {
            if !db.query_row("SELECT EXISTS(SELECT 1 FROM management_account_profiles WHERE account_id=?1 AND profile=?2)", params![input.account_id,profile], |r| r.get::<_,bool>(0))? { return Err(Error::LocalAuthority); }
            Ok(())
        };
        owned(&self.db)?;
        if let Some(original) = receipt.lookup::<crate::management::AccountRetirement>(&self.db)? {
            check()?;
            return Ok(original);
        }
        let result = |a: &crate::AccountChoice| crate::management::AccountRetirement { account_id:a.id.clone(), state:"retired".into(), revision:a.revision.clone(), provider_logout:"unobserved".into() };
        let retired = self.retire_account_checked(&input.account_id, crate::LogoutObservation::unobserved(), |tx, choice| {
            check()?; owned(tx)?;
            if choice.revision != input.expected_revision { return Err(Error::Conflict); }
            // Installation-wide, including resources invisible to this profile.
            let blocked: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM resource_accounts WHERE account_id=?1) OR EXISTS(SELECT 1 FROM owner_resource_namespaces WHERE account_id=?1) OR EXISTS(SELECT 1 FROM account_login_attempts WHERE account_id=?1 AND state='attempting')", [&input.account_id], |r|r.get(0))?;
            if blocked || !matches!(choice.state, crate::AccountState::Active) { return Err(Error::State); }
            Ok(())
        }, |tx, choice| {
            check()?; receipt.record(tx, &result(choice))?; check()
        })?;
        check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result(&retired))
    }
}
