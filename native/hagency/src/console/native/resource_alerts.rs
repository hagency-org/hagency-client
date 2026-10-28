//! Retained resource alerts with current SDK scope and original command custody.
use super::{NativeError, NativeManagement, management::store_error};
use hagency_store::management::{Intent,IntentOperation,Permission,ResourceAlert,ResourceAlertPage,ResourceAlertQuery,ResourceAlertDetail,ResourceAlertDetailQuery,TriageResourceAlert};
impl NativeManagement {
    pub async fn resource_alerts(&self,query:ResourceAlertQuery)->Result<ResourceAlertPage,NativeError>{
        let (context,session)=self.authorized().await?;
        let result=session.resource_alerts(&context,query).await;
        self.finish(&context,result,false).await
    }
    pub async fn resource_alert(&self,query:ResourceAlertDetailQuery)->Result<ResourceAlertDetail,NativeError>{
        let (context,session)=self.authorized().await?;
        let result=session.resource_alert(&context,query).await;
        self.finish(&context,result,false).await
    }
    pub async fn triage_resource_alert(&self,input:TriageResourceAlert)->Result<ResourceAlert,NativeError>{
        input.validate().map_err(store_error)?;
        let (context,session)=self.authorized().await?;
        session.authorize(&context,Permission::TriageAlerts).map_err(store_error)?;
        let intent=Intent::new(IntentOperation::TriageResourceAlert,input.command_id.clone(),&input).map_err(store_error)?;
        self.with_intent(intent,async {
            let (context,session)=self.authorized().await?;
            let result=session.triage_resource_alert(&context,input).await;
            self.finish(&context,result,true).await
        }).await
    }
}
