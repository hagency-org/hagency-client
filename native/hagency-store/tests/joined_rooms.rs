//! ADR-188: rooms an agent joined by invitation.
mod common;
use common::*;
use hagency_core::{ingress::*, messages::*, replies::*, tasks::*};
use hagency_store::{DomainRepository, EffectOutcome, Error, JoinedRoomState, MAX_JOINED_ROOMS};
use std::collections::BTreeSet;

const SIDE: &str = "!side:example.test";

struct Fixture {
    _root: tempfile::TempDir,
    db: DomainRepository,
    engagement: String,
}
impl Fixture {
    /// One agent `@a` with its project room observed and bound.
    fn new() -> Self {
        let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
        let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
        db.register(&registration()).unwrap();
        let pool = resource("pool", "seat", 1000);
        db.put_resource(&pool).unwrap();
        let proof = proof(&request("a", "a", &pool, 100));
        let agent = db.admit(&proof, 1000).unwrap();
        db.approve("approve_a", &proof, 1000).unwrap();
        let effect = db.claim_effect().unwrap().unwrap();
        db.observe_effect(
            &effect.id,
            effect.fence,
            &EffectOutcome::Applied {
                receipt: "fixture account".into(),
            },
        )
        .unwrap();
        db.observe_matrix_transport(
            &MatrixTransportObservation {
                engagement_id: agent.id.clone(),
                registration_generation: 1,
                generation: 1,
                sender_mxid: "@a:example.test".into(),
                device_id: "DEVICE_a".into(),
            },
            1001,
        )
        .unwrap();
        let fixture = Self {
            _root: root,
            db,
            engagement: agent.id,
        };
        let mut project = fixture.room("!project:example.test", &["@other:example.test"], false);
        project.room_id = "!project:example.test".into();
        let mut db = fixture;
        db.db.observe_matrix_room(&project, 1002).unwrap();
        db
    }
    fn room(&self, id: &str, others: &[&str], encrypted: bool) -> MatrixRoomObservation {
        let mut joined: BTreeSet<String> =
            BTreeSet::from(["@a:example.test".into(), "@owner:example.test".into()]);
        joined.extend(others.iter().map(|o| (*o).to_owned()));
        MatrixRoomObservation {
            engagement_id: self.engagement.clone(),
            registration_generation: 1,
            transport_generation: 1,
            room_id: id.into(),
            generation: 1,
            privacy: RoomPrivacy::Group {},
            joined,
            invite_only: false,
            encrypted,
        }
    }
    fn bind(&mut self, session: &str, room: &str) {
        self.db
            .resolve_verified_matrix_session(
                &SessionBinding {
                    id: session.into(),
                    engagement_id: self.engagement.clone(),
                    room_id: room.into(),
                    thread_root: None,
                },
                1003,
            )
            .unwrap();
    }
    fn event(
        &self,
        session: &str,
        id: &str,
        sender: &str,
        mentions: &[&str],
        encrypted: bool,
    ) -> MatrixEventObservation {
        MatrixEventObservation {
            scope: self.db.matrix_ingress_scope(session).unwrap(),
            event: InboundMessage {
                server_name: "example.test".into(),
                room_id: SIDE.into(),
                event_id: format!("${id}"),
                sender_mxid: sender.into(),
                thread_root: None,
                body: format!("Message {id}"),
                kind: "m.text".into(),
                origin_ts: 2009,
            },
            mentions: mentions.iter().map(|s| (*s).into()).collect(),
            encrypted,
        }
    }
}

#[test]
fn native_joined_room_record_state_and_notice() {
    let mut f = Fixture::new();
    let eng = f.engagement.clone();
    assert!(f.db.joined_rooms(&eng).unwrap().is_empty());
    let first = f.db.record_joined_room(&eng, SIDE, 2000).unwrap();
    assert_eq!(first.state, JoinedRoomState::Working);
    // A repeated join keeps the original row.
    assert_eq!(f.db.record_joined_room(&eng, SIDE, 2001).unwrap(), first);
    assert!(f.db.record_joined_room(&eng, "not-a-room", 2001).is_err());

    // The notice belongs to encrypted_shared rooms only, and is claimed once.
    assert!(!f.db.claim_joined_room_notice(&eng, SIDE, 2002).unwrap());
    f.db.set_joined_room_state(&eng, SIDE, JoinedRoomState::EncryptedShared, 2003)
        .unwrap();
    assert!(f.db.claim_joined_room_notice(&eng, SIDE, 2004).unwrap());
    assert!(!f.db.claim_joined_room_notice(&eng, SIDE, 2005).unwrap());
    // A reminder repeats it only once the gap since the last one has passed.
    assert!(
        !f.db
            .claim_joined_room_renotice(&eng, SIDE, 2005, 2003)
            .unwrap()
    );
    assert!(
        f.db.claim_joined_room_renotice(&eng, SIDE, 2005, 2004)
            .unwrap()
    );
    assert!(
        !f.db
            .claim_joined_room_renotice(&eng, SIDE, 2005, 2004)
            .unwrap()
    );

    // Retired rooms leave the live list and stay retired until a new join,
    // which makes them working again with the notice cleared.
    f.db.set_joined_room_state(&eng, SIDE, JoinedRoomState::Retired, 2006)
        .unwrap();
    assert!(f.db.joined_rooms(&eng).unwrap().is_empty());
    assert_eq!(
        f.db.set_joined_room_state(&eng, SIDE, JoinedRoomState::Working, 2007)
            .unwrap()
            .state,
        JoinedRoomState::Retired
    );
    let again = f.db.record_joined_room(&eng, SIDE, 2008).unwrap();
    assert_eq!(again.state, JoinedRoomState::Working);
    assert_eq!(again.notice_at, None);
    assert!(matches!(
        f.db.set_joined_room_state(
            &eng,
            "!unknown:example.test",
            JoinedRoomState::Working,
            2009
        ),
        Err(Error::RunnerAuthority)
    ));
}

#[test]
fn native_joined_rooms_are_bounded() {
    let mut f = Fixture::new();
    let eng = f.engagement.clone();
    for n in 0..MAX_JOINED_ROOMS {
        f.db.record_joined_room(&eng, &format!("!r{n}:example.test"), 2000)
            .unwrap();
    }
    assert!(matches!(
        f.db.record_joined_room(&eng, "!over:example.test", 2001),
        Err(Error::Capacity)
    ));
    f.db.set_joined_room_state(&eng, "!r0:example.test", JoinedRoomState::Retired, 2002)
        .unwrap();
    f.db.record_joined_room(&eng, "!over:example.test", 2003)
        .unwrap();
}

/// A group room other than the project room becomes a room scope only while it
/// is one of the engagement's working joined rooms.
#[test]
fn native_joined_group_room_scope_needs_a_working_join() {
    let mut f = Fixture::new();
    let eng = f.engagement.clone();
    let side = f.room(SIDE, &["@guest:example.test"], false);
    assert!(matches!(
        f.db.observe_matrix_room(&side, 2000),
        Err(Error::RunnerAuthority)
    ));
    f.db.record_joined_room(&eng, SIDE, 2001).unwrap();
    f.db.observe_matrix_room(&side, 2002).unwrap();
    f.bind("joined_side", SIDE);
    assert!(f.db.matrix_intake_route("joined_side").is_ok());

    // An encrypted shared room is not working: a fresh room in that state is
    // refused like any foreign group room.
    let other = f.room("!other:example.test", &["@guest:example.test"], true);
    f.db.record_joined_room(&eng, &other.room_id, 2003).unwrap();
    f.db.set_joined_room_state(&eng, &other.room_id, JoinedRoomState::EncryptedShared, 2004)
        .unwrap();
    assert!(matches!(
        f.db.observe_matrix_room(&other, 2005),
        Err(Error::RunnerAuthority)
    ));
}

/// ADR-188 §3: in a joined group room an @mention wakes the agent; in a joined
/// room whose only human is the owner, every owner message does.
#[test]
fn native_joined_room_wake_rules() {
    let mut f = Fixture::new();
    let eng = f.engagement.clone();
    f.db.record_joined_room(&eng, SIDE, 2000).unwrap();
    let shared = f.room(SIDE, &["@guest:example.test"], false);
    f.db.observe_matrix_room(&shared, 2001).unwrap();
    f.bind("joined_shared", SIDE);
    for (id, sender, mentions, expected) in [
        ("owner_plain", "@owner:example.test", vec![], false),
        (
            "owner_mention",
            "@owner:example.test",
            vec!["@a:example.test"],
            true,
        ),
        (
            "guest_mention",
            "@guest:example.test",
            vec!["@a:example.test"],
            true,
        ),
        ("guest_plain", "@guest:example.test", vec![], false),
    ] {
        let event = f.event("joined_shared", id, sender, &mentions, false);
        assert_eq!(
            f.db.admit_matrix_event(&event, 2010).unwrap().wake,
            expected,
            "shared room: {id}"
        );
    }

    // The guest leaves: the room is the owner's and the agent's alone.
    let prior = f.db.matrix_room_state(&eng, SIDE).unwrap().unwrap();
    let alone = f.room(SIDE, &[], false);
    let observed =
        f.db.refresh_matrix_group_room(&alone, Some(&prior), 2020)
            .unwrap();
    assert_eq!(observed.generation, 2);
    f.bind("joined_alone", SIDE);
    let mut event = f.event(
        "joined_alone",
        "alone_plain",
        "@owner:example.test",
        &[],
        false,
    );
    event.event.origin_ts = 2025;
    assert!(f.db.admit_matrix_event(&event, 2030).unwrap().wake);
}

/// The claim profile keeps its identity rooms and may gain or drop joined rooms.
#[test]
fn native_claim_profile_carries_joined_rooms() {
    use hagency_store::{OwnedClaimProfile, OwnedClaimRoom};
    let transport = MatrixTransportObservation {
        engagement_id: "eng".into(),
        registration_generation: 1,
        generation: 1,
        sender_mxid: "@a:example.test".into(),
        device_id: "DEVICE_a".into(),
    };
    let dm = || {
        OwnedClaimRoom::new(
            "!dm:example.test".into(),
            1,
            RoomPrivacy::Direct {
                human_mxid: "@owner:example.test".into(),
            },
        )
        .unwrap()
    };
    let joined = |id: &str| {
        OwnedClaimRoom::new(id.into(), 1, RoomPrivacy::Group {})
            .unwrap()
            .joined_group()
            .unwrap()
    };
    let profile = OwnedClaimProfile::new(transport.clone(), vec![dm()], vec!["ws".into()]).unwrap();
    let grown = profile
        .clone()
        .refresh_matrix_rooms(transport.clone(), vec![dm(), joined(SIDE)])
        .unwrap();
    let shrunk = grown
        .clone()
        .refresh_matrix_rooms(transport.clone(), vec![dm()])
        .unwrap();
    // An identity room can never be dropped or swapped for a joined one.
    assert!(matches!(
        shrunk
            .clone()
            .refresh_matrix_rooms(transport.clone(), vec![joined(SIDE)]),
        Err(Error::RunnerAuthority)
    ));
    // A plain group room cannot sneak in as an identity room.
    assert!(matches!(
        shrunk.refresh_matrix_rooms(
            transport,
            vec![
                dm(),
                OwnedClaimRoom::new(SIDE.into(), 1, RoomPrivacy::Group {}).unwrap()
            ]
        ),
        Err(Error::RunnerAuthority)
    ));
    // Joined rooms are group rooms only.
    assert!(
        OwnedClaimRoom::new(
            SIDE.into(),
            1,
            RoomPrivacy::Direct {
                human_mxid: "@owner:example.test".into()
            }
        )
        .unwrap()
        .joined_group()
        .is_err()
    );
}
