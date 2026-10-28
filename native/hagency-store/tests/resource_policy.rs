#[path = "common/mod.rs"]
mod common;
use hagency_core::{execution::ExecutionPolicy, project::Resource};
use hagency_store::*;
use serde_json::json;
use std::time::{Duration, Instant};

fn reserve(db: &mut DomainRepository, resource: &Resource, name: &str) -> Effect {
    let proof = common::proof(&common::request(name, name, resource, 100));
    db.admit(&proof, 1000).unwrap();
    let agent = db.approve(&format!("approve_{name}"), &proof, 1000).unwrap();
    db.claim_effect_for(&format!("provision_{}", agent.id)).unwrap().unwrap()
}
fn applied() -> EffectOutcome {
    EffectOutcome::Applied { receipt: "original fixture provision".into() }
}
fn set_default(db: &mut DomainRepository, id: &str, yolo: Option<bool>, create: bool) -> Resource {
    let resource = db.resource_configuration(id).unwrap();
    let access = ResourceConfigurationAccess::new(
        Instant::now() + Duration::from_secs(60), ResourcePublicationRetirement::default());
    let mut command = access.prepare(id.into(), resource_publication_revision(&resource).unwrap(),
        create, ProfileChange::Preserve {}, CeilingChange::Preserve {},
        Instant::now() + Duration::from_secs(30)).unwrap();
    if let Some(yolo) = yolo { command = command.with_execution_policy(ExecutionPolicy { yolo }); }
    let result = db.configure_resource(command).unwrap();
    db.resource_configuration(&result.resource_id).unwrap()
}
fn mode(db: &DomainRepository, effect: &Effect) -> bool {
    db.execution_policy(&effect.engagement_id).unwrap()["executionPolicy"]["yolo"].as_bool().unwrap()
}

#[test]
fn resource_default_only_applies_at_new_agent_creation_and_survives_restart() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let path = root.path().join("state");
    let mut db = DomainRepository::open(&path).unwrap();
    let resource = common::resource("pool", "seat", 5000);
    db.put_resource(&resource).unwrap();
    db.register(&common::registration()).unwrap();
    let old = reserve(&mut db, &resource, "old");
    db.observe_effect(&old.id, old.fence, &applied()).unwrap();
    assert!(!mode(&db, &old));

    let pending = reserve(&mut db, &resource, "pending");
    let explicit = reserve(&mut db, &resource, "explicit");
    db.set_execution_policy(&explicit.engagement_id, Some(&json!({"yolo":false})), 1001).unwrap();
    let configured = set_default(&mut db, &resource.id(), Some(true), false);
    assert_eq!(configured.execution_policy, Some(ExecutionPolicy { yolo: true }));
    assert_ne!(resource_publication_revision(&configured).unwrap(), resource_publication_revision(&resource).unwrap());
    // As in the original registration route, a still-new agent uses the
    // resource default at creation; prior explicit choices remain authoritative.
    assert!(!mode(&db, &pending));
    db.observe_effect(&pending.id, pending.fence, &applied()).unwrap();
    db.observe_effect(&explicit.id, explicit.fence, &applied()).unwrap();
    db.observe_effect(&old.id, old.fence, &applied()).unwrap();
    assert!(mode(&db, &pending));
    assert!(!mode(&db, &explicit));
    assert!(!mode(&db, &old));

    let preserved = set_default(&mut db, &resource.id(), None, false);
    assert_eq!(preserved.execution_policy, configured.execution_policy);
    let cloned = set_default(&mut db, &resource.id(), None, true);
    assert_eq!(cloned.execution_policy, configured.execution_policy);
    set_default(&mut db, &resource.id(), Some(false), false);
    db.observe_effect(&pending.id, pending.fence, &applied()).unwrap();
    assert!(mode(&db, &pending));
    drop(db);
    let db = DomainRepository::open(&path).unwrap();
    assert!(mode(&db, &pending));
    assert!(!mode(&db, &old));
    assert!(!mode(&db, &explicit));
    assert_eq!(db.resource_configuration(&resource.id()).unwrap().execution_policy, Some(ExecutionPolicy { yolo:false }));
}

#[test]
fn resource_policy_persistence_failure_rolls_back_provision_and_can_retry_original_effect() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let path = root.path().join("state");
    let mut db = DomainRepository::open(&path).unwrap();
    let mut resource = common::resource("pool", "seat", 5000);
    resource.execution_policy = Some(ExecutionPolicy { yolo:true });
    db.put_resource(&resource).unwrap();
    db.register(&common::registration()).unwrap();
    let effect = reserve(&mut db, &resource, "rollback");
    let sql = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    sql.execute_batch("CREATE TRIGGER refuse_policy BEFORE INSERT ON agent_execution_policies BEGIN SELECT RAISE(ABORT,'fixture policy persistence failure'); END;").unwrap();
    assert!(db.observe_effect(&effect.id, effect.fence, &applied()).is_err());
    assert_eq!(sql.query_row("SELECT state FROM engagements WHERE id=?1", [&effect.engagement_id], |r|r.get::<_,String>(0)).unwrap(), "reserved");
    assert_eq!(sql.query_row("SELECT state FROM effects WHERE id=?1", [&effect.id], |r|r.get::<_,String>(0)).unwrap(), "started");
    assert!(!mode(&db, &effect));
    sql.execute_batch("DROP TRIGGER refuse_policy;").unwrap();
    db.observe_effect(&effect.id, effect.fence, &applied()).unwrap();
    assert!(mode(&db, &effect));
}
