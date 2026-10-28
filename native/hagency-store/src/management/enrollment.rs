//! First-resource onboarding from a provider model verified by the native host.
//! No caller-selected seat, credential reference, filesystem path or capability.
use super::*;
use hagency_core::{canonical, project::Resource};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollResource {
    pub command_id: String,
    pub runtime: String,
    pub model_id: String,
    pub reasoning: Option<String>,
    pub monthly_tokens: u64,
}
impl EnrollResource {
    pub fn validate(&self) -> Result<(), Error> {
        identifier(&self.command_id, 128)?;
        if !matches!(self.runtime.as_str(), "codex" | "claude" | "octos")
            || self.model_id.is_empty()
            || self.model_id.len() > 256
            || self.model_id.chars().any(char::is_control)
            || self
                .reasoning
                .as_ref()
                .is_some_and(|r| r.is_empty() || r.len() > 64 || r.chars().any(char::is_control))
            || self.monthly_tokens == 0
            || self.monthly_tokens > i64::MAX as u64
        {
            return Err(hagency_core::InvalidInput("invalid provider resource selection").into());
        }
        Ok(())
    }
}
/// Native-only proof: the host already checked the connected provider's model
/// catalog. Its account reference is hashed; neither it nor its home is stored
/// in script-readable resource data. Fresh provider verification is still
/// required before execution; this proof authorizes only a resource definition.
pub struct ProviderSelection {
    context: HostContext,
    input_digest: String,
    seat: String,
    model: String,
    octos_profile: Option<String>,
    expires: Instant,
}
impl ProviderSelection {
    pub fn new(
        context: HostContext,
        input: &EnrollResource,
        reference: &str,
        model: String,
        octos_profile: Option<String>,
        expires: Instant,
    ) -> Result<Self, Error> {
        input.validate()?;
        if reference.is_empty()
            || reference.len() > 512
            || reference.chars().any(char::is_control)
            || model.is_empty()
            || model.len() > 256
            || model.chars().any(char::is_control)
            || expires <= Instant::now()
            || expires > Instant::now() + Duration::from_secs(60)
        {
            return Err(Error::LocalAuthority);
        }
        let seat = super::connection::provider_seat(&context, &input.runtime, reference)?;
        Ok(Self {
            context,
            input_digest: canonical::digest(&serde_json::to_value(input)?)?,
            seat,
            model,
            octos_profile,
            expires,
        })
    }
}
impl Session {
    pub async fn enroll_resource(
        &self,
        context: &HostContext,
        input: EnrollResource,
        proof: ProviderSelection,
    ) -> Result<ResourceConfigurationResult, Error> {
        self.enroll_resource_scoped(context, input, proof, None)
            .await
    }
    pub async fn enroll_resource_with_legacy_lease(
        &self,
        context: &HostContext,
        input: EnrollResource,
        proof: ProviderSelection,
        lease: crate::installation_authority::LegacyResourceLease,
    ) -> Result<ResourceConfigurationResult, Error> {
        self.enroll_resource_scoped(context, input, proof, Some(lease))
            .await
    }
    async fn enroll_resource_scoped(
        &self,
        context: &HostContext,
        input: EnrollResource,
        proof: ProviderSelection,
        lease: Option<crate::installation_authority::LegacyResourceLease>,
    ) -> Result<ResourceConfigurationResult, Error> {
        let _pending = self.scope.pending.try_acquire().map_err(|_| Error::Busy)?;
        let _command = self.scope.commands.lock().await;
        self.check(context, Permission::EnrollResources)?;
        input.validate()?;
        if proof.context != *context
            || proof.expires <= Instant::now()
            || proof.input_digest != canonical::digest(&serde_json::to_value(&input)?)?
        {
            return Err(Error::LocalAuthority);
        }
        let receipt_input = serde_json::json!({"input":input,"seat":proof.seat,"model":proof.model,"octosProfile":proof.octos_profile});
        let receipt = CommandReceipt::new(
            context,
            "resource.enroll",
            &input.command_id,
            &receipt_input,
        )?;
        let preset = canonical::digest(
            &serde_json::json!({"actor":receipt.actor,"command":input.command_id}),
        )?;
        let resource: Resource = serde_json::from_value(serde_json::json!({
            "presetId":format!("embedded_{preset}"),"seatId":proof.seat,"framework":input.runtime,
            "model":proof.model,"reasoning":input.reasoning,"octosProfile":proof.octos_profile,
            "published":false,"ceiling":{"tokens":input.monthly_tokens,"period":"monthly"}
        }))?;
        resource.validate()?;
        if resource_capabilities(&resource).is_empty() {
            return Err(Error::Unqualified);
        }
        let mut command = self.scope.configuration.prepare_source(
            resource,
            receipt,
            proof.expires.min(Instant::now() + Duration::from_secs(2)),
        )?;
        if let Some(lease) = lease {
            command = command.with_legacy_lease(lease)?;
        }
        let result = self
            .store
            .legacy()?
            .enroll_management_resource(command)
            .await?;
        self.check(context, Permission::EnrollResources)
            .map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
