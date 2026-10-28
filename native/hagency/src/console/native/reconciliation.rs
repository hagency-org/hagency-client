//! Read-only source observation. The native host chooses the profile and store.
use super::{NativeError, NativeManagement, failure};
use hagency_store::management::{Permission, ProfileLedgerAudit};

impl NativeManagement {
    /// Inspect only this already-open profile ledger. This does not inspect or
    /// open other profiles, migrate records, or enable owner-registry capacity.
    pub async fn reconciliation_audit(&self) -> Result<ProfileLedgerAudit, NativeError> {
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::ReadResources)
            .map_err(|_| failure(403, "management_permission_required"))?;
        let device = self
            .owner
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        device
            .bearer()
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        let profile = crate::console::owned_agents::profile_identity(
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .map_err(|_| failure(401, "management_profile_mismatch"))?;
        let result = session.reconciliation_audit(&context, profile).await;
        self.finish(&context, result, false).await
    }
}
