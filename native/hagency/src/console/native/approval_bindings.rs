//! Binding removal retains the original command and current resource authority.
use super::{NativeError, NativeManagement, management::store_error};
use hagency_store::management::{
    Intent, IntentOperation, Permission, ResourceApprovalBindingPage, ResourceApprovalBindingQuery,
    RetireResourceApprovalBinding, RetiredResourceApprovalBinding,
};
impl NativeManagement {
    pub async fn resource_approval_bindings(
        &self,
        query: ResourceApprovalBindingQuery,
    ) -> Result<ResourceApprovalBindingPage, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.resource_approval_bindings(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn retire_resource_approval_binding(
        &self,
        input: RetireResourceApprovalBinding,
    ) -> Result<RetiredResourceApprovalBinding, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::RetireApprovalBindings)
            .map_err(store_error)?;
        let intent = Intent::new(
            IntentOperation::RetireApprovalBinding,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, async {
            let (context, session) = self.authorized().await?;
            let result = session
                .retire_resource_approval_binding(&context, input)
                .await;
            self.finish(&context, result, true).await
        })
        .await
    }
}
