//! One-shot consumption before response writes; no durable cross-session grants.
use super::*;
use hagency_runtime::{codex::{approval::ApprovalRequest,session::{ApprovalControlPolicy,PreparedUpdate,ObservationKind}},owned::{OwnedSession}};
use serde_json::{Value,json};
struct Pending {request:ApprovalRequest,id:String,digest:String,deadline:tokio::time::Instant}
#[derive(Default)]
pub(super) struct Approvals {pending:Vec<Pending>}
pub(super) fn enable(process:&mut OwnedSession)->Result<(),NativeError>{
 process.enable_approval_control(ApprovalControlPolicy{owner_wait_ms:120_000,response_reserve_ms:5_000}).map_err(|_|invalid())?;
 process.enable_owner_wait_expiry().map_err(|_|invalid())
}
impl Approvals {
 pub async fn register(&mut self,b:&Bound,work:&SharedWorkItem,a:&SharedActivityRef,until:Instant,process:&OwnedSession,request:ApprovalRequest)->Result<(),NativeError>{
  if self.pending.len()>=16 || self.pending.iter().any(|p|p.request.id()==request.id()){return Err(invalid());}
  let deadline=process.approval_deadline(request.id()).map_err(|_|invalid())?;
  let remaining=deadline.saturating_duration_since(tokio::time::Instant::now()).as_millis().min(120_000) as u64;
  let id=format!("sap_{}",super::digest(&json!({"work":work.id,"request":request.id()}))?);
  let request_json=json!({"id":request.id(),"method":request.method(),"params":request.params(),"sessionAllowed":request.session_response().is_ok(),"managerCanDecide":within_workspace(request.method(),request.params(),process.shared_workspace())});
  let reply:Timed<Value>=b.upload(Action::ApprovalRegister{authority:a.clone(),work:work.clone(),id:id.clone(),request:request_json.clone(),remaining_ms:remaining},until).await?;
  if reply.value["approval"]["id"]!=id || reply.value["approval"]["workId"]!=work.id || reply.value["approval"]["request"]!=request_json {return Err(invalid());}
  let digest=reply.value["digest"].as_str().ok_or_else(invalid)?.to_owned();hash(&digest)?;
  if super::digest(&reply.value["approval"])?!=digest {return Err(invalid());}
  self.pending.push(Pending{request,id,digest,deadline});Ok(())
 }
 pub async fn poll(&mut self,b:&Bound,work:&SharedWorkItem,a:&SharedActivityRef,until:Instant,process:&mut OwnedSession,capture:&mut SharedCallSink)->Result<Option<SharedWorkReceipt>,NativeError>{
  let mut i=0;
  while i<self.pending.len() {
   let p=&self.pending[i];
   let decision=if tokio::time::Instant::now()>=p.deadline {
    process.expire_approval(p.request.id()).map_err(|_|invalid())?;Some("deny".to_owned())
   }else{
    let reply:Timed<Value>=b.upload(Action::ApprovalTake{authority:a.clone(),id:p.id.clone(),digest:p.digest.clone()},until).await?;
    if reply.value["id"]!=p.id || reply.value["digest"]!=p.digest{return Err(invalid());}
    if matches!(reply.value["decision"].as_str(),Some("approve_once"|"approve_session")) && reply.value["ownerApproved"]!=true && !within_workspace(p.request.method(),p.request.params(),process.shared_workspace()) {Some("deny".into())}else{reply.value["decision"].as_str().map(str::to_owned)}
   };
   let Some(decision)=decision else {i+=1;continue};
   let p=self.pending.remove(i);
   let response=match decision.as_str(){"approve_once"=>p.request.response(true),"approve_session"=>p.request.session_response().map_err(|_|invalid())?,"deny"=>p.request.response(false),_=>return Err(invalid())};
   // A consumed server decision cannot be replayed after uncertain IO or restart.
   let mut prepared=process.prepare_approval(response).map_err(|_|invalid())?;
   loop {match process.send_prepared_approval(&mut prepared).await.map_err(|_|invalid())? {
    PreparedUpdate::WriteAccepted(_)=>break,
    PreparedUpdate::Update(update,observation)=>{
     let event=process.shared_event(update,*observation).map_err(|_|invalid())?;
     let next=event.approval().cloned();let ended=matches!(event.observation().kind(),ObservationKind::TurnEnded(_));
     let receipt=capture.observe(event).await.map_err(store)?;
     if ended{return Ok(Some(receipt.ok_or_else(invalid)?));}
     if let Some(next)=next {self.register(b,work,a,until,process,next).await?;}
    }
   }}
  }
  Ok(None)
 }
}

// Command approvals can execute outside the sandbox, so only the owner may
// grant them. Delegation is limited to literal, canonical workspace file paths.
fn within_workspace(method:&str,params:&Value,workspace:&std::path::Path)->bool {
 let Ok(root)=workspace.canonicalize()else{return false};
 let inside=|value:&Value|->bool {
  let Some(raw)=value.as_str()else{return false};let path=std::path::Path::new(raw);
  if !path.is_absolute() || !path.starts_with(&root) || path.components().any(|c|matches!(c,std::path::Component::ParentDir|std::path::Component::CurDir)){return false;}
  let mut existing=path;loop {match std::fs::symlink_metadata(existing) {
   Ok(_)=>return existing.canonicalize().is_ok_and(|p|p.starts_with(&root)),
   Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{let Some(parent)=existing.parent()else{return false};existing=parent;},
   Err(_)=>return false,
  }}
 };
 if method=="item/fileChange/requestApproval" {return inside(&params["grantRoot"]);}
 if method!="item/permissions/requestApproval" {return false;}
 let profile=&params["permissions"];
 if profile["network"]["enabled"]==true {return false;}
 let Some(fs)=profile["fileSystem"].as_object()else{return false};
 if fs.get("globScanMaxDepth").is_some_and(|v|!v.is_null()){return false;}
 let mut count=0;
 for key in ["read","write"] {if let Some(paths)=fs.get(key).filter(|v|!v.is_null()) {let Some(paths)=paths.as_array()else{return false};for path in paths {if !inside(path){return false;}count+=1;}}}
 if let Some(entries)=fs.get("entries").filter(|v|!v.is_null()) {let Some(entries)=entries.as_array()else{return false};for e in entries {if !matches!(e["access"].as_str(),Some("read"|"write"))||e["path"]["type"]!="path"||!inside(&e["path"]["path"]){return false;}count+=1;}}
 count>0
}
#[cfg(test)]
mod tests {
 use super::*;
 #[test]fn workspace_delegation_rejects_network_commands_traversal_and_symlink_escape(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let p=root.join("new.txt");
  let mut params=json!({"permissions":{"fileSystem":{"write":[p]}}});
  assert!(within_workspace("item/permissions/requestApproval",&params,&root));
  params["permissions"]["network"]=json!({"enabled":true});assert!(!within_workspace("item/permissions/requestApproval",&params,&root));
  assert!(!within_workspace("item/commandExecution/requestApproval",&json!({"cwd":root,"command":"anything"}),&root));
  assert!(!within_workspace("item/fileChange/requestApproval",&json!({"grantRoot":root.join("../escape")}),&root));
  #[cfg(unix)] {std::os::unix::fs::symlink("/",root.join("escape")).unwrap();assert!(!within_workspace("item/fileChange/requestApproval",&json!({"grantRoot":root.join("escape/tmp")}),&root));}
 }
}
