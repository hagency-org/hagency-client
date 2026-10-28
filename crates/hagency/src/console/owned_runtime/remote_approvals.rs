//! Host-owned original approval publication. No metadata can reconstruct a send
//! capsule or an execution grant. PendingApproval remains in the live registry.
use super::*;
use crate::console::native::MatrixTokenSource;
use hagency_agent_local::remote_approval::{self as remote, Publication, Request};
use std::sync::atomic::{AtomicBool, Ordering};

struct RemoteJob {
    ledger: Ledger,
    publication: Publication,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::console) struct RemoteView {
    pub phase: &'static str,
    pub approval_room: String,
    pub request_event_id: Option<String>,
}
pub(super) struct RemoteSlot {
    room: String,
    job: Arc<Mutex<RemoteJob>>,
    live: Arc<AtomicBool>,
    status: watch::Receiver<RemoteView>,
}
impl Drop for RemoteSlot {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
    }
}
async fn bindings(
    console: &Console,
    cookie: &str,
    agent: &str,
    device: &AuthorizedDevice,
) -> Result<Vec<Binding>, RuntimeError> {
    let owner = console
        .owner_api(
            cookie,
            OwnerOperation::Bindings {
                agent: agent.into(),
            },
        )
        .await
        .map_err(|_| RuntimeError::Authorization)?;
    if owner.owner != device.owner_mxid()
        || owner.origin != device.origin()
        || owner.issuer != device.issuer()
        || owner.subject != device.subject()
    {
        return Err(RuntimeError::Authorization);
    }
    device.bearer().map_err(|_| RuntimeError::Authorization)?;
    serde_json::from_value(owner.value["bindings"].clone()).map_err(|_| RuntimeError::Profile)
}
async fn source(
    console: &Console,
    device: &AuthorizedDevice,
) -> Result<Arc<dyn MatrixTokenSource>, RuntimeError> {
    console
        .0
        .server_login
        .matrix_source(&DevicePin::from(device))
        .await
        .filter(|s| s.supports_remote_approvals())
        .ok_or(RuntimeError::Execution(
            "remote_approval_transport_unavailable",
        ))
}
fn live(
    device: AuthorizedDevice,
    cancel: watch::Receiver<bool>,
    turn_cancel: watch::Receiver<bool>,
    pending: codex::ApprovalLiveness,
    current: Arc<AtomicBool>,
    expires: i64,
) -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(move || {
        !*cancel.borrow()
            && !*turn_cancel.borrow()
            && pending.is_open()
            && current.load(Ordering::Acquire)
            && now() < expires
            && device.bearer().is_ok()
    })
}
impl OwnedRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::console) async fn publish_remote(
        &self,
        console: &Console,
        cookie: &str,
        agent: &str,
        id: &str,
        digest: &str,
        room: &str,
    ) -> Result<RemoteView, RuntimeError> {
        let device = console.authorized_device().await.map_err(device_error)?;
        device.bearer().map_err(|_| RuntimeError::Authorization)?;
        let pin = DevicePin::from(&device);
        let source = source(console, &device).await?;
        let current = bindings(console, cookie, agent, &device).await?;
        let gate = {
            let entries = self.entries_for(&device).await?;
            let entry = entries
                .get(&identity(&device, agent))
                .ok_or(RuntimeError::Approval)?;
            let slot = entry.check_approval(&pin, agent, id, digest, &current, now())?;
            if let Some(remote) = &slot.remote {
                if remote.room != room {
                    return Err(RuntimeError::Approval);
                }
                return Ok(remote.status.borrow().clone());
            }
            slot.gate.clone().ok_or(RuntimeError::Approval)?
        };
        let publisher = source
            .prepare_remote_approval(room)
            .await
            .map_err(|_| RuntimeError::Approval)?;
        let mut ledger = approval_ledger(console, &device)?;
        let (_, profile) = intent_location(console, &device)?;
        // Refresh authority after the SDK preflight and immediately before the
        // first durable reservation. No network runs under the registry lock.
        let current = bindings(console, cookie, agent, &device).await?;
        let (proposal, cancel, turn_cancel, pending) = {
            let entries = self.entries_for(&device).await?;
            let entry = entries
                .get(&identity(&device, agent))
                .ok_or(RuntimeError::Approval)?;
            let slot = entry.check_approval(&pin, agent, id, digest, &current, now())?;
            (
                slot.view.proposal.clone(),
                entry.cancel.subscribe(),
                slot.turn_cancel.clone(),
                slot.pending.liveness(),
            )
        };
        let record = local(ledger.inbox_record(device.owner_mxid(), &proposal.dispatch))?;
        let execution = record.execution_id.clone().ok_or(RuntimeError::Approval)?;
        if !gate.authorize(&proposal.dispatch, &execution).await {
            return Err(RuntimeError::Authorization);
        }
        let request = Request {
            version: 1,
            owner: device.owner_mxid().into(),
            profile,
            approval_id: id.into(),
            approval_room: room.into(),
            execution_id: execution.clone(),
            binding_generation: record.dispatch.binding_generation,
            dispatch_epoch: record
                .dispatch
                .dispatch_epoch
                .ok_or(RuntimeError::Approval)?,
            execution_device: remote::ExecutionDevice {
                id: device.device_id().into(),
                generation: device.generation(),
            },
            publisher_matrix_device: publisher,
            proposal_digest: digest.into(),
            proposal: proposal.clone(),
        };
        let mut joins = self.joins.lock().await;
        let mut entries = self.entries_for(&device).await?;
        let entry = entries
            .get_mut(&identity(&device, agent))
            .ok_or(RuntimeError::Approval)?;
        let slot = entry.check_approval(&pin, agent, id, digest, &current, now())?;
        if slot.remote.is_some() {
            return Err(RuntimeError::Approval);
        }
        let mut publication =
            local(ledger.reserve_remote_approval(device.owner_mxid(), &request, now))?;
        let permit = local(publication.begin_publication())?;
        let job = Arc::new(Mutex::new(RemoteJob {
            ledger,
            publication,
        }));
        let active = Arc::new(AtomicBool::new(true));
        let view = RemoteView {
            phase: "publishing",
            approval_room: room.into(),
            request_event_id: None,
        };
        let (status, receiver) = watch::channel(view.clone());
        entry
            .approvals
            .get_mut(id)
            .ok_or(RuntimeError::Approval)?
            .remote = Some(RemoteSlot {
            room: room.into(),
            job: job.clone(),
            live: active.clone(),
            status: receiver,
        });
        let check = live(
            device.clone(),
            cancel,
            turn_cancel,
            pending,
            active,
            proposal.expires,
        );
        let owner = device.owner_mxid().to_owned();
        // Track before the caller can be cancelled at another await. Removing
        // the approval retires authority but retains this original possible IO.
        joins.push(tokio::spawn(async move {
            let mut held = job.lock().await;
            let result = if check() && gate.authorize(&proposal.dispatch, &execution).await {
                source
                    .publish_remote_approval(permit, check.clone())
                    .await
                    .ok()
            } else {
                None
            };
            let mut final_view = view;
            final_view.phase = "unknown";
            if let Some(event) = result {
                final_view.request_event_id = Some(event.clone());
                let RemoteJob {
                    ledger,
                    publication,
                } = &mut *held;
                if ledger
                    .record_remote_approval_receipt(&owner, publication, &event, now())
                    .is_ok()
                {
                    final_view.phase = "sent";
                }
            }
            status.send_replace(final_view);
        }));
        // The receiver above, not this response, observes the actual acceptance.
        Ok(RemoteView {
            phase: "publishing",
            approval_room: room.into(),
            request_event_id: None,
        })
    }
    pub(in crate::console) async fn remote_status(
        &self,
        console: &Console,
        agent: &str,
        id: &str,
    ) -> Result<Option<RemoteView>, RuntimeError> {
        let device = console.authorized_device().await.map_err(device_error)?;
        device.bearer().map_err(|_| RuntimeError::Authorization)?;
        {
            let entries = self.entries_for(&device).await?;
            if let Some(slot) = entries
                .get(&identity(&device, agent))
                .and_then(|e| e.approvals.get(id))
                && let Some(remote) = &slot.remote
            {
                return Ok(Some(remote.status.borrow().clone()));
            }
        }
        let ledger = approval_ledger(console, &device)?;
        let record = local(ledger.remote_approval_record(device.owner_mxid(), agent, id))?;
        Ok(record.map(|r| RemoteView {
            phase: match r.decision.as_ref().map(|d| d.approved) {
                Some(true) => "approved",
                Some(false) => "denied",
                None if r.request_event_id.is_some() => "sent",
                None => "unknown",
            },
            approval_room: r.request.approval_room,
            request_event_id: r.request_event_id,
        }))
    }
    #[allow(clippy::too_many_arguments)]
    pub(in crate::console) async fn observe_remote(
        &self,
        console: &Console,
        cookie: &str,
        agent: &str,
        id: &str,
        digest: &str,
        verdict: Option<&str>,
    ) -> Result<(), RuntimeError> {
        let device = console.authorized_device().await.map_err(device_error)?;
        device.bearer().map_err(|_| RuntimeError::Authorization)?;
        let pin = DevicePin::from(&device);
        let source = source(console, &device).await?;
        let current = bindings(console, cookie, agent, &device).await?;
        let (job, gate, check) = {
            let entries = self.entries_for(&device).await?;
            let entry = entries
                .get(&identity(&device, agent))
                .ok_or(RuntimeError::Approval)?;
            let slot = entry.check_approval(&pin, agent, id, digest, &current, now())?;
            let remote = slot.remote.as_ref().ok_or(RuntimeError::Approval)?;
            (
                remote.job.clone(),
                slot.gate.clone().ok_or(RuntimeError::Approval)?,
                live(
                    device.clone(),
                    entry.cancel.subscribe(),
                    slot.turn_cancel.clone(),
                    slot.pending.liveness(),
                    remote.live.clone(),
                    slot.view.proposal.expires,
                ),
            )
        };
        let mut held = job.lock().await;
        let request = held.publication.request().clone();
        if !check()
            || !gate
                .authorize(&request.proposal.dispatch, &request.execution_id)
                .await
        {
            return Err(RuntimeError::Authorization);
        }
        let event = local(
            held.ledger
                .remote_approval_record(device.owner_mxid(), agent, id),
        )?
        .and_then(|r| r.request_event_id)
        .ok_or(RuntimeError::Approval)?;
        let observed = match verdict {
            Some(verdict_event) => {
                source
                    .observe_remote_approval(
                        &held.publication,
                        &event,
                        verdict_event,
                        check.clone(),
                    )
                    .await
            }
            None => {
                source
                    .find_remote_approval(&held.publication, &event, check.clone())
                    .await
            }
        }
        .map_err(|error| match error.code.as_str() {
            "remote_approval_waiting" => RuntimeError::Execution("remote_approval_waiting"),
            "remote_approval_incomplete" => RuntimeError::Execution("remote_approval_incomplete"),
            _ => RuntimeError::Approval,
        })?;
        let current = bindings(console, cookie, agent, &device).await?;
        if !check()
            || !gate
                .authorize(&request.proposal.dispatch, &request.execution_id)
                .await
        {
            return Err(RuntimeError::Authorization);
        }
        let mut entries = self.entries_for(&device).await?;
        let entry = entries
            .get_mut(&identity(&device, agent))
            .ok_or(RuntimeError::Approval)?;
        let slot = entry.check_approval(&pin, agent, id, digest, &current, now())?;
        if slot
            .remote
            .as_ref()
            .is_none_or(|r| !Arc::ptr_eq(&job, &r.job))
        {
            return Err(RuntimeError::Approval);
        }
        let RemoteJob {
            ledger,
            publication,
        } = &mut *held;
        let decision = local(ledger.record_remote_approval_decision(
            device.owner_mxid(),
            publication,
            &remote::ExecutionDevice {
                id: device.device_id().into(),
                generation: device.generation(),
            },
            &observed,
            now,
        ))?;
        let pending = entry.take_approval(&pin, agent, id, digest, &current, now());
        let delivered = pending.is_ok_and(|pending| pending.decide(decision.approved()));
        if !delivered && decision.approved() {
            local(ledger.record_approval_event(
                device.owner_mxid(),
                id,
                hagency_agent_local::audit::Phase::Unknown,
                now(),
                &serde_json::json!({"reason":"remote_decision_receiver_closed"}),
            ))?;
            return Err(RuntimeError::Approval);
        }
        Ok(())
    }
}
