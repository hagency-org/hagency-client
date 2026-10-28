//! Board #53: agent-scheduled self-reminders (TS lib/delivery-queue.js:1725-1760).
mod common;
use common::*;
use hagency_core::{JSON_SAFE_MAX, tasks::*};
use hagency_store::*;

struct Fixture {
    root: tempfile::TempDir,
    db: DomainRepository,
    cap: RunnerCapability,
    session: String,
}
impl Fixture {
    fn new() -> Self {
        let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
        let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
        db.register(&registration()).unwrap();
        let pool = resource("reminder_pool", "reminder_seat", 1000);
        db.put_resource(&pool).unwrap();
        let p = proof(&request("reminder_request", "Worker", &pool, 100));
        let engagement = db.admit(&p, 1000).unwrap();
        db.approve("approve", &p, 1000).unwrap();
        let effect = db.claim_effect().unwrap().unwrap();
        db.observe_effect(
            &effect.id,
            effect.fence,
            &EffectOutcome::Applied {
                receipt: "offline fixture".into(),
            },
        )
        .unwrap();
        // A live verified Matrix route is what `fire_reminders` reads to build
        // the wake's sender and room; the continuous-agent path needs it.
        db.observe_matrix_transport(
            &hagency_core::replies::MatrixTransportObservation {
                engagement_id: engagement.id.clone(),
                registration_generation: 1,
                generation: 1,
                sender_mxid: "@worker:example.test".into(),
                device_id: "DEVICE".into(),
            },
            1000,
        )
        .unwrap();
        db.observe_matrix_room(
            &hagency_core::replies::MatrixRoomObservation {
                engagement_id: engagement.id.clone(),
                registration_generation: 1,
                transport_generation: 1,
                room_id: "!project:example.test".into(),
                generation: 1,
                privacy: hagency_core::replies::RoomPrivacy::Group {},
                joined: std::collections::BTreeSet::from([
                    "@worker:example.test".into(),
                    "@owner:example.test".into(),
                ]),
                invite_only: true,
                encrypted: false,
            },
            1000,
        )
        .unwrap();
        let session = "reminder_session";
        db.resolve_verified_matrix_session(
            &SessionBinding {
                id: session.into(),
                engagement_id: engagement.id.clone(),
                room_id: "!project:example.test".into(),
                thread_root: Some("$reminder_thread".into()),
            },
            1000,
        )
        .unwrap();
        db.create_canonical_task("reminder_task", session, "Remind yourself", 1000)
            .unwrap();
        db.register_workspace("reminder_work").unwrap();
        db.enqueue_dispatch(&DispatchInput {
            id: "reminder_dispatch".into(),
            session_id: session.into(),
            task_id: Some("reminder_task".into()),
            resources: vec![ResourceLease {
                id: "reminder_work".into(),
                exclusive: true,
            }],
            payload: serde_json::json!({"instruction":"offline"}),
        })
        .unwrap();
        let cap = db
            .claim_dispatch("reminder_runner", 1001, 60_000, 120_000, 128)
            .unwrap()
            .unwrap();
        let admission = db.owned_dispatch_scope(&cap, 1002).unwrap();
        db.start_owned_dispatch(&cap, admission.fingerprint(), 1003)
            .unwrap();
        Self {
            root,
            db,
            cap,
            session: session.into(),
        }
    }
}

#[test]
fn native_reminder_schedule_list_fire_delete() {
    let mut f = Fixture::new();
    // Schedule: positive delay, valid msg.
    let receipt =
        f.db.schedule_reminder(&f.cap, "第一件事", 5_000, 2000)
            .unwrap();
    assert_eq!(receipt.remaining_ms, 5_000);
    assert_eq!(receipt.fire_at, 7_000);
    // Listed before firing.
    let listed = f.db.list_reminders().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].msg, "第一件事");
    assert!(listed[0].fired_at.is_none());
    // A second reminder, then fire only the due one.
    f.db.schedule_reminder(&f.cap, "第二件事", 5_000, 2000)
        .unwrap();
    // Not due yet at fire time 3000 < 7000.
    let sweep = f.db.fire_reminders(3_000, 512).unwrap();
    assert_eq!(sweep.fired, 0);
    assert_eq!(sweep.remaining, 2);
    // Fire at 8000: both due, both woken with the TS `[Self Time Reminder]` text.
    let sweep = f.db.fire_reminders(8_000, 512).unwrap();
    assert_eq!(sweep.fired, 2);
    assert_eq!(sweep.remaining, 0);
    // The wake landed in session_inputs with wake=1 and the exact TS text.
    let rows: Vec<(String, bool)> = {
        let conn = rusqlite::Connection::open(f.root.path().join("state/domain.sqlite3")).unwrap();
        let mut stmt = conn
            .prepare("SELECT m.config,i.wake FROM session_inputs i JOIN admitted_messages m ON m.sequence=i.message_sequence WHERE i.session_id=?1 ORDER BY m.sequence")
            .unwrap();
        stmt.query_map([&f.session], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert!(rows.iter().all(|(_, wake)| *wake));
    assert!(
        rows.iter()
            .any(|(config, _)| config.contains("[Self Time Reminder]")
                && config.contains("Msg: 第一件事"))
    );
    // Re-firing is idempotent: fired_at is set, nothing is re-woken.
    let sweep = f.db.fire_reminders(9_000, 512).unwrap();
    assert_eq!(sweep.fired, 0);
    assert_eq!(sweep.remaining, 0);
    // Delete by id, then NotFound on a second delete.
    f.db.delete_reminder(receipt.id).unwrap();
    assert_eq!(f.db.list_reminders().unwrap().len(), 1);
    assert!(matches!(
        f.db.delete_reminder(receipt.id),
        Err(Error::NotFound)
    ));
}

#[test]
fn native_reminder_refuses_bad_input() {
    let mut f = Fixture::new();
    // Missing msg (empty), zero/overflow delay.
    assert!(f.db.schedule_reminder(&f.cap, "", 1_000, 2000).is_err());
    assert!(f.db.schedule_reminder(&f.cap, "ok", 0, 2000).is_err());
    assert!(
        f.db.schedule_reminder(&f.cap, "ok", JSON_SAFE_MAX + 1, 2000)
            .is_err()
    );
    // A foreign capability cannot schedule (authorize_work fails).
    let mut foreign = f.cap.clone();
    foreign.secret = "0".repeat(64);
    assert!(f.db.schedule_reminder(&foreign, "ok", 1_000, 2000).is_err());
}
