mod common;
use common::*;
use hagency_core::tasks::*;
use hagency_store::{DomainRepository, EffectOutcome, ThreadDirective, ThreadMode, confirmation};
use serde_json::json;

fn setup() -> (tempfile::TempDir, DomainRepository) {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    (dir, db)
}

/// Fully provision an agent (admit → approve → apply the provision effect) and
/// register a host session, mirroring `domain.rs`'s dispatch fixture.
fn provision(db: &mut DomainRepository, name: &str, at: u64) -> String {
    let pool = resource("pool", "seat", 1000);
    db.put_resource(&pool).unwrap();
    let req = request("one", name, &pool, 100);
    let proof = proof(&req);
    db.admit(&proof, at).unwrap();
    db.approve("approve_one", &proof, at).unwrap();
    let effect = db.claim_effect().unwrap().unwrap();
    db.observe_effect(
        &effect.id,
        effect.fence,
        &EffectOutcome::Applied {
            receipt: "fixture account".into(),
        },
    )
    .unwrap();
    let engagement = proof.request().engagement_id().unwrap();
    db.register_session(&SessionBinding {
        id: "session".into(),
        engagement_id: engagement.clone(),
        room_id: "!project:example.test".into(),
        thread_root: None,
    })
    .unwrap();
    db.create_canonical_task("task", "session", "Directive work", at)
        .unwrap();
    db.register_workspace("workspace").unwrap();
    engagement
}

/// TS case 1: "operator overrides persist on the session and survive
/// re-resolution" — `setSessionOverrides` then `sessionById`/`resolveSession`
/// both report the override. Native: `set_session_overrides` persists and
/// `session_overrides` re-reads it; a `default` verb clears it.
#[test]
fn native_thread_directive_session_override_persists_and_clears() {
    let (_dir, mut db) = setup();
    provision(&mut db, "Worker", 1000);

    let applied = db
        .set_session_overrides(
            "session",
            &ThreadDirective::Model(Some("claude-sonnet-5".into())),
        )
        .unwrap();
    assert_eq!(applied.model.as_deref(), Some("claude-sonnet-5"));
    assert_eq!(applied.mode, None);

    let reread = db.session_overrides("session").unwrap();
    assert_eq!(reread.model.as_deref(), Some("claude-sonnet-5"));
    assert_eq!(reread.mode, None);

    // A mode grant on the same session keeps the model, sets the mode.
    let granted = db
        .set_session_overrides("session", &ThreadDirective::Mode(Some(ThreadMode::Auto)))
        .unwrap();
    assert_eq!(granted.model.as_deref(), Some("claude-sonnet-5"));
    assert_eq!(granted.mode, Some(ThreadMode::Auto));

    // `default` clears the model override but keeps the mode.
    let cleared = db
        .set_session_overrides("session", &ThreadDirective::Model(None))
        .unwrap();
    assert_eq!(cleared.model, None);
    assert_eq!(cleared.mode, Some(ThreadMode::Auto));
}

/// TS case 4: "the launch descriptor carries the session model override" —
/// `getLaunchDescriptor` reports `modelOverride` after `setSessionOverrides`.
/// Native: `owned_dispatch_scope` reads the session override into the resource
/// the host uses to build the Codex `Settings`.
#[test]
fn native_thread_directive_model_override_reaches_launch_scope() {
    let (_dir, mut db) = setup();
    provision(&mut db, "Worker", 1000);

    db.set_session_overrides(
        "session",
        &ThreadDirective::Model(Some("claude-haiku-4-5".into())),
    )
    .unwrap();

    db.enqueue_dispatch(&DispatchInput {
        id: "dispatch".into(),
        session_id: "session".into(),
        task_id: Some("task".into()),
        resources: vec![ResourceLease {
            id: "workspace".into(),
            exclusive: true,
        }],
        payload: json!({}),
    })
    .unwrap();
    let cap = db
        .claim_dispatch("runner", 1002, 60000, 120000, 128)
        .unwrap()
        .unwrap();
    let scope = db.owned_dispatch_scope(&cap, 1003).unwrap();
    // The provisioned model is `gpt-5.6-sol` (common::resource); the override
    // replaces it on the launch descriptor.
    assert_eq!(scope.resource().model, "claude-haiku-4-5");
}

/// The confirmation text (TS case 5's notice body) is the retained product's
/// words, asserted here through the public `confirmation` re-export.
#[test]
fn native_thread_directive_confirmation_is_retained_text() {
    assert_eq!(
        confirmation(Some("claude-sonnet-5"), None),
        "Thread session updated: model=claude-sonnet-5, mode=default (read-only)."
    );
    assert_eq!(
        confirmation(None, Some(ThreadMode::Auto)),
        "Thread session updated: model=default, mode=auto. Runners in this thread may now write to the agent workspace (writes stay serialized by workspace lease)."
    );
}
