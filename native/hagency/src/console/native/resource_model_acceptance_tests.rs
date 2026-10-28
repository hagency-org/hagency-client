//! Real native account/model/configuration APIs with an offline metadata-only CLI.
//! The fixture rejects login, inference, execution and credential export calls.
use super::*;
use crate::console::native::{ManagementPackage,initialize_resource_activation_fixture,reconciliation_sources::tests::Fixture,AccountModelQuery,NamespaceEnrollment};
use hagency_store::management::{PrepareAccount,AccountInventoryQuery,ResourceDetailQuery};
use std::{path::{Path,PathBuf},time::Duration};
fn package()->ManagementPackage{ManagementPackage{digest:"a".repeat(64),instance:1,account_epoch:1,permissions:vec![Permission::ReadResources,Permission::ConfigureResources,Permission::PrepareSourceQuiescence,Permission::ReadIntents,Permission::ReadAccounts,Permission::PrepareAccounts,Permission::ReadAccountModels,Permission::EnrollResources]}}
fn until()->Instant{Instant::now()+Duration::from_secs(30)}
fn find(parent:&Path,name:&str)->Option<PathBuf>{for entry in std::fs::read_dir(parent).ok()?.flatten(){let p=entry.path();if p.file_name().is_some_and(|n|n==name){return Some(p)}if p.is_dir(){if let Some(p)=find(&p,name){return Some(p)}}}None}
fn executable(f:&Fixture)->PathBuf{
    use std::os::unix::fs::PermissionsExt;
    let path=f.temp.path().join("offline-ready-model");
    std::fs::write(&path,r#"#!/usr/bin/env python3
import json,os,sys,pathlib
home=pathlib.Path(os.environ['CODEX_HOME']); assert home==pathlib.Path.cwd()
for line in sys.stdin:
 r=json.loads(line);i=r.get('id');m=r.get('method')
 if i is None:continue
 with (home/'fixture-methods').open('a') as f:f.write(m+'\n')
 if m=='initialize':v={}
 elif m=='config/read':v={'config':{'model_provider':'openai','chatgpt_base_url':'https://chatgpt.com/backend-api'}}
 elif m=='account/read':v={'requiresOpenaiAuth':True,'account':{'type':'chatgpt','email':'fixture-private@example.test','planType':'plus'}}
 elif m=='account/rateLimits/read':v={'accountId':'fixture-quota-identity','rateLimits':{}}
 elif m=='model/list':v={'data':([] if (home/'fixture-withheld').exists() else [{'hidden':False,'id':'gpt-5.6-sol','model':'gpt-5.6-sol','displayName':'Offline verified model','description':'Metadata-only acceptance fixture','isDefault':True,'defaultReasoningEffort':'high','supportedReasoningEfforts':[{'reasoningEffort':'medium','description':'Medium'},{'reasoningEffort':'high','description':'High'}]}]),'nextCursor':None}
 else:raise Exception('Only read-only provider metadata is permitted')
 print(json.dumps({'id':i,'result':v}),flush=True)
"#).unwrap();
    std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o700)).unwrap();
    *f.owner.0.account_logins.executable.lock().unwrap()=Some(path.clone());path
}
#[test]
fn native_ready_model_configuration_applies_and_recovers_without_login_or_execution(){
    let result=std::process::Command::new(std::env::current_exe().unwrap()).args(["console::native::resource_edit::process_tests::native_ready_model_child","--exact","--nocapture"]).env("HAGENCY_READY_MODEL_FIXTURE","1").output().unwrap();
    assert!(result.status.success(),"{} {}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
}
#[tokio::test]
async fn native_ready_model_child(){
    if std::env::var("HAGENCY_READY_MODEL_FIXTURE").as_deref()!=Ok("1"){return}
    let installation=tempfile::tempdir().unwrap();initialize_resource_activation_fixture(&installation.path().join("installation")).unwrap();
    let f=Fixture::with_configured_installation().await;let(db,path,_)=f.domain();drop(db);
    let db=rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();db.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,true).unwrap();db.execute("DELETE FROM resources",[]).unwrap();drop(db);
    let review=f.owner.review_device_setup(package(),None,None).await.unwrap();let selected=f.owner.confirm_device_setup(review,package(),||true).await.unwrap();
    let mut f=f.restart_onboarding(selected.clone()).await;let fake=executable(&f);let m=f.owner.management(package()).await.unwrap();
    let account=m.prepare_account(PrepareAccount{command_id:"ready_account".into()}).await.unwrap();
    let row=m.account_inventory(AccountInventoryQuery{id:Some(account.account_id.clone()),after:None}).await.unwrap().account.unwrap();
    let query=AccountModelQuery{account_id:account.account_id.clone(),expected_revision:row.revision.unwrap()};
    let models=m.account_models(query.clone()).await.unwrap();assert_eq!(models["enrollmentReady"],true);assert_eq!(models["provider"]["connected"],true);assert!(!models.to_string().contains("fixture-private"));
    let resource=m.enroll_account_resource(NamespaceEnrollment{command_id:"ready_resource".into(),account:query,model:"gpt-5.6-sol".into(),reasoning:Some("high".into()),monthly_tokens:700},until()).await.unwrap();
    let id=resource.resource_id;let namespace=find(installation.path(),&account.account_id).unwrap();
    let detail=m.resource_detail(ResourceDetailQuery{id:id.clone()}).await.unwrap();assert!(detail.editing.allowed);
    let input=EditLocalResource{command_id:"ready_model_apply".into(),resource_id:id.clone(),expected_revision:detail.resource.revision,profile:hagency_store::ProfileChange::Select{model:"gpt-5.6-sol".into(),reasoning:Some("medium".into())},ceiling:hagency_store::CeilingChange::Preserve{},execution_policy:None};
    let copy=||serde_json::from_value::<EditLocalResource>(serde_json::to_value(&input).unwrap()).unwrap();
    assert!(matches!(m.review_local_resource_edit(&input).await.unwrap(),LocalResourceEditReview::Editable(_)));
    let applied=m.edit_local_resource(copy(),until()).await.unwrap();let changed=m.resource_detail(ResourceDetailQuery{id:id.clone()}).await.unwrap();
    assert_eq!(changed.resource.reasoning.as_deref(),Some("medium"));assert_eq!(changed.resource.budget.seat.quota.map(u64::from),Some(700));
    std::fs::write(namespace.join("fixture-withheld"),"").unwrap();
    let unavailable=EditLocalResource{command_id:"ready_model_withheld".into(),resource_id:id.clone(),expected_revision:applied.revision.clone(),profile:hagency_store::ProfileChange::Select{model:"gpt-5.6-sol".into(),reasoning:Some("high".into())},ceiling:hagency_store::CeilingChange::Preserve{},execution_policy:None};
    assert_eq!(m.edit_local_resource(unavailable,until()).await.unwrap_err().code,"provider_model_changed");assert_eq!(m.resource_detail(ResourceDetailQuery{id:id.clone()}).await.unwrap().resource.revision,applied.revision);
    let calls=std::fs::read_to_string(namespace.join("fixture-methods")).unwrap();assert!(calls.lines().all(|m|matches!(m,"initialize"|"config/read"|"account/read"|"account/rateLimits/read"|"model/list")));
    drop(m);f=f.restart_onboarding(selected).await;std::fs::remove_file(fake).unwrap();let m=f.owner.management(package()).await.unwrap();
    assert!(matches!(m.review_local_resource_edit(&input).await.unwrap(),LocalResourceEditReview::Recorded(_)));
    assert_eq!(m.edit_local_resource(copy(),until()).await.unwrap().revision,applied.revision);
    assert_eq!(std::fs::read_to_string(namespace.join("fixture-methods")).unwrap(),calls,"original result recovery does not query provider again");
    assert_eq!(m.resource_detail(ResourceDetailQuery{id}).await.unwrap().resource.reasoning.as_deref(),Some("medium"));
    drop(m);f.stop().await;
}
