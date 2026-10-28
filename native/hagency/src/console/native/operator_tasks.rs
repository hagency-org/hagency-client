//! Exact-profile retained operator reads, distinct from shared execution work.
use super::{NativeError, NativeManagement};
use hagency_store::management as service;
impl NativeManagement {
    pub async fn operator_task_page(
        &self,
        input: service::OperatorTaskQuery,
    ) -> Result<service::OperatorTaskPage, NativeError> {
        input.validate().map_err(super::management::store_error)?;
        let (context, session) = self.authorized().await?;
        let result = session.operator_task_page(&context, input).await;
        self.finish(&context, result, false).await
    }
    pub async fn operator_task_detail(
        &self,
        input: service::OperatorTaskDetailQuery,
    ) -> Result<service::OperatorTaskDetail, NativeError> {
        input.validate().map_err(super::management::store_error)?;
        let (context, session) = self.authorized().await?;
        let result = session.operator_task_detail(&context, input).await;
        self.finish(&context, result, false).await
    }
    pub async fn operator_board(
        &self,
        input: service::OperatorBoardQuery,
    ) -> Result<service::OperatorBoardPage, NativeError> {
        input.validate().map_err(super::management::store_error)?;
        let (context, session) = self.authorized().await?;
        let result = session.operator_board(&context, input).await;
        self.finish(&context, result, false).await
    }
}
