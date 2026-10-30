use super::*;
use {AGENT_MATRIX_CAPABILITY as MATRIX, ManagedAgentMatrixChallenge as Challenge, VerifiedManagedMatrixCheck as MatrixCheck};

const OWNER: &str = "@owner:c.test";
const MANAGER: &str = "@manager:c.test";

struct Chain {
    temp: tempfile::TempDir,
    registry: OwnerResourceRegistry,
    profile: ProfileAuthority,
    resource: String,
    revision: String,
    root: NativeGrantReceipt,
}
fn profile() -> ProfileAuthority {
    ProfileAuthority::new(
        "c".repeat(64),
        OWNER.into(),
        Instant::now() + Duration::from_secs(120),
    )
    .unwrap()
}
fn enrollment(f: &Chain) -> VerifiedManagementEnrollment {
    VerifiedManagementEnrollment::new(
        f.profile.clone(),
        "chain_enrollment".into(),
        "chain_contribution".into(),
        f.resource.clone(),
        f.revision.clone(),
        OWNER.into(),
        "chain_device".into(),
        1,
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap()
    .with_admission_request("chain_admission".into())
    .unwrap()
}
fn month_end() -> u64 {
    month(graphs::now_ms().unwrap()).unwrap().2
}
fn selection(id: &str, tokens: u64, managers: &[&str], delegation: u64) -> OwnerGrantSelection {
    OwnerGrantSelection {
        command_id: format!("{id}_consent"),
        grant_id: id.into(),
        coordinator_mxid: OWNER.into(),
        delegation_expires_at_ms: delegation,
        allow_self_approval: false,
        eligible_managers: managers.iter().map(|m| m.to_string()).collect(),
        allocated_tokens: tokens,
    }
}
async fn chain(ceiling: u64, root_tokens: u64) -> Chain {
    let temp = tempfile::tempdir().unwrap();
    let registry = OwnerResourceRegistry::open_root(&temp.path().join("owner")).unwrap();
    let profile = profile();
    let resource = registry
        .prepare_enrollment(
            profile.clone(),
            "chain_enroll".into(),
            VerifiedProviderAccount::new(
                "codex",
                "https://provider.test",
                "chain-fixture",
                Instant::now() + Duration::from_secs(30),
            )
            .unwrap(),
            "gpt-5.6-sol".into(),
            Some("medium".into()),
            ceiling,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap()
        .approve()
        .await
        .unwrap()
        .resource_id;
    let revision = registry
        .resource(&profile, resource.clone())
        .await
        .unwrap()
        .revision;
    let mut f = Chain {
        temp,
        registry,
        profile,
        resource,
        revision,
        root: dummy(),
    };
    f.root = f
        .registry
        .prepare_capacity_grant(
            enrollment(&f),
            selection("chain_root", root_tokens, &[MANAGER], month_end()),
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap()
        .approve()
        .await
        .unwrap();
    f
}
fn dummy() -> NativeGrantReceipt {
    serde_json::from_str(include_str!("../pool-grant-succession-v3.json"))
        .map(|v: serde_json::Value| serde_json::from_value(v["predecessor"].clone()).unwrap())
        .unwrap()
}
fn consent(f: &Chain, head: &NativeGrantReceipt, kind: &str, id: &str, tokens: u64, managers: &[&str]) -> Result<OwnerGrantConsent, Error> {
    f.registry.prepare_capacity_grant_successor(
        enrollment(f),
        head.clone(),
        kind,
        selection(id, tokens, managers, head.delegation_expires_at_ms),
        Instant::now() + Duration::from_secs(30),
    )
}
async fn amend(f: &Chain, head: &NativeGrantReceipt, id: &str, tokens: u64) -> Result<NativeGrantReceipt, Error> {
    consent(f, head, SUCCESSION_AMENDMENT, id, tokens, &[MANAGER])?.approve().await
}
async fn confirm(f: &Chain, id: &str) -> CapacityGrantSuccession {
    f.registry
        .confirm_capacity_grant_succession(f.profile.clone(), "chain_device".into(), id.into())
        .await
        .unwrap()
}
fn command(_f: &Chain, grant: &NativeGrantReceipt, id: &str, tokens: u64) -> ManagedAgentReserveCommand {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("managed_agents/pool-agent-v1.json")).unwrap();
    let mut c: ManagedAgentReserveCommand =
        serde_json::from_value(fixture["command"].clone()).unwrap();
    c.command_id = format!("chain_reserve_{id}");
    c.request_id = format!("chain_request_{id}");
    c.proposed_agent_id = format!("chain_agent_{id}");
    c.allocation_id = c.request_id.clone();
    c.manager_mxid = MANAGER.into();
    c.decision_actor_mxid = OWNER.into();
    c.pool_grant = Some(grant.clone());
    c.grant_receipt_digest = canonical::digest(&serde_json::to_value(grant).unwrap()).unwrap();
    c.enrollment_id = grant.enrollment_id.clone();
    c.executor_device_id = grant.executor_device_id.clone();
    c.executor_generation = grant.executor_generation;
    c.resource_id = grant.resource_id.clone();
    c.resource_revision = grant.resource_revision.clone();
    c.authority_revision = grant.grant_revision;
    c.requested_tokens = tokens;
    c.period_key = grant.period_key.clone();
    c.issued_at_ms = graphs::now_ms().unwrap();
    c.expires_at_ms = grant.delegation_expires_at_ms.min(grant.period_end_ms);
    c
}
async fn reserve(f: &Chain, c: ManagedAgentReserveCommand) -> Result<ManagedAgentReservationReceipt, Error> {
    let digest = canonical::digest(&serde_json::to_value(&c).unwrap()).unwrap();
    f.registry
        .reserve_managed_agent(
            VerifiedManagedAgentDelivery::for_pool(
                enrollment(f),
                c,
                digest,
                true,
                Instant::now() + Duration::from_secs(20),
            )
            .unwrap(),
        )
        .await
}
async fn child(f: &Chain, grant: &NativeGrantReceipt, id: &str, tokens: u64) -> (ManagedAgentReserveCommand, String) {
    let c = command(f, grant, id, tokens);
    let r = reserve(f, c.clone()).await.unwrap();
    assert_eq!(r.state, "reserved", "{:?}", r.reason);
    (c, r.agent_reservation_id.unwrap())
}
async fn sql(f: &Chain, statement: &'static str, child: String) {
    f.registry
        .0
        .store
        .call(1024, move |db| {
            db.db.execute(statement, [child])?;
            Ok(())
        })
        .await
        .unwrap();
}
/// Fenced with unknown usage: the provision effect was issued.
async fn fence_unknown(f: &Chain, child: &str, at: Option<u64>) {
    if let Some(at) = at {
        let id = child.to_owned();
        f.registry.0.store.call(1024, move |db| {
            db.db.execute("INSERT INTO owner_managed_fence_times(child_id,fenced_at_ms) VALUES(?1,?2)", params![id, at])?;
            Ok(())
        }).await.unwrap();
    }
    sql(f, "UPDATE owner_managed_effects SET state='issued' WHERE child_id=?1", child.into()).await;
    sql(f, "UPDATE owner_managed_agents SET permanent_fence=1,state='fenced' WHERE id=?1", child.into()).await;
}
async fn commitments(f: &Chain, at: u64) -> std::collections::BTreeMap<String, u64> {
    let resource = f.resource.clone();
    f.registry.0.store.call(4096, move |db| {
        let resource = read_resource(&db.db, &resource)?;
        Ok(additional_commitments(&db.db, &resource, at)?
            .into_iter()
            .map(|c| (c.id.trim_start_matches("owner_grant_").to_owned(), c.allocated_tokens.map(u64::from).unwrap_or(0)))
            .collect())
    }).await.unwrap()
}
fn total(c: &std::collections::BTreeMap<String, u64>) -> u64 {
    c.values().sum()
}
async fn remaining(f: &Chain, at: u64) -> Option<u64> {
    let resource = f.resource.clone();
    f.registry.0.store.call(4096, move |db| {
        let resource = read_resource(&db.db, &resource)?;
        Ok(super::super::super::budget_at(&db.db, &resource, None, false, at)?.remaining_tokens.map(u64::from))
    }).await.unwrap()
}
/// The single-grant formula before succession existed (the 1a rule).
async fn legacy(f: &Chain, at: u64) -> std::collections::BTreeMap<String, u64> {
    let resource = f.resource.clone();
    f.registry.0.store.call(4096, move |db| {
        let resource = read_resource(&db.db, &resource)?;
        let mut q = db.db.prepare("SELECT g.id,COALESCE(p.retained_tokens,g.allocated_tokens),json_extract(g.receipt,'$.periodEndMs') FROM owner_capacity_grants g LEFT JOIN owner_parent_settlements p ON p.grant_id=g.id WHERE g.preset_id=?1 OR g.seat_id=?2")?;
        let rows = q.query_map(params![resource.preset_id, resource.seat_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?, r.get::<_, u64>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        let mut out = std::collections::BTreeMap::new();
        for (id, tokens, end) in rows {
            let floor = managed_agents::grant_floor(&db.db, &id)?;
            let value = if end <= at {
                if floor.complete { floor.unresolved } else { floor.retained }
            } else {
                tokens.max(floor.retained)
            };
            out.insert(id, value);
        }
        Ok(out)
    }).await.unwrap()
}
async fn eligible(f: &Chain, reserve: &ManagedAgentReserveCommand, child: &str, id: &str) -> bool {
    let saved = f
        .registry
        .recover_managed_agent_reservation(
            f.profile.clone(),
            "chain_device".into(),
            reserve.command_id.clone(),
            canonical::digest(&serde_json::to_value(reserve).unwrap()).unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    let digest = |v: &serde_json::Value| canonical::digest(v).unwrap();
    let now = graphs::now_ms().unwrap();
    let matrix = Challenge {
        version: 1,
        capability: MATRIX.into(),
        operation: "agent.matrix.check".into(),
        command_id: format!("chain_matrix_{id}"),
        assignment_id: format!("chain_assignment_{}", &child[4..20]),
        assignment_generation: 1,
        agent_reservation_id: child.into(),
        reserve_command: reserve.clone(),
        reserve_command_digest: digest(&serde_json::to_value(reserve).unwrap()),
        reserve_receipt_digest: digest(&serde_json::to_value(&saved).unwrap()),
        space_id: String::new(),
        puppet_mxid: "@_hagency_chain:c.test".into(),
        issued_at_ms: now,
        expires_at_ms: now + 20000,
    };
    f.registry
        .check_managed_matrix_reservation(
            MatrixCheck::new(
                enrollment(f),
                matrix.clone(),
                digest(&serde_json::to_value(&matrix).unwrap()),
                true,
                Instant::now() + Duration::from_secs(20),
            )
            .unwrap(),
        )
        .await
        .map(|o| o.eligible)
        .unwrap_or(false)
}

#[test]
fn shared_v3_vector_has_the_server_digests_and_omits_succession_on_v2() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../pool-grant-succession-v3.json")).unwrap();
    let digest = |name: &str| {
        let receipt: NativeGrantReceipt = serde_json::from_value(fixture[name].clone()).unwrap();
        assert!(receipt.supported_shape(), "{name}");
        let value = serde_json::to_value(&receipt).unwrap();
        assert_eq!(value, fixture[name], "{name} round trip");
        canonical::digest(&value).unwrap()
    };
    assert_eq!(digest("predecessor"), "e75ea704484fca4a40db8387b855e0b2c2d438ec273d61e6dfd7c4543c8d3616");
    assert_eq!(digest("amendment"), "c69dbd92dc068046cbf6930d455c0258d31dea9093dcd0d415f068c3dbba8937");
    assert_eq!(digest("renewal"), "1fdb875c843afef52fc4f2ba2720395fda5445de37f1a0f8ada6c799874035ff");
    // Applicants are strictly byte-ordered, as both writers require.
    for name in ["predecessor", "amendment", "renewal"] {
        let receipt: NativeGrantReceipt = serde_json::from_value(fixture[name].clone()).unwrap();
        assert!(receipt.eligible_managers.windows(2).all(|w| w[0] < w[1]), "{name}");
    }
    for (name, digest) in [("predecessor", "predecessorDigest"), ("amendment", "amendmentDigest"), ("renewal", "renewalDigest")] {
        assert_eq!(canonical::digest(&fixture[name]).unwrap(), fixture[digest]);
    }
    assert_eq!(fixture["amendment"]["succession"]["predecessorReceiptDigest"], fixture["predecessorDigest"]);
    assert_eq!(fixture["renewal"]["succession"]["predecessorReceiptDigest"], fixture["amendmentDigest"]);
    let v2: NativeGrantReceipt = serde_json::from_value(fixture["predecessor"].clone()).unwrap();
    assert!(serde_json::to_value(&v2).unwrap().get("succession").is_none());
    // Shape rules shared with the server.
    let mut bad: NativeGrantReceipt = serde_json::from_value(fixture["amendment"].clone()).unwrap();
    bad.version = 2;
    assert!(!bad.supported_shape());
    let mut bad: NativeGrantReceipt = serde_json::from_value(fixture["amendment"].clone()).unwrap();
    bad.succession.as_mut().unwrap().carried_retained_tokens = bad.allocated_tokens + 1;
    assert!(!bad.supported_shape());
    let mut bad: NativeGrantReceipt = serde_json::from_value(fixture["amendment"].clone()).unwrap();
    bad.succession.as_mut().unwrap().kind = "transfer".into();
    assert!(!bad.supported_shape());
    let mut bad: NativeGrantReceipt = serde_json::from_value(fixture["amendment"].clone()).unwrap();
    bad.grant_revision = 1;
    assert!(!bad.supported_shape());
    let mut bad = v2.clone();
    bad.succession = serde_json::from_value(fixture["amendment"]["succession"].clone()).unwrap();
    assert!(!bad.supported_shape());
    let mut unknown = fixture["amendment"].clone();
    unknown["succession"]["extra"] = json!(1);
    assert!(serde_json::from_value::<NativeGrantReceipt>(unknown).is_err());
}

#[tokio::test]
async fn a_chain_of_one_gives_exactly_the_single_grant_numbers() {
    let f = chain(1000, 700).await;
    let (_, live) = child(&f, &f.root, "live", 200).await;
    let (_, unknown) = child(&f, &f.root, "unknown", 100).await;
    let (_, settled) = child(&f, &f.root, "settled", 150).await;
    let _ = live;
    fence_unknown(&f, &unknown, None).await;
    sql(&f, "UPDATE owner_managed_agents SET permanent_fence=1,state='fenced' WHERE id=?1", settled).await;
    let now = graphs::now_ms().unwrap();
    for at in [now, f.root.period_start_ms, month_end() - 1, month_end(), month_end() + 86_400_000] {
        assert_eq!(commitments(&f, at).await, legacy(&f, at).await, "at {at}");
    }
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn lowering_a_full_ceiling_is_allowed_and_the_total_never_rises_at_the_flip() {
    let f = chain(1000, 1000).await;
    child(&f, &f.root, "a", 300).await;
    child(&f, &f.root, "b", 300).await;
    assert_eq!(remaining(&f, graphs::now_ms().unwrap()).await, Some(0));
    let s = amend(&f, &f.root, "chain_amend", 800).await.unwrap();
    assert_eq!(s.version, 3);
    assert_eq!(s.grant_revision, 2);
    let link = s.succession.clone().unwrap();
    assert_eq!((link.kind.as_str(), link.carried_children, link.carried_retained_tokens), ("amendment", 2, 600));
    assert_eq!(link.predecessor_receipt_digest, canonical::digest(&serde_json::to_value(&f.root).unwrap()).unwrap());
    let now = graphs::now_ms().unwrap();
    let pending = commitments(&f, now).await;
    assert_eq!(pending["chain_root"], 1000);
    assert_eq!(pending["chain_amend"], 0);
    // The server observed it: the amendment heads the month from the flip.
    let state = confirm(&f, "chain_amend").await;
    assert_eq!(state.state, "effective");
    assert!(state.effective_from_ms >= now);
    let after = commitments(&f, graphs::now_ms().unwrap()).await;
    assert_eq!(after["chain_root"], 0);
    assert_eq!(after["chain_amend"], 800);
    assert!(total(&after) <= total(&pending));
    assert_eq!(remaining(&f, graphs::now_ms().unwrap()).await, Some(200));
    // Per-Agent budgets are untouched by the amendment.
    let allocated = f.registry.0.store.call(1024, |db| {
        Ok(db.db.prepare("SELECT allocated_tokens FROM owner_managed_agents ORDER BY id")?.query_map([], |r| r.get::<_, u64>(0))?.collect::<Result<Vec<_>, _>>()?)
    }).await.unwrap();
    assert_eq!(allocated, vec![300, 300]);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_child_fenced_before_the_flip_stays_inside_the_new_cap() {
    // Ceiling 1000, A_P = 1000, two running children hold 300 each; amend to
    // 800. One child is fenced before the flip with unknown usage.
    let f = chain(1000, 1000).await;
    let (_, a) = child(&f, &f.root, "a", 300).await;
    let a_id = a.clone();
    child(&f, &f.root, "b", 300).await;
    amend(&f, &f.root, "chain_amend", 800).await.unwrap();
    let pending = commitments(&f, graphs::now_ms().unwrap()).await;
    fence_unknown(&f, &a, Some(1)).await;
    let fenced_pending = commitments(&f, graphs::now_ms().unwrap()).await;
    confirm(&f, "chain_amend").await;
    let now = graphs::now_ms().unwrap();
    let after = commitments(&f, now).await;
    // The fenced child stays with the root, yet inside the month's cap.
    let fenced = f.registry.0.store.call(1024, move |db| {
        succession::accounting_grant(&db.db, "chain_root", succession::fenced_at(&db.db, &a)?, now)
    }).await.unwrap();
    assert_eq!(fenced, "chain_root");
    assert_eq!(after["chain_root"], 0);
    assert_eq!(after["chain_amend"], 800);
    assert!(total(&after) <= 1000);
    assert!(total(&after) <= total(&fenced_pending) && total(&after) <= total(&pending));
    // Unknown usage still blocks new Agents (E20). Once the fenced child's
    // usage settles at 300, only 800 - 600 remains under the amendment.
    let s: NativeGrantReceipt = f.registry.recover_capacity_grant(f.profile.clone(), "chain_device".into(), "chain_amend".into()).await.unwrap().unwrap();
    assert_eq!(reserve(&f, command(&f, &s, "blocked", 1)).await.unwrap().reason.as_deref(), Some("unsettled_usage"));
    let settled = a_id.clone();
    f.registry.0.store.call(1024, move |db| {
        db.db.execute("INSERT INTO owner_managed_calls(id,child_id,execution_id,immutable_digest,assignment_generation,state,reserved_tokens,known_consumed_tokens,usage_complete) VALUES('chain_call_a',?1,'chain_execution_a',?2,1,'settled',300,300,1)", params![settled, "d".repeat(64)])?;
        db.db.execute("UPDATE owner_managed_effects SET state='failed' WHERE child_id=?1", [settled])?;
        Ok(())
    }).await.unwrap();
    let over = reserve(&f, command(&f, &s, "over", 201)).await.unwrap();
    assert_eq!(over.reason.as_deref(), Some("insufficient_capacity"));
    assert_eq!(reserve(&f, command(&f, &s, "fits", 200)).await.unwrap().state, "reserved");
    // When the month ends each grant keeps its own children's liability:
    // the fenced child's usage is settled, the others' holds are unresolved.
    let ended = commitments(&f, month_end()).await;
    assert_eq!(ended["chain_root"], 0);
    assert_eq!(ended["chain_amend"], 500);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_floor_or_capacity_shortfall_writes_nothing() {
    let f = chain(1000, 700).await;
    child(&f, &f.root, "a", 250).await;
    child(&f, &f.root, "b", 150).await;
    let before = f.registry.0.store.call(1024, |db| {
        Ok(db.db.query_row("SELECT (SELECT COUNT(*) FROM owner_capacity_grants),(SELECT COUNT(*) FROM owner_capacity_grant_successions),revision FROM owner_resource_identity", [], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?, r.get::<_, u64>(2)?)))?)
    }).await.unwrap();
    assert!(matches!(amend(&f, &f.root, "below_floor", 399).await, Err(Error::InsufficientCapacity)));
    // Raising needs headroom: 300 free beside 700, so 1000 fits but 1001 not.
    assert!(matches!(amend(&f, &f.root, "too_large", 1001).await, Err(Error::InsufficientCapacity)));
    // Removing an applicant is refused before anything is written.
    assert!(matches!(consent(&f, &f.root, SUCCESSION_AMENDMENT, "fewer", 500, &["@other:c.test"]), Err(Error::LocalAuthority)));
    let after = f.registry.0.store.call(1024, |db| {
        Ok(db.db.query_row("SELECT (SELECT COUNT(*) FROM owner_capacity_grants),(SELECT COUNT(*) FROM owner_capacity_grant_successions),revision FROM owner_resource_identity", [], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?, r.get::<_, u64>(2)?)))?)
    }).await.unwrap();
    assert_eq!(before, after);
    let s = consent(&f, &f.root, SUCCESSION_AMENDMENT, "raised", 1000, &[MANAGER, "@second:c.test"]).unwrap().approve().await.unwrap();
    assert_eq!(s.eligible_managers, vec![MANAGER.to_string(), "@second:c.test".into()]);
    assert_eq!(s.succession.unwrap().carried_retained_tokens, 400);
    // One live successor per grant: a second amendment of the same head is refused.
    assert!(matches!(amend(&f, &f.root, "second", 600).await, Err(Error::OwnerGrantReviewChanged)));
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn rejection_frees_the_slot_and_pending_reserves_are_deferred() {
    let f = chain(1000, 700).await;
    let s = amend(&f, &f.root, "chain_amend", 500).await.unwrap();
    // The server is ahead: a reserve built from the pending successor is not
    // answered and leaves no receipt.
    let early = command(&f, &s, "early", 100);
    assert!(matches!(reserve(&f, early.clone()).await, Err(Error::Busy)));
    let saved = f.registry.recover_managed_agent_reservation(f.profile.clone(), "chain_device".into(), early.command_id.clone(), canonical::digest(&serde_json::to_value(&early).unwrap()).unwrap()).await.unwrap();
    assert!(saved.is_none());
    // A definitive rejection: the successor never held capacity.
    let rejected = f.registry.reject_capacity_grant_succession(f.profile.clone(), "chain_device".into(), "chain_amend".into()).await.unwrap();
    assert_eq!(rejected.state, "rejected");
    let now = graphs::now_ms().unwrap();
    let c = commitments(&f, now).await;
    assert_eq!((c["chain_root"], c["chain_amend"]), (700, 0));
    assert!(f.registry.confirm_capacity_grant_succession(f.profile.clone(), "chain_device".into(), "chain_amend".into()).await.is_err());
    assert_eq!(reserve(&f, early).await.unwrap().reason.as_deref(), Some("authority_changed"));
    // The slot is free again; this one is confirmed and then serves reserves.
    let next = amend(&f, &f.root, "chain_amend_2", 600).await.unwrap();
    confirm(&f, "chain_amend_2").await;
    assert_eq!(reserve(&f, command(&f, &next, "after", 100)).await.unwrap().state, "reserved");
    // The superseded root no longer accepts new Agents.
    assert_eq!(reserve(&f, command(&f, &f.root, "stale", 100)).await.unwrap().reason.as_deref(), Some("authority_changed"));
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_reserve_beside_a_pending_amendment_must_fit_both_grants() {
    let f = chain(1000, 700).await;
    child(&f, &f.root, "a", 300).await;
    amend(&f, &f.root, "chain_amend", 450).await.unwrap();
    // 700 - 300 leaves 400 under the root, but the amendment covers 450 - 300.
    assert_eq!(reserve(&f, command(&f, &f.root, "big", 151)).await.unwrap().reason.as_deref(), Some("insufficient_capacity"));
    assert_eq!(reserve(&f, command(&f, &f.root, "small", 150)).await.unwrap().state, "reserved");
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn custody_follows_the_effective_grant_of_the_chain() {
    let f = chain(1000, 700).await;
    let (reserved, id) = child(&f, &f.root, "carried", 200).await;
    assert!(eligible(&f, &reserved, &id, "before").await);
    amend(&f, &f.root, "chain_amend", 500).await.unwrap();
    // Pending: the root still governs.
    assert!(eligible(&f, &reserved, &id, "pending").await);
    confirm(&f, "chain_amend").await;
    assert!(eligible(&f, &reserved, &id, "after").await);
    // Closing the amendment (not the root) ends the carried child's authority.
    sql(&f, "UPDATE owner_capacity_grants SET active=0 WHERE id=?1", "chain_amend".into()).await;
    assert!(!eligible(&f, &reserved, &id, "closed").await);
    let head = f.registry.capacity_grant_chain_head(f.profile.clone(), "chain_device".into(), "chain_root".into()).await.unwrap();
    assert_eq!(head.effective.grant_id, "chain_amend");
    assert!(!head.open);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn in_flight_carried_usage_counts_as_held_for_a_successor_only() {
    let f = chain(1000, 700).await;
    let (_, busy) = child(&f, &f.root, "busy", 300).await;
    sql(&f, "UPDATE owner_managed_effects SET state='issued' WHERE child_id=?1", busy).await;
    let now = graphs::now_ms().unwrap();
    // A new root or a new Agent sees unknown capacity, a successor does not.
    assert_eq!(remaining(&f, now).await, None);
    let facts = f.registry.capacity_grant_succession_facts(f.profile.clone(), f.root.clone(), SUCCESSION_AMENDMENT.into()).await.unwrap();
    assert_eq!((facts.carried_children, facts.floor_tokens, facts.max_tokens), (1, 300, Some(1000)));
    assert_eq!(facts.period_key, f.root.period_key);
    let s = amend(&f, &f.root, "chain_amend", 1000).await.unwrap();
    assert_eq!(s.succession.unwrap().carried_retained_tokens, 300);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_successor_table_allows_one_live_successor_and_one_decision() {
    let f = chain(1000, 700).await;
    amend(&f, &f.root, "chain_amend", 500).await.unwrap();
    f.registry.0.store.call(4096, |db| {
        let d = &db.db;
        assert!(d.execute("INSERT INTO owner_capacity_grant_successions(grant_id,predecessor_grant_id,profile,kind,state,effective_from_ms,command_id,created_at_ms) VALUES('chain_root','chain_amend',?1,'amendment','pending',0,'x',1)", ["c".repeat(64)]).is_err());
        assert!(d.execute("UPDATE owner_capacity_grant_successions SET kind='renewal'", []).is_err());
        assert!(d.execute("DELETE FROM owner_capacity_grant_successions", []).is_err());
        assert!(d.execute("UPDATE owner_capacity_grant_successions SET state='cancelled',decided_at_ms=1", []).is_err());
        d.execute("UPDATE owner_capacity_grant_successions SET state='effective',effective_from_ms=5,decided_at_ms=5", [])?;
        assert!(d.execute("UPDATE owner_capacity_grant_successions SET state='rejected'", []).is_err());
        assert!(d.execute("UPDATE owner_capacity_grant_successions SET effective_from_ms=6", []).is_err());
        Ok(())
    }).await.unwrap();
    f.registry.shutdown().await.unwrap();
    drop(f.temp);
}
