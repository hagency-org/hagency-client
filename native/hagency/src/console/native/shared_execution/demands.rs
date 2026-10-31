//! Native B consumer. Saved demand requests never enter manual Start recovery.
use super::demand_io::{DemandAction, Status};
use super::*;

#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SharedDemandProgress {
    Idle,
    Pending {
        demand_id: String,
    },
    Blocked {
        demand_id: String,
        reason: String,
    },
    Historical {
        demand_id: String,
        status: String,
    },
    Execution {
        demand_id: String,
        progress: SharedExecutionProgress,
    },
}
/// Advance this cursor even when the selected demand's independent operation
/// fails. It names an assignment, never a newly generated Start request.
pub struct SharedDemandPoll {
    pub next_after: Option<String>,
    pub outcome: Result<SharedDemandProgress, NativeError>,
}
pub struct SharedDemandResume {
    pub demand_id: String,
    pub outcome: Result<SharedDemandProgress, NativeError>,
}
fn enabled() -> Result<(), NativeError> {
    if cfg!(feature = "shared-intake") {
        Ok(())
    } else {
        Err(failure(409, "shared_intake_disabled"))
    }
}
async fn offer_proof(
    b: &Bound,
    r: &OwnerResourceRegistry,
    c: &SharedRuntimeCommand,
    s: &Status,
    observed: u64,
) -> Result<VerifiedSharedDemand, NativeError> {
    s.validate()?;
    if s.offer.assignment_id != c.assignment_id {
        return Err(invalid());
    }
    let enrollment = b.enrollment(r, c, true).await?;
    VerifiedSharedDemand::new(
        enrollment,
        s.offer.clone(),
        s.demand_digest.clone(),
        observed,
        b.scope
            .deadline
            .min(Instant::now() + Duration::from_secs(30)),
    )
    .map_err(store)
}
impl SharedExecutionTransport {
    #[cfg(all(test, feature = "shared-intake"))]
    pub(in crate::console::native) async fn poll_demand_fixture(
        &self,
        r: &OwnerResourceRegistry,
        after: Option<String>,
        launch: Option<super::super::managed_runtime::PreparedLaunch>,
    ) -> Result<SharedDemandPoll, NativeError> {
        self.poll_demand_with_launch(r, after, launch).await
    }

    pub async fn poll_demand(
        &self,
        r: &OwnerResourceRegistry,
        after: Option<String>,
    ) -> Result<SharedDemandPoll, NativeError> {
        self.poll_demand_with_launch(r, after, None).await
    }
    pub(super) async fn poll_demand_with_launch(
        &self,
        r: &OwnerResourceRegistry,
        after: Option<String>,
        launch: Option<super::super::managed_runtime::PreparedLaunch>,
    ) -> Result<SharedDemandPoll, NativeError> {
        enabled()?;
        let b = self.io.bound().await?;
        demand_io::require_pair(&b).await?;
        let reply: Timed<demand_io::PollReply> = b
            .request(DemandAction::Poll { after }.into(), false)
            .await?;
        if let Some(id) = &reply.value.next_after {
            super::super::key(id)?;
        }
        let outcome = match reply.value.demand {
            Some(s) => self.consume_demand(r, s, reply.observed, launch).await,
            None => Ok(SharedDemandProgress::Idle),
        };
        Ok(SharedDemandPoll {
            next_after: reply.value.next_after,
            outcome,
        })
    }
    /// Recover only an immutable original offer. No new demand/start identity is
    /// selected, even when the original POST or its reply was lost.
    pub async fn resume_next_demand(
        &self,
        r: &OwnerResourceRegistry,
        after: Option<String>,
    ) -> Result<Option<SharedDemandResume>, NativeError> {
        enabled()?;
        let b = self.io.bound().await?;
        let Some(saved) = r
            .next_shared_demand(
                b.scope.profile.clone(),
                b.scope.device.device_id().into(),
                after,
            )
            .await
            .map_err(store)?
        else {
            return Ok(None);
        };
        let id = saved.intent().offer().demand_id.clone();
        let expected = saved.intent().offer().digest().map_err(store)?;
        let outcome = async {
            let reply: Timed<demand_io::RecoverReply> = b
                .request(
                    DemandAction::Recover {
                        id: id.clone(),
                        digest: expected.clone(),
                    }
                    .into(),
                    false,
                )
                .await?;
            let s = reply.value.demand.ok_or_else(invalid)?;
            if s.offer.demand_id != id || s.demand_digest != expected {
                return Err(invalid());
            }
            self.consume_demand(r, s, reply.observed, None).await
        }
        .await;
        Ok(Some(SharedDemandResume {
            demand_id: id,
            outcome,
        }))
    }
    async fn consume_demand(
        &self,
        r: &OwnerResourceRegistry,
        s: Status,
        observed: u64,
        launch: Option<super::super::managed_runtime::PreparedLaunch>,
    ) -> Result<SharedDemandProgress, NativeError> {
        s.validate()?;
        let id = s.offer.demand_id.clone();
        let b = self.io.bound().await?;
        let target = r
            .shared_successor_selection(b.scope.profile.clone(), s.offer.assignment_id.clone())
            .await
            .map_err(store)?
            .ok_or_else(|| failure(409, "shared_demand_original_missing"))?;
        io::binding(&b.scope, target.original(), false)?;
        if !matches!(s.state.as_str(), "offered" | "start_committed") {
            if s.state == "bound" {
                r.recover_shared_demand_binding(
                    b.cleanup(r, target.original()).await?,
                    s.offer.clone(),
                    s.binding.clone().ok_or_else(invalid)?,
                ).await.map_err(store)?;
            }
            // Terminal history never becomes fresh launch authority. A central
            // cancellation restricts the original Run revision locally too.
            if s.state == "cancelled" {
                let a = b.cleanup(r, target.original()).await?;
                r.stop_shared_demand_revision(a, s.offer.control_revision)
                    .await
                    .map_err(store)?;
                self.manager
                    .owner
                    .0
                    .shared_execution
                    .stop_assignment(&s.offer.assignment_id, s.offer.control_revision)?;
            }
            return Ok(SharedDemandProgress::Historical {
                demand_id: id,
                status: s.state,
            });
        }
        let proof = offer_proof(&b, r, target.original(), &s, observed).await?;
        let saved = r
            .prepare_shared_demand(
                proof,
                target.original().clone(),
                target.predecessor().cloned(),
            )
            .await
            .map_err(store)?;
        match saved.disposition() {
            SharedDemandDisposition::Pending => Ok(SharedDemandProgress::Pending { demand_id: id }),
            SharedDemandDisposition::HeadChanged => Ok(SharedDemandProgress::Blocked {
                demand_id: id,
                reason: "head_changed".into(),
            }),
            SharedDemandDisposition::BudgetBlocked => {
                let budget = saved.budget().ok_or_else(invalid)?;
                let reason = if budget.state == "exhausted" {
                    "budget_blocked"
                } else {
                    "usage_unrepresentable"
                };
                let reply: Timed<demand_io::RecoverReply> = b
                    .request(
                        DemandAction::Block {
                            id: id.clone(),
                            input: demand_io::BlockInput {
                                demand_digest: s.demand_digest,
                                expected_control_revision: s.offer.control_revision,
                                reason: reason.into(),
                                budget: budget.observation.clone(),
                            },
                        }
                        .into(),
                        true,
                    )
                    .await?;
                let current = reply.value.demand.ok_or_else(invalid)?;
                current.validate()?;
                if current.offer.digest().map_err(store)?
                    != saved.offer().digest().map_err(store)?
                    || current.state != reason
                {
                    return Err(invalid());
                }
                Ok(SharedDemandProgress::Blocked {
                    demand_id: id,
                    reason: reason.into(),
                })
            }
            SharedDemandDisposition::StartPrepared => {
                let proof = offer_proof(&b, r, target.original(), &s, observed).await?;
                let submission = r
                    .shared_demand_submission(proof, target.original().clone())
                    .await
                    .map_err(store)?;
                demand_io::require_pair(&b).await?;
                submission.check().map_err(store)?;
                let progress = self
                    .process_attempt(
                        r,
                        attempt_io::AttemptAction::DemandStart {
                            id: id.clone(),
                            input: submission.input().clone(),
                        },
                        None,
                        launch,
                    )
                    .await?;
                Ok(SharedDemandProgress::Execution {
                    demand_id: id,
                    progress,
                })
            }
        }
    }
}
/// Promotion is descriptive. Store binding is durable before inbox/ACK, and the
/// ordinary native call hold + central one-shot start + issue gates still follow.
pub(super) async fn bind_work(
    b: &Bound,
    r: &OwnerResourceRegistry,
    original: &SharedRuntimeCommand,
    w: &SharedWorkItem,
    activity: VerifiedSharedActivity,
    until: Instant,
) -> Result<(), NativeError> {
    enabled()?;
    if !demand_io::paired(b).await? {
        return r
            .require_manual_work_without_demands(activity, w.clone())
            .await
            .map_err(store);
    }
    let reply: Timed<demand_io::BindingReply> = b
        .upload(
            DemandAction::Binding {
                work: w.id.clone(),
                work_digest: digest(w)?,
                runtime_digest: w.runtime_command_digest.clone(),
            }
            .into(),
            until,
        )
        .await?;
    let Some(link) = reply.value.demand else {
        return Ok(());
    };
    if link.offer.digest().map_err(store)? != link.demand_digest {
        return Err(invalid());
    }
    link.binding.validate_for(&link.offer, w).map_err(store)?;
    let target = r
        .shared_successor_selection(b.scope.profile.clone(), original.assignment_id.clone())
        .await
        .map_err(store)?
        .ok_or_else(invalid)?;
    if digest(target.original())? != digest(original)? {
        return Err(invalid());
    }
    let status = Status {
        offer: link.offer,
        demand_digest: link.demand_digest,
        state: "bound".into(),
        runtime_command_id: Some(link.binding.runtime_command_id.clone()),
        binding: Some(link.binding.clone()),
    };
    let proof = offer_proof(b, r, original, &status, reply.observed).await?;
    r.prepare_shared_demand(proof, original.clone(), target.predecessor().cloned())
        .await
        .map_err(store)?;
    r.bind_shared_demand_work(activity, link.binding, w.clone())
        .await
        .map_err(store)?;
    b.transport.check_scope(&b.scope, false).await
}

#[cfg(feature="shared-budget-execution")]
pub(super) async fn bind_budget_work(
    b: &Bound,
    r: &OwnerResourceRegistry,
    original: &SharedRuntimeCommand,
    w: &SharedWorkItem,
    activity: VerifiedSharedActivity,
    until: Instant,
) -> Result<(), NativeError> {
    enabled()?;
    if !demand_io::paired(b).await? {
        return r
            .require_manual_work_without_demands(activity, w.clone())
            .await
            .map_err(store);
    }
    let reply: Timed<demand_io::BindingReply> = b
        .upload(
            budget_work_io::WorkAction::Binding {
                work: w.id.clone(),
                input:SharedWorkEvidenceInput {work_digest:digest(w)?,runtime_command_digest:w.runtime_command_digest.clone()},
            }
            .into(),
            until,
        )
        .await?;
    let Some(link) = reply.value.demand else {
        return Ok(());
    };
    if link.offer.digest().map_err(store)? != link.demand_digest {
        return Err(invalid());
    }
    link.binding.validate_for(&link.offer, w).map_err(store)?;
    let status = Status {
        offer: link.offer,
        demand_digest: link.demand_digest,
        state: "bound".into(),
        runtime_command_id: Some(link.binding.runtime_command_id.clone()),
        binding: Some(link.binding.clone()),
    };
    let saved=r.recover_shared_demand(b.cleanup(r,original).await?,status.offer.demand_id.clone()).await.map_err(store)?;
    if let Some(saved)=saved {
        if saved.offer().digest().map_err(store)?!=status.demand_digest{return Err(invalid())}
    } else {
        let proof = offer_proof(b, r, original, &status, reply.observed).await?;
        r.prepare_shared_demand(proof, original.clone(), None).await.map_err(store)?;
    }
    r.bind_shared_demand_work(activity, link.binding, w.clone())
        .await
        .map_err(store)?;
    b.transport.check_scope(&b.scope, false).await
}

/// Receipt identity only. A later control projection never revises this effect.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedDemandStopReceipt {
    pub command_id: String,
    pub applied_revision: u64,
}
pub struct SharedDemandStopResume {
    pub command_id: String,
    pub outcome: Result<SharedDemandStopReceipt, NativeError>,
}
impl SharedExecutionTransport {
    /// Native owner Stop only. There is deliberately no owner Run adapter.
    pub async fn stop_demand_assignment(
        &self,
        r: &OwnerResourceRegistry,
        assignment: String,
    ) -> Result<SharedDemandStopReceipt, NativeError> {
        enabled()?;
        super::super::key(&assignment)?;
        let b = self.io.bound().await?;
        let target = r
            .shared_original_selection(b.scope.profile.clone(), assignment.clone())
            .await
            .map_err(store)?
            .ok_or_else(invalid)?;
        let reply: Timed<demand_io::ControlReply> = b
            .request(
                DemandAction::Control {
                    assignment: assignment.clone(),
                }
                .into(),
                false,
            )
            .await?;
        reply.value.control.validate(&assignment)?;
        let a = b.cleanup(r, &target).await?;
        // Synchronous restriction precedes every following await. Issued tasks
        // retain their exact cleanup evidence even if the command writer fails.
        self.manager
            .owner
            .0
            .shared_execution
            .stop_assignment(&assignment, reply.value.control.revision)?;
        let registry = r.clone();
        let io = self.io.clone();
        let result = self
            .manager
            .owner
            .0
            .shared_execution
            .retain_stop(async move {
                let intent = registry
                    .prepare_shared_demand_stop(a, reply.value.control.revision)
                    .await
                    .map_err(store)?;
                send_demand_stop(&io, &registry, intent).await
            })?;
        result
            .await
            .map_err(|_| failure(409, "shared_execution_outcome_unknown"))?
    }
}
async fn send_demand_stop(
    io: &Io,
    r: &OwnerResourceRegistry,
    intent: SharedDemandStopIntent,
) -> Result<SharedDemandStopReceipt, NativeError> {
    let b = io.bound().await?;
    let _proof = b.cleanup(r, intent.original()).await?;
    let reply: Timed<demand_io::StopReply> = b
        .request(
            DemandAction::Stop {
                assignment: intent.original().assignment_id.clone(),
                input: intent.input().clone(),
            }
            .into(),
            true,
        )
        .await?;
    reply
        .value
        .control
        .validate(&intent.original().assignment_id)?;
    if reply.value.command_id != intent.input().command_id
        || reply.value.applied_revision < intent.input().expected_revision
        || reply.value.applied_revision > intent.input().expected_revision + 1
        || reply.value.control.revision < reply.value.applied_revision
    {
        return Err(invalid());
    }
    // Store only immutable command/appliedRevision, not latest control.
    let a = b.cleanup(r, intent.original()).await?;
    r.record_shared_demand_stop(
        a,
        reply.value.command_id.clone(),
        reply.value.applied_revision,
    )
    .await
    .map_err(store)?;
    Ok(SharedDemandStopReceipt {
        command_id: reply.value.command_id,
        applied_revision: reply.value.applied_revision,
    })
}
impl SharedExecutionTransport {
    pub async fn resume_next_demand_stop(
        &self,
        r: &OwnerResourceRegistry,
        after: Option<String>,
    ) -> Result<Option<SharedDemandStopResume>, NativeError> {
        enabled()?;
        let b = self.io.bound().await?;
        let Some(intent) = r
            .next_shared_demand_stop(
                b.scope.profile.clone(),
                b.scope.device.device_id().into(),
                after,
            )
            .await
            .map_err(store)?
        else {
            return Ok(None);
        };
        let command_id = intent.input().command_id.clone();
        self.manager.owner.0.shared_execution.stop_assignment(
            &intent.original().assignment_id,
            intent.input().expected_revision,
        )?;
        let outcome = send_demand_stop(&self.io, r, intent).await;
        Ok(Some(SharedDemandStopResume {
            command_id,
            outcome,
        }))
    }
}

#[cfg(feature = "shared-intake")]
impl SharedExecutionTransport {
    pub(in crate::console::native) async fn prepare_reviewed_owner_stop(
        &self,
        r: &OwnerResourceRegistry,
        assignment: String,
        input: SharedDemandStopInput,
    ) -> Result<Option<SharedDemandStopIntent>, NativeError> {
        enabled()?;
        super::super::key(&assignment)?;
        super::super::key(&input.command_id)?;
        if input.expected_revision == 0
            || input.expected_revision >= 9_007_199_254_740_991
            || input.mode != "paused"
        {
            return Err(failure(400, "invalid_management_revision"));
        }
        let b = self.io.bound().await?;
        let Some(target) = r
            .shared_successor_selection(b.scope.profile.clone(), assignment.clone())
            .await
            .map_err(store)?
        else {
            return Ok(None);
        };
        io::binding(&b.scope, target.original(), false)?;
        let original = target.original().clone();
        let registry = r.clone();
        let keep = self.manager.clone();
        let result = self
            .manager
            .owner
            .0
            .shared_execution
            .retain_stop(async move {
                let intent = registry
                    .prepare_local_owner_demand_stop(
                        b.scope.profile.clone(),
                        b.scope.device.device_id().into(),
                        b.scope.device.generation(),
                        original,
                        input,
                    )
                    .await
                    .map_err(store)?;
                // Validate/persist the exact original request before cancellation:
                // a conflicting retry must never cancel a later Run revision.
                keep.owner
                    .0
                    .shared_execution
                    .stop_assignment(&assignment, intent.input().expected_revision)
                    .map_err(|_| failure(409, "management_agent_control_local_stop_pending"))?;
                Ok(Some(intent))
            })?;
        result
            .await
            .map_err(|_| failure(409, "shared_execution_outcome_unknown"))?
    }
    pub(in crate::console::native) async fn finish_reviewed_owner_stop(
        &self,
        r: &OwnerResourceRegistry,
        intent: SharedDemandStopIntent,
    ) -> Result<SharedDemandStopReceipt, NativeError> {
        send_demand_stop(&self.io, r, intent).await
    }
    #[cfg(test)]
    pub(in crate::console::native) async fn stop_reviewed_demand_assignment(
        &self,
        r: &OwnerResourceRegistry,
        assignment: String,
        input: SharedDemandStopInput,
    ) -> Result<SharedDemandStopReceipt, NativeError> {
        let intent = self
            .prepare_reviewed_owner_stop(r, assignment, input)
            .await?
            .ok_or_else(|| failure(409, "owner_stop_local_custody_required"))?;
        self.finish_reviewed_owner_stop(r, intent).await
    }
}
