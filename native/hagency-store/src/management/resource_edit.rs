//! Closed in-place editing. Creating configurations and amending an admitted
//! resource are separate operations, never caller-controlled flags here.
use super::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditLocalResource {
    pub command_id: String,
    pub resource_id: String,
    pub expected_revision: String,
    pub profile: ProfileChange,
    pub ceiling: CeilingChange,
    /// Omission preserves both the stored default and legacy receipt hashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_policy: Option<hagency_core::execution::ExecutionPolicy>,
}
impl EditLocalResource {
    pub fn validate(&self) -> Result<(), Error> {
        identifier(&self.command_id, 128)?;
        identifier(&self.resource_id, 128)?;
        if self.expected_revision.len() != 64
            || !self
                .expected_revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(hagency_core::InvalidInput("invalid resource revision").into());
        }
        if let ProfileChange::Select { model, reasoning } = &self.profile
            && (model.is_empty()
                || model.len() > 256
                || model.chars().any(char::is_control)
                || reasoning.as_ref().is_some_and(|s| {
                    s.is_empty() || s.len() > 128 || s.chars().any(char::is_control)
                }))
        {
            return Err(hagency_core::InvalidInput("invalid resource configuration").into());
        }
        Ok(())
    }

    #[cfg(feature = "resource-adoption")]
    fn receipt(&self, context: &HostContext, profile: &str) -> Result<CommandReceipt, Error> {
        CommandReceipt::new(
            context,
            "resource.edit_local",
            &self.command_id,
            &serde_json::json!({"profile":profile,"input":self}),
        )
    }
}

#[cfg(feature = "resource-adoption")]
impl Session {
    /// Current native writer selection, never a serialized mini-app capability.
    pub fn local_resource_editor_registry(
        &self,
        context: &HostContext,
    ) -> Result<
        (
            crate::owner_resources::OwnerResourceRegistry,
            crate::owner_resources::ProfileAuthority,
        ),
        Error,
    > {
        self.check(context, Permission::ConfigureResources)?;
        let SessionStore::Adopted(store) = &self.store else {
            return Err(Error::LocalAuthority);
        };
        let profile = self.adopted_profile(store)?;
        let registry = store.enrollment_registry(&profile)?;
        self.check(context, Permission::ConfigureResources)?;
        Ok((registry, profile))
    }
    /// Read only: recover the exact original effect before requesting fresh
    /// provider metadata. A completed edit may since have been offered.
    pub async fn recover_local_resource_edit(
        &self,
        context: &HostContext,
        input: &EditLocalResource,
    ) -> Result<Option<ResourceConfigurationResult>, Error> {
        self.check(context, Permission::ConfigureResources)?;
        input.validate()?;
        let SessionStore::Adopted(store) = &self.store else {
            return Err(Error::LocalAuthority);
        };
        let receipt = input.receipt(context, store.identity().0)?;
        let result = store
            .recover_local_resource_edit(
                &self.adopted_profile(store)?,
                input.resource_id.clone(),
                receipt,
            )
            .await?;
        self.check(context, Permission::ConfigureResources)?;
        Ok(result)
    }

    /// Trusted host only. The binding cannot be deserialized from mini-app data.
    pub async fn edit_local_resource(
        &self,
        context: &HostContext,
        input: EditLocalResource,
        binding: crate::owner_resources::ResourceEditBinding,
    ) -> Result<ResourceConfigurationResult, Error> {
        let _pending = self.scope.pending.try_acquire().map_err(|_| Error::Busy)?;
        let _command = self.scope.commands.lock().await;
        self.check(context, Permission::ConfigureResources)?;
        input.validate()?;
        let SessionStore::Adopted(store) = &self.store else {
            return Err(Error::LocalAuthority);
        };
        let receipt = input.receipt(context, store.identity().0)?;
        let result = store
            .edit_local_resource(&self.adopted_profile(store)?, input, binding, receipt)
            .await?;
        self.check(context, Permission::ConfigureResources)
            .map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
