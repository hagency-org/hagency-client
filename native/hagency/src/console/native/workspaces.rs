//! Same owner/package authority as resource configuration; no local web server.
use super::{NativeError, NativeManagement, management::store_error};
use hagency_store::management::{
    ClearWorkspaceDirty, Intent, IntentOperation, Permission, WorkspaceListQuery, WorkspacePage,
    WorkspaceQuery, WorkspaceRelease, WorkspaceStatus,
};

impl NativeManagement {
    pub async fn resource_workspaces(
        &self,
        query: WorkspaceListQuery,
    ) -> Result<WorkspacePage, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.resource_workspaces(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn resource_workspace(
        &self,
        query: WorkspaceQuery,
    ) -> Result<WorkspaceStatus, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.resource_workspace(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn clear_workspace_dirty(
        &self,
        input: ClearWorkspaceDirty,
    ) -> Result<WorkspaceRelease, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::ClearWorkspaceDirty)
            .map_err(store_error)?;
        let intent = Intent::new(
            IntentOperation::ClearWorkspaceDirty,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, async {
            let (context, session) = self.authorized().await?;
            let result = session.clear_workspace_dirty(&context, input).await;
            self.finish(&context, result, true).await
        })
        .await
    }
}
