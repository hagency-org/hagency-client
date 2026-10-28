//! Current native execution-device history. This projection is not readiness.
use super::*;
pub use hagency_store::owner_resources::shared_execution::{
    SharedSuccessorPage, SharedSuccessorSummary,
};

impl SharedExecutionTransport {
    #[cfg(all(test, feature = "shared-successors"))]
    pub(in crate::console::native) async fn inventory_fixture_claim_original(
        &self,
        registry: &OwnerResourceRegistry,
        command: SharedRuntimeCommand,
    ) -> Result<(), NativeError> {
        let b = self.io.bound().await?;
        let enrollment = b.enrollment(registry, &command, true).await?;
        let proof = VerifiedSharedRuntimeDelivery::new(
            enrollment,
            command.clone(),
            digest(&command)?,
            true,
            b.scope
                .deadline
                .min(Instant::now() + Duration::from_secs(20)),
        )
        .map_err(store)?;
        registry.claim_shared_runtime(proof).await.map_err(store)?;
        Ok(())
    }
    pub async fn list_successors(
        &self,
        registry: &OwnerResourceRegistry,
        after: Option<String>,
    ) -> Result<SharedSuccessorPage, NativeError> {
        if !cfg!(feature = "shared-successors") {
            return Err(failure(409, "shared_successors_disabled"));
        }
        if let Some(id) = &after {
            super::super::key(id)?;
        }
        let b = self.io.bound().await?;
        let page = registry
            .shared_successor_inventory(
                b.scope.profile.clone(),
                b.scope.device.device_id().into(),
                b.scope.device.generation(),
                after,
            )
            .await
            .map_err(store)?;
        b.transport.check_scope(&b.scope, false).await?;
        Ok(page)
    }
}
