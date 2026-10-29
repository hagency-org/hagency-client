use super::*;

struct Fixture {
    _root: tempfile::TempDir,
    registry: OwnerResourceRegistry,
    profile: ProfileAuthority,
    resource: String,
    revision: String,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let registry = OwnerResourceRegistry::open_root(&root.path().join("owner")).unwrap();
        let profile = Self::profile();
        let receipt = registry
            .prepare_enrollment(
                profile.clone(),
                "period_resource".into(),
                VerifiedProviderAccount::new(
                    "codex",
                    "https://provider.test",
                    "period-fixture",
                    Instant::now() + Duration::from_secs(30),
                )
                .unwrap(),
                "gpt-5.6-sol".into(),
                None,
                1000,
                Instant::now() + Duration::from_secs(30),
            )
            .unwrap()
            .approve()
            .await
            .unwrap();
        let resource = receipt.resource_id;
        let revision = registry
            .resource(&profile, resource.clone())
            .await
            .unwrap()
            .revision;
        Self {
            _root: root,
            registry,
            profile,
            resource,
            revision,
        }
    }
    fn profile() -> ProfileAuthority {
        ProfileAuthority::new(
            "a".repeat(64),
            "@owner:test".into(),
            Instant::now() + Duration::from_secs(60),
        )
        .unwrap()
    }
    fn consent(&self, id: &str, now: u64, amount: u64) -> OwnerGrantConsent {
        let enrollment = VerifiedManagementEnrollment::new(
            self.profile.clone(),
            "period_enrollment".into(),
            "period_contribution".into(),
            self.resource.clone(),
            self.revision.clone(),
            "@coordinator:test".into(),
            "period_device".into(),
            1,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        self.registry
            .prepare_capacity_grant_at(
                enrollment,
                OwnerGrantSelection {
                    command_id: format!("command_{id}"),
                    grant_id: id.into(),
                    coordinator_mxid: "@coordinator:test".into(),
                    delegation_expires_at_ms: now + 60_000,
                    allow_self_approval: false,
                    eligible_managers: vec!["@manager:test".into()],
                    allocated_tokens: amount,
                },
                Instant::now() + Duration::from_secs(30),
                now,
            )
            .unwrap()
    }
    async fn apply(
        &self,
        consent: OwnerGrantConsent,
        start: u64,
        finish: u64,
    ) -> Result<NativeGrantReceipt, Error> {
        self.registry
            .0
            .store
            .call(32 * 1024, move |db| {
                let mut first = true;
                db.owner_capacity_grant_with_clock(consent, || {
                    let value = if first { start } else { finish };
                    first = false;
                    Ok(value)
                })
            })
            .await
    }
    async fn state(&self) -> (u64, u64, u64) {
        self.registry.0.store.call(1024, |db| {
            Ok(db.db.query_row("SELECT (SELECT COUNT(*) FROM owner_capacity_grants),(SELECT COUNT(*) FROM owner_capacity_receipts),revision FROM owner_resource_identity WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)
        }).await.unwrap()
    }
    async fn remaining(&self) -> u64 {
        self.registry
            .resource(&self.profile, self.resource.clone())
            .await
            .unwrap()
            .budget
            .seat
            .remaining
            .map(u64::from)
            .unwrap()
    }
    /// A consent on the fixture's single v2 admission, with no history check.
    fn admission_consent(&self, id: &str, now: u64, amount: u64) -> OwnerGrantConsent {
        self.consent(id, now, amount)
            .with_admission_request("period_admission".into())
            .unwrap()
    }
    /// A new month's grant on the fixture's admission.
    fn regrant(&self, id: &str, now: u64, amount: u64) -> OwnerGrantConsent {
        self.admission_consent(id, now, amount)
            .require_regrant_after_period_end()
            .unwrap()
    }
    /// The resource budget as the writer evaluates it at native time `at`.
    async fn remaining_at(&self, at: u64) -> Option<u64> {
        let resource = self.resource.clone();
        self.registry
            .0
            .store
            .call(4096, move |db| {
                let resource = read_resource(&db.db, &resource)?;
                Ok(
                    super::super::super::budget_at(&db.db, &resource, None, false, at)?
                        .seat
                        .remaining
                        .map(u64::from),
                )
            })
            .await
            .unwrap()
    }
}
fn boundary() -> u64 {
    month(graphs::now_ms().unwrap()).unwrap().2
}

#[tokio::test]
async fn displayed_period_is_exact_receipt_period_before_utc_boundary() {
    let f = Fixture::new().await;
    let last = boundary() - 1;
    let consent = f.consent("period_approved", last, 700);
    let review = serde_json::to_value(consent.period()).unwrap();
    assert_eq!(review["periodKey"], month(last).unwrap().0);
    assert_eq!(review["periodStartMs"], month(last).unwrap().1);
    assert_eq!(review["periodEndMs"], last + 1);
    let receipt = f.apply(consent, last, last).await.unwrap();
    assert_eq!(
        review,
        json!({"periodKey":receipt.period_key,"periodStartMs":receipt.period_start_ms,"periodEndMs":receipt.period_end_ms})
    );
    assert_eq!(f.remaining().await, 300);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn queued_consent_rejects_utc_rollover_without_a_hold_or_receipt() {
    let f = Fixture::new().await;
    let end = boundary();
    let consent = f.consent("queued_period", end - 1, 700);
    let original = f.state().await;
    let store = f.registry.0.store.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (unblock, blocked) = std::sync::mpsc::channel();
    let blocker = tokio::spawn(async move {
        store
            .call(1024, move |_| {
                started.send(()).unwrap();
                blocked.recv_timeout(Duration::from_secs(10)).unwrap();
                Ok(())
            })
            .await
            .unwrap();
    });
    ready.await.unwrap();
    let mut pending = Box::pin(f.apply(consent, end, end));
    tokio::select! {
        biased;
        result = &mut pending => panic!("writer was not blocked: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }
    unblock.send(()).unwrap();
    blocker.await.unwrap();
    assert!(matches!(pending.await, Err(Error::LocalAuthority)));
    assert_eq!(f.state().await, original);
    assert_eq!(f.remaining().await, 1000);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn rollover_during_transaction_rolls_back_inserted_receipt_and_revision() {
    let f = Fixture::new().await;
    let end = boundary();
    let original = f.state().await;
    assert!(matches!(
        f.apply(f.consent("mid_write", end - 1, 700), end - 1, end)
            .await,
        Err(Error::LocalAuthority)
    ));
    assert_eq!(f.state().await, original);
    assert_eq!(f.remaining().await, 1000);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn original_replay_and_recovery_preserve_old_period_and_hold() {
    let f = Fixture::new().await;
    let end = boundary();
    let first = f.consent("original_period", end - 1, 700);
    let replay = f.consent("original_period", end - 1, 700);
    let original = f.apply(first, end - 1, end - 1).await.unwrap();
    let saved = serde_json::to_value(&original).unwrap();
    let state = f.state().await;
    let result = f
        .registry
        .0
        .store
        .call(32 * 1024, move |db| {
            db.owner_capacity_grant_with_clock(replay, || {
                panic!("original receipt recovery must precede current-period validation")
            })
        })
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(result).unwrap(), saved);
    let recovered = f
        .registry
        .recover_capacity_grant(
            Fixture::profile(),
            "period_device".into(),
            original.grant_id.clone(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(recovered).unwrap(), saved);
    // The hold is kept for its own month: no more than the rest fits there.
    assert!(
        f.apply(f.consent("same_period", end - 1, 301), end - 1, end - 1)
            .await
            .is_err()
    );
    assert_eq!(f.state().await, state);
    assert_eq!(f.remaining().await, 300);
    // The ceiling is monthly. Once the month has ended, the unused allocation
    // of a grant without children no longer counts; its receipt is unchanged.
    assert_eq!(f.remaining_at(end - 1).await, Some(300));
    assert_eq!(f.remaining_at(end).await, Some(1000));
    let recovered = f
        .registry
        .recover_capacity_grant(
            Fixture::profile(),
            "period_device".into(),
            original.grant_id.clone(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(recovered).unwrap(), saved);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn same_admission_regrant_waits_for_month_end_and_is_refused_twice_in_a_month() {
    let f = Fixture::new().await;
    let end = boundary();
    let first = f
        .apply(
            f.admission_consent("first_month", end - 1, 700)
                .require_unissued_contribution()
                .unwrap(),
            end - 1,
            end - 1,
        )
        .await
        .unwrap();
    assert_eq!(
        first.admission_request_id.as_deref(),
        Some("period_admission")
    );
    // Before the boundary the month still runs: refused although 300 are free.
    let state = f.state().await;
    assert!(matches!(
        f.apply(f.regrant("early", end - 1, 300), end - 1, end - 1)
            .await,
        Err(Error::OwnerGrantReviewChanged)
    ));
    // Without the regrant restriction the same admission stays refused.
    assert!(matches!(
        f.apply(
            f.admission_consent("unissued", end, 300)
                .require_unissued_contribution()
                .unwrap(),
            end,
            end
        )
        .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        f.apply(
            f.admission_consent("readmitted", end, 300)
                .require_closed_prior_admissions()
                .unwrap(),
            end,
            end
        )
        .await,
        Err(Error::OwnerGrantReviewChanged)
    ));
    assert_eq!(f.state().await, state);
    // After the boundary the ended month's unused 700 is free again.
    assert_eq!(f.remaining_at(end).await, Some(1000));
    let next = f
        .apply(f.regrant("second_month", end, 600), end, end)
        .await
        .unwrap();
    assert_eq!(next.version, 2);
    assert_eq!(next.admission_request_id, first.admission_request_id);
    assert_eq!(next.enrollment_id, first.enrollment_id);
    assert_eq!(next.grant_revision, 1);
    assert_eq!(next.period_key, month(end).unwrap().0);
    assert_eq!(next.period_start_ms, end);
    assert!(next.ledger_revision > first.ledger_revision);
    assert_eq!(f.remaining_at(end).await, Some(400));
    // One grant per admission per month: a second one is refused with room left.
    let state = f.state().await;
    assert!(matches!(
        f.apply(f.regrant("twice", end + 1, 100), end + 1, end + 1)
            .await,
        Err(Error::OwnerGrantReviewChanged)
    ));
    assert_eq!(f.state().await, state);
    assert_eq!(f.remaining_at(end + 1).await, Some(400));
    // Both receipts stay exactly as issued.
    for receipt in [&first, &next] {
        let recovered = f
            .registry
            .recover_capacity_grant(
                Fixture::profile(),
                "period_device".into(),
                receipt.grant_id.clone(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(recovered).unwrap(),
            serde_json::to_value(receipt).unwrap()
        );
    }
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn regrant_needs_this_admission_history_and_every_other_admission_closed() {
    let f = Fixture::new().await;
    let end = boundary();
    // The restriction exists only for a v2 admission and excludes readmission.
    assert!(matches!(
        f.consent("v1", end, 100).require_regrant_after_period_end(),
        Err(Error::LocalAuthority)
    ));
    assert!(matches!(
        f.regrant("both", end, 100)
            .require_closed_prior_admissions(),
        Err(Error::LocalAuthority)
    ));
    assert!(matches!(
        f.admission_consent("both_other_way", end, 100)
            .require_closed_prior_admissions()
            .unwrap()
            .require_regrant_after_period_end(),
        Err(Error::LocalAuthority)
    ));
    // No earlier grant on this admission: a first grant is not a regrant.
    assert!(matches!(
        f.apply(f.regrant("first_as_regrant", end, 100), end, end)
            .await,
        Err(Error::OwnerGrantReviewChanged)
    ));
    // An earlier, still open grant of another admission blocks a regrant even
    // after this admission's own month has ended.
    f.apply(
        f.consent("other_admission", end - 1, 100)
            .with_admission_request("older_admission".into())
            .unwrap(),
        end - 1,
        end - 1,
    )
    .await
    .unwrap();
    f.apply(
        f.admission_consent("this_admission", end - 1, 100),
        end - 1,
        end - 1,
    )
    .await
    .unwrap();
    let state = f.state().await;
    assert!(matches!(
        f.apply(f.regrant("blocked", end, 100), end, end).await,
        Err(Error::OwnerGrantReviewChanged)
    ));
    assert_eq!(f.state().await, state);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn regrant_history_scan_stays_bounded_and_fails_closed() {
    let f = Fixture::new().await;
    let end = boundary();
    let first = f
        .apply(
            f.admission_consent("bounded_first", end - 1, 100),
            end - 1,
            end - 1,
        )
        .await
        .unwrap();
    // Unrelated grants sort after this pool's grant. With 2048 rows the scan
    // still reads them all; 2049 rows, which no writer creates, must be refused
    // rather than skipped.
    let scan = |extra: usize| {
        let first = first.clone();
        f.registry.0.store.call(64 * 1024, move |db| {
            let tx = db.db.transaction()?;
            let (profile, resource, preset, seat): (String, String, String, String) = tx.query_row(
                "SELECT profile,resource_id,preset_id,seat_id FROM owner_capacity_grants WHERE id='bounded_first'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
            for index in 0..extra {
                let mut other = first.clone();
                other.grant_id = format!("zz_other_{index:04}");
                other.contribution_id = "other_contribution".into();
                tx.execute(
                    "INSERT OR IGNORE INTO owner_capacity_grants(id,profile,resource_id,preset_id,seat_id,allocated_tokens,receipt,active,accounting_mode) VALUES(?1,?2,?3,?4,?5,1,?6,0,?7)",
                    params![other.grant_id, profile, resource, preset, seat, serde_json::to_string(&other)?, parent::RESERVATION_ONLY],
                )?;
            }
            tx.commit()?;
            let profile = Fixture::profile();
            let result = regrant::prior_periods_ended(&db.db, &profile, "period_device",
                "period_contribution", "period_admission", None, &OwnerGrantPeriod::at(end)?, end);
            Ok(result)
        })
    };
    assert_eq!(scan(2047).await.unwrap().unwrap(), ["bounded_first"]);
    assert!(matches!(scan(2048).await.unwrap(), Err(Error::Capacity)));
    let state = f.state().await;
    assert!(matches!(
        f.apply(f.regrant("bounded_next", end, 100), end, end).await,
        Err(Error::Capacity)
    ));
    assert_eq!(f.state().await, state);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn monotonic_expiry_is_independent_of_frozen_utc_period() {
    let f = Fixture::new().await;
    let now = graphs::now_ms().unwrap();
    let mut consent = f.consent("expired_consent", now, 700);
    consent.expires = Instant::now() - Duration::from_millis(1);
    let state = f.state().await;
    assert!(matches!(
        f.apply(consent, now, now).await,
        Err(Error::LocalAuthority)
    ));
    assert_eq!(f.state().await, state);
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn legacy_pre_period_digest_recovers_without_rewriting_original_evidence() {
    let f = Fixture::new().await;
    let end = boundary();
    let consent = f.consent("legacy_period", end - 1, 700);
    let e = &consent.enrollment;
    // Exact old local-command digest preimage: the original writer included
    // selection, but no reviewed UTC period. Its published receipt did include
    // the period and retains that same closed wire representation.
    let legacy_digest = canonical::digest(&json!({
        "operation":"owner.capacity.grant", "profile":e.profile.digest,
        "enrollment":e.enrollment_id, "contribution":e.contribution_id,
        "resource":e.resource_id, "resourceRevision":e.resource_revision,
        "device":e.executor_device_id, "generation":e.executor_generation,
        "selection":consent.selection
    }))
    .unwrap();
    let original = f.apply(consent, end - 1, end - 1).await.unwrap();
    let old = legacy_digest.clone();
    let original_bytes: String = f.registry.0.store.call(8192, move |db| {
        db.db.execute("UPDATE owner_capacity_receipts SET digest=?1 WHERE command_id='command_legacy_period'", [&old])?;
        Ok(db.db.query_row("SELECT result FROM owner_capacity_receipts WHERE command_id='command_legacy_period'", [], |r| r.get(0))?)
    }).await.unwrap();
    let recovered = f
        .registry
        .recover_capacity_grant(
            Fixture::profile(),
            "period_device".into(),
            original.grant_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_string(&recovered).unwrap(), original_bytes);
    // A fresh consent cannot reinterpret the old command using a new reviewed
    // period. Recovery is read-only and separate from new consent replay.
    assert!(matches!(
        f.apply(f.consent("legacy_period", end, 700), end, end)
            .await,
        Err(Error::Conflict)
    ));
    let saved: (String, String) = f.registry.0.store.call(8192, |db| {
        Ok(db.db.query_row("SELECT digest,result FROM owner_capacity_receipts WHERE command_id='command_legacy_period'", [], |r| Ok((r.get(0)?,r.get(1)?)))?)
    }).await.unwrap();
    assert_eq!(saved, (legacy_digest, original_bytes));
    assert_eq!(f.state().await.0, 1);
    assert_eq!(f.state().await.1, 1);
    assert_eq!(f.remaining().await, 300);
    f.registry.shutdown().await.unwrap();
}

// Test-only gate delays the caller's post-worker observation. Production gates
// remain synchronous/non-blocking; worker checks all run on a different thread.
type ReturnPause = (
    ProfileAuthority,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<Mutex<Option<Instant>>>,
);
fn pause_after_worker(profile: ProfileAuthority) -> ReturnPause {
    let caller = std::thread::current().id();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = calls.clone();
    let pause = Arc::new(Mutex::new(None::<Instant>));
    let boundary = pause.clone();
    let profile = profile
        .with_native_gate(move || {
            if std::thread::current().id() == caller
                && observed.fetch_add(1, Ordering::AcqRel) == 1
                && let Some(deadline) = *boundary.lock().unwrap()
            {
                std::thread::sleep(
                    deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(5),
                );
            }
            true
        })
        .unwrap();
    (profile, calls, pause)
}

#[tokio::test]
async fn grant_return_expiry_preserves_original_receipt_for_fresh_recovery() {
    for enrollment_expires_first in [false, true] {
        let mut f = Fixture::new().await;
        let (profile, calls, pause) = pause_after_worker(f.profile);
        f.profile = profile;
        let now = graphs::now_ms().unwrap();
        let mut consent = f.consent("return_expired", now, 700);
        let expires = Instant::now() + Duration::from_millis(300);
        if enrollment_expires_first {
            consent.enrollment.expires = expires;
        } else {
            consent.expires = expires;
        }
        *pause.lock().unwrap() = Some(expires);
        calls.store(0, Ordering::Release);
        assert!(matches!(
            consent.approve().await,
            Err(Error::OutcomeUnknown)
        ));
        let recovered = f
            .registry
            .recover_capacity_grant(
                Fixture::profile(),
                "period_device".into(),
                "return_expired".into(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered.allocated_tokens, 700);
        assert_eq!(f.state().await.0, 1);
        assert_eq!(f.state().await.1, 1);
        assert_eq!(f.remaining().await, 300);
        f.registry.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn resource_return_expiry_keeps_exact_receipt_and_no_duplicate_capacity() {
    for provider_expires_first in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let registry = OwnerResourceRegistry::open_root(&root.path().join("owner")).unwrap();
        let (profile, calls, pause) = pause_after_worker(Fixture::profile());
        let proof = || {
            VerifiedProviderAccount::new(
                "codex",
                "https://provider.test",
                "resource-return-fixture",
                Instant::now() + Duration::from_secs(30),
            )
            .unwrap()
        };
        let mut consent = registry
            .prepare_enrollment(
                profile,
                "resource_return".into(),
                proof(),
                "gpt-5.6-sol".into(),
                None,
                1000,
                Instant::now() + Duration::from_secs(30),
            )
            .unwrap();
        let expires = Instant::now() + Duration::from_millis(300);
        if provider_expires_first {
            let Action::Enroll { proof, .. } = &mut consent.action else {
                unreachable!()
            };
            proof.expires = expires;
        } else {
            consent.expires = expires;
        }
        *pause.lock().unwrap() = Some(expires);
        calls.store(0, Ordering::Release);
        assert!(matches!(
            consent.approve().await,
            Err(Error::OutcomeUnknown)
        ));
        let original: OwnerResourceReceipt = registry
            .0
            .store
            .call(8192, |db| {
                let value: String = db.db.query_row(
                    "SELECT result FROM owner_resource_receipts WHERE command_id='resource_return'",
                    [],
                    |r| r.get(0),
                )?;
                Ok(serde_json::from_str(&value)?)
            })
            .await
            .unwrap();
        let replay = registry
            .prepare_enrollment(
                Fixture::profile(),
                "resource_return".into(),
                proof(),
                "gpt-5.6-sol".into(),
                None,
                1000,
                Instant::now() + Duration::from_secs(30),
            )
            .unwrap()
            .approve()
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(&replay).unwrap()
        );
        let count: u64 = registry
            .0
            .store
            .call(1024, |db| {
                Ok(db
                    .db
                    .query_row("SELECT COUNT(*) FROM owner_resource_receipts", [], |r| {
                        r.get(0)
                    })?)
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            registry
                .resource(&Fixture::profile(), replay.resource_id)
                .await
                .unwrap()
                .budget
                .seat
                .remaining
                .map(u64::from),
            Some(1000)
        );
        registry.shutdown().await.unwrap();
    }
}
