//! One original managed-namespace login, reserved before provider effects.
use super::*;
use crate::management::{AccountLoginRequest, AccountLoginStatus, CommandReceipt};

#[cfg(test)]
mod tests;
impl DomainRepository {
    pub(crate) fn management_settle_account_login(
        &mut self,
        attempt: crate::LoginAttempt,
        verdict: crate::LoginVerdict,
    ) -> Result<(), Error> {
        self.settle_account_login(attempt, verdict, graphs::now_ms()?)
            .map(|_| ())
    }

    fn login_account_owner(&self, profile: &str, id: &str) -> Result<(), Error> {
        let owned: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM management_account_profiles WHERE account_id=?1 AND profile=?2)", params![id,profile], |r| r.get(0))?;
        if !owned {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    pub(crate) fn management_begin_account_login(
        &mut self,
        profile: &str,
        input: AccountLoginRequest,
        receipt: CommandReceipt,
        check: impl Fn() -> Result<(), Error>,
    ) -> Result<
        (
            AccountLoginStatus,
            Option<(crate::LoginAttempt, crate::ManagedLaunch)>,
        ),
        Error,
    > {
        check()?;
        self.login_account_owner(profile, &input.account_id)?;
        if let Some(attempt) = receipt.lookup::<crate::LoginAttempt>(&self.db)? {
            return Ok((self.management_login_observation(&input, &attempt)?, None));
        }
        let bound: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM resource_accounts WHERE account_id=?1) OR EXISTS(SELECT 1 FROM owner_resource_namespaces WHERE account_id=?1)",
            [&input.account_id],
            |r| r.get(0),
        )?;
        if bound {
            return Err(Error::Conflict);
        }
        let choice = self
            .account_choices()?
            .into_iter()
            .find(|a| a.id == input.account_id)
            .ok_or(Error::NotFound)?;
        if choice.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        let launch = self.managed_account(&input.account_id)?.prepare_login()?;
        let attempt = self.begin_account_login_checked(
            &input.account_id,
            graphs::now_ms()?,
            |tx, attempt| {
                check()?;
                receipt.record(tx, attempt)?;
                check()
            },
        )?;
        // Returning the original non-serializable handle is the only spawn path.
        // Replay after the SQLite/effect gap only observes the saved attempt.
        let status = self.management_login_observation(&input, &attempt)?;
        check().map_err(|_| Error::OutcomeUnknown)?;
        Ok((status, Some((attempt, launch))))
    }
    pub(crate) fn management_account_login_status(
        &self,
        profile: &str,
        input: AccountLoginRequest,
        receipt: CommandReceipt,
    ) -> Result<AccountLoginStatus, Error> {
        self.login_account_owner(profile, &input.account_id)?;
        let attempt = receipt
            .lookup::<crate::LoginAttempt>(&self.db)?
            .ok_or(Error::NotFound)?;
        self.management_login_observation(&input, &attempt)
    }
    fn management_login_observation(
        &self,
        input: &AccountLoginRequest,
        attempt: &crate::LoginAttempt,
    ) -> Result<AccountLoginStatus, Error> {
        if attempt.account_id() != input.account_id {
            return Err(Error::Conflict);
        }
        let observation: Option<(String,String,String,u64)> = self.db.query_row(
            "SELECT mode,outcome,provider_state,expires_at_ms FROM account_login_observations WHERE account_id=?1 AND attempt=?2",
            params![attempt.account_id(),attempt.number()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let now = graphs::now_ms()?;
        let current: Option<(String,u64)> = self.db.query_row("SELECT a.state,l.attempt FROM managed_accounts a JOIN account_login_attempts l ON l.account_id=a.id WHERE a.id=?1",[attempt.account_id()],|r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let state = if current
            .as_ref()
            .is_some_and(|(state, number)| state != "active" || *number != attempt.number())
        {
            "superseded"
        } else {
            match observation {
                Some((mode, outcome, _, until))
                    if mode == "subscription" && outcome == "observed" && until > now =>
                {
                    "authenticated"
                }
                Some((_, _, word, _)) if word == "cancelled" => "cancelled",
                Some((_, _, word, _)) if word == "expired" => "expired",
                Some((_, outcome, _, _)) if outcome == "refused" => "failed",
                _ => "uncertain",
            }
        };
        Ok(AccountLoginStatus {
            command_id: input.command_id.clone(),
            account_id: input.account_id.clone(),
            state: state.into(),
        })
    }
}
