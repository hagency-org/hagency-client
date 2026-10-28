//! Trusted provider-account proof for the installation-level owner registry.
use super::{ManagementRuntime, NativeError, NativeManagement, failure};
use hagency_store::{management::Permission, owner_resources::VerifiedProviderAccount};

impl NativeManagement {
    /// Obtain a short-lived, non-deserializable identity from an already
    /// connected provider. This does not enroll a resource, merge old ledgers,
    /// approve capacity, read credential files, or initiate provider login.
    /// The native host must still obtain owner consent for registry enrollment.
    pub async fn verified_provider_account(
        &self,
        runtime: ManagementRuntime,
    ) -> Result<VerifiedProviderAccount, NativeError> {
        let _guard = self.owner.0.calls.lock().await;
        let (context, session) = self.authorized().await?;
        for permission in [Permission::ReadProviders, Permission::ConnectProviders] {
            session
                .authorize(&context, permission)
                .map_err(|_| failure(403, "management_permission_required"))?;
        }
        // Claude's public auth status currently has email/orgId, not the
        // individual quota account UUID. Octos profiles expose no such proof.
        // Their existing onboarding remains usable; neither can mint a new
        // installation-level capacity declaration from a path or email.
        if runtime != ManagementRuntime::Codex {
            return Err(failure(409, "provider_identity_unavailable"));
        }
        let cookie = self.owner.0.cookie.lock().await.clone();
        let raw_cookie = cookie
            .strip_prefix("hagency_console=")
            .ok_or_else(|| failure(401, "owner_authorization_required"))?;
        let result = crate::console::owner_provider::identity::verify(
            &self.owner.0.host.console,
            raw_cookie,
        )
        .await;
        let (fresh, live) = self.authorized().await?;
        if fresh != context {
            return Err(failure(401, "management_scope_changed"));
        }
        for permission in [Permission::ReadProviders, Permission::ConnectProviders] {
            live.authorize(&context, permission)
                .map_err(|_| failure(403, "management_permission_required"))?;
        }
        let (proof, deadline) = result.map_err(|error| failure(error.status, error.code))?;
        if std::time::Instant::now() >= deadline {
            return Err(failure(409, "provider_identity_unavailable"));
        }
        Ok(proof)
    }
}

impl NativeManagement {
    pub(super) async fn namespace_provider(
        &self,
        namespace: &hagency_store::owner_resources::ResourceNamespace,
        deadline: std::time::Instant,
    ) -> Result<VerifiedProviderAccount, NativeError> {
        namespace.check().await.map_err(super::management::store_error)?;
        let (proof, _) = self.observe_account_namespace(namespace.launch(), deadline).await?;
        namespace.check().await.map_err(super::management::store_error)?;
        Ok(proof)
    }
    pub(super) async fn observe_account_namespace(
        &self, launch: &hagency_store::ManagedLaunch, deadline: std::time::Instant,
    ) -> Result<(VerifiedProviderAccount, super::super::owner_provider::ProviderStatus), NativeError> {
        #[cfg(test)]
        let fixture=self.owner.0.account_logins.executable.lock().unwrap().clone();
        #[cfg(test)]
        let executable=match fixture {Some(p)=>p,None=>super::super::owner_provider::executable().map_err(|e|failure(e.status,e.code))?};
        #[cfg(not(test))]
        let executable=super::super::owner_provider::executable().map_err(|e|failure(e.status,e.code))?;
        super::super::owner_provider::managed::inspect_resource(
            launch,&self.owner.0.host.console.0.owner_provider,&executable,deadline,
        ).await.map_err(|e|failure(e.status,e.code))
    }
    pub(super) async fn verified_resource_provider_account(
        &self,
        registry: &hagency_store::owner_resources::OwnerResourceRegistry,
        profile: hagency_store::owner_resources::ProfileAuthority,
        runtime: ManagementRuntime,
        resource: String,
        revision: String,
    ) -> Result<VerifiedProviderAccount, NativeError> {
        let namespace=registry.resource_namespace(profile,resource,revision).await.map_err(super::management::store_error)?;
        let Some(namespace)=namespace else {return self.verified_provider_account(runtime).await};
        if runtime!=ManagementRuntime::Codex {return Err(failure(409,"provider_identity_unavailable"));}
        let _guard=self.owner.0.calls.lock().await;
        let (context,session)=self.authorized().await?;
        for p in [Permission::ReadProviders,Permission::ConnectProviders] {session.authorize(&context,p).map_err(super::management::store_error)?;}
        let deadline=session.valid_until().min(std::time::Instant::now()+std::time::Duration::from_secs(30));
        let result=self.namespace_provider(&namespace,deadline).await;
        let (fresh,live)=self.authorized().await?;
        if fresh!=context {return Err(failure(401,"management_scope_changed"));}
        for p in [Permission::ReadProviders,Permission::ConnectProviders] {live.authorize(&context,p).map_err(super::management::store_error)?;}
        result
    }
}
