#[derive(Default)]
struct PublicationFixtureState {
    admission_receipts: bool,
    /// The server accepts one grant per admission per UTC month.
    monthly_regrant: bool,
    allocations_disabled: bool,
    commands: std::collections::BTreeMap<String, serde_json::Value>,
    posts: usize,
    device_override: Option<String>,
    device_generation: Option<u64>,
    device_posts: usize,
    #[cfg(feature = "resource-adoption")]
    connections: ConnectionFixtureState,
    contribution_reads: usize,
    recover_reads: usize,
    lose_reply: bool,
    reject: bool,
    revoked: bool,
    invalidate_at: Option<&'static str>,
    gate: Option<Arc<AtomicBool>>,
    /// The server advertises `pool-grant-succession-v1`.
    succession: bool,
    /// The successor route answers with this audited pre-effect code.
    successor_reject: Option<&'static str>,
    successor_posts: usize,
}
type PublicationFixture = Arc<std::sync::Mutex<PublicationFixtureState>>;
impl PublicationFixtureState {
    fn invalidate(&mut self, at: &str) {
        if self.invalidate_at == Some(at) {
            self.gate.as_ref().unwrap().store(false, Ordering::Release);
        }
    }
}
fn publication_fixture_view(receipt: &serde_json::Value, revoked: bool) -> serde_json::Value {
    json!({"id":receipt["grantId"],"receipt":receipt,"receiptDigest":digest(receipt).unwrap(),
        "state":if revoked {"revoked"}else{"active"},"revision":if revoked{2}else{1},"observedAtMs":now_ms().unwrap()})
}
fn publication_fixture_reply(
    path: &str,
    body: &serde_json::Value,
    state: &PublicationFixture,
) -> Option<serde_json::Value> {
    let mut s = state.lock().unwrap();
    if path.ends_with("/grants") {
        s.posts += 1;
        s.invalidate("publish");
        if s.reject {
            return Some(json!({"code":"management_revision_changed"}));
        }
        let id = body["commandId"].as_str().unwrap();
        if let Some(old) = s.commands.get(id) {
            assert_eq!(old, body);
        }
        s.commands.insert(id.into(), body.clone());
        if std::mem::take(&mut s.lose_reply) {
            return None;
        }
        Some(
            json!({"grant":publication_fixture_view(&body["receipt"], false),"observedAtMs":now_ms().unwrap()}),
        )
    } else {
        s.recover_reads += 1;
        s.invalidate("recover");
        let id = path.split('/').rev().nth(1).unwrap();
        Some(match s.commands.get(id) {
            Some(command) => {
                json!({"recorded":true,"grant":publication_fixture_view(&command["receipt"],s.revoked),"observedAtMs":now_ms().unwrap()})
            }
            None => json!({"recorded":false,"observedAtMs":now_ms().unwrap()}),
        })
    }
}
impl Fixture {
    async fn enable_publication(&mut self) {
        self.service.manager.retire();
        let mut p = package(8, true);
        p.permissions
            .extend([Permission::PublishGrants, Permission::ReadIntents]);
        self.service = OwnerGrantIntents::new(self.owner.management(p).await.unwrap());
    }
    async fn recorded(&mut self) -> String {
        self.enable_publication().await;
        let review = self.review().await;
        let id = review.summary().intent_id;
        let result = self.service.confirm(&self.registry, review).await.unwrap();
        assert!(matches!(result.state, OwnerGrantState::Recorded));
        id
    }
    async fn publication_review(&self, id: &str) -> OwnerGrantPublicationReview {
        match self
            .service
            .publication_review(&self.registry, id.into())
            .await
            .unwrap()
        {
            OwnerGrantPublicationResult::Review(r) => *r,
            _ => panic!("expected an explicit publication review"),
        }
    }
    async fn publication_recover(&self, id: &str) -> OwnerGrantPublicationProgress {
        match self
            .service
            .publication_review(&self.registry, id.into())
            .await
            .unwrap()
        {
            OwnerGrantPublicationResult::Progress(p) => *p,
            _ => panic!("expected original publication recovery"),
        }
    }
}

#[tokio::test]
async fn publication_form_reads_budget_without_grants_or_publication() {
    let mut f = Fixture::new().await;
    f.enable_publication().await;
    for _ in 0..2 {
        let form = f.service.publication_form(&f.registry, "fixture_contribution".into()).await.unwrap();
        assert_eq!(form.resource_id, f.resource);
        assert_eq!(form.context.resource_revision, f.revision);
        assert_eq!(form.context.remaining_tokens, Some(1000));
        assert_eq!(form.context.resource_total_tokens, Some(1000));
        assert_eq!(form.context.server_origin, f.origin);
    }
    assert!(f.history().await.data.records.is_empty());
    assert_eq!(f.count("owner_capacity_grants"), 0);
    assert_eq!(f.publications.lock().unwrap().posts, 0);
}

#[tokio::test]
async fn publication_form_rejects_changed_admission_and_revision() {
    let mut f = Fixture::new().await;
    f.enable_publication().await;
    for (key, value) in [("ownerMxid", "@other:test"), ("resourceRevision", &"a".repeat(64)), ("state", "pending")] {
        let saved = f.admission.lock().unwrap()["admission"]["contribution"][key].clone();
        f.admission.lock().unwrap()["admission"]["contribution"][key] = json!(value);
        assert!(f.service.publication_form(&f.registry, "fixture_contribution".into()).await.is_err());
        f.admission.lock().unwrap()["admission"]["contribution"][key] = saved;
    }
    assert!(f.history().await.data.records.is_empty());
    assert_eq!(f.count("owner_capacity_grants"), 0);
    assert_eq!(f.publications.lock().unwrap().posts, 0);
}
#[tokio::test]
async fn native_grant_publication_cancel_restart_preserves_original_cas_and_one_hold() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let review = f.publication_review(&id).await;
    assert_eq!(review.contribution_revision(), 2);
    let original = review.publication.clone();
    drop(review);
    assert_eq!(f.publications.lock().unwrap().posts, 0);
    let mut f = f.restart().await;
    f.enable_publication().await;
    let review = f.publication_review(&id).await;
    assert_eq!(review.publication, original);
    let p = f
        .service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    assert_eq!(p.state, OwnerGrantPublicationState::ServerObserved);
    assert_eq!(f.count("owner_capacity_grants"), 1);
    let serialized = serde_json::to_string(&p).unwrap();
    assert!(!serialized.contains("receipt") && !serialized.contains("provider"));
    assert_eq!(f.publications.lock().unwrap().posts, 1);
    f.publications.lock().unwrap().revoked = true;
    let p = f.publication_recover(&id).await;
    assert_eq!(p.server_state.as_deref(), Some("revoked"));
    assert_eq!(f.publications.lock().unwrap().posts, 1);
    f.stop().await;
}
#[tokio::test]
async fn native_grant_publication_lost_reply_recovers_original_after_restart_without_post() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let review = f.publication_review(&id).await;
    let command = review.publication.clone();
    f.publications.lock().unwrap().lose_reply = true;
    let p = f
        .service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    assert_eq!(p.state, OwnerGrantPublicationState::OutcomeUnknown);
    let mut f = f.restart().await;
    f.enable_publication().await;
    let p = f.publication_recover(&id).await;
    assert_eq!(p.state, OwnerGrantPublicationState::ServerObserved);
    assert_eq!(
        f.history().await.data.records[0]
            .publication
            .as_ref()
            .unwrap()
            .command_id,
        command.command_id
    );
    assert_eq!(f.publications.lock().unwrap().posts, 1);
    assert_eq!(f.count("owner_capacity_grants"), 1);
    f.stop().await;
}
#[tokio::test]
async fn native_grant_publication_rejection_preserves_hold_and_does_not_refresh_cas() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let review = f.publication_review(&id).await;
    let original = review.publication.clone();
    f.publications.lock().unwrap().reject = true;
    let p = f
        .service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    assert_eq!(p.state, OwnerGrantPublicationState::Rejected);
    let p = f.publication_recover(&id).await;
    assert_eq!(p.state, OwnerGrantPublicationState::Rejected);
    assert_eq!(f.publications.lock().unwrap().posts, 1);
    assert_eq!(f.count("owner_capacity_grants"), 1);
    assert_eq!(
        f.history().await.data.records[0]
            .publication
            .as_ref()
            .unwrap()
            .expected_contribution_revision,
        original.expected_contribution_revision
    );
    f.stop().await;
}
#[tokio::test]
async fn native_grant_publication_legacy_journal_omits_optional_publication_field() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let history = f.history().await;
    let bytes = std::fs::read(&history.path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("publication"));
    assert!(
        History::load(&history.path, history.data.identity.clone())
            .unwrap()
            .data
            .records[0]
            .publication
            .is_none()
    );
    let review = f.publication_review(&id).await;
    drop(review);
    let mut history = f.history().await;
    history.data.records[0]
        .publication
        .as_mut()
        .unwrap()
        .expected_contribution_revision += 1;
    history.save().unwrap();
    let error = f
        .service
        .publication_review(&f.registry, id)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "owner_grant_publication_review_changed");
    assert_eq!(f.publications.lock().unwrap().posts, 0);
    f.stop().await;
}
#[tokio::test]
async fn native_grant_publication_host_retirement_at_each_http_boundary_keeps_original_pending() {
    for boundary in ["contribution", "publish", "recover"] {
        let mut f = Fixture::new().await;
        let id = f.recorded().await;
        let review = f.publication_review(&id).await;
        let original = review.publication.clone();
        let gate = Arc::new(AtomicBool::new(true));
        if boundary == "recover" {
            f.publications.lock().unwrap().lose_reply = true;
            let p = f
                .service
                .publication_apply(&f.registry, id.clone(), Some(review))
                .await
                .unwrap();
            assert_eq!(p.state, OwnerGrantPublicationState::OutcomeUnknown);
        } else {
            drop(review);
        }
        {
            let mut state = f.publications.lock().unwrap();
            state.invalidate_at = Some(boundary);
            state.gate = Some(gate.clone());
        }
        f.service.host_gate = Some({
            let gate = gate.clone();
            Arc::new(move || gate.load(Ordering::Acquire))
        });
        if boundary == "contribution" {
            let err = f
                .service
                .publication_review(&f.registry, id.clone())
                .await
                .err()
                .unwrap();
            assert_eq!(err.code, "management_scope_changed");
        } else if boundary == "publish" {
            let r = f.publication_review(&id).await;
            let err = f
                .service
                .publication_apply(&f.registry, id.clone(), Some(r))
                .await
                .err()
                .unwrap();
            assert_eq!(err.code, "management_outcome_unknown");
        } else {
            assert!(
                f.service
                    .publication_review(&f.registry, id.clone())
                    .await
                    .is_err()
            );
        }
        f.service = OwnerGrantIntents::new(f.service.manager.clone()); // a distinct fresh native host
        let h = f.history().await;
        assert_eq!(
            h.data.records[0].publication.as_ref().unwrap().command_id,
            original.command_id
        );
        let cookie = f.service.cookie().await.unwrap();
        let io = f
            .service
            .io(&cookie, Permission::ReadResources)
            .await
            .unwrap();
        let pending = io.session.pending_intents(&io.context).await.unwrap();
        if boundary != "contribution" {
            let saved = pending
                .iter()
                .find(|p| p.command_id == original.command_id)
                .unwrap();
            assert!(
                saved.reply.is_none(),
                "result after retirement must remain pending"
            );
        }
        drop(io);
        {
            let mut state = f.publications.lock().unwrap();
            state.invalidate_at = None;
            state.gate = None;
        }
        if boundary != "contribution" {
            assert_eq!(
                f.publication_recover(&id).await.state,
                OwnerGrantPublicationState::ServerObserved
            );
        }
        assert_eq!(f.count("owner_capacity_grants"), 1);
        f.stop().await;
    }
}

#[tokio::test]
async fn native_grant_publication_missing_metadata_cannot_replace_replied_or_scrubbed_history() {
    for scrubbed in [false, true] {
        let mut f = Fixture::new().await;
        let id = f.recorded().await;
        let review = f.publication_review(&id).await;
        let command = review.publication.command_id.clone();
        f.service
            .publication_apply(&f.registry, id.clone(), Some(review))
            .await
            .unwrap();
        if scrubbed {
            f.service.manager.retire();
            let mut p = package(9, true);
            p.permissions.extend([
                Permission::ReadIntents,
                Permission::PublishGrants,
                Permission::AcknowledgeIntents,
            ]);
            f.service = OwnerGrantIntents::new(f.owner.management(p).await.unwrap());
            let cookie = f.service.cookie().await.unwrap();
            let io = f
                .service
                .io(&cookie, Permission::PublishGrants)
                .await
                .unwrap();
            io.session
                .acknowledge_intent(&io.context, command)
                .await
                .unwrap();
        }
        let mut history = f.history().await;
        history.data.records[0].publication = None;
        history.save().unwrap();
        let e = f
            .service
            .publication_review(&f.registry, id)
            .await
            .err()
            .unwrap();
        assert_eq!(e.code, "owner_grant_publication_history_unavailable");
        assert!(f.history().await.data.records[0].publication.is_none());
        assert_eq!(f.publications.lock().unwrap().posts, 1);
        assert_eq!(f.count("owner_capacity_grants"), 1);
        f.stop().await;
    }
}
#[tokio::test]
async fn native_grant_publication_local_commit_loss_is_recovered_before_network_review() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let mut history = f.history().await;
    let original_receipt = history.data.records[0].receipt.clone();
    history.data.records[0].state = State::Applying;
    history.data.records[0].receipt = None;
    history.save().unwrap();
    let review = f.publication_review(&id).await;
    assert_eq!(f.history().await.data.records[0].receipt, original_receipt);
    f.service
        .publication_apply(&f.registry, id, Some(review))
        .await
        .unwrap();
    assert_eq!(f.count("owner_capacity_grants"), 1);
    assert_eq!(f.publications.lock().unwrap().posts, 1);
    f.stop().await;
}
#[tokio::test]
async fn native_grant_publication_retired_host_cannot_confirm_or_send() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let gate = Arc::new(AtomicBool::new(true));
    f.service.host_gate = Some({
        let gate = gate.clone();
        Arc::new(move || gate.load(Ordering::Acquire))
    });
    let review = f.publication_review(&id).await;
    gate.store(false, Ordering::Release);
    assert!(review.expired());
    assert!(
        f.service
            .publication_apply(&f.registry, id, Some(review))
            .await
            .is_err()
    );
    assert_eq!(f.publications.lock().unwrap().posts, 0);
    // Replace the host-bound access object; never revive its revoked closure.
    f.service = OwnerGrantIntents::new(f.service.manager.clone());
    assert!(
        !f.history().await.data.records[0]
            .publication
            .as_ref()
            .unwrap()
            .confirmed
    );
    assert_eq!(f.count("owner_capacity_grants"), 1);
    f.stop().await;
}
impl Fixture {
    async fn close_local_parent(&self) {
        let history = self.history().await;
        let grant: NativeGrantReceipt =
            serde_json::from_str(history.data.records[0].receipt.as_ref().unwrap()).unwrap();
        let command = GrantClosureCommand {
            version: 1,
            command_id: "close_original_parent".into(),
            operation: "grant.close".into(),
            grant_id: grant.grant_id.clone(),
            grant_receipt_digest: digest(&grant).unwrap(),
            previous_settlement_digest: None,
            reason: "owner_withdrawal".into(),
            requested_at_ms: now_ms().unwrap(),
        };
        let delivery = VerifiedParentDelivery::new(
            self.enrollment().await,
            command.clone(),
            digest(&command).unwrap(),
            true,
            Instant::now() + Duration::from_secs(20),
        )
        .unwrap();
        let receipt = self
            .registry
            .apply_parent_settlement(delivery)
            .await
            .unwrap();
        assert_eq!(receipt.state, "closed");
        assert_eq!(receipt.retained_tokens, 0);
    }
}
#[tokio::test]
async fn native_grant_publication_local_close_after_review_blocks_first_post_but_history_recovers()
{
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let review = f.publication_review(&id).await;
    let original = f.history().await.data.records[0].receipt.clone();
    f.close_local_parent().await;
    // The permanent fence must remain authoritative even if an old active bit
    // is accidentally restored. This mutation is isolated test corruption.
    rusqlite::Connection::open(f.root.path().join("registry/domain.sqlite3"))
        .unwrap()
        .execute("UPDATE owner_capacity_grants SET active=1", [])
        .unwrap();
    let p = f
        .service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    assert_eq!(p.state, OwnerGrantPublicationState::Rejected);
    assert_eq!(f.publications.lock().unwrap().posts, 0);
    assert_eq!(f.history().await.data.records[0].receipt, original);
    assert!(matches!(
        f.service.recover(&f.registry, id).await.unwrap().state,
        OwnerGrantState::Recorded
    ));
    f.stop().await;
}
#[tokio::test]
async fn native_grant_publication_local_close_does_not_block_existing_server_result_recovery() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let review = f.publication_review(&id).await;
    f.publications.lock().unwrap().lose_reply = true;
    f.service
        .publication_apply(&f.registry, id.clone(), Some(review))
        .await
        .unwrap();
    f.close_local_parent().await;
    f.publications.lock().unwrap().revoked = true;
    let p = f.publication_recover(&id).await;
    assert_eq!(p.state, OwnerGrantPublicationState::ServerObserved);
    assert_eq!(p.server_state.as_deref(), Some("revoked"));
    assert_eq!(f.publications.lock().unwrap().posts, 1);
    f.stop().await;
}

#[tokio::test]
async fn native_grant_publication_resource_access_revocation_blocks_new_review_without_refund() {
    let mut f = Fixture::new().await;
    let id = f.recorded().await;
    let cookie = f.service.cookie().await.unwrap();
    let io = f
        .service
        .io(&cookie, Permission::ReadResources)
        .await
        .unwrap();
    f.registry
        .prepare_revoke(
            io.profile.clone(),
            "revoke_resource".into(),
            f.resource.clone(),
            io.deadline,
        )
        .unwrap()
        .approve()
        .await
        .unwrap();
    drop(io);
    let error = f
        .service
        .publication_review(&f.registry, id)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "native_grant_not_publishable");
    assert_eq!(f.publications.lock().unwrap().posts, 0);
    assert_eq!(f.count("owner_capacity_grants"), 1);
    assert!(f.history().await.data.records[0].publication.is_none());
    f.stop().await;
}
