//! Original identity custody before any native namespace filesystem effect.
use super::{NativeError, NativeManagement, management::store_error};
use hagency_store::management::{PrepareAccount, AccountPreparation, Intent, IntentOperation, Permission};
impl NativeManagement {
    pub async fn prepare_account(&self, input: PrepareAccount) -> Result<AccountPreparation, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session.authorize(&context, Permission::PrepareAccounts).map_err(store_error)?;
        let intent = Intent::new(IntentOperation::PrepareAccount, input.command_id.clone(), &input).map_err(store_error)?;
        self.with_intent(intent, async {
            let (context, session) = self.authorized().await?;
            let result = session.prepare_account(&context, input).await;
            self.finish(&context, result, true).await
        }).await
    }
}
