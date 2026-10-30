//! Closed v3 issuance routes; historical result/reply routes retain v1 shapes.
use super::*;
use serde_json::{Value, json};
pub(super) enum WorkAction {
    Renew(SharedActivityRenewInput),
    Poll(SharedActivityRef),
    Ack(SharedWorkInput),
    Start(SharedWorkInput),
    Context(Box<context_io::ContextAction>),
    Binding { work: String, input: SharedWorkEvidenceInput },
    Refuse { work: String, input: SharedBudgetWorkRefusalInput },
}
impl WorkAction {
    pub(super) fn validate(&self) -> Result<(), NativeError> {
        match self {
            Self::Renew(v) => { v.authority.validate().map_err(store)?; super::super::key(&v.renewal_id)?; }
            Self::Poll(v) => v.validate().map_err(store)?,
            Self::Ack(v) | Self::Start(v) => { v.authority.validate().map_err(store)?; super::super::key(&v.work_id)?; hash(&v.work_digest)?; }
            Self::Context(v) => v.validate()?,
            Self::Binding {work,input} => {super::super::key(work)?;hash(&input.runtime_command_digest)?;hash(&input.work_digest)?;}
            Self::Refuse {work,input} => {super::super::key(work)?;input.validate().map_err(store)?;}
        }
        Ok(())
    }
    pub(super) fn body(self) -> (String,Value) {
        match self {
            Self::Renew(input) => ("budget-activity/renew".into(),json!(input)),
            Self::Poll(authority) => ("budget-work/poll".into(),json!({"authority":authority})),
            Self::Ack(input) => ("budget-work/ack".into(),json!(input)),
            Self::Start(input) => ("budget-work/start".into(),json!(input)),
            Self::Binding {work,input} => (format!("budget-work/{work}/demand"),json!(input)),
            Self::Refuse {work,input} => (format!("budget-work/{work}/refuse-unissued"),json!(input)),
            Self::Context(action) => {
                let fresh=matches!(*action,context_io::ContextAction::Mode{..}|context_io::ContextAction::Freeze{..}|context_io::ContextAction::Start{..});
                let (path,body)=action.body();
                (if fresh {format!("budget-{path}")}else{path},body)
            }
        }
    }
}
impl From<WorkAction> for Action {
    fn from(v:WorkAction)->Self { Self::BudgetWork(Box::new(v)) }
}

#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct RefusalReply { refusal: Option<SharedBudgetWorkRefusalRecord> }

/// After an uncertain pre-issue failure, ask for a canonical never-started
/// fence. Failure leaves the original hold; no error is interpreted as zero use.
pub(super) async fn refuse_unissued(
    io:&Io,r:&OwnerResourceRegistry,c:&SharedBudgetAttemptCommand,w:&SharedWorkItem,input:Option<&SharedPreparedInput>,
)->Result<Option<SharedWorkReceipt>,NativeError>{
    let b=io.bound().await?;
    let sent=Instant::now();
    let response:Timed<RefusalReply>=b.request(WorkAction::Refuse{
        work:w.id.clone(),input:SharedBudgetWorkRefusalInput{
            runtime_command_digest:digest(c)?,work_digest:digest(w)?,input_digest:if w.encrypted {w.body_digest.clone()} else {input.map_or_else(||w.body_digest.clone(),SharedPreparedInput::input_digest)},
        },
    }.into(),false).await?;
    let Some(record)=response.value.refusal else{return Ok(None)};
    let e=b.enrollment(r,&c.original_runtime_command,false).await?;
    let proof=VerifiedSharedBudgetWorkRefusal::new(e,c.clone(),w.clone(),record,(sent+Duration::from_secs(30)).min(b.scope.deadline)).map_err(store)?;
    let receipt=r.refuse_unissued_shared_budget_work(proof).await.map_err(store)?;
    b.transport.check_scope(&b.scope,false).await?;
    Ok(Some(receipt))
}
