//! Scoped tool-status reads and original-command saved-rule revocation.
use super::{NativeError, NativeManagement, management::store_error};
use hagency_store::management::{
    Intent, IntentOperation, Permission, ToolApprovalQuery, ToolApprovalLookup, ToolApprovalSnapshot,
    ToolApprovalPage, ToolGrantQuery, ToolGrantPage, RevokeToolGrant, RevokedToolGrant,
};
impl NativeManagement {
    pub async fn tool_approvals(
        &self,
        query: ToolApprovalQuery,
    ) -> Result<ToolApprovalPage, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.tool_approvals(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn tool_approval(
        &self,
        query: ToolApprovalLookup,
    ) -> Result<ToolApprovalSnapshot, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.tool_approval(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn tool_grants(&self, query: ToolGrantQuery) -> Result<ToolGrantPage, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.tool_grants(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn revoke_tool_grant(
        &self,
        input: RevokeToolGrant,
    ) -> Result<RevokedToolGrant, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::RevokeToolGrants)
            .map_err(store_error)?;
        let intent = Intent::new(
            IntentOperation::RevokeToolGrant,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, async {
            let (context, session) = self.authorized().await?;
            let result = session.revoke_tool_grant(&context, input).await;
            self.finish(&context, result, true).await
        })
        .await
    }
}
