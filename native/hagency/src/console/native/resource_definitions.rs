//! Resource agent templates with original-command custody. No agent is rewritten.
use super::{NativeError, NativeManagement, failure, management::store_error};
use hagency_store::management::{
    Intent, IntentOperation, IntentReply, Permission, PendingIntent, ResourceTemplateChange,
    ResourceTemplateCommand, ResourceTemplateResult, ResourceTemplatePage, ResourceTemplateDetail,
    TemplateCreateInput, TemplateUpdateInput, TemplateDeleteInput,
};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedResourceTemplate {
    pub command_id: String,
    pub resource_id: String,
    pub definition_id: String,
    pub operation: IntentOperation,
    pub change: ResourceTemplateChange,
    pub expected_revision: String,
    pub result: Option<ResourceTemplateResult>,
    pub error: Option<String>,
}
fn id(prefix: &str) -> Result<String, NativeError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| failure(503, "native_random_unavailable"))?;
    Ok(format!(
        "{prefix}{}",
        bytes.iter().map(|v| format!("{v:02x}")).collect::<String>()
    ))
}
pub(super) fn is_operation(operation: IntentOperation) -> bool {
    matches!(
        operation,
        IntentOperation::TemplateCreate
            | IntentOperation::TemplateUpdate
            | IntentOperation::TemplateDelete
    )
}
pub(super) fn known_failure(error: &NativeError) -> bool {
    error.status == 400
        || error.status == 404
        || matches!(
            error.code.as_str(),
            "management_revision_conflict"
                | "management_resource_in_use"
                | "management_model_unqualified"
        )
}
fn decode(saved: &PendingIntent) -> Result<ResourceTemplateCommand, NativeError> {
    if !is_operation(saved.operation) {
        return Err(failure(400, "invalid_management_arguments"));
    }
    let command: ResourceTemplateCommand = serde_json::from_value(saved.input.clone())
        .map_err(|_| failure(503, "management_saved_command_invalid"))?;
    command
        .validate()
        .map_err(|_| failure(503, "management_saved_command_invalid"))?;
    if command.command_id != saved.command_id
        || command.operation().map_err(store_error)? != saved.operation
    {
        return Err(failure(503, "management_saved_command_invalid"));
    }
    Ok(command)
}
fn validate_result(
    command: &ResourceTemplateCommand,
    result: &ResourceTemplateResult,
) -> Result<(), NativeError> {
    if command.command_id != result.command_id
        || command.resource_id != result.resource_id
        || command.definition_id != result.definition_id
        || result.deleted != (command.change.operation == "delete")
        || result.revision.len() != 64
        || !result
            .revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(failure(503, "management_saved_result_invalid"));
    }
    Ok(())
}
impl NativeManagement {
    pub async fn resource_templates(
        &self,
        resource_id: String,
        after: Option<String>,
    ) -> Result<ResourceTemplatePage, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session
            .resource_templates(&context, resource_id, after)
            .await;
        self.finish(&context, result, false).await
    }
    pub async fn resource_template(
        &self,
        resource_id: String,
        id: String,
    ) -> Result<ResourceTemplateDetail, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.resource_template(&context, resource_id, id).await;
        self.finish(&context, result, false).await
    }
    pub async fn prepare_template_create(
        &self,
        input: TemplateCreateInput,
    ) -> Result<PreparedResourceTemplate, NativeError> {
        input.validate().map_err(store_error)?;
        self.prepare_template(ResourceTemplateCommand {
            command_id: id("templatecmd_")?,
            resource_id: input.resource_id,
            definition_id: id("rad_")?,
            expected_revision: input.expected_revision,
            change: ResourceTemplateChange {
                operation: "create".into(),
                name: input.name,
                role: input.role,
                enabled: input.enabled,
            },
        })
        .await
    }
    pub async fn prepare_template_update(
        &self,
        input: TemplateUpdateInput,
    ) -> Result<PreparedResourceTemplate, NativeError> {
        input.validate().map_err(store_error)?;
        self.prepare_template(ResourceTemplateCommand {
            command_id: id("templatecmd_")?,
            resource_id: input.resource_id,
            definition_id: input.id,
            expected_revision: input.expected_revision,
            change: ResourceTemplateChange {
                operation: "update".into(),
                name: input.name,
                role: input.role,
                enabled: input.enabled,
            },
        })
        .await
    }
    pub async fn prepare_template_delete(
        &self,
        input: TemplateDeleteInput,
    ) -> Result<PreparedResourceTemplate, NativeError> {
        input.validate().map_err(store_error)?;
        self.prepare_template(ResourceTemplateCommand {
            command_id: id("templatecmd_")?,
            resource_id: input.resource_id,
            definition_id: input.id,
            expected_revision: input.expected_revision,
            change: ResourceTemplateChange {
                operation: "delete".into(),
                name: String::new(),
                role: String::new(),
                enabled: false,
            },
        })
        .await
    }
    async fn prepare_template(
        &self,
        mut command: ResourceTemplateCommand,
    ) -> Result<PreparedResourceTemplate, NativeError> {
        if command.change.operation != "delete" {
            command.validate().map_err(store_error)?;
        }
        let _mutation = self.owner.0.management.mutations.lock().await;
        let (context, session) = self.authorized().await?;
        session
            .resource_template_access(&context, Permission::ManageResourceTemplates)
            .map_err(store_error)?;
        if command.change.operation == "create" {
            let page = session
                .resource_templates(&context, command.resource_id.clone(), None)
                .await
                .map_err(store_error)?;
            if page.revision != command.expected_revision {
                return Err(failure(409, "management_revision_conflict"));
            }
            if !page.roles.contains(&command.change.role) {
                return Err(failure(409, "management_model_unqualified"));
            }
            if !page.can_create {
                return Err(failure(503, "management_capacity"));
            }
        } else {
            let detail = session
                .resource_template(
                    &context,
                    command.resource_id.clone(),
                    command.definition_id.clone(),
                )
                .await
                .map_err(store_error)?;
            if detail.revision != command.expected_revision {
                return Err(failure(409, "management_revision_conflict"));
            }
            if command.change.operation == "delete" {
                if !detail.can_delete {
                    return Err(failure(409, "management_resource_in_use"));
                }
                command.change.name = detail.definition.name;
                command.change.role = detail.definition.role;
                command.change.enabled = detail.definition.enabled;
            } else {
                let identity_changed = command.change.name != detail.definition.name
                    || command.change.role != detail.definition.role;
                if identity_changed && !detail.can_edit_identity {
                    return Err(failure(409, "management_resource_in_use"));
                }
                if (identity_changed || command.change.enabled)
                    && !detail.roles.contains(&command.change.role)
                {
                    return Err(failure(409, "management_model_unqualified"));
                }
            }
        }
        let (fresh, session) = self.authorized().await?;
        if fresh != context {
            return Err(failure(401, "management_scope_changed"));
        }
        session
            .resource_template_access(&context, Permission::ManageResourceTemplates)
            .map_err(store_error)?;
        command.validate().map_err(store_error)?;
        let operation = command.operation().map_err(store_error)?;
        let intent =
            Intent::new(operation, command.command_id.clone(), &command).map_err(store_error)?;
        let result = session.prepare_intent(&context, intent).await;
        self.finish(&context, result, true).await?;
        Ok(PreparedResourceTemplate {
            command_id: command.command_id,
            resource_id: command.resource_id,
            definition_id: command.definition_id,
            operation,
            change: command.change,
            expected_revision: command.expected_revision,
            result: None,
            error: None,
        })
    }
    async fn saved_template(
        &self,
        command_id: String,
    ) -> Result<(PendingIntent, ResourceTemplateCommand), NativeError> {
        super::key(&command_id)?;
        let saved = self
            .saved_commands()
            .await?
            .into_iter()
            .find(|s| s.command_id == command_id)
            .ok_or_else(|| failure(404, "management_saved_command_not_found"))?;
        let command = decode(&saved)?;
        let (context, session) = self.authorized().await?;
        let access =
            session.resource_template_access(&context, Permission::ManageResourceTemplates);
        self.finish(&context, access, false).await?;
        Ok((saved, command))
    }
    pub async fn review_template_command(
        &self,
        command_id: String,
    ) -> Result<PreparedResourceTemplate, NativeError> {
        let (saved, command) = self.saved_template(command_id).await?;
        let (result, error) = match saved.reply {
            Some(IntentReply::Success(v)) => {
                let result: ResourceTemplateResult = serde_json::from_value(v)
                    .map_err(|_| failure(503, "management_saved_result_invalid"))?;
                validate_result(&command, &result)?;
                (Some(result), None)
            }
            Some(IntentReply::Failure { code, .. }) => (None, Some(code)),
            None => (None, None),
        };
        Ok(PreparedResourceTemplate {
            command_id: command.command_id,
            resource_id: command.resource_id,
            definition_id: command.definition_id,
            operation: saved.operation,
            change: command.change,
            expected_revision: command.expected_revision,
            result,
            error,
        })
    }
    pub async fn apply_saved_template(
        &self,
        command_id: String,
    ) -> Result<ResourceTemplateResult, NativeError> {
        let (_, command) = self.saved_template(command_id).await?;
        let operation = command.operation().map_err(store_error)?;
        let intent =
            Intent::new(operation, command.command_id.clone(), &command).map_err(store_error)?;
        let result = self
            .with_intent(intent, async {
                let (context, session) = self.authorized().await?;
                let result = session
                    .apply_resource_template(&context, command.clone())
                    .await;
                self.finish(&context, result, true).await
            })
            .await?;
        validate_result(&command, &result)?;
        Ok(result)
    }
}

#[cfg(all(test, feature = "resource-adoption"))]
#[path = "resource_definition_tests.rs"]
mod tests;
