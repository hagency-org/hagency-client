//! Native successor intents for a published pool grant: this month's
//! amendment or next month's renewal. A script may only propose terms; the
//! owner confirms them natively, then the existing publication confirmation
//! publishes the successor to `grants/{predecessor}/successors`.
use super::*;

/// The recorded, server-active, natively governing grant of a pool's chain.
pub(super) struct Head {
    pub(super) original: Original,
    pub(super) summary: OwnerGrantSummary,
    pub(super) receipt: NativeGrantReceipt,
}

fn pending() -> NativeError {
    failure(409, "owner_grant_succession_pending")
}
fn predecessor_required() -> NativeError {
    failure(409, "owner_grant_predecessor_required")
}

impl OwnerGrantIntents {
    /// Resolve every original of `contribution` and return its chain head:
    /// the grant governing the pool now, recorded natively, open, with no
    /// pending successor, and observed active on the server.
    pub(super) async fn published_head(
        &self,
        io: &NativeIo<'_>,
        registry: &OwnerResourceRegistry,
        contribution: &str,
    ) -> Result<Head, NativeError> {
        // 1. No unresolved original; collect successors still pending natively.
        let pending_successors = {
            let _lane = self.manager.owner.0.management.mutations.lock().await;
            io.check().await?;
            let mut history = History::load(&io.path, io.identity.clone())?;
            let mut out = Vec::new();
            for index in 0..history.data.records.len() {
                if history.data.records[index].original.binding.contribution_id != contribution {
                    continue;
                }
                let progress = self.recovered(io, registry, &mut history, index).await?;
                match progress.state {
                    OwnerGrantState::Declined => {}
                    OwnerGrantState::Recorded => {
                        if progress.succession_state == Some(OwnerGrantSuccessionState::Pending) {
                            out.push(progress.summary.intent_id.clone());
                        }
                    }
                    _ => return Err(pending()),
                }
            }
            out
        };
        // 2. A pending successor flips only by observing its publication.
        for intent in pending_successors {
            let observed = self.publication_observe(registry, intent).await?;
            if !observed.is_some_and(|p| {
                !matches!(p.state, OwnerGrantPublicationState::OutcomeUnknown)
            }) {
                return Err(pending());
            }
        }
        // 3. The grant governing the latest recorded chain now.
        let (original, summary, receipt) = {
            let _lane = self.manager.owner.0.management.mutations.lock().await;
            io.check().await?;
            let history = History::load(&io.path, io.identity.clone())?;
            let latest = history
                .data
                .records
                .iter()
                .rev()
                .find(|r| {
                    r.original.binding.contribution_id == contribution
                        && r.state == State::Recorded
                })
                .ok_or_else(predecessor_required)?;
            let head = registry
                .capacity_grant_chain_head(
                    io.profile.clone(),
                    io.identity.device.clone(),
                    latest.original.selection.grant_id.clone(),
                )
                .await
                .map_err(storage)?;
            io.check().await?;
            if head.successor.as_ref().is_some_and(|s| s.state == "pending") {
                return Err(pending());
            }
            if head.successor.is_some() {
                return Err(failure(409, "owner_grant_successor_exists"));
            }
            let now = now_ms()?;
            let r = &head.effective;
            if !head.open
                || !matches!(r.version, 2 | 3)
                || now < r.period_start_ms
                || now >= r.period_end_ms
                || now >= r.delegation_expires_at_ms
            {
                return Err(predecessor_required());
            }
            let record = history
                .data
                .records
                .iter()
                .find(|x| {
                    x.original.selection.grant_id == r.grant_id && x.state == State::Recorded
                })
                .ok_or_else(predecessor_required)?;
            if !io.accepts(&record.original.binding, registry) {
                return Err(failure(401, "owner_grant_binding_changed"));
            }
            (record.original.clone(), record.summary(), head.effective)
        };
        // 4. Its publication was accepted and the server still holds it active.
        let observed = self
            .publication_observe(registry, original.intent_id.clone())
            .await?;
        if !observed.is_some_and(|p| {
            matches!(p.state, OwnerGrantPublicationState::ServerObserved)
                && p.server_state.as_deref() == Some("active")
        }) {
            return Err(predecessor_required());
        }
        Ok(Head {
            original,
            summary,
            receipt,
        })
    }

    async fn succession_facts(
        &self,
        io: &NativeIo<'_>,
        registry: &OwnerResourceRegistry,
        head: &Head,
        kind: OwnerGrantSuccessionKind,
    ) -> Result<CapacityGrantSuccessionFacts, NativeError> {
        let facts = registry
            .capacity_grant_succession_facts(io.profile.clone(), head.receipt.clone(), kind.wire().into())
            .await
            .map_err(storage)?;
        io.check().await?;
        let now = now_ms()?;
        if now < facts.window_opens_at_ms || now >= facts.window_closes_at_ms {
            return Err(match kind {
                OwnerGrantSuccessionKind::Renewal => failure(409, "owner_grant_renewal_window_closed"),
                OwnerGrantSuccessionKind::Amendment => predecessor_required(),
            });
        }
        Ok(facts)
    }

    /// Read-only form facts. Opening it creates no hold, intent or publication.
    pub async fn succession_form(
        &self,
        registry: &OwnerResourceRegistry,
        contribution: String,
        kind: OwnerGrantSuccessionKind,
    ) -> Result<OwnerGrantSuccessionForm, NativeError> {
        super::super::key(&contribution)?;
        let cookie = self.cookie().await?;
        let io = self.io(&cookie, Permission::ConsentGrants).await?;
        io.session
            .authorize(&io.context, Permission::PublishGrants)
            .map_err(storage)?;
        if !Box::pin(self.supports_succession(&io)).await? {
            return Err(failure(409, "management_grant_succession_required"));
        }
        let head = Box::pin(self.published_head(&io, registry, &contribution)).await?;
        let facts = self.succession_facts(&io, registry, &head, kind).await?;
        let form = Box::pin(self.publication_form(registry, contribution.clone())).await?;
        io.check().await?;
        if form.resource_id != head.receipt.resource_id
            || form.context.resource_revision != head.receipt.resource_revision
        {
            return Err(failure(409, "management_enrollment_changed"));
        }
        let mut context = form.context;
        context.remaining_tokens = facts.max_tokens;
        Ok(OwnerGrantSuccessionForm {
            kind,
            contribution_id: contribution,
            resource_id: form.resource_id,
            label: form.label,
            context,
            predecessor: head.summary,
            floor: OwnerGrantFloor {
                children: facts.carried_children,
                retained_tokens: facts.floor_tokens,
            },
            period_key: facts.period_key,
            period_start_ms: facts.period_start_ms,
            period_end_ms: facts.period_end_ms,
            window_opens_at_ms: facts.window_opens_at_ms,
            window_closes_at_ms: facts.window_closes_at_ms,
        })
    }

    /// Record a reviewable successor intent. Nothing is reserved until the
    /// owner confirms the review natively.
    pub async fn prepare_successor(
        &self,
        registry: &OwnerResourceRegistry,
        enrollment: VerifiedManagementEnrollment,
        contribution: String,
        kind: OwnerGrantSuccessionKind,
        choices: OwnerGrantChoices,
    ) -> Result<OwnerGrantReview, NativeError> {
        super::super::key(&contribution)?;
        let cookie = self.cookie().await?;
        let io = self.io(&cookie, Permission::ConsentGrants).await?;
        io.check().await?;
        if choices.coordinator_mxid != io.identity.owner {
            return Err(failure(409, "resource_owner_is_coordinator"));
        }
        if !Box::pin(self.supports_succession(&io)).await? {
            return Err(failure(409, "management_grant_succession_required"));
        }
        let head = Box::pin(self.published_head(&io, registry, &contribution)).await?;
        let facts = self.succession_facts(&io, registry, &head, kind).await?;
        let p = &head.receipt;
        if p.eligible_managers
            .iter()
            .any(|m| choices.eligible_managers.binary_search(m).is_err())
        {
            return Err(failure(409, "owner_grant_manager_removed"));
        }
        if choices.allocated_tokens < facts.floor_tokens.max(1) {
            return Err(failure(409, "owner_grant_below_floor"));
        }
        let _lane = self.manager.owner.0.management.mutations.lock().await;
        io.check().await?;
        let selection = OwnerGrantSelection {
            command_id: random_id()?,
            grant_id: random_id()?,
            coordinator_mxid: choices.coordinator_mxid,
            delegation_expires_at_ms: choices.delegation_expires_at_ms,
            // Identity of the chain: never changed by a successor.
            allow_self_approval: p.allow_self_approval,
            eligible_managers: choices.eligible_managers,
            allocated_tokens: choices.allocated_tokens,
        };
        let admission = Box::pin(self.check_grant_admission(&io, &head.original)).await?;
        let request = p.admission_request_id.clone().ok_or_else(history_error)?;
        if admission.request_id != request {
            return Err(failure(409, "management_enrollment_changed"));
        }
        let consent = registry
            .prepare_capacity_grant_successor(
                enrollment.with_admission_request(request).map_err(storage)?,
                p.clone(),
                kind.wire(),
                selection,
                io.deadline,
            )
            .map_err(storage)?
            .with_native_confirmation_gate(io.gate())
            .map_err(storage)?;
        let binding = Binding::from_native(&consent.binding().map_err(storage)?);
        let original = Original {
            intent_id: random_id()?,
            binding,
            admission: Some(admission.clone()),
            receipt_version: 3,
            selection: Selection::from_native(consent.selection()),
            period: Period::from_native(consent.period()),
            succession: Some(OriginalSuccession {
                kind,
                predecessor_intent_id: head.original.intent_id.clone(),
                predecessor_grant_id: p.grant_id.clone(),
                predecessor_receipt_digest: digest(p)?,
                predecessor_grant_revision: p.grant_revision,
                predecessor_allocated_tokens: p.allocated_tokens,
                reviewed_children: facts.carried_children,
                reviewed_floor_tokens: facts.floor_tokens,
            }),
        };
        if !io.accepts(&original.binding, registry)
            || original.binding.executor_generation != io.device.generation()
        {
            return Err(failure(401, "owner_grant_binding_changed"));
        }
        let (consent, mut context) = self.review_context(&io, registry, consent).await?;
        context.remaining_tokens = facts.max_tokens;
        let mut history = History::load(&io.path, io.identity.clone())?;
        history.insert_successor(original.clone())?;
        io.check().await?;
        consent.binding().map_err(storage)?;
        Ok(OwnerGrantReview {
            manager: self.manager.clone(),
            identity: io.identity,
            original,
            admission,
            consent,
            context,
            regrant: false,
        })
    }

    /// Rebuild the consent of an unattempted successor intent for a fresh
    /// review. The predecessor is its exact recorded receipt.
    pub(super) async fn successor_consent(
        &self,
        io: &NativeIo<'_>,
        registry: &OwnerResourceRegistry,
        enrollment: VerifiedManagementEnrollment,
        original: &Original,
    ) -> Result<(OwnerGrantConsent, Option<u64>), NativeError> {
        let link = original.succession.as_ref().ok_or_else(history_error)?;
        let receipt = registry
            .recover_capacity_grant(
                io.profile.clone(),
                original.binding.executor_device_id.clone(),
                link.predecessor_grant_id.clone(),
            )
            .await
            .map_err(storage)?
            .ok_or_else(predecessor_required)?;
        if digest(&receipt)? != link.predecessor_receipt_digest {
            return Err(history_error());
        }
        let request = receipt.admission_request_id.clone().ok_or_else(history_error)?;
        let facts = registry
            .capacity_grant_succession_facts(io.profile.clone(), receipt.clone(), link.kind.wire().into())
            .await
            .map_err(storage)?;
        let consent = registry
            .prepare_capacity_grant_successor(
                enrollment.with_admission_request(request).map_err(storage)?,
                receipt,
                link.kind.wire(),
                original.selection.native(),
                io.deadline,
            )
            .map_err(|_| failure(409, "owner_grant_original_review_expired"))?
            .with_native_confirmation_gate(io.gate())
            .map_err(storage)?;
        Ok((consent, facts.max_tokens))
    }
}

impl OwnerGrantAccess {
    /// Whether the server accepts successors (`pool-grant-succession-v1`).
    pub async fn supports_succession(&self) -> Result<bool, NativeError> {
        let service = self.service();
        let cookie = service.cookie().await?;
        let io = service.io(&cookie, Permission::ReadResources).await?;
        let supported = Box::pin(service.supports_succession(&io)).await?;
        io.check().await?;
        Ok(supported)
    }
    /// Native successor form facts for the pool's published chain head.
    pub async fn succession_form(
        &self,
        contribution: String,
        kind: OwnerGrantSuccessionKind,
    ) -> Result<OwnerGrantSuccessionForm, NativeError> {
        self.service()
            .succession_form(&self.registry, contribution, kind)
            .await
    }
    /// Prepare the owner's natively chosen successor terms for review.
    pub async fn prepare_successor(
        &self,
        contribution: String,
        kind: OwnerGrantSuccessionKind,
        choices: OwnerGrantChoices,
    ) -> Result<OwnerGrantReview, NativeError> {
        let service = self.service();
        let proof = service
            .verified_enrollment(&self.registry, contribution.clone())
            .await?;
        service
            .prepare_successor(&self.registry, proof, contribution, kind, choices)
            .await
    }
}
