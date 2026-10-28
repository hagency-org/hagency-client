//! A native-confirmed retirement never signs out of the provider or deletes credentials.
use super::{NativeManagement, NativeError, failure, management::store_error};
use hagency_store::management::{RetireAccount, AccountRetirement, Intent, IntentOperation, Permission};
use std::time::Instant;
impl NativeManagement {
    pub async fn retire_account(&self, input: RetireAccount, confirmed_until: Instant) -> Result<AccountRetirement, NativeError> {
        input.validate().map_err(store_error)?;
        if Instant::now() >= confirmed_until { return Err(failure(403,"provider_confirmation_expired")); }
        let (context, session) = self.authorized().await?;
        session.authorize(&context, Permission::RetireAccounts).map_err(store_error)?;
        let intent = Intent::new(IntentOperation::RetireAccount, input.command_id.clone(), &input).map_err(store_error)?;
        self.with_intent(intent, async {
            let _guard = self.owner.0.calls.lock().await;
            if Instant::now() >= confirmed_until { return Err(failure(403,"provider_confirmation_expired")); }
            let (context, session) = self.authorized().await?;
            let result = session.retire_account(&context, input, confirmed_until).await;
            self.finish(&context, result, true).await.map_err(|e| match e.code.as_str() {
                "management_resource_in_use" => failure(409,"account_retirement_dependencies"),
                "management_revision_conflict" => failure(409,"account_retirement_revision_changed"),
                _ => e,
            })
        }).await
    }
}
