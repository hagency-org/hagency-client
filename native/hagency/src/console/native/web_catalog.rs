//! Reviewed original web-console settings. No server project approval authority.
use super::{NativeManagement,NativeError,failure,management::store_error};
use hagency_store::management::{Intent,IntentOperation,IntentReply,PendingIntent,WebCatalogQuery,WebCatalogView,WebCatalogPrepare,WebCatalogChange,WebCatalogCommand,WebCatalogResult,WebOfferPage,WebTrustPage};
use serde::Serialize;

#[derive(Debug,Serialize)]
#[serde(rename_all="camelCase")]
pub struct PreparedWebCatalog {
    pub command_id:String,pub resource_id:String,pub expected_revision:String,pub change:WebCatalogChange,
    pub result:Option<WebCatalogResult>,pub error:Option<String>,
}
pub(super) fn known_failure(error:&NativeError)->bool{error.status==400||error.status==404||matches!(error.code.as_str(),"management_revision_conflict"|"management_resource_in_use"|"management_model_unqualified"|"management_change_cancelled")}
fn decode(saved:&PendingIntent)->Result<WebCatalogCommand,NativeError>{
    if saved.operation!=IntentOperation::WebCatalog{return Err(failure(400,"invalid_management_arguments"))}
    let command:WebCatalogCommand=serde_json::from_value(saved.input.clone()).map_err(|_|failure(503,"management_saved_command_invalid"))?;
    command.validate().map_err(|_|failure(503,"management_saved_command_invalid"))?;
    if command.command_id!=saved.command_id{return Err(failure(503,"management_saved_command_invalid"))}Ok(command)
}
fn validate_result(command:&WebCatalogCommand,result:&WebCatalogResult)->Result<(),NativeError>{
    if command.command_id!=result.command_id||command.resource_id!=result.resource_id||result.server_auto_approval||result.retired!=matches!(command.change,WebCatalogChange::Retire{})||result.revision.len()!=64||result.operation!=command.change.operation(){return Err(failure(503,"management_saved_result_invalid"))}Ok(())
}
impl NativeManagement {
    pub async fn web_catalog(&self,query:WebCatalogQuery)->Result<WebCatalogView,NativeError>{let(c,s)=self.authorized().await?;let result=s.web_catalog(&c,query).await;self.finish(&c,result,false).await}
    pub async fn web_offers(&self,resource_id:String)->Result<WebOfferPage,NativeError>{let(c,s)=self.authorized().await?;let result=s.web_offers(&c,resource_id).await;self.finish(&c,result,false).await}
    pub async fn web_trust(&self,query:WebCatalogQuery)->Result<WebTrustPage,NativeError>{let(c,s)=self.authorized().await?;let result=s.web_trust(&c,query).await;self.finish(&c,result,false).await}
    pub async fn prepare_web_catalog(&self,input:WebCatalogPrepare)->Result<PreparedWebCatalog,NativeError>{
        input.validate().map_err(store_error)?;let _guard=self.owner.0.management.mutations.lock().await;
        let(c,s)=self.authorized().await?;s.web_catalog_access(&c,true).map_err(store_error)?;
        let detail=s.web_catalog(&c,WebCatalogQuery{resource_id:input.resource_id.clone(),after:None}).await.map_err(store_error)?;
        if detail.revision!=input.expected_revision{return Err(failure(409,"management_revision_conflict"))}
        if matches!(input.change,WebCatalogChange::Retire{})&&!detail.can_retire{return Err(failure(409,"management_resource_in_use"))}
        let mut random=[0u8;16];getrandom::fill(&mut random).map_err(|_|failure(503,"native_random_unavailable"))?;
        let command=WebCatalogCommand{command_id:format!("webcatalog_{}",random.iter().map(|v|format!("{v:02x}")).collect::<String>()),resource_id:input.resource_id,expected_revision:input.expected_revision,change:input.change};
        let intent=Intent::new(IntentOperation::WebCatalog,command.command_id.clone(),&command).map_err(store_error)?;
        let result=s.prepare_intent(&c,intent).await;self.finish(&c,result,true).await?;
        Ok(PreparedWebCatalog{command_id:command.command_id,resource_id:command.resource_id,expected_revision:command.expected_revision,change:command.change,result:None,error:None})
    }
    async fn saved_web_catalog(&self,id:String)->Result<(PendingIntent,WebCatalogCommand),NativeError>{
        super::key(&id)?;let saved=self.saved_commands().await?.into_iter().find(|s|s.command_id==id).ok_or_else(||failure(404,"management_saved_command_not_found"))?;let command=decode(&saved)?;
        let(c,s)=self.authorized().await?;let access=s.web_catalog_access(&c,true);self.finish(&c,access,false).await?;Ok((saved,command))
    }
    pub async fn review_web_catalog_command(&self,id:String)->Result<PreparedWebCatalog,NativeError>{
        let(saved,command)=self.saved_web_catalog(id).await?;
        let(result,error)=match saved.reply {Some(IntentReply::Success(v))=>{let result:WebCatalogResult=serde_json::from_value(v).map_err(|_|failure(503,"management_saved_result_invalid"))?;validate_result(&command,&result)?;(Some(result),None)},Some(IntentReply::Failure{code,..})=>(None,Some(code)),None=>(None,None)};
        Ok(PreparedWebCatalog{command_id:command.command_id,resource_id:command.resource_id,expected_revision:command.expected_revision,change:command.change,result,error})
    }
    pub async fn cancel_saved_web_catalog(&self,id:String)->Result<PreparedWebCatalog,NativeError>{
        let _mutation=self.owner.0.management.mutations.lock().await;
        let(saved,command)=self.saved_web_catalog(id.clone()).await?;
        if saved.reply.is_some(){return self.review_web_catalog_command(id).await}
        let(context,session)=self.authorized().await?;
        let original=session.cancel_web_catalog(&context,command.clone()).await;
        let original=self.finish(&context,original,true).await?;
        let reply=match original {
            Some(result)=>{validate_result(&command,&result)?;IntentReply::Success(serde_json::to_value(result).map_err(|_|failure(503,"management_saved_result_invalid"))?)},
            None=>IntentReply::Failure{status:409,code:"management_change_cancelled".into()},
        };
        let intent=Intent::new(IntentOperation::WebCatalog,command.command_id.clone(),&command).map_err(store_error)?;
        let recorded=session.record_intent_reply(&context,intent,reply).await;
        self.finish(&context,recorded,true).await?;
        self.review_web_catalog_command(id).await
    }
    pub async fn apply_saved_web_catalog(&self,id:String)->Result<WebCatalogResult,NativeError>{
        let(_,command)=self.saved_web_catalog(id).await?;let intent=Intent::new(IntentOperation::WebCatalog,command.command_id.clone(),&command).map_err(store_error)?;
        let result=self.with_intent(intent,async{let(c,s)=self.authorized().await?;let result=s.apply_web_catalog(&c,command.clone()).await;self.finish(&c,result,true).await}).await?;
        validate_result(&command,&result)?;Ok(result)
    }
}

#[cfg(all(test,feature="resource-adoption"))]
#[path="web_catalog_tests.rs"]
mod tests;
