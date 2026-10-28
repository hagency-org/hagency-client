//! Store-level tests for the project-side credential/lifecycle module
//! (board #14), TS parity with `lib/project-side-store.js:509-560` and
//! `backend-v2.js` routes. The store owns every guarantee; these tests prove
//! the staged-vs-live split, the verdict, the representative server rule, the
//! project room-uniqueness rule and the active-side removal refusal.
mod common;
use hagency_store::DomainRepository;
use serde_json::json;

fn side_id() -> &'static str {
    "example.test"
}

fn appservice() -> serde_json::Value {
    json!({
        "kind": "appservice",
        "asToken": "as_token_abc123",
        "hsToken": "hs_token_def456",
        "namespace": "_ac_.*",
        "senderLocalpart": "hagency",
        "url": "http://127.0.0.1:8008",
    })
}

/// A side is created idempotently, and its projection never carries the
/// credential value (TS `publicSide` allow-list).
#[test]
fn side_record_projection_omits_credential_bytes() {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.ensure_side(side_id()).unwrap();
    let side = db.side(side_id()).unwrap().expect("side exists");
    assert_eq!(side.id, "example.test");
    assert!(!side.has_credential);
    assert_eq!(side.credential_kind, None);
    assert_eq!(side.access_state, "unverified");
    assert!(side.active);

    db.set_credential(side_id(), Some(appservice()), false)
        .unwrap();
    let side = db.side(side_id()).unwrap().expect("side exists");
    assert!(side.has_credential);
    assert_eq!(side.credential_kind.as_deref(), Some("appservice"));
    assert_eq!(side.sender_localpart.as_deref(), Some("hagency"));
    let text = serde_json::to_string(&side).unwrap();
    assert!(
        !text.contains("as_token_abc123"),
        "no asToken value in projection"
    );
    assert!(
        !text.contains("hs_token_def456"),
        "no hsToken value in projection"
    );
    // The value IS readable by the one caller that talks to the homeserver.
    let credential = db.credential_for(side_id()).unwrap().expect("credential");
    assert_eq!(credential.as_token.as_deref(), Some("as_token_abc123"));
}

/// TS `setCredential(..., {stage:true})` keeps the old live credential and
/// writes a pending one WITHOUT touching the access verdict; the old
/// credential keeps working until a `verify` proves the new one (TS :509-560).
#[test]
fn staged_replacement_keeps_old_credential_until_promoted() {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.ensure_side(side_id()).unwrap();
    // A live credential, verified accepted.
    db.set_credential(
        side_id(),
        Some(json!({
            "kind": "appservice",
            "asToken": "old_token",
            "hsToken": "old_hs",
            "namespace": "_ac_.*",
            "senderLocalpart": "hagency",
        })),
        false,
    )
    .unwrap();
    db.observe_access(side_id(), "accepted", None).unwrap();

    // Stage a replacement: the live credential and the verdict are untouched.
    let new_credential = json!({
        "kind": "appservice",
        "asToken": "new_token",
        "hsToken": "new_hs",
        "namespace": "_ac_.*",
        "senderLocalpart": "hagency",
    });
    let side = db
        .set_credential(side_id(), Some(new_credential.clone()), true)
        .unwrap()
        .expect("side exists");
    assert!(side.awaiting_install, "staged credential is visible state");
    assert_eq!(
        side.access_state, "accepted",
        "staging does not touch the verdict"
    );
    assert_eq!(
        db.credential_for(side_id())
            .unwrap()
            .unwrap()
            .as_token
            .as_deref(),
        Some("old_token"),
        "the old credential is still live"
    );
    assert_eq!(
        db.pending_credential_for(side_id())
            .unwrap()
            .unwrap()
            .as_token
            .as_deref(),
        Some("new_token"),
        "the new credential waits as pending"
    );

    // A verify that proves the new credential promotes it.
    db.promote_pending_credential(side_id()).unwrap();
    assert_eq!(
        db.credential_for(side_id())
            .unwrap()
            .unwrap()
            .as_token
            .as_deref(),
        Some("new_token"),
        "promotion makes the staged credential live"
    );
    assert!(
        db.pending_credential_for(side_id()).unwrap().is_none(),
        "pending is cleared on promotion"
    );
}

/// A non-stage `set_credential` replaces live, clears pending, and resets the
/// verdict to unverified (TS :515-520).
#[test]
fn non_stage_set_credential_resets_the_verdict() {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.ensure_side(side_id()).unwrap();
    db.set_credential(side_id(), Some(appservice()), false)
        .unwrap();
    db.observe_access(side_id(), "accepted", None).unwrap();
    assert_eq!(
        db.side(side_id()).unwrap().unwrap().access_state,
        "accepted"
    );

    let replacement = json!({
        "kind": "appservice",
        "asToken": "rotated",
        "hsToken": "rotated_hs",
        "namespace": "_ac_.*",
        "senderLocalpart": "hagency",
    });
    let side = db
        .set_credential(side_id(), Some(replacement), false)
        .unwrap()
        .unwrap();
    assert_eq!(
        side.access_state, "unverified",
        "a new credential resets the verdict"
    );
    assert!(!side.awaiting_install);
    // Withdrawing clears the credential.
    let side = db.set_credential(side_id(), None, false).unwrap().unwrap();
    assert!(!side.has_credential);
    assert!(db.credential_for(side_id()).unwrap().is_none());
}

/// The representative mxid must live on the side's server (TS `setRepresentative`).
#[test]
fn representative_mxid_must_live_on_the_side_server() {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.ensure_side(side_id()).unwrap();
    // Correct host.
    db.set_representative(side_id(), "@rep:example.test")
        .unwrap();
    assert_eq!(
        db.side(side_id())
            .unwrap()
            .unwrap()
            .representative
            .unwrap()
            .mxid,
        "@rep:example.test"
    );
    // Wrong host refused.
    assert!(
        db.set_representative(side_id(), "@rep:elsewhere.test")
            .is_err()
    );
}

/// A room may belong to one project only, and its server must be the side's.
#[test]
fn project_room_is_unique_and_must_live_on_the_side() {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.ensure_side(side_id()).unwrap();
    let project = db
        .upsert_project(
            side_id(),
            &json!({"name": "BigLittle", "roomId": "!room:example.test"}),
        )
        .unwrap()
        .expect("project");
    assert_eq!(project.id, "biglittle", "the id is derived from the name");
    assert_eq!(project.room_id.as_deref(), Some("!room:example.test"));

    // A second project claiming the same room is a conflict.
    assert!(matches!(
        db.upsert_project(
            side_id(),
            &json!({"name": "Other", "roomId": "!room:example.test"})
        ),
        Err(hagency_store::Error::Conflict)
    ));
    // A room on another server is refused.
    assert!(
        db.upsert_project(
            side_id(),
            &json!({"name": "Foreign", "roomId": "!room:elsewhere.test"})
        )
        .is_err()
    );
}

/// An active side refuses removal; deactivation first, then remove (TS :10584).
#[test]
fn active_side_refuses_removal() {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.ensure_side(side_id()).unwrap();
    assert!(matches!(
        db.remove_side(side_id(), false),
        Err(hagency_store::Error::State)
    ));
    db.deactivate_side(side_id()).unwrap();
    db.remove_side(side_id(), false).unwrap();
    assert!(db.side(side_id()).unwrap().is_none());
}
