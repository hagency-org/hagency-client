//! A fresh native observation of the provider bound to an enrolled resource.
//! This read grants no execution, publication, reservation or allocation rights.
use super::*;

pub(super) fn provider_seat(
    context: &HostContext,
    runtime: &str,
    reference: &str,
) -> Result<String, Error> {
    if !matches!(runtime, "codex" | "claude" | "octos")
        || reference.is_empty()
        || reference.len() > 512
        || reference.chars().any(char::is_control)
    {
        return Err(Error::LocalAuthority);
    }
    Ok(format!(
        "seat_embedded_{}",
        hagency_core::canonical::digest(&serde_json::json!({
            "account":context.account,"framework":runtime,"reference":reference
        }))?
    ))
}

/// Constructed only by the native provider adapter, never deserialized from UI
/// input. No credential reference is retained or exposed in the read result.
pub struct ProviderWitness {
    context: HostContext,
    seat: String,
    runtime: String,
    model: String,
    reasoning: Option<String>,
    expires: Instant,
}
impl ProviderWitness {
    pub fn new(
        context: HostContext,
        runtime: String,
        reference: &str,
        model: String,
        reasoning: Option<String>,
        expires: Instant,
    ) -> Result<Self, Error> {
        if model.is_empty()
            || model.len() > 256
            || model.chars().any(char::is_control)
            || reasoning
                .as_ref()
                .is_some_and(|v| v.len() > 64 || v.chars().any(char::is_control))
            || expires <= Instant::now()
            || expires > Instant::now() + Duration::from_secs(60)
        {
            return Err(Error::LocalAuthority);
        }
        Ok(Self {
            seat: provider_seat(&context, &runtime, reference)?,
            context,
            runtime,
            model,
            reasoning,
            expires,
        })
    }
}
impl Session {
    /// Select the native proof adapter without exposing a path or accepting an
    /// account identity from the mini app. This is not execution authority.
    #[cfg(feature = "resource-adoption")]
    pub fn uses_canonical_resources(&self, context: &HostContext) -> Result<bool, Error> {
        self.check(context, Permission::ReadResources)?;
        Ok(matches!(self.store, SessionStore::Adopted(_)))
    }
    /// Explicit reconnect/verify against the adopted canonical resource. The
    /// original central command still performs owner/device/generation/CAS
    /// checks; this read never changes a grant, capacity or the legacy fence.
    #[cfg(feature = "resource-adoption")]
    pub async fn verify_canonical_connection(
        &self,
        context: &HostContext,
        id: String,
        expected_revision: String,
        provider: crate::owner_resources::VerifiedProviderAccount,
        resume: bool,
    ) -> Result<ResourceSummary, Error> {
        let permission = if resume {
            Permission::ResumeConnections
        } else {
            Permission::VerifyConnections
        };
        self.check(context, Permission::ReadResources)?;
        self.check(context, permission)?;
        identifier(&id, 128)?;
        let SessionStore::Adopted(store) = &self.store else {
            return Err(Error::LocalAuthority);
        };
        let result = store
            .verify_connection(
                &self.adopted_profile(store)?,
                id,
                expected_revision,
                provider,
            )
            .await?;
        self.check(context, Permission::ReadResources)?;
        self.check(context, permission)?;
        Ok(result)
    }
    pub async fn verify_resource(
        &self,
        context: &HostContext,
        id: String,
        expected_revision: String,
        proof: ProviderWitness,
    ) -> Result<ResourceSummary, Error> {
        self.check_connection_resource(
            context,
            id,
            expected_revision,
            proof,
            Permission::VerifyConnections,
        )
        .await
    }
    pub async fn resume_resource(
        &self,
        context: &HostContext,
        id: String,
        expected_revision: String,
        proof: ProviderWitness,
    ) -> Result<ResourceSummary, Error> {
        self.check_connection_resource(
            context,
            id,
            expected_revision,
            proof,
            Permission::ResumeConnections,
        )
        .await
    }
    async fn check_connection_resource(
        &self,
        context: &HostContext,
        id: String,
        expected_revision: String,
        proof: ProviderWitness,
        permission: Permission,
    ) -> Result<ResourceSummary, Error> {
        self.check(context, permission)?;
        identifier(&id, 128)?;
        if proof.context != *context || proof.expires <= Instant::now() {
            return Err(Error::LocalAuthority);
        }
        let expires = proof.expires;
        let result = self
            .store
            .legacy()?
            .verify_management_resource(id, expected_revision, proof)
            .await?;
        self.check(context, permission)?;
        if expires <= Instant::now() {
            return Err(Error::LocalAuthority);
        }
        Ok(result)
    }
}
impl DomainRepository {
    pub(crate) fn verify_management_resource(
        &self,
        id: &str,
        revision: &str,
        proof: ProviderWitness,
        at: u64,
    ) -> Result<ResourceSummary, Error> {
        if proof.expires <= Instant::now() {
            return Err(Error::LocalAuthority);
        }
        let resource = self.resource_configuration(id)?;
        if resource_publication_revision(&resource)? != revision {
            return Err(Error::Conflict);
        }
        if resource.seat_id != proof.seat
            || resource.framework != proof.runtime
            || resource.model != proof.model
            || resource.reasoning != proof.reasoning
        {
            return Err(Error::LocalAuthority);
        }
        self.management_resource(id, at)
    }
}

#[cfg(feature="resource-adoption")]
impl Session {
    /// Host-only original namespace for the explicit connection operation.
    /// A resource ID never selects a filesystem path or a default profile.
    pub async fn connection_namespace(
        &self, context: &HostContext, resource: &ResourceSummary, resume: bool,
    ) -> Result<Option<crate::owner_resources::ResourceNamespace>, Error> {
        let permission=if resume {Permission::ResumeConnections} else {Permission::VerifyConnections};
        self.check(context, Permission::ReadResources)?;
        self.check(context, permission)?;
        let SessionStore::Adopted(store)=&self.store else {return Err(Error::LocalAuthority)};
        let result=store.resource_namespace(&self.adopted_profile(store)?,resource.id.clone(),resource.revision.clone()).await?;
        self.check(context, permission)?;
        Ok(result)
    }
}
