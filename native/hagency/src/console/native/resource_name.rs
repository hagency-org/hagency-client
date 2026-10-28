//! Narrow local-name mutation; no configuration, provider, or central effect.
use super::{NativeError, NativeManagement, management::store_error};
use hagency_store::management::{
    Intent, IntentOperation, Permission, RenameResource, ResourceDetailQuery, ResourceNameResult,
};

pub(super) fn known_failure(error: &NativeError) -> bool {
    // Revision change and invalid input are emitted only before the local
    // metadata transaction commits. Command conflicts/auth/unknown stay pending.
    error.code == "management_resource_name_changed"
        || (error.status == 400 && error.code == "invalid_management_arguments")
}
impl NativeManagement {
    pub async fn rename_resource(
        &self,
        input: RenameResource,
    ) -> Result<ResourceNameResult, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::RenameResources)
            .map_err(store_error)?;
        // Revalidate current canonical resource visibility even when private
        // custody already contains an original success. Never replay authority.
        self.resource_detail(ResourceDetailQuery {
            id: input.resource_id.clone(),
        })
        .await?;
        let intent = Intent::new(
            IntentOperation::RenameResource,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, async {
            let (context, session) = self.authorized().await?;
            let result = session.rename_resource(&context, input).await;
            self.finish(&context, result, true).await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_name_terminal_failures_exclude_original_command_or_scope_uncertainty() {
        for (status, code, terminal) in [
            (409, "management_resource_name_changed", true),
            (400, "invalid_management_arguments", true),
            (409, "management_revision_conflict", false),
            (409, "management_command_conflict", false),
            (403, "management_scope_changed", false),
            (503, "management_outcome_unknown", false),
            (503, "invalid_management_arguments", false),
        ] {
            let error = NativeError {
                status,
                code: code.into(),
            };
            assert_eq!(known_failure(&error), terminal, "{status}/{code}");
        }
    }
}
