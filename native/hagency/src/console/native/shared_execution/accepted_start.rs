//! Explicit native preparation versus background resumption of existing custody.
use super::*;
use std::sync::Weak;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedSuccessorReview {
    pub agent_name: String,
    pub project_id: String,
    pub assignment_id: String,
    pub predecessor_command_id: String,
    pub process_attempt: u64,
    pub allocated_tokens: u64,
    pub known_consumed_tokens: u64,
    pub retained_tokens: u64,
    pub remaining_tokens: u64,
    pub usage_complete: bool,
}
/// Immutable display context from the original native reservation. This is not
/// a start permission and contains no provider identity or credential.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedSuccessorDescription {
    pub agent_name: String,
    pub project_id: String,
    pub assignment_id: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedSuccessorBlockReason {
    Exhausted,
    UsageUnresolved,
}
/// Read-only evidence. A blocked assessment never contains a Start handle.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedSuccessorBudgetReview {
    #[serde(flatten)]
    pub description: SharedSuccessorDescription,
    pub reason: SharedSuccessorBlockReason,
    pub allocated_tokens: u64,
    pub known_consumed_tokens: u64,
    pub retained_tokens: u64,
    pub remaining_tokens: Option<u64>,
    pub known_overrun_tokens: u64,
    pub usage_complete: bool,
}
pub enum SharedSuccessorAssessment {
    Ready(Box<PreparedSharedSuccessor>),
    Blocked(SharedSuccessorBudgetReview),
}
fn blocked_reason(
    state: SharedSuccessorBudgetState,
    complete: bool,
    consumed: u64,
    allocated: u64,
) -> Option<SharedSuccessorBlockReason> {
    match state {
        SharedSuccessorBudgetState::Available => None,
        SharedSuccessorBudgetState::Exhausted if complete || consumed >= allocated => {
            Some(SharedSuccessorBlockReason::Exhausted)
        }
        SharedSuccessorBudgetState::Exhausted | SharedSuccessorBudgetState::Unrepresentable => {
            Some(SharedSuccessorBlockReason::UsageUnresolved)
        }
    }
}
fn description(target: &SharedSuccessorSelection) -> SharedSuccessorDescription {
    SharedSuccessorDescription {
        agent_name: target.original().reserve_command.definition.name.clone(),
        project_id: target.original().reserve_command.project_id.clone(),
        assignment_id: target.original().assignment_id.clone(),
    }
}
/// Native-only prepared target. Descriptions do not constitute new owner consent
/// or execution permission; Start recomputes all live writer authority.
pub struct PreparedSharedSuccessor {
    manager: Weak<NativeManagement>,
    registry: String,
    target: SharedSuccessorSelection,
    review: SharedSuccessorReview,
    expires: Instant,
    caller_gate: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}
impl PreparedSharedSuccessor {
    pub fn expired(&self) -> bool {
        Instant::now() >= self.expires || self.caller_gate.as_ref().is_some_and(|gate| !gate())
    }
    /// Restriction only: no script can deserialize or construct this preparation.
    /// Caller lease ends at original submission; retained native process custody
    /// continues under the original service/owner gates after acceptance.
    pub fn restrict_to_host(
        mut self,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self, NativeError> {
        let prior = self.caller_gate.take();
        self.caller_gate = Some(Arc::new(move || {
            gate() && prior.as_ref().is_none_or(|p| p())
        }));
        if self.expired() {
            return Err(failure(409, "shared_successor_preparation_changed"));
        }
        Ok(self)
    }
    pub fn review(&self) -> &SharedSuccessorReview {
        &self.review
    }
}
/// The cursor remains available when reporting fails, so one unavailable saved
/// request cannot starve other original intents. No synthesized completion state.
pub struct SharedSuccessorResume {
    pub intent_id: String,
    pub outcome: Result<SharedExecutionProgress, NativeError>,
}
impl SharedExecutionTransport {
    pub async fn prepare_successor(
        &self,
        r: &OwnerResourceRegistry,
        assignment: String,
    ) -> Result<PreparedSharedSuccessor, NativeError> {
        match self.assess_successor(r, assignment).await? {
            SharedSuccessorAssessment::Ready(prepared) => Ok(*prepared),
            SharedSuccessorAssessment::Blocked(_) => {
                Err(failure(409, "shared_successor_capacity_unavailable"))
            }
        }
    }
    pub async fn successor_description(
        &self,
        r: &OwnerResourceRegistry,
        assignment: String,
    ) -> Result<SharedSuccessorDescription, NativeError> {
        if !cfg!(feature = "shared-successors") {
            return Err(failure(409, "shared_successors_disabled"));
        }
        super::super::key(&assignment)?;
        let b = self.io.bound().await?;
        let target = r
            .shared_successor_selection(b.scope.profile.clone(), assignment)
            .await
            .map_err(store)?
            .ok_or_else(|| failure(404, "shared_successor_original_missing"))?;
        io::binding(&b.scope, target.original(), false)?;
        let result = description(&target);
        b.transport.check_scope(&b.scope, false).await?;
        Ok(result)
    }
    pub async fn assess_successor(
        &self,
        r: &OwnerResourceRegistry,
        assignment: String,
    ) -> Result<SharedSuccessorAssessment, NativeError> {
        if !cfg!(feature = "shared-successors") {
            return Err(failure(409, "shared_successors_disabled"));
        }
        super::super::key(&assignment)?;
        let b = self.io.bound().await?;
        let target = r
            .shared_successor_selection(b.scope.profile.clone(), assignment.clone())
            .await
            .map_err(store)?
            .ok_or_else(|| failure(404, "shared_successor_original_missing"))?;
        let e = b.enrollment(r, target.original(), true).await?;
        let budget = if let Some(head) = target.predecessor() {
            r.shared_attempt_successor_budget(e, head.clone()).await
        } else {
            r.shared_successor_budget(e, target.original().clone())
                .await
        }
        .map_err(store)?;
        if let Some(reason) = blocked_reason(
            budget.state,
            budget.usage_complete,
            budget.known_consumed_tokens,
            budget.allocated_tokens,
        ) {
            let review = SharedSuccessorBudgetReview {
                description: description(&target),
                reason,
                allocated_tokens: budget.allocated_tokens,
                known_consumed_tokens: budget.known_consumed_tokens,
                retained_tokens: budget.retained_tokens,
                remaining_tokens: (budget.state != SharedSuccessorBudgetState::Unrepresentable)
                    .then_some(budget.remaining_tokens),
                known_overrun_tokens: budget
                    .known_consumed_tokens
                    .saturating_sub(budget.allocated_tokens),
                usage_complete: budget.usage_complete,
            };
            b.transport.check_scope(&b.scope, false).await?;
            return Ok(SharedSuccessorAssessment::Blocked(review));
        }
        let (id, number) = target
            .predecessor()
            .map(|h| (h.command_id.clone(), h.process_attempt))
            .unwrap_or_else(|| (target.original().command_id.clone(), 1));
        let review = SharedSuccessorReview {
            agent_name: target.original().reserve_command.definition.name.clone(),
            project_id: target.original().reserve_command.project_id.clone(),
            assignment_id: assignment,
            predecessor_command_id: id,
            process_attempt: number.checked_add(1).ok_or_else(invalid)?,
            allocated_tokens: budget.allocated_tokens,
            known_consumed_tokens: budget.known_consumed_tokens,
            retained_tokens: budget.retained_tokens,
            remaining_tokens: budget.remaining_tokens,
            usage_complete: budget.usage_complete,
        };
        b.transport.check_scope(&b.scope, false).await?;
        Ok(SharedSuccessorAssessment::Ready(Box::new(
            PreparedSharedSuccessor {
                manager: Arc::downgrade(&self.manager),
                registry: r.id().into(),
                target,
                review,
                expires: b.scope.deadline,
                caller_gate: None,
            },
        )))
    }
    pub async fn start_prepared_successor(
        &self,
        r: &OwnerResourceRegistry,
        prepared: PreparedSharedSuccessor,
    ) -> Result<SharedExecutionProgress, NativeError> {
        if !cfg!(feature = "shared-successors") {
            return Err(failure(409, "shared_successors_disabled"));
        }
        if prepared.expired()
            || prepared.registry != r.id()
            || prepared
                .manager
                .upgrade()
                .is_none_or(|m| !Arc::ptr_eq(&m, &self.manager))
        {
            return Err(failure(409, "shared_successor_preparation_changed"));
        }
        let (original, predecessor) = prepared.target.into_parts();
        self.start_successor_scoped(r, original, predecessor, prepared.caller_gate)
            .await
    }
    async fn resume_saved_start(
        &self,
        r: &OwnerResourceRegistry,
        saved: SharedAttemptStartIntent,
    ) -> Result<SharedExecutionProgress, NativeError> {
        let b = self.io.bound().await?;
        io::binding(&b.scope, saved.original(), false)?;
        let _cleanup = b.cleanup(r, saved.original()).await?;
        b.transport.check_scope(&b.scope, false).await?;
        self.process_attempt(
            r,
            attempt_io::AttemptAction::Start(saved.input().clone()),
            None,
            None,
        )
        .await
    }
    pub async fn resume_successor(
        &self,
        r: &OwnerResourceRegistry,
        assignment: String,
    ) -> Result<Option<SharedExecutionProgress>, NativeError> {
        if !cfg!(feature = "shared-successors") {
            return Err(failure(409, "shared_successors_disabled"));
        }
        super::super::key(&assignment)?;
        let b = self.io.bound().await?;
        let saved = r
            .latest_shared_attempt_start(b.scope.profile.clone(), assignment)
            .await
            .map_err(store)?;
        b.transport.check_scope(&b.scope, false).await?;
        match saved {
            Some(saved) => self.resume_saved_start(r, saved).await.map(Some),
            None => Ok(None),
        }
    }
    /// Background recovery is deliberately unable to call preparation or choose
    /// an identifier. Only an existing immutable journal row can be submitted.
    pub async fn resume_next_successor(
        &self,
        r: &OwnerResourceRegistry,
        after: Option<String>,
    ) -> Result<Option<SharedSuccessorResume>, NativeError> {
        if !cfg!(feature = "shared-successors") {
            return Err(failure(409, "shared_successors_disabled"));
        }
        let b = self.io.bound().await?;
        let saved = r
            .next_shared_attempt_start(b.scope.profile.clone(), after)
            .await
            .map_err(store)?;
        b.transport.check_scope(&b.scope, false).await?;
        let Some(saved) = saved else { return Ok(None) };
        let intent_id = saved.input().start_intent_id.clone();
        let outcome = self.resume_saved_start(r, saved).await;
        Ok(Some(SharedSuccessorResume { intent_id, outcome }))
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn successor_review_distinguishes_spent_from_uncertain_holds() {
        use SharedSuccessorBlockReason::UsageUnresolved;
        use SharedSuccessorBudgetState::*;
        assert_eq!(
            blocked_reason(Exhausted, true, 4813, 4096),
            Some(SharedSuccessorBlockReason::Exhausted)
        );
        assert_eq!(
            blocked_reason(SharedSuccessorBudgetState::Exhausted, false, 17, 4096),
            Some(UsageUnresolved)
        );
        assert_eq!(
            blocked_reason(SharedSuccessorBudgetState::Exhausted, false, 4813, 4096),
            Some(SharedSuccessorBlockReason::Exhausted)
        );
        assert_eq!(
            blocked_reason(Unrepresentable, false, 4813, 4096),
            Some(UsageUnresolved)
        );
        assert_eq!(blocked_reason(Available, false, 17, 4096), None);
    }
}
