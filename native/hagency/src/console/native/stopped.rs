//! Interrupted task recovery with native command custody and current ownership.
use super::{NativeError, NativeManagement, management::store_error};
use hagency_store::management::{
    Intent, IntentOperation, Permission, ResolveStopped, ReviewStopped, StoppedInspection,
    StoppedListQuery, StoppedPage, StoppedQuery, StoppedResolution, StoppedReview,
};

impl NativeManagement {
    pub async fn stopped_tasks(&self, query: StoppedListQuery) -> Result<StoppedPage, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.stopped_tasks(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn stopped_inspection(
        &self,
        query: StoppedQuery,
    ) -> Result<StoppedInspection, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.stopped_inspection(&context, query).await;
        self.finish(&context, result, false).await
    }
    pub async fn review_stopped(&self, input: ReviewStopped) -> Result<StoppedReview, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::InspectStoppedTasks)
            .map_err(store_error)?;
        let intent = Intent::new(
            IntentOperation::ReviewStopped,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, async {
            let (context, session) = self.authorized().await?;
            let result = session.review_stopped(&context, input).await;
            self.finish(&context, result, true).await
        })
        .await
    }
    pub async fn resolve_stopped(
        &self,
        input: ResolveStopped,
    ) -> Result<StoppedResolution, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::ResolveStoppedTasks)
            .map_err(store_error)?;
        let intent = Intent::new(
            IntentOperation::ResolveStopped,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, async {
            let (context, session) = self.authorized().await?;
            let result = session.resolve_stopped(&context, input).await;
            self.finish(&context, result, true).await
        })
        .await
    }
}
