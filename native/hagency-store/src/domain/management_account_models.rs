//! Account inventory is not authority to inspect another private namespace.
use super::*;
use crate::management::AccountModelQuery;
impl DomainRepository {
    pub(crate) fn management_inspect_account(
        &self,
        profile: &str,
        query: &AccountModelQuery,
    ) -> Result<crate::ManagedLaunch, Error> {
        let owned:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM management_account_profiles WHERE account_id=?1 AND profile=?2)",params![query.account_id,profile],|r|r.get(0))?;
        if !owned {
            return Err(Error::LocalAuthority);
        }
        let account = self
            .account_choices()?
            .into_iter()
            .find(|a| a.id == query.account_id)
            .ok_or(Error::NotFound)?;
        if account.revision != query.expected_revision {
            return Err(Error::Conflict);
        }
        let pending:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM account_login_attempts WHERE account_id=?1 AND state='attempting')",[&query.account_id],|r|r.get(0))?;
        if pending {
            return Err(Error::Conflict);
        }
        self.managed_account(&query.account_id)?.prepare_login()
    }
}
#[cfg(test)]
mod tests;
