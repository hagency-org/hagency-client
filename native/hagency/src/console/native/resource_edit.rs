//! Native-confirmed local edits, with fresh original-namespace metadata and
//! immutable command recovery. No inference or legacy configuration dispatch.
use super::{NativeManagement, NativeError, failure, management::store_error};
use hagency_store::{
    ResourceConfigurationResult,
    management::{EditLocalResource, Intent, IntentOperation, Permission},
};
use std::time::Instant;

/// Native-only review. A recorded original result is not the current resource
/// definition and must be presented as recovery, never another configuration.
#[derive(Debug)]
pub enum LocalResourceEditReview {
    Editable(Box<hagency_store::management::ResourceDetail>),
    Recorded(ResourceConfigurationResult),
}

pub(super) fn known_failure(error: &NativeError) -> bool {
    (error.status == 400 && error.code == "invalid_management_arguments")
        || (error.status == 409
            && matches!(
                error.code.as_str(),
                "management_resource_edit_changed"
                    | "management_resource_edit_not_available"
                    | "management_model_unqualified"
                    | "provider_model_changed"
                    | "resource_namespace_required"
                    | "provider_connection_required"
            ))
}

impl NativeManagement {
    /// Pure readback before showing native consent. Creating or accepting an
    /// intent is deliberately excluded; cancelling a review has no write.
    pub async fn review_local_resource_edit(
        &self,
        input: &EditLocalResource,
    ) -> Result<LocalResourceEditReview, NativeError> {
        input.validate().map_err(store_error)?;
        #[cfg(not(feature = "resource-adoption"))]
        {
            Err(failure(409, "resource_adoption_required"))
        }
        #[cfg(feature = "resource-adoption")]
        {
            let (context, session) = self.authorized().await?;
            session
                .local_resource_editor_registry(&context)
                .map_err(store_error)?;
            if let Some(saved) = session
                .recover_local_resource_edit(&context, input)
                .await
                .map_err(store_error)?
            {
                return self
                    .finish(
                        &context,
                        Ok(LocalResourceEditReview::Recorded(saved)),
                        false,
                    )
                    .await;
            }
            let detail = session
                .resource_detail(
                    &context,
                    hagency_store::management::ResourceDetailQuery {
                        id: input.resource_id.clone(),
                    },
                )
                .await
                .map_err(store_error)?;
            if detail.resource.revision != input.expected_revision {
                return Err(failure(409, "management_resource_edit_changed"));
            }
            if let Some(policy) = input.execution_policy {
                policy.validate_for(&detail.resource.framework)
                    .map_err(|_| failure(400, "invalid_management_arguments"))?;
            }
            if !detail.editing.allowed {
                return Err(failure(409, "management_resource_edit_not_available"));
            }
            if let hagency_store::ProfileChange::Select { model, reasoning } = &input.profile
                && !detail
                    .choices
                    .iter()
                    .any(|choice| &choice.model == model && &choice.reasoning == reasoning)
            {
                return Err(failure(409, "management_model_unqualified"));
            }
            self.finish(
                &context,
                Ok(LocalResourceEditReview::Editable(Box::new(detail))),
                false,
            )
            .await
        }
    }

    /// Only a trusted native review or explicit original-command recovery calls
    /// this method. The mini-app has no generic configuration writer.
    pub async fn edit_local_resource(
        &self,
        input: EditLocalResource,
        confirmed_until: Instant,
    ) -> Result<ResourceConfigurationResult, NativeError> {
        input.validate().map_err(store_error)?;
        if Instant::now() >= confirmed_until {
            return Err(failure(403, "provider_confirmation_expired"));
        }
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::ConfigureResources)
            .map_err(store_error)?;
        session
            .resource(&context, input.resource_id.clone())
            .await
            .map_err(store_error)?;
        let intent = Intent::new(
            IntentOperation::EditLocalResource,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, self.edit_local_resource_now(input, confirmed_until))
            .await
    }
    async fn edit_local_resource_now(
        &self,
        input: EditLocalResource,
        confirmed_until: Instant,
    ) -> Result<ResourceConfigurationResult, NativeError> {
        #[cfg(not(feature = "resource-adoption"))]
        {
            let _ = (input, confirmed_until);
            Err(failure(409, "resource_adoption_required"))
        }
        #[cfg(feature = "resource-adoption")]
        {
            let _guard = self.owner.0.calls.lock().await;
            let (context, session) = self.authorized().await?;
            if let Some(result) = session
                .recover_local_resource_edit(&context, &input)
                .await
                .map_err(store_error)?
            {
                return self.finish(&context, Ok(result), false).await;
            }
            let (registry, profile) = session
                .local_resource_editor_registry(&context)
                .map_err(store_error)?;
            let current = session
                .resource(&context, input.resource_id.clone())
                .await
                .map_err(store_error)?;
            if current.revision != input.expected_revision {
                return Err(failure(409, "management_resource_edit_changed"));
            }
            let namespace = registry
                .resource_namespace(
                    profile.clone(),
                    input.resource_id.clone(),
                    input.expected_revision.clone(),
                )
                .await
                .map_err(store_error)?
                .ok_or_else(|| failure(409, "resource_namespace_required"))?;
            namespace.check().await.map_err(store_error)?;
            let deadline = session
                .valid_until()
                .min(confirmed_until)
                .min(Instant::now() + std::time::Duration::from_secs(30));
            if Instant::now() >= deadline {
                return Err(failure(403, "provider_confirmation_expired"));
            }
            let observed = self
                .observe_account_namespace(namespace.launch(), deadline)
                .await;
            let (fresh, live) = self.authorized().await?;
            if fresh != context {
                return Err(failure(401, "management_scope_changed"));
            }
            live.authorize(&context, Permission::ConfigureResources)
                .map_err(store_error)?;
            namespace.check().await.map_err(store_error)?;
            let (provider, catalog) = observed?;
            let (model, reasoning) = match &input.profile {
                hagency_store::ProfileChange::Preserve {} => (&current.model, &current.reasoning),
                hagency_store::ProfileChange::Select { model, reasoning } => (model, reasoning),
            };
            validate_model(
                &current.framework,
                model,
                reasoning,
                &serde_json::json!(catalog),
            )?;
            let verified = registry
                .verify_provider_resource(profile, input.resource_id.clone(), provider)
                .await
                .map_err(store_error)?;
            let binding =
                hagency_store::owner_resources::ResourceEditBinding::new(verified, Some(namespace))
                    .map_err(store_error)?;
            if Instant::now() >= deadline {
                return Err(failure(403, "provider_confirmation_expired"));
            }
            let result = live.edit_local_resource(&context, input, binding).await;
            self.finish(&context, result, true).await
        }
    }
}

#[cfg(any(feature = "resource-adoption", test))]
fn validate_model(
    runtime: &str,
    model: &str,
    reasoning: &Option<String>,
    catalog: &serde_json::Value,
) -> Result<(), NativeError> {
    let projected = super::resource_onboarding::projection(runtime, catalog);
    let row = projected["models"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["model"].as_str() == Some(model)))
        .ok_or_else(|| failure(409, "provider_model_changed"))?;
    if !row["sharedReasoning"]
        .as_array()
        .is_some_and(|values| values.contains(&serde_json::json!(reasoning)))
    {
        return Err(failure(409, "management_model_unqualified"));
    }
    Ok(())
}

#[cfg(all(test, unix, feature="resource-adoption", feature="test-support"))]
#[path="resource_model_acceptance_tests.rs"]
mod process_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_editor_model_validation_uses_fresh_names_and_reasoning() {
        let catalog = serde_json::json!({"models":[{"id":"coding-alias","model":"gpt-5.6-sol","supportedReasoningEfforts":[{"reasoningEffort":"medium"},{"reasoningEffort":"unsupported"}]}]});
        validate_model("codex", "gpt-5.6-sol", &Some("medium".into()), &catalog).unwrap();
        assert_eq!(
            validate_model("codex", "coding-alias", &Some("medium".into()), &catalog)
                .unwrap_err()
                .code,
            "provider_model_changed"
        );
        assert_eq!(
            validate_model(
                "codex",
                "gpt-5.6-sol",
                &Some("unsupported".into()),
                &catalog
            )
            .unwrap_err()
            .code,
            "management_model_unqualified"
        );
        assert!(validate_model("codex", "gpt-5.6-sol", &None, &catalog).is_err());
        assert!(
            validate_model(
                "codex",
                "gpt-5.6-sol",
                &Some("medium".into()),
                &serde_json::json!({"models":[]})
            )
            .is_err()
        );
    }
    #[test]
    fn local_editor_known_failures_never_settle_uncertain_or_scope_errors() {
        for (status, code, terminal) in [
            (409, "management_resource_edit_changed", true),
            (409, "management_resource_edit_not_available", true),
            (409, "provider_model_changed", true),
            (400, "invalid_management_arguments", true),
            (409, "management_revision_conflict", false),
            (409, "management_outcome_unknown", false),
            (401, "management_scope_changed", false),
            (503, "provider_model_changed", false),
        ] {
            assert_eq!(
                known_failure(&failure(status, code)),
                terminal,
                "{status}/{code}"
            );
        }
    }
}
