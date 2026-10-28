//! Authenticated historical task-graph browsing, entirely in process.
use super::{NativeError, NativeManagement, failure};
use crate::operator_graphs::{GraphError, reader};
use hagency_store::owner_resources::graph_archives::OperatorGraphArchiveReceipt;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphHistoryArchives {
    pub supported: bool,
    pub archives: Vec<OperatorGraphArchiveReceipt>,
    pub read_only: bool,
    pub identity_mapped: bool,
}
impl NativeManagement {
    pub async fn graph_history_archives(&self) -> Result<GraphHistoryArchives, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.operator_graph_archives(&context).await;
        let archives = self.finish(&context, result, false).await?;
        Ok(GraphHistoryArchives {
            supported: archives.is_some(),
            archives: archives.unwrap_or_default(),
            read_only: true,
            identity_mapped: false,
        })
    }
    async fn graph_history_project<T: Send + 'static>(
        &self,
        archive_id: String,
        project: impl FnOnce(Option<Vec<u8>>) -> Result<T, GraphError> + Send + 'static,
    ) -> Result<T, NativeError> {
        let (context, session) = self.authorized().await?;
        let read = session
            .operator_graph_history_document(&context, archive_id)
            .await;
        let bytes = self.finish(&context, read, false).await?;
        let result = tokio::task::spawn_blocking(move || project(bytes))
            .await
            .map_err(|_| failure(503, "graph_history_unavailable"))?
            .map_err(|e| {
                failure(
                    if e.code.ends_with("not_found") {
                        404
                    } else {
                        409
                    },
                    e.code,
                )
            })?;
        let (fresh, _) = self.authorized().await?;
        if fresh != context {
            return Err(failure(401, "management_scope_changed"));
        }
        Ok(result)
    }
    pub async fn graph_history_page(
        &self,
        q: reader::GraphHistoryQuery,
    ) -> Result<reader::GraphHistoryPage, NativeError> {
        q.validate()
            .map_err(|_| failure(400, "invalid_management_arguments"))?;
        self.graph_history_project(q.archive_id.clone(), move |b| reader::page(b, q))
            .await
    }
    pub async fn graph_history_detail(
        &self,
        q: reader::GraphHistoryDetailQuery,
    ) -> Result<reader::GraphHistoryDetail, NativeError> {
        q.validate()
            .map_err(|_| failure(400, "invalid_management_arguments"))?;
        self.graph_history_project(q.archive_id.clone(), move |b| reader::detail(b, q))
            .await
    }
    pub async fn graph_history_node(
        &self,
        q: reader::GraphHistoryNodeQuery,
    ) -> Result<reader::GraphHistoryNode, NativeError> {
        q.validate()
            .map_err(|_| failure(400, "invalid_management_arguments"))?;
        self.graph_history_project(q.archive_id.clone(), move |b| reader::node(b, q))
            .await
    }
}
