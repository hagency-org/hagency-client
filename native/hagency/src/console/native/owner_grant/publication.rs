//! Separate native confirmation for publishing an already recorded local hold.
//! Private journal metadata never substitutes for a canonical ledger receipt.
use super::*;
use crate::console::native::AllocationTransport;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PublicationIntent {
    pub command_id: String,
    pub expected_contribution_revision: u64,
    pub confirmed: bool,
    /// A successor's publication also pins its predecessor's server revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_predecessor_revision: Option<u64>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnerGrantPublicationState {
    OutcomeUnknown,
    Rejected,
    ServerObserved,
}
/// Public descriptive projection only. No raw native receipt or provider ID.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerGrantPublicationProgress {
    pub summary: OwnerGrantSummary,
    pub state: OwnerGrantPublicationState,
    pub server_state: Option<String>,
    pub server_revision: Option<u64>,
    /// Native time of this authenticated response, not the historical receipt timestamp.
    pub observed_at_ms: Option<u64>,
}
/// The review is tied to its original native manager, journal and immutable CAS.
/// Dropping it does not publish, decline or remove the original local hold.
pub struct OwnerGrantPublicationReview {
    manager: Arc<NativeManagement>,
    identity: Identity,
    original: Original,
    pub(super) publication: PublicationIntent,
    deadline: Instant,
    gate: Arc<dyn Fn() -> bool + Send + Sync>,
    observed_at_ms: u64,
}
impl OwnerGrantPublicationReview {
    pub fn summary(&self) -> OwnerGrantSummary {
        self.original.summary()
    }
    pub fn server_origin(&self) -> &str {
        &self.identity.origin
    }
    pub fn contribution_revision(&self) -> u64 {
        self.publication.expected_contribution_revision
    }
    pub fn observed_at_ms(&self) -> u64 {
        self.observed_at_ms
    }
    pub fn expired(&self) -> bool {
        self.manager.is_retired()
            || Instant::now() >= self.deadline
            || !(self.gate)()
            || now_ms().map_or(true, |now| {
                now >= self.original.period.end_ms
                    || now >= self.original.selection.delegation_expires_at_ms
            })
    }
}
pub enum OwnerGrantPublicationResult {
    Review(Box<OwnerGrantPublicationReview>),
    Progress(Box<OwnerGrantPublicationProgress>),
}
impl OwnerGrantIntents {
    /// Native status lookup. Reading a grant must never create a publication
    /// intent or resume its effects, including after an interrupted confirmation.
    pub(super) async fn publication_observe(
        &self,
        registry: &OwnerResourceRegistry,
        id: String,
    ) -> Result<Option<OwnerGrantPublicationProgress>, NativeError> {
        use super::super::allocation_transport::PublicationObservation;
        super::super::key(&id)?;
        let cookie = self.cookie().await?;
        let io = self.io(&cookie, Permission::ReadIntents).await?;
        io.session
            .authorize(&io.context, Permission::PublishGrants)
            .map_err(storage)?;
        let lane = self.manager.owner.0.management.mutations.lock().await;
        io.check().await?;
        let history = History::load(&io.path, io.identity.clone())?;
        let record = &history.data.records[history.index(&id)?];
        if !io.accepts(&record.original.binding, registry) {
            return Err(failure(401, "owner_grant_binding_changed"));
        }
        let Some(publication) = record.publication.clone().filter(|p| p.confirmed) else {
            io.check().await?;
            return Ok(None);
        };
        let original = record.original.clone();
        let summary = record.summary();
        drop(lane);
        let transport = self.publication_transport();
        let result = match publication.expected_predecessor_revision {
            Some(predecessor) => {
                transport
                    .observe_successor_publication(
                        registry,
                        &publication.command_id,
                        &original.selection.grant_id,
                        publication.expected_contribution_revision,
                        predecessor,
                    )
                    .await?
            }
            None => {
                transport
                    .observe_publication(
                        registry,
                        &publication.command_id,
                        &original.selection.grant_id,
                        publication.expected_contribution_revision,
                    )
                    .await?
            }
        };
        io.check().await?;
        let mut progress = OwnerGrantPublicationProgress {
            summary,
            state: OwnerGrantPublicationState::OutcomeUnknown,
            server_state: None,
            server_revision: None,
            observed_at_ms: None,
        };
        match result {
            PublicationObservation::Unconfirmed => {}
            PublicationObservation::Rejected => {
                self.settle_succession(&io, registry, &original, false).await?;
                progress.state = OwnerGrantPublicationState::Rejected
            }
            PublicationObservation::ServerObserved(view) => {
                self.settle_succession(&io, registry, &original, true).await?;
                progress.state = OwnerGrantPublicationState::ServerObserved;
                progress.server_state = Some(view.state);
                progress.server_revision = Some(view.revision);
                progress.observed_at_ms = Some(now_ms()?);
            }
        }
        Ok(Some(progress))
    }

    /// Apply the server's decision on a successor natively: its observed
    /// acceptance makes it effective, its audited rejection rejected. Roots
    /// are unchanged.
    async fn settle_succession(
        &self,
        io: &NativeIo<'_>,
        registry: &OwnerResourceRegistry,
        original: &Original,
        accepted: bool,
    ) -> Result<(), NativeError> {
        if original.succession.is_none() {
            return Ok(());
        }
        let (profile, device, grant) = (
            io.profile.clone(),
            original.binding.executor_device_id.clone(),
            original.selection.grant_id.clone(),
        );
        if accepted {
            registry.confirm_capacity_grant_succession(profile, device, grant).await
        } else {
            registry.reject_capacity_grant_succession(profile, device, grant).await
        }
        .map_err(|_| failure(409, "management_outcome_unknown"))?;
        io.check().await
    }
    fn publication_transport(&self) -> AllocationTransport {
        let transport =
            AllocationTransport::new(self.manager.clone()).require_complete_publication_history();
        match &self.host_gate {
            Some(gate) => transport.restrict_to_host(gate.clone()),
            None => transport,
        }
    }
    pub(super) async fn publication_review(
        &self,
        registry: &OwnerResourceRegistry,
        id: String,
    ) -> Result<OwnerGrantPublicationResult, NativeError> {
        super::super::key(&id)?;
        let cookie = self.cookie().await?;
        let io = self.io(&cookie, Permission::PublishGrants).await?;
        io.session
            .authorize(&io.context, Permission::ReadIntents)
            .map_err(storage)?;
        let lane = self.manager.owner.0.management.mutations.lock().await;
        io.check().await?;
        let mut history = History::load(&io.path, io.identity.clone())?;
        let index = history.index(&id)?;
        let progress = self.recovered(&io, registry, &mut history, index).await?;
        if !matches!(progress.state, OwnerGrantState::Recorded) {
            return Err(failure(409, "native_grant_receipt_required"));
        }
        if history.data.records[index]
            .publication
            .as_ref()
            .is_some_and(|p| p.confirmed)
        {
            drop(lane);
            return self
                .publication_apply(registry, id, None)
                .await
                .map(|progress| OwnerGrantPublicationResult::Progress(Box::new(progress)));
        }
        let original = history.data.records[index].original.clone();
        Box::pin(self.check_grant_admission(&io, &original)).await?;
        if original.succession.is_some() && !Box::pin(self.supports_succession(&io)).await? {
            return Err(failure(409, "management_grant_succession_required"));
        }
        if history.data.records[index].publication.is_none() {
            self.publication_transport()
                .ensure_unpublished_intent(&original.selection.grant_id)
                .await?;
            io.check().await?;
        }
        let (revision, predecessor, observed_at_ms) = if original.succession.is_some() {
            let (revision, predecessor, observed) = self
                .publication_transport()
                .review_successor_publication(registry, &original.selection.grant_id)
                .await?;
            (revision, Some(predecessor), observed)
        } else {
            let (revision, observed) = self
                .publication_transport()
                .review_publication(registry, &original.selection.grant_id)
                .await?;
            (revision, None, observed)
        };
        io.check().await?;
        let publication = history.data.records[index]
            .publication
            .get_or_insert_with(|| PublicationIntent {
                // Random generation is fallible and performed below before saving.
                command_id: String::new(),
                expected_contribution_revision: revision,
                confirmed: false,
                expected_predecessor_revision: predecessor,
            });
        if publication.command_id.is_empty() {
            publication.command_id = random_id()?;
        }
        if publication.expected_contribution_revision != revision
            || publication.expected_predecessor_revision != predecessor
        {
            // Never silently refresh an old authorization into a new CAS.
            return Err(failure(409, "owner_grant_publication_review_changed"));
        }
        let publication = publication.clone();
        history.save()?;
        io.check().await?;
        Ok(OwnerGrantPublicationResult::Review(Box::new(
            OwnerGrantPublicationReview {
                manager: self.manager.clone(),
                identity: io.identity.clone(),
                original,
                publication,
                deadline: io.deadline,
                gate: Arc::new(io.gate()),
                observed_at_ms,
            },
        )))
    }
    pub(super) async fn publication_apply(
        &self,
        registry: &OwnerResourceRegistry,
        id: String,
        review: Option<OwnerGrantPublicationReview>,
    ) -> Result<OwnerGrantPublicationProgress, NativeError> {
        super::super::key(&id)?;
        let cookie = self.cookie().await?;
        let io = self.io(&cookie, Permission::PublishGrants).await?;
        io.session
            .authorize(&io.context, Permission::ReadIntents)
            .map_err(storage)?;
        let lane = self.manager.owner.0.management.mutations.lock().await;
        io.check().await?;
        let mut history = History::load(&io.path, io.identity.clone())?;
        let index = history.index(&id)?;
        let local = self.recovered(&io, registry, &mut history, index).await?;
        if !matches!(local.state, OwnerGrantState::Recorded) {
            return Err(failure(409, "native_grant_receipt_required"));
        }
        let record = &mut history.data.records[index];
        let publication = record.publication.as_mut().ok_or_else(history_error)?;
        if !publication.confirmed {
            let review = review
                .ok_or_else(|| failure(409, "owner_grant_publication_confirmation_required"))?;
            if !Arc::ptr_eq(&review.manager, &self.manager)
                || review.identity != io.identity
                || review.original != record.original
                || review.publication != *publication
            {
                return Err(history_error());
            }
            if review.expired() {
                return Err(failure(409, "owner_grant_publication_review_expired"));
            }
            publication.confirmed = true;
            // Durable confirmation is written before the transport may journal
            // or send anything. A crash here resumes this exact original CAS.
        }
        let publication = publication.clone();
        let original = record.original.clone();
        history.save()?;
        io.check()
            .await
            .map_err(|_| failure(409, "management_outcome_unknown"))?;
        let summary = history.data.records[index].summary();
        drop(lane); // AllocationTransport owns this same mutation lane.
        let transport = self.publication_transport();
        let result = match publication.expected_predecessor_revision {
            Some(predecessor) => {
                transport
                    .publish_successor(
                        registry,
                        publication.command_id.clone(),
                        original.selection.grant_id.clone(),
                        publication.expected_contribution_revision,
                        predecessor,
                    )
                    .await
            }
            None => {
                transport
                    .publish_grant(
                        registry,
                        publication.command_id.clone(),
                        original.selection.grant_id.clone(),
                        publication.expected_contribution_revision,
                    )
                    .await
            }
        };
        // Even a valid response after retirement remains unknown to this host;
        // AllocationTransport leaves its original pending command recoverable.
        io.check()
            .await
            .map_err(|_| failure(409, "management_outcome_unknown"))?;
        let mut progress = OwnerGrantPublicationProgress {
            summary,
            state: OwnerGrantPublicationState::OutcomeUnknown,
            server_state: None,
            server_revision: None,
            observed_at_ms: None,
        };
        match result {
            Ok(view) => {
                self.settle_succession(&io, registry, &original, true).await?;
                progress.state = OwnerGrantPublicationState::ServerObserved;
                progress.server_state = Some(view.state);
                progress.server_revision = Some(view.revision);
                progress.observed_at_ms = Some(now_ms()?);
            }
            Err(_) => {
                let rejected = if publication.expected_predecessor_revision.is_some() {
                    transport
                        .successor_publication_rejected(&publication.command_id)
                        .await?
                } else {
                    transport.publication_rejected(&publication.command_id).await?
                };
                if rejected {
                    self.settle_succession(&io, registry, &original, false).await?;
                    progress.state = OwnerGrantPublicationState::Rejected;
                }
            }
        }
        io.check()
            .await
            .map_err(|_| failure(409, "management_outcome_unknown"))?;
        Ok(progress)
    }
}
impl OwnerGrantAccess {
    /// Observe existing publication state only. Unlike explicit recovery, this
    /// does not retry a previously confirmed command or prepare a new review.
    pub async fn observe_publication(
        &self,
        id: String,
    ) -> Result<Option<OwnerGrantPublicationProgress>, NativeError> {
        self.service().publication_observe(&self.registry, id).await
    }

    /// Native UI only: returns original publication review, or recovers an
    /// already confirmed original command. Never accepts script command/CAS.
    pub async fn review_publication(
        &self,
        id: String,
    ) -> Result<OwnerGrantPublicationResult, NativeError> {
        self.service().publication_review(&self.registry, id).await
    }
    pub async fn confirm_publication(
        &self,
        review: OwnerGrantPublicationReview,
    ) -> Result<OwnerGrantPublicationProgress, NativeError> {
        let id = review.original.intent_id.clone();
        self.service()
            .publication_apply(&self.registry, id, Some(review))
            .await
    }
    pub async fn recover_publication(
        &self,
        id: String,
    ) -> Result<OwnerGrantPublicationResult, NativeError> {
        self.review_publication(id).await
    }
}
