//! Remote owner decisions use the same profile ledger but never require this
//! device to own the Agent runtime. No network route accepts these commands.
use super::*;
use crate::console::owned_agents;
use hagency_agent_local::remote_approval::review::{ObservedRequest, VerdictRecord};
static SENDS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteApprovalAction {
    List {
        agent: String,
        room: String,
        cursor: Option<String>,
    },
    Decide {
        agent: String,
        room: String,
        event_id: String,
        request_digest: String,
        approved: bool,
    },
    Status {
        approval_id: String,
    },
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(i64::MAX)
}
fn local_error(_: hagency_agent_local::Error) -> NativeError {
    failure(409, "remote_approval_unavailable")
}
fn view(record: Option<VerdictRecord>) -> Value {
    match record {
        None => Value::Null,
        Some(r) => {
            json!({"approved":r.approved,"phase":if r.event_id.is_some(){"sent"}else{"unknown"},"eventId":r.event_id})
        }
    }
}
fn validate(
    o: &ObservedRequest,
    identity: &Identity,
    profile: &str,
    agent: &str,
    room: &str,
    event: Option<&str>,
    at: i64,
) -> Result<(), NativeError> {
    o.validate(&identity.owner, profile, at)
        .map_err(local_error)?;
    if o.request.proposal.scope.agent != agent
        || o.request.approval_room != room
        || event.is_some_and(|e| e != o.event_id)
    {
        return Err(failure(409, "remote_approval_unavailable"));
    }
    Ok(())
}
pub(super) async fn execute(
    native: &NativeOwner,
    action: RemoteApprovalAction,
) -> Result<Value, NativeError> {
    let identity = native.identity().await?;
    let device = native
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    let pin = crate::console::device_execution::DeviceIdentity::from(&device);
    if identity.owner != pin.owner
        || identity.origin != pin.origin
        || identity.issuer != pin.issuer
        || identity.subject != pin.subject
    {
        return Err(failure(401, "owner_authorization_required"));
    }
    let profile = owned_agents::profile_identity(
        &identity.origin,
        &identity.issuer,
        &identity.subject,
        &identity.owner,
    )
    .map_err(|_| failure(403, "owner_scope_required"))?;
    let source = native
        .0
        .matrix_source
        .lock()
        .map_err(|_| failure(503, "native_state_unavailable"))?
        .clone()
        .filter(|s| s.supports_remote_approvals())
        .ok_or_else(|| failure(503, "remote_approval_transport_unavailable"))?;
    let mut book = ledger::local_ledger(native, &identity, &profile)?;
    match action {
        RemoteApprovalAction::Status { approval_id } => Ok(
            json!({"approvalId":approval_id,"publication":view(book.remote_verdict_record(&identity.owner,&approval_id).map_err(local_error)?)}),
        ),
        RemoteApprovalAction::List {
            agent,
            room,
            cursor,
        } => {
            key(&agent)?;
            let page = source
                .list_remote_requests(&room, cursor.as_deref())
                .await?;
            if page.requests.len() > 10 || page.next_batch.as_ref().is_some_and(|s| s.len() > 1024)
            {
                return Err(failure(503, "remote_approval_unavailable"));
            }
            device
                .bearer()
                .map_err(|_| failure(401, "owner_authorization_required"))?;
            let mut requests = Vec::new();
            for o in page.requests {
                if o.request.proposal.scope.agent != agent {
                    continue;
                }
                // Listing is presentation only. Decide re-observes the exact
                // event and enforces fresh proof at the actual writer lock.
                validate(&o, &identity, &profile, &agent, &room, None, o.observed_at)?;
                if o.request.proposal.expires <= now() {
                    continue;
                }
                let digest = o.request.digest().map_err(local_error)?;
                let publication = view(
                    book.remote_verdict_record(&identity.owner, &o.request.approval_id)
                        .map_err(local_error)?,
                );
                requests.push(json!({"request":o.request,"requestEventId":o.event_id,"requestDigest":digest,"publication":publication}));
            }
            Ok(
                json!({"agentId":agent,"roomId":room,"requests":requests,"nextBatch":page.next_batch}),
            )
        }
        RemoteApprovalAction::Decide {
            agent,
            room,
            event_id,
            request_digest,
            approved,
        } => {
            key(&agent)?;
            let slot = SENDS
                .try_acquire()
                .map_err(|_| failure(409, "remote_approval_busy"))?;
            let o = source.inspect_remote_request(&room, &event_id).await?;
            validate(
                &o,
                &identity,
                &profile,
                &agent,
                &room,
                Some(&event_id),
                now(),
            )?;
            native.check_matrix_source().await?;
            device
                .bearer()
                .map_err(|_| failure(401, "owner_authorization_required"))?;
            let weak = Arc::downgrade(&native.0);
            let update = crate::update_guard::PROCESS
                .continue_task()
                .ok_or_else(|| failure(409, "update_in_progress"))?;
            let live: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
                weak.upgrade().is_some_and(|n| {
                    !n.configuration_stopped
                        .load(std::sync::atomic::Ordering::Acquire)
                }) && device.bearer().is_ok()
            });
            // Register the original send task synchronously after reservation;
            // cancellation at any later await cannot orphan its possible IO.
            let mut tasks = native
                .0
                .review_tasks
                .lock()
                .map_err(|_| failure(503, "native_state_unavailable"))?;
            if !live() {
                return Err(failure(401, "owner_authorization_required"));
            }
            let mut publication = book
                .reserve_remote_verdict(&identity.owner, &o, &request_digest, approved, now)
                .map_err(local_error)?;
            let permit = publication.begin_publication().map_err(local_error)?;
            let approval_id = o.request.approval_id.clone();
            tasks.push(tokio::spawn(async move {
                let _slot = slot;
                let _update = update;
                if let Ok(event) = source.publish_remote_verdict(permit, live).await {
                    let _ = book.record_remote_verdict_receipt(
                        &identity.owner,
                        &publication,
                        &event,
                        now(),
                    );
                }
            }));
            Ok(
                json!({"approvalId":approval_id,"publication":{"phase":"unknown","approved":approved}}),
            )
        }
    }
}
