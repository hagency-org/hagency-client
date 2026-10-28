//! Explicit cancellation of an original, never-attempted grant intent. Closing
//! a window is not cancellation; uncertain or recorded effects stay recoverable.
use super::*;

pub struct OwnerGrantCancellationReview {
    manager: Arc<NativeManagement>,
    identity: Identity,
    original: Original,
    deadline: Instant,
    gate: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl OwnerGrantCancellationReview {
    pub fn summary(&self) -> OwnerGrantSummary {
        self.original.summary()
    }
    pub fn server_origin(&self) -> &str {
        &self.identity.origin
    }
    pub fn expired(&self) -> bool {
        self.manager.is_retired() || Instant::now() >= self.deadline || !(self.gate)()
    }
}
pub enum OwnerGrantCancellationResult {
    Review(Box<OwnerGrantCancellationReview>),
    Progress(Box<OwnerGrantProgress>),
}
impl OwnerGrantIntents {
    /// Requires current owner authority but no provider login or live central
    /// admission. Withdrawal must not make an unattempted intent impossible to
    /// cancel after restart. The original ledger result always takes precedence.
    pub async fn review_cancellation(
        &self,
        registry: &OwnerResourceRegistry,
        intent_id: String,
    ) -> Result<OwnerGrantCancellationResult, NativeError> {
        super::super::key(&intent_id)?;
        let cookie = self.cookie().await?;
        let io = self.io(&cookie, Permission::ConsentGrants).await?;
        let _lane = self.manager.owner.0.management.mutations.lock().await;
        io.check().await?;
        let mut history = History::load(&io.path, io.identity.clone())?;
        let index = history.index(&intent_id)?;
        let progress = self.recovered(&io, registry, &mut history, index).await?;
        if !matches!(progress.state, OwnerGrantState::ReviewRequired | OwnerGrantState::ReviewExpired) {
            return Ok(OwnerGrantCancellationResult::Progress(Box::new(progress)));
        }
        Ok(OwnerGrantCancellationResult::Review(Box::new(OwnerGrantCancellationReview {
            manager: self.manager.clone(),
            identity: io.identity.clone(),
            original: history.data.records[index].original.clone(),
            deadline: io.deadline,
            gate: Arc::new(io.gate()),
        })))
    }

    pub async fn confirm_cancellation(
        &self,
        registry: &OwnerResourceRegistry,
        review: OwnerGrantCancellationReview,
    ) -> Result<OwnerGrantProgress, NativeError> {
        if !Arc::ptr_eq(&self.manager, &review.manager) || review.expired() {
            return Err(failure(401, "owner_grant_review_changed"));
        }
        let cookie = self.cookie().await?;
        let io = self.io(&cookie, Permission::ConsentGrants).await?;
        let _lane = self.manager.owner.0.management.mutations.lock().await;
        io.check().await?;
        if io.identity != review.identity || review.expired() {
            return Err(failure(401, "owner_grant_review_changed"));
        }
        let mut history = History::load(&io.path, io.identity.clone())?;
        let index = history.index(&review.original.intent_id)?;
        if history.data.records[index].original != review.original {
            return Err(history_error());
        }
        let progress = self.recovered(&io, registry, &mut history, index).await?;
        if !matches!(progress.state, OwnerGrantState::ReviewRequired | OwnerGrantState::ReviewExpired) {
            return Ok(progress);
        }
        if review.expired() {
            return Err(failure(401, "owner_grant_review_changed"));
        }
        // This lane also spans grant confirmation and its queued SQLite writer.
        // Applying is never reset merely because no receipt is visible yet.
        let record = &mut history.data.records[index];
        record.state = State::Declined;
        let result = record.progress(OwnerGrantState::Declined);
        history.save()?;
        io.check().await?;
        Ok(result)
    }
}
impl OwnerGrantAccess {
    pub async fn review_cancellation(&self, id: String) -> Result<OwnerGrantCancellationResult, NativeError> {
        self.service().review_cancellation(&self.registry, id).await
    }
    pub async fn confirm_cancellation(&self, review: OwnerGrantCancellationReview) -> Result<OwnerGrantProgress, NativeError> {
        self.service().confirm_cancellation(&self.registry, review).await
    }
}
