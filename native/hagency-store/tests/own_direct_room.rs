mod common;
use common::*;
use hagency_core::{replies::*, tasks::*};
use hagency_store::{DomainRepository, EffectOutcome, Error};
use std::collections::BTreeSet;

/// One provisioned agent with an observed transport and room; returns the
/// repository, its engagement and the observed room.
fn agent(
    direct: bool,
) -> (
    tempfile::TempDir,
    DomainRepository,
    String,
    MatrixRoomObservation,
) {
    let temp = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&temp.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    let pool = resource("pool", "seat", 1000);
    db.put_resource(&pool).unwrap();
    let p = proof(&request("one", "Worker", &pool, 100));
    let e = db.admit(&p, 1000).unwrap();
    db.approve("approve", &p, 1000).unwrap();
    let effect = db.claim_effect().unwrap().unwrap();
    db.observe_effect(
        &effect.id,
        effect.fence,
        &EffectOutcome::Applied {
            receipt: "fixture provision".into(),
        },
    )
    .unwrap();
    db.observe_matrix_transport(
        &MatrixTransportObservation {
            engagement_id: e.id.clone(),
            registration_generation: 1,
            generation: 1,
            sender_mxid: "@worker:example.test".into(),
            device_id: "DEVICE_1".into(),
        },
        5000,
    )
    .unwrap();
    let room = MatrixRoomObservation {
        engagement_id: e.id.clone(),
        registration_generation: 1,
        transport_generation: 1,
        room_id: if direct {
            "!direct:example.test"
        } else {
            "!project:example.test"
        }
        .into(),
        generation: 1,
        privacy: if direct {
            RoomPrivacy::Direct {
                human_mxid: "@owner:example.test".into(),
            }
        } else {
            RoomPrivacy::Group {}
        },
        joined: BTreeSet::from(["@worker:example.test".into(), "@owner:example.test".into()]),
        invite_only: true,
        encrypted: true,
    };
    db.observe_matrix_room(&room, 5001).unwrap();
    (temp, db, e.id, room)
}
fn cutoff(temp: &tempfile::TempDir, session: &str) -> u64 {
    rusqlite::Connection::open(temp.path().join("state/domain.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT ingress_since FROM matrix_session_routes WHERE session_id=?1",
            [session],
            |r| r.get(0),
        )
        .unwrap()
}
fn resolve(db: &mut DomainRepository, engagement: &str, room: &str, id: &str) {
    db.resolve_verified_matrix_session(
        &SessionBinding {
            id: id.into(),
            engagement_id: engagement.into(),
            room_id: room.into(),
            thread_root: None,
        },
        5002,
    )
    .unwrap();
}

/// The agent's own DM is visible to it from the room's creation (ADR-184):
/// the owner can write as soon as they join, before the agent is first
/// observed, and that message must not be cut off as history. Both orders
/// hold: the creation time recorded before or after the route exists.
#[test]
fn native_own_direct_room_cutoff_is_its_creation() {
    // Route first, creation time second: the existing route moves earlier.
    let (temp, mut db, engagement, room) = agent(true);
    resolve(&mut db, &engagement, &room.room_id, "dm");
    assert_eq!(cutoff(&temp, "dm"), 5001);
    db.own_direct_room_created(&engagement, &room.room_id, 4000, 6000)
        .unwrap();
    assert_eq!(cutoff(&temp, "dm"), 4000);
    // Never later: a newer creation time does not move the cutoff forward.
    db.own_direct_room_created(&engagement, &room.room_id, 4500, 6000)
        .unwrap();
    assert_eq!(cutoff(&temp, "dm"), 4000);

    // Creation time first, route second: the new route starts at creation.
    let (temp, mut db, engagement, room) = agent(true);
    db.own_direct_room_created(&engagement, &room.room_id, 4000, 6000)
        .unwrap();
    resolve(&mut db, &engagement, &room.room_id, "dm");
    assert_eq!(cutoff(&temp, "dm"), 4000);
}

/// Only the agent's own direct room is backdated; a group room refuses and a
/// creation time in the future is invalid.
#[test]
fn native_own_direct_room_refuses_other_rooms() {
    let (_temp, mut db, engagement, room) = agent(false);
    assert!(matches!(
        db.own_direct_room_created(&engagement, &room.room_id, 4000, 6000),
        Err(Error::RunnerAuthority)
    ));
    let (_temp, mut db, engagement, room) = agent(true);
    assert!(matches!(
        db.own_direct_room_created(&engagement, &room.room_id, 7000, 6000),
        Err(Error::Invalid(_))
    ));
}
