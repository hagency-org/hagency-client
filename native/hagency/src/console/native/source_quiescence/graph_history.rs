//! Descriptive graph review tied to the exact host and current SDK identity.
//! Names in legacy documents are not identities or project authorization.
use super::*;
use files::graph_history::GraphDocuments;
use hagency_store::owner_resources::{
    ProfileAuthority,
    graph_archives::{GRAPH_ARCHIVE_CHUNK_BYTES, OperatorGraphArchiveReceipt},
};

/// Host-owned, non-serializable observation; cannot be fabricated by Splash.
/// Capture occurs before retiring the original SDK session. After shutdown,
/// validate the same observation under its independently retained native gate.
pub struct NativeOperatorGraphInventory {
    scope: Arc<Scope>,
    documents: Arc<GraphDocuments>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorGraphInventoryReview {
    pub scope: &'static str,
    pub history: OperatorGraphHistorySummary,
    pub blockers: Vec<&'static str>,
    pub imported: bool,
    pub runtime_delivery_authorized: bool,
    pub external_roots_complete: bool,
}
impl SourceQuiescence {
    pub async fn inspect_operator_graph_documents(
        &self,
    ) -> Result<NativeOperatorGraphInventory, NativeError> {
        self.scope.gate.check().map_err(err)?;
        let phase = self.phase.lock().await;
        if !matches!(*phase, Phase::Ready) {
            return Err(err("graph_history_review_requires_live_source"));
        }
        verify_source(&self.scope.owner, &self.scope.device).await?;
        let scope = self.scope.clone();
        let documents = tokio::task::spawn_blocking(move || {
            check_scope(&scope)?;
            let documents =
                GraphDocuments::capture(scope.owner.0.host.state_directory()).map_err(err)?;
            check_scope(&scope)?;
            Ok::<_, NativeError>(Arc::new(documents))
        })
        .await
        .map_err(|_| err("graph_history_source_unavailable"))??;
        let inventory = NativeOperatorGraphInventory {
            scope: self.scope.clone(),
            documents,
        };
        inventory.review().await?;
        drop(phase);
        Ok(inventory)
    }
}
fn check_scope(scope: &Scope) -> Result<(), NativeError> {
    scope.gate.check().map_err(err)?;
    scope
        .owner
        .0
        .host
        .check_state_lock()
        .map_err(|_| err("source_root_changed"))
}
impl NativeOperatorGraphInventory {
    pub async fn review(&self) -> Result<OperatorGraphInventoryReview, NativeError> {
        check_scope(&self.scope)?;
        verify_source(&self.scope.owner, &self.scope.device).await?;
        let documents = self.documents.clone();
        let scope = self.scope.clone();
        let history = tokio::task::spawn_blocking(move || {
            check_scope(&scope)?;
            documents.check().map_err(err)?;
            check_scope(&scope)?;
            Ok::<_, NativeError>(documents.summary())
        })
        .await
        .map_err(|_| err("graph_history_source_unavailable"))??;
        verify_source(&self.scope.owner, &self.scope.device).await?;
        check_scope(&self.scope)?;
        Ok(review(history))
    }
}
impl QuiescedSources {
    /// Explicit native archival of the exact pre-shutdown review. It preserves
    /// history even before identities can be mapped, but grants no runtime use
    /// and does not publish an installation refusal or fence downlevel writers.
    pub async fn archive_operator_graph_inventory(
        self: &Arc<Self>,
        inventory: &NativeOperatorGraphInventory,
        command_id: String,
    ) -> Result<OperatorGraphArchiveReceipt, NativeError> {
        if !inventory.scope.graph_archive_allowed {
            return Err(failure(403, "graph_history_archive_permission_required"));
        }
        super::super::key(&command_id)?;
        let review = self.validate_operator_graph_inventory(inventory).await?;
        let binding = self
            .files
            .lock()
            .map_err(|_| err("source_quiescence_unknown"))?
            .identities()
            .empty_device_binding(&self.profile)
            .map_err(err)?;
        let gate = self.gate.clone();
        let profile = ProfileAuthority::new(
            self.profile.clone(),
            self.owner.0.expected_owner.clone(),
            self.gate.expires,
        )
        .and_then(|p| p.with_native_gate(move || gate.check().is_ok()))
        .map_err(super::super::management::store_error)?;
        let source = self.clone();
        let documents = inventory.documents.clone();
        let lengths = documents.lengths();
        let final_documents = documents.clone();
        let preparation = self
            .registry
            .prepare_operator_graph_archive(
                profile,
                command_id,
                binding,
                review.history,
                lengths,
                self.gate.expires,
                move || source.check().is_ok() && documents.check_identity().is_ok(),
                move || final_documents.check().is_ok(),
            )
            .map_err(super::super::management::store_error)?;
        preparation
            .begin()
            .await
            .map_err(super::super::management::store_error)?;
        for (document, length) in lengths.into_iter().enumerate() {
            if let Some(length) = length {
                for index in 0..(length as usize).div_ceil(GRAPH_ARCHIVE_CHUNK_BYTES) {
                    preparation
                        .write_chunk(document, index, inventory.documents.chunk(document, index))
                        .await
                        .map_err(super::super::management::store_error)?;
                }
            }
        }
        preparation
            .seal()
            .await
            .map_err(super::super::management::store_error)
    }

    /// Match the exact review after closing the original SDK. That SDK is
    /// intentionally retired; only the original finite host gate and custody
    /// may authorize this comparison. This is still not a graph writer fence.
    pub async fn validate_operator_graph_inventory(
        self: &Arc<Self>,
        inventory: &NativeOperatorGraphInventory,
    ) -> Result<OperatorGraphInventoryReview, NativeError> {
        self.check()?;
        if !Arc::ptr_eq(&self.owner.0, &inventory.scope.owner.0)
            || !Arc::ptr_eq(&self.gate, &inventory.scope.gate)
            || self.registry.id() != inventory.scope.registry.id()
            || self.profile != inventory.scope.profile
            || self.device_id != inventory.scope.device.device_id()
        {
            return Err(err("graph_history_review_scope_changed"));
        }
        let source = self.clone();
        let documents = inventory.documents.clone();
        let history = tokio::task::spawn_blocking(move || {
            source.check()?;
            documents.check().map_err(err)?;
            source.check()?;
            Ok::<_, NativeError>(documents.summary())
        })
        .await
        .map_err(|_| err("graph_history_source_unavailable"))??;
        self.check()?;
        Ok(review(history))
    }
}
fn review(history: OperatorGraphHistorySummary) -> OperatorGraphInventoryReview {
    let mut blockers = vec![
        "graph_document_adoption_not_completed",
        "graph_writer_fence_not_completed",
        "external_roots_not_complete",
    ];
    if history.unresolved_principal_names > 0 {
        blockers.push("graph_principal_identity_mapping_required");
    }
    if history.orphan_dispatch_messages > 0 {
        blockers.push("graph_orphan_dispatch_review_required");
    }
    OperatorGraphInventoryReview {
        scope: "selected_native_owner_root",
        history,
        blockers,
        imported: false,
        runtime_delivery_authorized: false,
        external_roots_complete: false,
    }
}
