//! Closed budget-aware demand endpoints. No generic URL or script authority.
use super::*;
use serde_json::{Value,json};
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub(super) struct RecoverReply {pub demand:Option<SharedBudgetDemandStatus>}
pub(super) enum BudgetDemandAction {
    Poll {after:Option<String>},
    Recover {id:String,digest:String},
    Block {id:String,input:Box<SharedBudgetDemandBlockInput>},
}
impl BudgetDemandAction {
    pub(super) fn validate(&self)->Result<(),NativeError>{
        match self {
            Self::Poll{after}=>{if let Some(id)=after{super::super::key(id)?}},
            Self::Recover{id,digest}=>{super::super::key(id)?;hash(digest)?;},
            Self::Block{id,input}=>{
                super::super::key(id)?;hash(&input.demand_digest)?;input.execution_budget.validate().map_err(store)?;
                if input.expected_control_revision==0||input.expected_control_revision>9_007_199_254_740_991||!matches!(input.reason.as_str(),"budget_blocked"|"usage_unrepresentable"){return Err(invalid())}
                if let Some(b)=&input.original_block{b.validate().map_err(store)?;}
            },
        }Ok(())
    }
    pub(super) fn body(self)->(String,Value){match self {
        Self::Poll{after}=>("budget-demands/poll".into(),json!({"after":after})),
        Self::Recover{id,digest}=>(format!("budget-demands/{id}/recover"),json!({"demandDigest":digest})),
        Self::Block{id,input}=>(format!("budget-demands/{id}/block"),json!(input)),
    }}
}
impl From<BudgetDemandAction> for Action{fn from(a:BudgetDemandAction)->Self{Self::BudgetDemand(Box::new(a))}}
pub(super) async fn require_pair(b:&Bound)->Result<(),NativeError>{
    let d=b.demand_discovery().await?;
    if d.capabilities.len()>64||d.capabilities.iter().any(|c|c.len()>128){return Err(invalid())}
    if !d.capabilities.iter().any(|c|c=="shared-budget-execution-v1")||!d.capabilities.iter().any(|c|c=="shared-durable-demand-v1"){return Err(failure(409,"shared_budget_protocol_unavailable"))}
    Ok(())
}
