/// Server side of the successor route for the native grant fixture:
/// `GET grants/{id}` and `POST grants/{id}/successors`. Recovery of a
/// successor command shares `allocation-commands/{id}/recover`.
fn succession_fixture_reply(
    path: &str,
    body: &serde_json::Value,
    state: &PublicationFixture,
) -> Option<serde_json::Value> {
    let mut s = state.lock().unwrap();
    let rest = path.trim_start_matches("/hagency/api/v1/management/grants/");
    let succeeded = |s: &PublicationFixtureState, id: &str| {
        s.commands.values().any(|c| {
            c["receipt"]["succession"]["predecessorGrantId"] == json!(id)
                && c["receipt"]["succession"]["kind"] == json!("amendment")
        })
    };
    if let Some(predecessor) = rest.strip_suffix("/successors") {
        s.successor_posts += 1;
        if let Some(code) = s.successor_reject {
            return Some(json!({"code": code}));
        }
        assert_eq!(body["receipt"]["succession"]["predecessorGrantId"], json!(predecessor));
        assert!(!succeeded(&s, predecessor), "one successor per grant");
        let id = body["commandId"].as_str().unwrap();
        s.commands.insert(id.into(), body.clone());
        if std::mem::take(&mut s.lose_reply) {
            return None;
        }
        return Some(
            json!({"grant":publication_fixture_view(&body["receipt"], false),"observedAtMs":now_ms().unwrap()}),
        );
    }
    let receipt = s
        .commands
        .values()
        .find(|c| c["receipt"]["grantId"] == json!(rest))?
        .get("receipt")?
        .clone();
    let mut view = publication_fixture_view(&receipt, false);
    if succeeded(&s, rest) {
        view["state"] = json!("superseded");
        view["revision"] = json!(2);
    }
    Some(json!({"grant": view, "observedAtMs": now_ms().unwrap()}))
}
impl Fixture {
    /// A v2 root grant recorded and published on a server that accepts successors.
    async fn published_root(&mut self) -> String {
        {
            let mut s = self.publications.lock().unwrap();
            s.admission_receipts = true;
            s.succession = true;
        }
        let id = self.recorded().await;
        let review = self.publication_review(&id).await;
        let p = self
            .service
            .publication_apply(&self.registry, id.clone(), Some(review))
            .await
            .unwrap();
        assert_eq!(p.state, OwnerGrantPublicationState::ServerObserved);
        id
    }
    fn amendment_choices(tokens: u64, managers: &[&str]) -> OwnerGrantChoices {
        OwnerGrantChoices {
            allocated_tokens: tokens,
            eligible_managers: managers.iter().map(|m| m.to_string()).collect(),
            ..Self::choices()
        }
    }
    async fn amend(&self, tokens: u64) -> Result<OwnerGrantReview, NativeError> {
        self.service
            .prepare_successor(
                &self.registry,
                self.enrollment().await,
                "fixture_contribution".into(),
                OwnerGrantSuccessionKind::Amendment,
                Self::amendment_choices(tokens, &["@manager:test"]),
            )
            .await
    }
    async fn remaining_now(&self) -> Option<u64> {
        let cookie = self.service.cookie().await.unwrap();
        let io = self.service.io(&cookie, Permission::ReadResources).await.unwrap();
        self.registry
            .resource(&io.profile, self.resource.clone())
            .await
            .unwrap()
            .budget
            .remaining_tokens
            .map(u64::from)
    }
}

#[tokio::test]
async fn native_amendment_is_reviewed_published_and_flipped_on_acceptance() {
    let mut f = Fixture::new().await;
    let root = f.published_root().await;
    assert_eq!(f.remaining_now().await, Some(300));
    let form = f
        .service
        .succession_form(&f.registry, "fixture_contribution".into(), OwnerGrantSuccessionKind::Amendment)
        .await
        .unwrap();
    assert_eq!(form.kind, OwnerGrantSuccessionKind::Amendment);
    assert_eq!(form.predecessor.intent_id, root);
    assert_eq!(form.predecessor.allocated_tokens, 700);
    assert_eq!((form.floor.children, form.floor.retained_tokens), (0, 0));
    // The predecessor's 700 is replaced, not added: up to the whole 1000.
    assert_eq!(form.context.remaining_tokens, Some(1000));
    assert_eq!(form.period_key, form.predecessor.period_key);
    // A removed applicant is refused natively before any intent exists.
    let error = f
        .service
        .prepare_successor(
            &f.registry,
            f.enrollment().await,
            "fixture_contribution".into(),
            OwnerGrantSuccessionKind::Amendment,
            Fixture::amendment_choices(500, &["@other:test"]),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "owner_grant_manager_removed");
    let review = f.amend(500).await.unwrap();
    assert!(review.capacity_available());
    let summary = review.summary();
    let link = summary.succession.clone().unwrap();
    assert_eq!(link.kind, OwnerGrantSuccessionKind::Amendment);
    assert_eq!(link.predecessor_allocated_tokens, 700);
    let id = summary.intent_id.clone();
    let progress = f.service.confirm(&f.registry, review).await.unwrap();
    assert!(matches!(progress.state, OwnerGrantState::Recorded));
    assert_eq!(progress.succession_state, Some(OwnerGrantSuccessionState::Pending));
    let serialized = serde_json::to_value(&progress).unwrap();
    assert_eq!(serialized["successionState"], "pending");
    assert_eq!(serialized["summary"]["succession"]["kind"], "amendment");
    assert_eq!(serialized["summary"]["grantId"], json!(summary.grant_id));
    // Pending: the root still counts, as the larger of the two holds.
    assert_eq!(f.remaining_now().await, Some(300));
    // A second successor waits for this one.
    assert_eq!(f.amend(600).await.err().unwrap().code, "owner_grant_succession_pending");
    let review = f.publication_review(&id).await;
    assert_eq!(review.publication.expected_predecessor_revision, Some(1));
    let p = f
        .service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    assert_eq!(p.state, OwnerGrantPublicationState::ServerObserved);
    assert_eq!(f.publications.lock().unwrap().successor_posts, 1);
    let recovered = f.service.recover(&f.registry, id.clone()).await.unwrap();
    assert_eq!(recovered.succession_state, Some(OwnerGrantSuccessionState::Effective));
    assert_eq!(f.remaining_now().await, Some(500));
    let command = f
        .publications
        .lock()
        .unwrap()
        .commands
        .values()
        .find(|c| c["receipt"]["version"] == json!(3))
        .cloned()
        .unwrap();
    assert_eq!(command["expectedPredecessorRevision"], json!(1));
    assert_eq!(command["receipt"]["allocatedTokens"], json!(500));
    // The amendment is now the head; the next one names it.
    let form = f
        .service
        .succession_form(&f.registry, "fixture_contribution".into(), OwnerGrantSuccessionKind::Amendment)
        .await
        .unwrap();
    assert_eq!(form.predecessor.intent_id, id);
    f.stop().await;
}

#[tokio::test]
async fn native_successor_rejection_and_lost_reply_settle_from_the_server() {
    let mut f = Fixture::new().await;
    f.published_root().await;
    let review = f.amend(500).await.unwrap();
    let id = review.summary().intent_id;
    f.service.confirm(&f.registry, review).await.unwrap();
    f.publications.lock().unwrap().successor_reject = Some("grant_successor_below_reserved");
    let review = f.publication_review(&id).await;
    let p = f
        .service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    assert_eq!(p.state, OwnerGrantPublicationState::Rejected);
    let recovered = f.service.recover(&f.registry, id.clone()).await.unwrap();
    assert_eq!(recovered.succession_state, Some(OwnerGrantSuccessionState::Rejected));
    assert_eq!(f.remaining_now().await, Some(300));
    // The slot is free again; this time the reply is lost.
    f.publications.lock().unwrap().successor_reject = None;
    let review = f.amend(600).await.unwrap();
    let next = review.summary().intent_id;
    f.service.confirm(&f.registry, review).await.unwrap();
    let review = f.publication_review(&next).await;
    f.publications.lock().unwrap().lose_reply = true;
    let p = f
        .service
        .publication_apply(&f.registry, next.clone(), Some(review))
        .await
        .unwrap();
    assert_eq!(p.state, OwnerGrantPublicationState::OutcomeUnknown);
    let pending = f.service.recover(&f.registry, next.clone()).await.unwrap();
    assert_eq!(pending.succession_state, Some(OwnerGrantSuccessionState::Pending));
    let observed = f
        .service
        .publication_observe(&f.registry, next.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.state, OwnerGrantPublicationState::ServerObserved);
    let effective = f.service.recover(&f.registry, next).await.unwrap();
    assert_eq!(effective.succession_state, Some(OwnerGrantSuccessionState::Effective));
    assert_eq!(f.remaining_now().await, Some(400));
    f.stop().await;
}

#[tokio::test]
async fn native_succession_needs_the_server_feature_and_a_published_head() {
    let mut f = Fixture::new().await;
    f.publications.lock().unwrap().admission_receipts = true;
    let id = f.recorded().await;
    let kind = OwnerGrantSuccessionKind::Amendment;
    let code = |r: Result<OwnerGrantSuccessionForm, NativeError>| r.err().unwrap().code;
    assert_eq!(
        code(f.service.succession_form(&f.registry, "fixture_contribution".into(), kind).await),
        "management_grant_succession_required"
    );
    f.publications.lock().unwrap().succession = true;
    // Recorded locally but never published: nothing to succeed yet.
    assert_eq!(
        code(f.service.succession_form(&f.registry, "fixture_contribution".into(), kind).await),
        "owner_grant_predecessor_required"
    );
    let review = f.publication_review(&id).await;
    f.service
        .publication_apply(&f.registry, id, Some(review))
        .await
        .unwrap();
    assert!(f
        .service
        .succession_form(&f.registry, "fixture_contribution".into(), kind)
        .await
        .is_ok());
    f.stop().await;
}
