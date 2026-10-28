//! Private owner history; a historical grant cannot re-enter the live queue.
use super::{NativeError, NativeOwner, failure, ledger};
use crate::console::{device_execution::DeviceIdentity, owned_agents};
use hagency_agent_local::recovery::Input as RecoveryInput;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HistoryAction {
    Files {
        agent: String,
        cursor: Option<String>,
    },
    File {
        agent: String,
        id: String,
    },
    RefreshFile {
        agent: String,
        id: String,
    },
    Stopped {
        agent: String,
        cursor: Option<String>,
    },
    InspectStopped {
        agent: String,
        dispatch: String,
        paths: Vec<String>,
    },
    DecideStopped {
        agent: String,
        id: String,
        input: RecoveryInput,
    },
    Approvals {
        agent: String,
        cursor: Option<String>,
    },
    Approval {
        agent: String,
        id: String,
    },
}
pub(super) async fn execute(
    native: &NativeOwner,
    action: HistoryAction,
) -> Result<Value, NativeError> {
    let identity = native.identity().await?;
    let device = native
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    let pin = DeviceIdentity::from(&device);
    if pin.owner != identity.owner
        || pin.subject != identity.subject
        || pin.issuer != identity.issuer
        || pin.origin != identity.origin
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
    let agent = match &action {
        HistoryAction::Files { agent, .. }
        | HistoryAction::File { agent, .. }
        | HistoryAction::RefreshFile { agent, .. }
        | HistoryAction::Approvals { agent, .. }
        | HistoryAction::Approval { agent, .. }
        | HistoryAction::Stopped { agent, .. }
        | HistoryAction::InspectStopped { agent, .. }
        | HistoryAction::DecideStopped { agent, .. } => agent,
    };
    super::key(agent)?;
    let active = native
        .0
        .host
        .console
        .0
        .owned_runtime
        .agent_busy(&device, agent)
        .await;
    device
        .bearer()
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    let mut ledger = ledger::local_ledger(native, &identity, &profile)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|t| t.as_secs().try_into().ok())
        .ok_or_else(|| failure(503, "local_state_unavailable"))?;
    ledger
        .close_approval_audits(&identity.owner, agent, !active, now)
        .map_err(local_error)?;
    let recovery = matches!(
        &action,
        HistoryAction::Stopped { .. }
            | HistoryAction::InspectStopped { .. }
            | HistoryAction::DecideStopped { .. }
    );
    if recovery && !active {
        ledger
            .recover_agent_executions(&identity.owner, agent)
            .map_err(local_error)?;
    }
    let result = match action {
        HistoryAction::Files { agent, cursor } => {
            json!({"agentId":agent,"files":ledger.sent_file_history(&identity.owner,&agent,cursor.as_deref()).map_err(file_error)?,"runtimeActive":active})
        }
        HistoryAction::File { agent, id } => {
            json!({"agentId":agent,"file":ledger.sent_file_record(&identity.owner,&agent,&id).map_err(file_error)?,"runtimeActive":active})
        }
        HistoryAction::RefreshFile { agent, id } => {
            let original = ledger
                .sent_file_record(&identity.owner, &agent, &id)
                .map_err(file_error)?;
            let request = hagency_agent_local::file_delivery::FileLookup {
                dispatch_id: original.dispatch_id.clone(),
                execution_id: original.execution_id.clone(),
                request_id: original.record.request_id.clone(),
            };
            let observation = if ledger
                .encrypted_input_transport_digest(&identity.owner, &original.dispatch_id)
                .map_err(file_error)?
                .is_some()
            {
                let dispatch = ledger
                    .inbox_record(&identity.owner, &original.dispatch_id)
                    .map_err(file_error)?
                    .dispatch;
                let delivery = crate::console::owned_runtime::encrypted_file_status(
                    &native.0.host.console,
                    &device,
                    &dispatch,
                    &original.execution_id,
                    &original.record,
                )
                .await
                .map_err(|_| failure(503, "file_history_unavailable"))?;
                hagency_agent_local::file_delivery::FileObservation {
                    request,
                    delivery,
                    preparation_closed: false,
                }
            } else {
                let metadata = crate::server_admission::discover(&identity.origin)
                    .await
                    .map_err(|_| failure(503, "file_history_unavailable"))?;
                if !metadata
                    .capabilities
                    .iter()
                    .any(|c| c == hagency_agent_local::file_delivery::HISTORY_CAPABILITY)
                {
                    return Err(failure(409, "file_history_upgrade_required"));
                }
                let response = native
                    .0
                    .host
                    .console
                    .execution_api(
                        crate::console::device_execution::DeviceOperation::LookupFile {
                            request,
                            file: original.record.manifest.clone(),
                        },
                    )
                    .await
                    .map_err(|e| failure(e.status, &e.code))?;
                let crate::console::device_execution::DeviceResponse::FileObservation(observation) =
                    response
                else {
                    return Err(failure(502, "invalid_file_observation_response"));
                };
                observation
            };
            native.check_matrix_source().await?;
            let fresh = native
                .0
                .host
                .console
                .authorized_device()
                .await
                .map_err(|_| failure(401, "owner_authorization_required"))?;
            if DeviceIdentity::from(&fresh) != pin || native.identity().await? != identity {
                return Err(failure(401, "owner_authorization_required"));
            }
            device
                .bearer()
                .map_err(|_| failure(401, "owner_authorization_required"))?;
            ledger
                .observe_file_lookup(
                    &original.scope,
                    &original.dispatch_id,
                    &original.execution_id,
                    &id,
                    observation,
                    epoch_now()?,
                )
                .map_err(file_error)?;
            json!({"agentId":agent,"file":ledger.sent_file_record(&identity.owner,&agent,&id).map_err(file_error)?,"runtimeActive":active})
        }
        HistoryAction::Stopped { agent, cursor } => {
            json!({"agentId":agent,"runtimeActive":active,"stopped":ledger.stopped_history(&identity.owner,&agent,cursor.as_deref()).map_err(local_error)?})
        }
        HistoryAction::InspectStopped {
            agent,
            dispatch,
            paths,
        } => {
            if active {
                return Err(failure(409, "recovery_runtime_busy"));
            }
            json!({"agentId":agent,"runtimeActive":false,"review":ledger.inspect_recovery(&identity.owner,&agent,&dispatch,&paths,now).map_err(local_error)?})
        }
        HistoryAction::DecideStopped { agent, id, input } => {
            if active {
                return Err(failure(409, "recovery_runtime_busy"));
            }
            let continuing = matches!(
                input.disposition,
                hagency_agent_local::recovery::Disposition::Continue { .. }
            );
            let source = if continuing {
                Some(
                    native
                        .0
                        .matrix_source
                        .lock()
                        .map_err(|_| failure(503, "native_state_unavailable"))?
                        .clone()
                        .ok_or_else(|| failure(409, "continuation_transport_unavailable"))?,
                )
            } else {
                None
            };
            // Retrying an existing action never reacquires send custody.
            let original = ledger
                .recovery_review(&identity.owner, &agent, &input.review_id)
                .map_err(local_error)?;
            let mut route = None;
            if continuing
                && !ledger
                    .has_recovery_action(&identity.owner, &agent, &id)
                    .map_err(local_error)?
            {
                let (method, path, body) = super::request(super::Command::AgentDetails {
                    agent_id: agent.clone(),
                })?;
                let cookie = native.0.cookie.lock().await.clone();
                let mut response = native.call(method, &path, body, &cookie).await;
                let metadata = super::read_response(&mut response).await?;
                let a = &metadata["agent"];
                if a["id"] != agent
                    || a["ownerUserId"] != device.user_id()
                    || a["state"] != "active"
                {
                    return Err(failure(403, "owner_scope_required"));
                }
                let puppet = a["puppetMxid"]
                    .as_str()
                    .filter(|s| s.starts_with('@') && s.contains(':'))
                    .ok_or_else(|| failure(503, "continuation_route_unavailable"))?
                    .to_owned();
                let (method, path, body) = super::request(super::Command::Bindings {
                    agent_id: agent.clone(),
                })?;
                let mut response = native.call(method, &path, body, &cookie).await;
                let bindings = super::read_response(&mut response).await?;
                let binding = bindings["bindings"]
                    .as_array()
                    .and_then(|rows| {
                        rows.iter().find(|b| {
                            b["id"] == original.evidence.scope.binding
                                && b["roomId"] == original.evidence.scope.room
                                && b["state"] == "active"
                        })
                    })
                    .ok_or_else(|| failure(409, "continuation_route_unavailable"))?;
                let private = binding["scopeKind"] == "owner_direct";
                if !private && a["visibility"] != "public" {
                    return Err(failure(409, "continuation_route_unavailable"));
                }
                route = Some((puppet, private));
            }
            native.check_matrix_source().await?;
            device
                .bearer()
                .map_err(|_| failure(401, "owner_authorization_required"))?;
            let decided = ledger
                .decide_recovery(
                    &identity.owner,
                    &agent,
                    &id,
                    &input,
                    &format!("{}/{}", device.device_id(), device.generation()),
                    epoch_now()?,
                )
                .map_err(local_error)?;
            let decision = if let Some(continuation) = decided.continuation {
                let (puppet, require_encrypted) =
                    route.ok_or_else(|| failure(503, "continuation_route_unavailable"))?;
                let message = super::ContinuationMessage {
                    owner: identity.owner.clone(),
                    room: continuation.scope.room.clone(),
                    thread: continuation.scope.thread.clone(),
                    agent_mxid: puppet.clone(),
                    transaction_id: continuation.transaction_id.clone(),
                    body: format!("{puppet}\n\n{}", continuation.instruction),
                    require_encrypted,
                };
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(45),
                    source
                        .ok_or_else(|| failure(503, "continuation_transport_unavailable"))?
                        .send_continuation(message),
                )
                .await;
                let event = result.ok().and_then(Result::ok);
                ledger
                    .settle_continuation(&identity.owner, continuation, event.as_deref())
                    .map_err(local_error)?
            } else {
                decided.action
            };
            json!({"agentId":agent,"runtimeActive":false,"decision":decision})
        }
        HistoryAction::Approvals { agent, cursor } => {
            json!({"agentId":agent,"approvals":ledger.approval_history(&identity.owner,&agent,cursor.as_deref()).map_err(local_error)?,"runtimeActive":active})
        }
        HistoryAction::Approval { agent, id } => {
            json!({"agentId":agent,"approval":ledger.approval_record(&identity.owner,&agent,&id).map_err(local_error)?,"runtimeActive":active})
        }
    };
    let fresh = native
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    if DeviceIdentity::from(&fresh) != pin || native.identity().await? != identity {
        return Err(failure(401, "owner_authorization_required"));
    }
    Ok(result)
}
fn local_error(error: hagency_agent_local::Error) -> NativeError {
    match error {
        hagency_agent_local::Error::Unauthorized => failure(403, "owner_scope_required"),
        hagency_agent_local::Error::Invalid(_) => failure(400, "invalid_history_query"),
        hagency_agent_local::Error::Denied => failure(409, "recovery_stop_unconfirmed"),
        hagency_agent_local::Error::Conflict => failure(409, "recovery_review_changed"),
        _ => failure(503, "owner_history_unavailable"),
    }
}

fn epoch_now() -> Result<i64, NativeError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|t| t.as_secs().try_into().ok())
        .ok_or_else(|| failure(503, "local_state_unavailable"))
}

fn file_error(error: hagency_agent_local::Error) -> NativeError {
    match error {
        hagency_agent_local::Error::Unauthorized => failure(403, "owner_scope_required"),
        hagency_agent_local::Error::Invalid(_) => failure(400, "invalid_file_history_query"),
        hagency_agent_local::Error::Conflict | hagency_agent_local::Error::Unknown => {
            failure(409, "file_history_changed")
        }
        _ => failure(503, "file_history_unavailable"),
    }
}
