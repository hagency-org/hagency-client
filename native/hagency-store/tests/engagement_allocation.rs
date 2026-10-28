//! ADR-186: an engagement's token allocation is chosen at approval (§A),
//! pauses its agent when used up (§B), and can be topped up (§C).
mod common;
use common::*;
use hagency_core::project::EngagementState;
use hagency_store::{DomainRepository, Error};

fn setup(ceiling: u64) -> (tempfile::TempDir, DomainRepository) {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    db.put_resource(&resource("alloc_pool", "alloc_seat", ceiling))
        .unwrap();
    (dir, db)
}
fn admitted(db: &mut DomainRepository, id: &str, name: &str, tokens: u64) -> String {
    let pool = resource("alloc_pool", "alloc_seat", 0);
    db.admit(&proof(&request(id, name, &pool, tokens)), 1000)
        .unwrap()
        .id
}
fn allocated_column(dir: &tempfile::TempDir, id: &str) -> Option<u64> {
    rusqlite::Connection::open(dir.path().join("state/domain.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT allocated_tokens FROM engagements WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
}

/// §A1/§A4: the operator grants less than the request. The engagement holds
/// the granted amount everywhere a commitment is counted — reservation,
/// draw and side commitment — while the request stays the requester's ask.
#[test]
fn native_allocation_approve_with_a_lower_amount() {
    let (dir, mut db) = setup(1000);
    let id = admitted(&mut db, "low_request", "LowWorker", 300);
    let pool = resource("alloc_pool", "alloc_seat", 0);
    let p = proof(&request("low_request", "LowWorker", &pool, 300));
    let approved = db
        .approve_allocating("approve_low", &p, 1000, Some(120))
        .unwrap();
    assert_eq!(approved.state, EngagementState::Reserved);
    assert_eq!(u64::from(approved.requested_tokens), 300, "the ask is kept");
    assert_eq!(approved.allocated_tokens.map(u64::from), Some(120));
    assert_eq!(u64::from(approved.allocation()), 120);
    assert_eq!(allocated_column(&dir, &id), Some(120));
    // Reservation and draw: the ceiling report and the pool budget count 120.
    let report = db.resource_ceiling(&pool.id(), 1000).unwrap();
    assert_eq!(report.reserved, 120);
    assert_eq!(report.drawn, 120);
    let budget = db.resource_budget(&pool.id()).unwrap();
    assert_eq!(u64::from(budget.pool.committed), 120);
    // Side commitment.
    let side = db.side_budget("example.test").unwrap();
    assert_eq!(side.committed, 120);
    assert_eq!(side.commitments[0].allocated_tokens, 120);
    // The headroom behind any engagement on the resource is what is left.
    assert_eq!(db.engagement_headroom(&id, 1000).unwrap(), Some(880));
    // The same command id replays the receipt; the same id with another
    // amount is a different act and conflicts.
    assert_eq!(
        db.approve_allocating("approve_low", &p, 1000, Some(120))
            .unwrap()
            .allocated_tokens
            .map(u64::from),
        Some(120)
    );
    assert!(matches!(
        db.approve_allocating("approve_low", &p, 1000, Some(121)),
        Err(Error::Conflict)
    ));
}

/// §A1: without an amount the approval is the plain one — the requested
/// amount is the allocation and no column is written.
#[test]
fn native_allocation_plain_approval_keeps_the_request() {
    let (dir, mut db) = setup(1000);
    let id = admitted(&mut db, "plain_request", "PlainWorker", 200);
    let pool = resource("alloc_pool", "alloc_seat", 0);
    let p = proof(&request("plain_request", "PlainWorker", &pool, 200));
    let approved = db
        .approve_allocating("approve_plain", &p, 1000, None)
        .unwrap();
    assert_eq!(approved.allocated_tokens, None);
    assert_eq!(u64::from(approved.allocation()), 200);
    assert_eq!(allocated_column(&dir, &id), None);
    // The plain `approve` shares the decision digest: it replays.
    assert!(db.approve("approve_plain", &p, 1000).is_ok());
    // A zero amount is not an allocation.
    let other = admitted(&mut db, "zero_request", "ZeroWorker", 10);
    let zero = proof(&request("zero_request", "ZeroWorker", &pool, 10));
    assert!(matches!(
        db.approve_allocating("approve_zero", &zero, 1000, Some(0)),
        Err(Error::Invalid(_))
    ));
    assert_eq!(db.get(&other).unwrap().state, EngagementState::Pending);
}

/// §A2: an amount above the headroom is refused with the over-commit
/// refusal and its human message; the engagement stays pending.
#[test]
fn native_allocation_approve_above_headroom_is_refused_with_the_message() {
    let (_dir, mut db) = setup(1000);
    let id = admitted(&mut db, "big_request", "BigWorker", 100);
    let pool = resource("alloc_pool", "alloc_seat", 0);
    let p = proof(&request("big_request", "BigWorker", &pool, 100));
    match db.approve_allocating("approve_big", &p, 1000, Some(1001)) {
        Err(Error::OverCommit { message }) => {
            assert!(message.contains("BigWorker"), "{message}");
            assert!(message.contains("would exceed"), "{message}");
            assert!(message.contains("ceiling"), "{message}");
        }
        other => panic!("expected over_commit, got {other:?}"),
    }
    assert_eq!(db.get(&id).unwrap().state, EngagementState::Pending);
    // A larger REQUEST approved with a smaller amount fits.
    let larger = admitted(&mut db, "larger_request", "LargerWorker", 5000);
    let q = proof(&request("larger_request", "LargerWorker", &pool, 5000));
    assert!(matches!(
        db.approve("approve_larger_plain", &q, 1000),
        Err(Error::OverCommit { .. })
    ));
    assert_eq!(
        db.approve_allocating("approve_larger", &q, 1000, Some(900))
            .unwrap()
            .allocated_tokens
            .map(u64::from),
        Some(900)
    );
    assert_eq!(db.get(&larger).unwrap().state, EngagementState::Reserved);
}

/// §A3: "All remaining" is exactly the headroom the approval is checked
/// against — approving it fits, and one token more does not.
#[test]
fn native_allocation_all_remaining() {
    let (_dir, mut db) = setup(1000);
    let pool = resource("alloc_pool", "alloc_seat", 0);
    let first = admitted(&mut db, "first_request", "FirstWorker", 250);
    db.approve(
        "approve_first",
        &proof(&request("first_request", "FirstWorker", &pool, 250)),
        1000,
    )
    .unwrap();
    let id = admitted(&mut db, "rest_request", "RestWorker", 100);
    let all = db.engagement_headroom(&id, 1000).unwrap().unwrap();
    assert_eq!(all, 750);
    let p = proof(&request("rest_request", "RestWorker", &pool, 100));
    assert!(matches!(
        db.approve_allocating("approve_rest_over", &p, 1000, Some(all + 1)),
        Err(Error::OverCommit { .. })
    ));
    let approved = db
        .approve_allocating("approve_rest", &p, 1000, Some(all))
        .unwrap();
    assert_eq!(approved.allocated_tokens.map(u64::from), Some(750));
    assert_eq!(db.engagement_headroom(&id, 1000).unwrap(), Some(0));
    assert_eq!(db.engagement_headroom(&first, 1000).unwrap(), Some(0));
}

mod quota {
    //! §B: the quota pause, on a verified Matrix thread session so the pause
    //! notice has a thread to land in (the `over_budget_notice.rs` fixture).
    use super::*;
    use hagency_core::{
        agent_inbox::{AgentInboxPlan, AgentInboxSelection},
        ingress::MatrixEventObservation,
        messages::InboundMessage,
        replies::*,
        tasks::*,
    };
    use hagency_metering::{Framework, observation::UsageObservation};
    use hagency_store::{EffectOutcome, OwnedDispatchScope, UsageSource};
    use serde_json::json;
    use std::collections::BTreeSet;

    const ROOM: &str = "!project:example.test";

    pub(super) struct Fixture {
        pub root: tempfile::TempDir,
        pub db: DomainRepository,
        pub engagement: String,
        #[allow(dead_code)]
        pub proof: hagency_core::authority::VerifiedRequest,
    }
    pub(super) fn codex(fresh: u64, output: u64, cached: u64) -> UsageObservation {
        UsageObservation::parse(Framework::Codex,&json!({"payload":{"info":{"total_token_usage":{"input_tokens":fresh+cached,"output_tokens":output,"cached_input_tokens":cached,"reasoning_output_tokens":0,"total_tokens":fresh+cached+output}}}}).to_string()).unwrap()
    }
    pub(super) fn empty() -> UsageObservation {
        UsageObservation::parse(Framework::Codex, "").unwrap()
    }
    impl Fixture {
        /// One agent approved for `allocated` of its 100-token request, its
        /// transport and Group room observed, and a verified thread session.
        pub fn new(allocated: u64) -> Self {
            let root =
                hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
            let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
            db.register(&registration()).unwrap();
            let pool = resource("pool", "seat", 1000);
            db.put_resource(&pool).unwrap();
            let p = proof(&request("one", "Worker", &pool, 100));
            let e = db.admit(&p, 1000).unwrap();
            db.approve_allocating("approve_one", &p, 1000, Some(allocated))
                .unwrap();
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
                    engagement_id: e.id.clone(),
                    registration_generation: 1,
                    generation: 1,
                    sender_mxid: "@worker:example.test".into(),
                    device_id: "DEVICE_WORKER".into(),
                },
                1001,
            )
            .unwrap();
            db.observe_matrix_room(
                &MatrixRoomObservation {
                    engagement_id: e.id.clone(),
                    registration_generation: 1,
                    transport_generation: 1,
                    room_id: ROOM.into(),
                    generation: 1,
                    privacy: RoomPrivacy::Group {},
                    joined: BTreeSet::from([
                        "@owner:example.test".to_owned(),
                        "@worker:example.test".to_owned(),
                    ]),
                    invite_only: true,
                    encrypted: true,
                },
                1002,
            )
            .unwrap();
            db.register_workspace("work_thread").unwrap();
            db.resolve_verified_matrix_session(
                &SessionBinding {
                    id: "threaded".into(),
                    engagement_id: e.id.clone(),
                    room_id: ROOM.into(),
                    thread_root: Some("$thread_threaded".into()),
                },
                1003,
            )
            .unwrap();
            Self {
                root,
                db,
                engagement: e.id,
                proof: p,
            }
        }
        pub fn sql(&self) -> rusqlite::Connection {
            rusqlite::Connection::open(self.root.path().join("state/domain.sqlite3")).unwrap()
        }
        /// The owner's addressed request in the thread.
        pub fn admit(&mut self, event_id: &str, origin_ts: u64) {
            let observation = MatrixEventObservation {
                scope: self.db.matrix_ingress_scope("threaded").unwrap(),
                event: InboundMessage {
                    server_name: "example.test".into(),
                    room_id: ROOM.into(),
                    event_id: event_id.into(),
                    sender_mxid: "@owner:example.test".into(),
                    thread_root: Some("$thread_threaded".into()),
                    body: "@worker:example.test draft the quarterly report".into(),
                    kind: "m.text".into(),
                    origin_ts,
                },
                mentions: BTreeSet::from(["@worker:example.test".to_owned()]),
                encrypted: true,
            };
            assert!(
                self.db
                    .admit_matrix_event(&observation, origin_ts + 1)
                    .unwrap()
                    .wake
            );
        }
        /// The inbox dispatch claimed and started as an owned turn, with
        /// its usage source bound: the running turn whose usage is observed.
        pub fn working(&mut self, now: u64) -> (RunnerCapability, OwnedDispatchScope, UsageSource) {
            let plan = AgentInboxPlan {
                session_id: "threaded".into(),
                workspace_id: "work_thread".into(),
            };
            let AgentInboxSelection::Selected { dispatch_id, .. } =
                self.db.select_agent_inbox(&plan, now).unwrap()
            else {
                panic!("the verified wake did not create a dispatch")
            };
            let cap = self
                .db
                .claim_dispatch("runner", now + 1, 60_000, 120_000, 8)
                .unwrap()
                .unwrap();
            assert_eq!(cap.dispatch_id, dispatch_id);
            let admission = self.db.owned_dispatch_scope(&cap, now + 2).unwrap();
            let started = self
                .db
                .start_owned_dispatch(&cap, admission.fingerprint(), now + 3)
                .unwrap();
            let source = self.db.bind_usage_source(&cap, &started, now + 4).unwrap();
            (cap, started, source)
        }
        /// A second, plain session of the same engagement with one queued
        /// dispatch: the work that arrives while the agent is paused.
        pub fn queue_more(&mut self) {
            self.db
                .register_session(&SessionBinding {
                    id: "later".into(),
                    engagement_id: self.engagement.clone(),
                    room_id: ROOM.into(),
                    thread_root: None,
                })
                .unwrap();
            self.db
                .create_canonical_task("later_task", "later", "Later work", 5000)
                .unwrap();
            self.db.register_workspace("work_later").unwrap();
            self.db
                .enqueue_dispatch(&DispatchInput {
                    id: "later_dispatch".into(),
                    session_id: "later".into(),
                    task_id: Some("later_task".into()),
                    resources: vec![ResourceLease {
                        id: "work_later".into(),
                        exclusive: true,
                    }],
                    payload: json!({"instruction":"fixture"}),
                })
                .unwrap();
        }
        pub fn dispatch_state(&self, id: &str) -> String {
            self.sql()
                .query_row(
                    "SELECT state FROM runner_dispatches WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap()
        }
        /// (kind, body) of every quota notice, oldest first.
        pub fn quota_notices(&self) -> Vec<(String, String)> {
            let sql = self.sql();
            let mut statement = sql
                .prepare(
                    "SELECT json_extract(config,'$.kind'),json_extract(config,'$.body') FROM task_notices \
                     WHERE json_extract(config,'$.kind') LIKE 'quota_%' ORDER BY rowid",
                )
                .unwrap();
            statement
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        }
    }

    /// §B1-§B3: spend below the allocation runs; spend reaching it opens the
    /// hold, the running turn is untouched, new work stays queued and is not
    /// claimed, and the project room hears exactly one notice.
    #[test]
    fn native_quota_pause_when_spend_reaches_the_allocation() {
        let mut f = Fixture::new(50);
        f.admit("$request", 3000);
        let (cap, _, source) = f.working(3004);
        f.db.record_usage_observation(&source, "under", &codex(30, 10, 7), 3100)
            .unwrap();
        let status = f.db.quota_status(&f.engagement).unwrap();
        assert_eq!(status.allocated_tokens, 50);
        // Cache reads are not counted: 30 + 10, not 47.
        assert_eq!(status.spent_tokens, Some(40));
        assert!(!status.paused);
        f.db.record_usage_observation(&source, "over", &codex(45, 10, 7), 3200)
            .unwrap();
        let status = f.db.quota_status(&f.engagement).unwrap();
        assert_eq!(status.spent_tokens, Some(55));
        assert!(status.paused, "spend reached the allocation");
        // The running turn finishes: its dispatch is still started.
        assert_eq!(f.dispatch_state(&cap.dispatch_id), "started");
        // Exactly one notice, in the thread of the turn, with §B3's words.
        assert_eq!(
            f.quota_notices(),
            [(
                "quota_paused".to_owned(),
                "Paused: used 55 of 50 tokens. The owner can add tokens in the Hagency console."
                    .to_owned()
            )]
        );
        let thread: String = f
            .sql()
            .query_row(
                "SELECT json_extract(config,'$.thread_root') FROM task_notices WHERE json_extract(config,'$.kind')='quota_paused'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(thread, "$thread_threaded");
        // The turn that crossed the line ends and its thread closes. The pause
        // notice must still go out: that close is exactly when it is said.
        // (The fixture's turn has no Matrix task intent, so record its thread
        // as a closed one, as the live turn's was.)
        let closed = f
            .sql()
            .execute(
                "INSERT INTO task_intents(task_id,request_scope,request_key,digest,session_id,root_sequence,state) \
                 SELECT t.id,'fixture','quota-thread','fixture',t.session_id,(SELECT MAX(sequence) FROM admitted_messages),'closed' \
                 FROM canonical_tasks t WHERE t.id=(SELECT task_id FROM task_notices WHERE json_extract(config,'$.kind')='quota_paused')",
                [],
            )
            .unwrap();
        assert_eq!(closed, 1, "the turn's thread is recorded as closed");
        // Live, the finished turn also moves its task to the next execution
        // epoch, which is what cancelled the notice on the rig.
        let advanced = f
            .sql()
            .execute(
                "UPDATE canonical_tasks SET config=json_set(config,'$.execution_epoch',json_extract(config,'$.execution_epoch')+1) \
                 WHERE id=(SELECT task_id FROM task_notices WHERE json_extract(config,'$.kind')='quota_paused')",
                [],
            )
            .unwrap();
        assert_eq!(advanced, 1);
        let _ = f.db.claim_verified_task_notice(3250, 60_000).unwrap();
        let state: String = f
            .sql()
            .query_row(
                "SELECT state FROM task_notices WHERE json_extract(config,'$.kind')='quota_paused'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(
            state, "cancelled",
            "the pause notice survives its thread closing"
        );
        // A further observation while paused says nothing more.
        f.db.record_usage_observation(&source, "more", &codex(60, 10, 7), 3300)
            .unwrap();
        assert_eq!(f.quota_notices().len(), 1);
        // Work that arrives meanwhile stays queued and is not claimed.
        f.queue_more();
        assert!(
            f.db.claim_dispatch("runner_two", 3400, 60_000, 120_000, 8)
                .unwrap()
                .is_none(),
            "no new turn is dispatched for a paused engagement"
        );
        assert_eq!(f.dispatch_state("later_dispatch"), "queued");
        // The engagement is not ended or revoked by the pause.
        assert_eq!(
            f.db.get(&f.engagement).unwrap().state,
            EngagementState::Active
        );
        // Both console reads show it.
        let label =
            f.db.engagement_labels("", None, 10)
                .unwrap()
                .into_iter()
                .find(|l| l.id == f.engagement)
                .unwrap();
        assert!(label.quota_paused);
        assert_eq!(label.allocated_tokens, 50);
        assert_eq!(label.spent_tokens, Some(70));
        let roster = f.db.agent_roster().unwrap();
        assert!(
            roster
                .iter()
                .any(|r| r.engagement_id == f.engagement && r.quota_paused)
        );
    }

    /// §B1/§B4: a runtime observation is always marked incomplete by the
    /// metering crate, yet its counts are a known lower bound of the spend —
    /// it pauses, or no live agent could ever pause.
    #[test]
    fn native_quota_runtime_usage_counts_toward_the_pause() {
        use hagency_metering::runtime_usage::{CodexUsage, CounterBreakdown};
        let mut f = Fixture::new(50);
        f.admit("$request", 3000);
        let (_, _, source) = f.working(3004);
        let observation = UsageObservation::codex_runtime(CodexUsage {
            total: CounterBreakdown {
                total_tokens: Some(72),
                input_tokens: Some(67),
                cached_input_tokens: Some(7),
                cache_write_input_tokens: Some(0),
                output_tokens: Some(5),
                reasoning_output_tokens: Some(0),
            },
            ..CodexUsage::default()
        })
        .unwrap();
        assert!(
            observation.incomplete(),
            "runtime usage is incomplete by construction"
        );
        f.db.record_usage_observation(&source, "runtime", &observation, 3100)
            .unwrap();
        let status = f.db.quota_status(&f.engagement).unwrap();
        assert_eq!(status.spent_tokens, Some(65));
        assert!(status.paused);
        assert_eq!(f.quota_notices().len(), 1);
    }

    /// §B4: unknown or count-less usage never pauses, whatever the
    /// allocation, and the spend reads as unknown.
    #[test]
    fn native_quota_no_pause_on_unknown_usage() {
        let mut f = Fixture::new(1);
        // Nothing observed at all.
        let status = f.db.quota_status(&f.engagement).unwrap();
        assert_eq!(status.spent_tokens, None);
        assert!(!status.paused);
        f.admit("$request", 3000);
        let (_, _, source) = f.working(3004);
        // An observation without counts is incomplete and adds nothing
        // known: still unknown.
        f.db.record_usage_observation(&source, "empty", &empty(), 3100)
            .unwrap();
        let status = f.db.quota_status(&f.engagement).unwrap();
        assert_eq!(status.spent_tokens, None, "count-less usage is unknown");
        assert!(!status.paused);
        assert!(f.quota_notices().is_empty());
        // Queued work still dispatches.
        f.queue_more();
        assert!(
            f.db.claim_dispatch("runner_two", 3400, 60_000, 120_000, 8)
                .unwrap()
                .is_some()
        );
    }

    /// §C: a top-up is checked like an approval, is idempotent by command
    /// id, lifts the hold only when the allocation is above the spend, says
    /// "Resumed" once, and lets queued work dispatch with no restart.
    #[test]
    fn native_quota_top_up_lifts_the_pause_and_is_idempotent() {
        let mut f = Fixture::new(50);
        f.admit("$request", 3000);
        let (_, _, source) = f.working(3004);
        f.db.record_usage_observation(&source, "over", &codex(45, 10, 0), 3100)
            .unwrap();
        assert!(f.db.quota_status(&f.engagement).unwrap().paused);
        f.queue_more();
        assert!(
            f.db.claim_dispatch("runner_two", 3200, 60_000, 120_000, 8)
                .unwrap()
                .is_none()
        );
        // More than the resource can give is refused like an approval, with
        // the human message; nothing changes.
        match f
            .db
            .raise_allocation("top_big", &f.engagement, 1_000_000, 3300)
        {
            Err(Error::OverCommit { message }) => {
                assert!(message.contains("would exceed"), "{message}")
            }
            other => panic!("expected over_commit, got {other:?}"),
        }
        assert_eq!(
            f.db.quota_status(&f.engagement).unwrap().allocated_tokens,
            50
        );
        // A top-up that still leaves the spend at or above the allocation is
        // recorded but keeps the hold, and says nothing.
        let small =
            f.db.raise_allocation("top_small", &f.engagement, 5, 3400)
                .unwrap();
        assert_eq!(small.allocated_tokens.map(u64::from), Some(55));
        let status = f.db.quota_status(&f.engagement).unwrap();
        assert_eq!(
            (status.allocated_tokens, status.spent_tokens),
            (55, Some(55))
        );
        assert!(status.paused, "55 of 55 is still used up");
        assert_eq!(f.quota_notices().len(), 1);
        // Headroom: the 1000-token pool less this engagement's 55.
        assert_eq!(
            f.db.engagement_headroom(&f.engagement, 3500).unwrap(),
            Some(945)
        );
        // "All remaining" raises by exactly the headroom; the hold lifts.
        let raised =
            f.db.raise_allocation("top_all", &f.engagement, 945, 3500)
                .unwrap();
        assert_eq!(raised.allocated_tokens.map(u64::from), Some(1000));
        assert_eq!(u64::from(raised.requested_tokens), 100, "the ask is kept");
        let status = f.db.quota_status(&f.engagement).unwrap();
        assert!(!status.paused);
        assert_eq!(status.allocated_tokens, 1000);
        // Idempotent: the same command replays, the allocation is not
        // raised twice; the same command with another amount conflicts.
        assert_eq!(
            f.db.raise_allocation("top_all", &f.engagement, 945, 3600)
                .unwrap()
                .allocated_tokens
                .map(u64::from),
            Some(1000)
        );
        assert_eq!(
            f.db.quota_status(&f.engagement).unwrap().allocated_tokens,
            1000
        );
        assert!(matches!(
            f.db.raise_allocation("top_all", &f.engagement, 944, 3600),
            Err(Error::Conflict)
        ));
        // One "Resumed" notice, in the thread the pause was said in (the
        // queued plain session has no admitted request to say it under).
        assert_eq!(
            f.quota_notices(),
            [
                (
                    "quota_paused".to_owned(),
                    "Paused: used 55 of 50 tokens. The owner can add tokens in the Hagency console."
                        .to_owned()
                ),
                (
                    "quota_resumed".to_owned(),
                    "Resumed: 945 tokens available.".to_owned()
                ),
            ]
        );
        // The queued work dispatches now, without a restart.
        let cap =
            f.db.claim_dispatch("runner_two", 3700, 60_000, 120_000, 8)
                .unwrap()
                .expect("the lifted hold lets queued work dispatch");
        assert_eq!(cap.dispatch_id, "later_dispatch");
        // The Palpo-facing projection and the draw carry the new figure.
        assert_eq!(
            u64::from(f.db.get(&f.engagement).unwrap().allocation()),
            1000
        );
        let report =
            f.db.resource_ceiling(&resource("pool", "seat", 0).id(), 3700)
                .unwrap();
        assert_eq!(report.reserved, 1000);
        // A decided-and-ended engagement cannot be topped up.
        f.db.revoke("revoke_one", &f.engagement).unwrap();
        assert!(matches!(
            f.db.raise_allocation("top_after", &f.engagement, 1, 3800),
            Err(Error::State)
        ));
    }
}
