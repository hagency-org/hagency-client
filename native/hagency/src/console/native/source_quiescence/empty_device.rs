//! Retained native custody of an empty source; no initializer or provider work.
use super::*;
use hagency_store::owner_resources::adoption::EmptyDeviceInventory;
mod prepare;
mod observation;
pub(in crate::console::native) use observation::EmptyDeviceObservation;
pub use prepare::NativeEmptyDeviceInitialization;

/// A native, display-only observation before any original writer is closed.
/// The captured scope cannot be replaced by a mini-app's serialized claim.
pub struct EmptyDeviceReview {
    scope: Arc<Scope>,
    binding: String,
    inventory: EmptyDeviceInventory,
}
impl EmptyDeviceReview {
    pub fn personal_present(&self) -> bool {
        self.inventory.personal_present()
    }
    pub fn completed_private_records(&self) -> u32 {
        self.inventory.completed_private_records()
    }
}

impl SourceQuiescence {
    /// Inspect without retiring the owner, creating a personal database,
    /// invoking a provider, fencing files or recording an initialization.
    pub async fn review_empty_device(&self) -> Result<EmptyDeviceReview, NativeError> {
        self.scope.gate.check().map_err(err)?;
        let phase = self.phase.lock().await;
        if !matches!(*phase, Phase::Ready) {
            return Err(err("empty_device_review_requires_live_source"));
        }
        let scope = self.scope.clone();
        let result = tokio::time::timeout_at(scope.gate.expires.into(), async {
            verify_source(&scope.owner, &scope.device).await?;
            scope
                .owner
                .0
                .host
                .check_state_lock()
                .map_err(|_| err("source_root_changed"))?;
            scope.gate.check().map_err(err)?;
            let captured = scope.clone();
            let result = tokio::task::spawn_blocking(move || {
                captured.gate.check().map_err(err)?;
                let files = captured
                    .sources
                    .lock()
                    .map_err(|_| err("source_quiescence_unknown"))?;
                let files = files
                    .as_ref()
                    .ok_or(err("empty_device_review_requires_live_source"))?;
                let result = files
                    .empty_device_inventory(&captured.profile, &captured.owner.0.expected_owner)
                    .map_err(err)?;
                captured.gate.check().map_err(err)?;
                Ok::<_, NativeError>(result)
            })
            .await
            .map_err(|_| err("source_quiescence_unknown"))??;
            verify_source(&scope.owner, &scope.device).await?;
            scope
                .owner
                .0
                .host
                .check_state_lock()
                .map_err(|_| err("source_root_changed"))?;
            scope.gate.check().map_err(err)?;
            Ok::<_, NativeError>(result)
        })
        .await
        .map_err(|_| err("source_quiescence_authority_changed"))??;
        drop(phase);
        Ok(EmptyDeviceReview {
            scope,
            binding: result.0,
            inventory: result.1,
        })
    }
}

/// Retains the original physical locks and finite native gate. This is not
/// serializable and cannot be constructed from a script claim of emptiness.
pub struct EmptyDeviceSource {
    source: Arc<QuiescedSources>,
    binding: String,
    inventory: EmptyDeviceInventory,
}
impl EmptyDeviceSource {
    pub fn personal_present(&self) -> bool {
        self.inventory.personal_present()
    }
    pub fn completed_private_records(&self) -> u32 {
        self.inventory.completed_private_records()
    }
    pub fn review_digest(&self) -> Result<String, NativeError> {
        self.source.check()?;
        let inventory = self
            .inventory
            .digest()
            .map_err(super::super::management::store_error)?;
        hagency_core::canonical::digest(&serde_json::json!({
            "version":1,"sourceBinding":self.binding,"inventory":inventory,
        }))
        .map_err(|_| err("empty_device_inventory_unavailable"))
    }
}
impl QuiescedSources {
    /// Match exactly the review shown before shutdown. Even another review of
    /// the same logical profile must not substitute its native scope or files.
    pub async fn validate_empty_device_review(
        self: &Arc<Self>,
        review: &EmptyDeviceReview,
    ) -> Result<EmptyDeviceSource, NativeError> {
        self.check()?;
        if !Arc::ptr_eq(&self.owner.0, &review.scope.owner.0)
            || !Arc::ptr_eq(&self.gate, &review.scope.gate)
            || self.registry.id() != review.scope.registry.id()
            || self.profile != review.scope.profile
            || self.device_id != review.scope.device.device_id()
        {
            return Err(err("empty_device_review_scope_changed"));
        }
        let current = self.empty_device_inventory().await?;
        if current.binding != review.binding
            || current
                .inventory
                .digest()
                .map_err(super::super::management::store_error)?
                != review
                    .inventory
                    .digest()
                    .map_err(super::super::management::store_error)?
        {
            return Err(err("empty_device_review_changed"));
        }
        self.check()?;
        Ok(current)
    }

    /// Recapture under the exact source locks after the original writers drain.
    /// The returned evidence alone does not fence sources or activate a registry.
    pub async fn empty_device_inventory(
        self: &Arc<Self>,
    ) -> Result<EmptyDeviceSource, NativeError> {
        self.check()?;
        let source = self.clone();
        let (binding, inventory) = tokio::task::spawn_blocking(move || {
            source.check()?;
            let result = source
                .files
                .lock()
                .map_err(|_| err("source_quiescence_unknown"))?
                .empty_device_inventory(&source.profile, &source.owner.0.expected_owner)
                .map_err(err)?;
            source.check()?;
            Ok::<_, NativeError>(result)
        })
        .await
        .map_err(|_| err("source_quiescence_unknown"))??;
        self.check()?;
        Ok(EmptyDeviceSource {
            source: self.clone(),
            binding,
            inventory,
        })
    }
}

#[cfg(test)]
mod tests;
