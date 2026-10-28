//! Read-only local account inventory. Recorded bindings are not live login proof.
use super::*;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountInventoryQuery {
    pub id: Option<String>,
    pub after: Option<String>,
}
impl AccountInventoryQuery {
    pub fn validate(&self) -> Result<(), Error> {
        for id in [&self.id, &self.after].into_iter().flatten() {
            identifier(id, 128)?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub id: String,
    pub kind: &'static str,
    pub runtime: String,
    pub state: String,
    pub ordinal: Option<u8>,
    pub revision: Option<String>,
    pub profile: Option<String>,
    pub readiness: &'static str,
    /// Profile-scoped display grouping from recorded provider identity only.
    /// This is neither a current login observation nor an authorization key.
    pub identity_key: Option<String>,
    /// Only resources visible to this Matrix profile. Not a retirement check.
    pub resource_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInventory {
    pub protocol: &'static str,
    pub observed_at_ms: u64,
    pub accounts: Vec<AccountSummary>,
    pub account: Option<AccountSummary>,
    pub resources: Vec<ResourceSummary>,
    pub next_after: Option<String>,
}

impl Session {
    pub async fn account_inventory(
        &self,
        context: &HostContext,
        query: AccountInventoryQuery,
    ) -> Result<AccountInventory, Error> {
        self.check(context, Permission::ReadAccounts)?;
        query.validate()?;
        let result = match &self.store {
            SessionStore::Legacy(store) => {
                let authority = self.authority();
                let original = context.clone();
                store
                    .call(4096, move |db| {
                        authority.authorize(&original, Permission::ReadAccounts)?;
                        let result = db.management_account_inventory(None, query)?;
                        authority.authorize(&original, Permission::ReadAccounts)?;
                        Ok(result)
                    })
                    .await?
            }
            #[cfg(feature = "resource-adoption")]
            SessionStore::Adopted(store) => {
                store
                    .account_inventory(&self.adopted_profile(store)?, query)
                    .await?
            }
        };
        self.check(context, Permission::ReadAccounts)?;
        Ok(result)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareAccount {
    pub command_id: String,
}
impl PrepareAccount {
    pub fn validate(&self) -> Result<(), Error> { identifier(&self.command_id, 128)?; Ok(()) }
}
/// The preparation observation never claims provider authentication or capacity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountPreparation {
    pub account_id: String,
    pub state: String,
}
impl Session {
    pub async fn prepare_account(&self, context: &HostContext, input: PrepareAccount) -> Result<AccountPreparation, Error> {
        let _pending = self.scope.pending.try_acquire().map_err(|_| Error::Busy)?;
        let _command = self.scope.commands.lock().await;
        self.check(context, Permission::PrepareAccounts)?;
        input.validate()?;
        let authority = self.authority();
        let original = context.clone();
        let result = match &self.store {
            SessionStore::Legacy(store) => {
                let receipt = CommandReceipt::new(context, "account.prepare", &input.command_id, &input)?;
                store.call(4096, move |db| db.management_prepare_account("", receipt, || authority.authorize(&original, Permission::PrepareAccounts))).await?
            }
            #[cfg(feature = "resource-adoption")]
            SessionStore::Adopted(store) => {
                let receipt = CommandReceipt::new(context, "account.prepare", &input.command_id, &serde_json::json!({"profile":store.identity().0,"input":input}))?;
                store.prepare_account(&self.adopted_profile(store)?, receipt, authority, original).await?
            }
        };
        self.check(context, Permission::PrepareAccounts).map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
