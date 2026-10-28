//! Native mini-app access: the original command survives a lost response.
use super::{NativeError, NativeManagement, management::store_error};
use hagency_store::management::{
    AgentExecutionPolicy, AgentExecutionPolicyPage, EditExecutionPolicy, ExecutionPolicyListQuery,
    ExecutionPolicyQuery, Intent, IntentOperation, Permission,
};

impl NativeManagement {
    pub async fn agent_execution_policies(
        &self,
        query: ExecutionPolicyListQuery,
    ) -> Result<AgentExecutionPolicyPage, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.agent_execution_policies(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn agent_execution_policy(
        &self,
        query: ExecutionPolicyQuery,
    ) -> Result<AgentExecutionPolicy, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.agent_execution_policy(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn edit_execution_policy(
        &self,
        input: EditExecutionPolicy,
    ) -> Result<AgentExecutionPolicy, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::EditExecutionPolicy)
            .map_err(store_error)?;
        let intent = Intent::new(
            IntentOperation::EditExecutionPolicy,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, async {
            let (context, session) = self.authorized().await?;
            let result = session.edit_execution_policy(&context, input).await;
            self.finish(&context, result, true).await
        })
        .await
    }
}
