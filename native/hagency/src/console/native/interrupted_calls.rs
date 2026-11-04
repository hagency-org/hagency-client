//! Owner reconciliation of managed calls whose original process is gone.
//! The owner declares the final consumption inside the observed bounds; the
//! store refuses live calls and never invents usage on its own.
use super::{NativeError, NativeManagement, management::store_error};
use hagency_store::management::{Intent, IntentOperation, Permission};
use hagency_store::owner_resources::{
    InterruptedCallPage, InterruptedCallQuery, InterruptedCallReconciliation,
    ReconcileInterruptedCall,
};

impl NativeManagement {
    pub async fn interrupted_calls(
        &self,
        query: InterruptedCallQuery,
    ) -> Result<InterruptedCallPage, NativeError> {
        query.validate().map_err(store_error)?;
        let (context, _) = self.authorized().await?;
        let (profile, registry) = self
            .owner_profile_registry(Permission::ReconcileAgentCalls)
            .await?;
        let result = registry.interrupted_calls(profile, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn reconcile_interrupted_call(
        &self,
        input: ReconcileInterruptedCall,
    ) -> Result<InterruptedCallReconciliation, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::ReconcileAgentCalls)
            .map_err(store_error)?;
        let intent = Intent::new(
            IntentOperation::ReconcileInterruptedCall,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, async {
            let (context, _) = self.authorized().await?;
            let (profile, registry) = self
                .owner_profile_registry(Permission::ReconcileAgentCalls)
                .await?;
            let result = registry.reconcile_interrupted_call(profile, input).await;
            self.finish(&context, result, true).await
        })
        .await
    }
}
