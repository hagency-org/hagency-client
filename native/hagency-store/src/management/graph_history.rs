//! Read-only graph history under the current package, profile and store epoch.
use super::*;
use crate::owner_resources::graph_archives::OperatorGraphArchiveReceipt;

impl Session {
    pub async fn operator_graph_archives(
        &self,
        context: &HostContext,
    ) -> Result<Option<Vec<OperatorGraphArchiveReceipt>>, Error> {
        self.check(context, Permission::ReadOperatorGraphHistory)?;
        let _pending = self.scope.pending.try_acquire().map_err(|_| Error::Busy)?;
        let SessionStore::Adopted(store) = &self.store else {
            return Ok(None);
        };
        let profile = self.adopted_profile(store)?;
        let registry = store.enrollment_registry(&profile)?;
        let result = registry.operator_graph_archives(&profile).await?;
        store.enrollment_registry(&profile)?;
        self.check(context, Permission::ReadOperatorGraphHistory)?;
        Ok(Some(result))
    }

    /// Native-only parsing input. Host services expose typed graph projections,
    /// never this document, source paths, message contents or registry handles.
    pub async fn operator_graph_history_document(
        &self,
        context: &HostContext,
        command_id: String,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.check(context, Permission::ReadOperatorGraphHistory)?;
        let _pending = self.scope.pending.try_acquire().map_err(|_| Error::Busy)?;
        let SessionStore::Adopted(store) = &self.store else {
            return Err(Error::Unqualified);
        };
        let profile = self.adopted_profile(store)?;
        let registry = store.enrollment_registry(&profile)?;
        let result = registry
            .operator_graph_history_document(&profile, command_id)
            .await?;
        store.enrollment_registry(&profile)?;
        self.check(context, Permission::ReadOperatorGraphHistory)?;
        Ok(result)
    }
}
