//! Current account graph reads expose bounded projections, never private dispatch
//! envelopes, view tokens, registry paths or a whole graph document.
use super::{NativeError, NativeManagement, failure, management::store_error};
use crate::operator_graphs::current::{
    self, OperatorGraphDetail, OperatorGraphDetailQuery, OperatorGraphNode, OperatorGraphNodeQuery,
    OperatorGraphPage, OperatorGraphQuery,
};
use hagency_store::{
    Error,
    management::{Permission, OPERATOR_GRAPH_SCOPE},
    owner_resources::graph_journal::GraphJournalRecord,
};

impl NativeManagement {
    pub async fn operator_graph_page(
        &self,
        query: OperatorGraphQuery,
    ) -> Result<OperatorGraphPage, NativeError> {
        query.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        let (registry, profile) = session
            .operator_graph_access(&context, Permission::ReadOperatorGraphs)
            .map_err(store_error)?;
        let index = registry
            .operator_graph_record_page(&profile, OPERATOR_GRAPH_SCOPE, query.after)
            .await;
        let index = self.finish(&context, index, false).await?;
        let mut graphs = Vec::new();
        // At most sixteen four-MiB records are inspected, one at a time. An
        // empty filtered page can still have a cursor; callers must retain it.
        for item in index.items {
            let record = registry
                .operator_graph_record(&profile, OPERATOR_GRAPH_SCOPE, &item.graph_id)
                .await;
            let record = self
                .finish(&context, record, false)
                .await?
                .ok_or_else(|| failure(409, "management_revision_conflict"))?;
            if record.revision != item.revision
                || record.document.len() as u64 != item.document_bytes
            {
                return Err(failure(409, "management_revision_conflict"));
            }
            let owner = profile.account().to_owned();
            let gated = profile.clone();
            let projected = tokio::task::spawn_blocking(move || {
                gated.check_current()?;
                let summary = current::summary(record, item.graph_id, owner)?;
                gated.check_current()?;
                Ok(summary)
            })
            .await
            .map_err(|_| failure(503, "management_unavailable"))?;
            let summary = self.finish(&context, projected, false).await?;
            if query
                .status
                .as_ref()
                .is_none_or(|status| summary.status == *status)
            {
                graphs.push(summary);
            }
        }
        self.finish(
            &context,
            Ok(OperatorGraphPage {
                supported: true,
                graphs,
                next_after: index.next,
            }),
            false,
        )
        .await
    }

    async fn operator_graph_project<T: Send + 'static>(
        &self,
        graph_id: String,
        project: impl FnOnce(GraphJournalRecord, String) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, NativeError> {
        let (context, session) = self.authorized().await?;
        let (registry, profile) = session
            .operator_graph_access(&context, Permission::ReadOperatorGraphs)
            .map_err(store_error)?;
        let read = registry
            .operator_graph_record(&profile, OPERATOR_GRAPH_SCOPE, &graph_id)
            .await;
        let record = self
            .finish(&context, read, false)
            .await?
            .ok_or_else(|| failure(404, "management_not_found"))?;
        let result = tokio::task::spawn_blocking(move || {
            profile.check_current()?;
            let result = project(record, profile.account().to_owned())?;
            profile.check_current()?;
            Ok(result)
        })
        .await
        .map_err(|_| failure(503, "management_unavailable"))?;
        self.finish(&context, result, false).await
    }

    pub async fn operator_graph_detail(
        &self,
        query: OperatorGraphDetailQuery,
    ) -> Result<OperatorGraphDetail, NativeError> {
        query.validate().map_err(store_error)?;
        self.operator_graph_project(query.id.clone(), move |record, owner| {
            current::detail(record, query, owner)
        })
        .await
    }

    pub async fn operator_graph_node(
        &self,
        query: OperatorGraphNodeQuery,
    ) -> Result<OperatorGraphNode, NativeError> {
        query.validate().map_err(store_error)?;
        self.operator_graph_project(query.id.clone(), move |record, owner| {
            current::node(record, query, owner)
        })
        .await
    }
}
