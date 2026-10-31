//! Ordinary Matrix devices on the enrolled execution computer. Wire records
//! contain event references; decrypted prompts and answers stay on this host.
use super::*;
use crate::console::{device_execution::E2eeDeviceGrant, owned_runtime::e2ee::AgentDevice};
use hagency_agent_local::inbox::Dispatch;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::PathBuf, sync::LazyLock};
use tokio::sync::Mutex;

static DEVICES: LazyLock<Mutex<HashMap<String, Arc<AgentDevice>>>> =
    LazyLock::new(Default::default);
static SENDS: Mutex<()> = Mutex::const_new(());
pub(super) enum CryptoAction {
    Poll {
        after: Option<String>,
    },
    Device {
        assignment: String,
    },
    Confirm {
        assignment: String,
        event: String,
        addressed: bool,
        thread: Option<String>,
    },
    Thread {
        assignment: String,
        thread: String,
    },
    Send {
        work: String,
    },
}
impl CryptoAction {
    pub(super) fn body(self) -> (String, Value) {
        match self {
            Self::Poll { after } => ("crypto/poll".into(), json!({"after":after})),
            Self::Device { assignment } => {
                ("crypto/device".into(), json!({"assignmentId":assignment}))
            }
            Self::Confirm {
                assignment,
                event,
                addressed,
                thread,
            } => (
                "crypto/confirm".into(),
                json!({"assignmentId":assignment,"eventId":event,"addressed":addressed,"threadRoot":thread}),
            ),
            Self::Thread { assignment, thread } => (
                "crypto/thread".into(),
                json!({"assignmentId":assignment,"threadRoot":thread}),
            ),
            Self::Send { work } => ("crypto/send".into(), json!({"workId":work})),
        }
    }
}
impl From<CryptoAction> for Action {
    fn from(v: CryptoAction) -> Self {
        Self::Crypto(v)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Poll {
    scopes: Vec<CryptoScope>,
    next_after: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CryptoScope {
    assignment_id: String,
    puppet_mxid: String,
    room_id: String,
    manager_mxid: String,
    direct: bool,
    events: Vec<Candidate>,
}
#[derive(Deserialize)]
struct Candidate {
    event_id: String,
    room_id: String,
    sender_mxid: String,
}
fn sha(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
fn root(io: &Io, b: &Bound) -> Result<PathBuf, NativeError> {
    let root = io.crypto_directory()?.join(sha(&format!(
        "{}:{}:{}",
        b.scope.device.origin(),
        b.scope.device.owner_mxid(),
        b.scope.device.device_id()
    )));
    hagency_store::private::directory(&root).map_err(store)?;
    Ok(root)
}
pub(super) async fn device(io: &Io, b: &Bound, assignment: &str) -> Result<Arc<AgentDevice>, NativeError> {
    let root = root(io, b)?;
    let key = format!(
        "{}:{assignment}:{}",
        root.display(),
        b.scope.device.generation()
    );
    let mut devices = DEVICES.lock().await;
    if let Some(d) = devices.get(&key)
        && !d.is_revoked()
    {
        return Ok(d.clone());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Grant {
        user_id: String,
        device_id: String,
        access_token: String,
    }
    let grant: Timed<Grant> = b
        .request(
            CryptoAction::Device {
                assignment: assignment.into(),
            }
            .into(),
            false,
        )
        .await?;
    if !grant.value.device_id.starts_with("HAGE2EE")
        || !grant
            .value
            .device_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric())
        || grant.value.device_id.len() > 64
    {
        return Err(invalid());
    }
    let grant = E2eeDeviceGrant {
        user_id: grant.value.user_id,
        device_id: grant.value.device_id,
        access_token: grant.value.access_token,
        homeserver: b.scope.device.origin().into(),
    };
    let d = Arc::new(
        AgentDevice::open(grant, &root, assignment)
            .await
            .map_err(|_| failure(503, "shared_crypto_device_unavailable"))?,
    );
    devices.insert(key, d.clone());
    Ok(d)
}
impl SharedExecutionTransport {
    /// Poll before demand admission, even while the provider is stopped. Device
    /// keys must be advertised before another client can share room keys.
    pub(super) async fn pump_crypto(&self) -> Result<(), NativeError> {
        let b = self.io.bound().await?;
        if !b
            .demand_discovery()
            .await?
            .capabilities
            .iter()
            .any(|v| v == "shared-agent-e2ee-v1")
        {
            return Ok(());
        }
        let mut after = None;
        loop {
            let reply: Timed<Poll> = b
                .request(
                    CryptoAction::Poll {
                        after: after.clone(),
                    }
                    .into(),
                    false,
                )
                .await?;
            if reply.value.scopes.len() > 16
                || reply
                    .value
                    .next_after
                    .as_ref()
                    .is_some_and(|n| after.as_ref().is_some_and(|a| n <= a))
            {
                return Err(invalid());
            }
            for scope in reply.value.scopes {
                super::super::key(&scope.assignment_id)?;
                let d = device(&self.io, &b, &scope.assignment_id).await?;
                for event in scope.events.into_iter().take(16) {
                    if event.room_id != scope.room_id || event.sender_mxid == scope.puppet_mxid {
                        continue;
                    }
                    let message = match d
                        .read_candidate(&event.room_id, &event.event_id, &event.sender_mxid)
                        .await
                    {
                        Ok(message) => message,
                        Err(_) => continue, // Missing keys retain the original event for retry.
                    };
                    let mut addressed = message.as_ref().is_some_and(|m| {
                        m.mentions.contains(&scope.puppet_mxid)
                            || scope.direct && event.sender_mxid == scope.manager_mxid
                    });
                    let thread = message.as_ref().and_then(|m| m.thread_root.clone());
                    if !addressed && let Some(thread) = &thread {
                        let known: Timed<Value> = b
                            .request(
                                CryptoAction::Thread {
                                    assignment: scope.assignment_id.clone(),
                                    thread: thread.clone(),
                                }
                                .into(),
                                false,
                            )
                            .await?;
                        addressed = known.value.get("known").and_then(Value::as_bool) == Some(true);
                    }
                    let _: Timed<Value> = b
                        .request(
                            CryptoAction::Confirm {
                                assignment: scope.assignment_id.clone(),
                                event: event.event_id,
                                addressed,
                                thread,
                            }
                            .into(),
                            false,
                        )
                        .await?;
                }
            }
            let Some(next) = reply.value.next_after else {
                break;
            };
            after = Some(next);
        }
        Ok(())
    }
}
pub(super) async fn input(io: &Io, b: &Bound, w: &SharedWorkItem) -> Result<String, NativeError> {
    if !w.encrypted {
        return Err(invalid());
    }
    #[cfg(test)]
    if let Some(fixture) = &io.crypto_fixture {
        return fixture.input.clone().ok_or_else(|| failure(503, "shared_room_keys_pending"));
    }
    let d = device(io, b, &w.assignment_id).await?;
    let m = d
        .read_candidate(&w.room_id, &w.event_id, &w.requester_mxid)
        .await
        .map_err(|_| failure(503, "shared_room_keys_pending"))?
        .ok_or_else(invalid)?;
    if m.thread_root.as_deref().unwrap_or(&w.event_id) != w.thread_root {
        return Err(invalid());
    }
    Ok(m.body)
}
fn dispatch(w: &SharedWorkItem) -> Dispatch {
    Dispatch {
        id: w.id.clone(),
        binding_id: w.assignment_id.clone(),
        agent_id: w.assignment_id.clone(),
        event_id: w.event_id.clone(),
        room_id: w.room_id.clone(),
        requester_mxid: w.requester_mxid.clone(),
        thread_root: w.thread_root.clone(),
        body: String::new(),
        state: "started".into(),
        binding_generation: 1,
        dispatch_epoch: None,
        dispatch_device_id: None,
        execution_id: Some(w.execution_id.clone()),
        outcome: None,
    }
}

async fn send_reply(io: &Io, b: &Bound, w: &SharedWorkItem, body: &str) -> Result<String, NativeError> {
    #[cfg(test)]
    if let Some(fixture) = &io.crypto_fixture {
        fixture.sent.lock().unwrap().push((w.id.clone(), body.into()));
        return Ok(format!("$encrypted_reply_{}:test", w.id));
    }
    device(io, b, &w.assignment_id).await?
        .send_shared_reply(&dispatch(w), body).await
        .map_err(|_| failure(503, "shared_encrypted_send_pending"))
}

// Only the Matrix I/O boundary is substituted in supervised-process tests.
// Input pins, authority checks, durable delivery projection and replay stay real.
#[cfg(test)]
pub(super) struct CryptoFixture {
    input: Option<String>,
    sent: Arc<std::sync::Mutex<Vec<(String, String)>>>,
}
#[cfg(test)]
impl SharedExecutionTransport {
    pub(in crate::console::native) fn with_crypto_fixture(
        mut self, input: Option<String>,
    ) -> (Self, Arc<std::sync::Mutex<Vec<(String, String)>>>) {
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        self.io.crypto_fixture = Some(Arc::new(CryptoFixture {input, sent: sent.clone()}));
        (self, sent)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Sent {
    source_digest: String,
    body: String,
    sent_at_ms: u64,
    content: Value,
}
/// The ledger retains the original full receipt locally. Its wire projection
/// replaces only the answer with a closed Matrix delivery receipt and hashes
/// that projection separately; retries never confuse the two digests.
pub(super) async fn wire_receipt(
    io: &Io,
    b: &Bound,
    registry: &OwnerResourceRegistry,
    command: &SharedRuntimeCommand,
    w: &SharedWorkItem,
    r: &SharedWorkReceipt,
) -> Result<SharedWorkReceipt, NativeError> {
    if !w.encrypted {
        return Ok(r.clone());
    }
    let mut wire = r.clone();
    let Some(body) = &r.body else { return Ok(wire) };
    let dir = root(io, b)?.join("sent");
    hagency_store::private::directory(&dir).map_err(store)?;
    let path = dir.join(sha(&w.id));
    let _send = SENDS.lock().await;
    let saved = match hagency_store::private::read_secret(&path) {
        Ok(bytes) => serde_json::from_slice::<Sent>(&bytes).map_err(|_| invalid())?,
        Err(hagency_store::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            let authorized: Result<Timed<Value>, NativeError> = b
                .request(CryptoAction::Send { work: w.id.clone() }.into(), false)
                .await;
            let event = match authorized {
                Ok(_) => {
                    Some(send_reply(io, b, w, body).await?)
                }
                // The model already ran. Preserve usage even when permission to
                // publish was revoked; do not claim an uncertain send never ran.
                Err(e) if matches!(e.status, 401 | 403 | 409) => None,
                Err(e) => return Err(e),
            };
            let content = crate::console::owned_runtime::e2ee::reply_content(&dispatch(w), body)
                .map_err(|_| invalid())?;
            let saved=Sent {
                source_digest:sha(body),
                body:json!({"algorithm":"m.megolm.v1.aes-sha2","roomId":w.room_id,"eventId":event,"delivery":if event.is_some(){"sent"}else{"unknown"}}).to_string(),
                content:serde_json::to_value(content).map_err(|_|invalid())?,
                sent_at_ms:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|invalid())?.as_millis() as u64,
            };
            let bytes = serde_json::to_vec(&saved).map_err(|_| invalid())?;
            // Atomic replacement makes a crash leave either no receipt or the complete receipt.
            hagency_store::private::replace(&path, &bytes).map_err(store)?;
            saved
        }
        Err(e) => return Err(store(e)),
    };
    if saved.source_digest != sha(body) {
        return Err(invalid());
    }
    let metadata: Value = serde_json::from_str(&saved.body).map_err(|_| invalid())?;
    if let Some(event) = metadata["eventId"].as_str() {
        let content = &saved.content;
        let receipt = SharedReplyDelivery {
            version: 1,
            work_id: w.id.clone(),
            work_digest: digest(w)?,
            assignment_id: w.assignment_id.clone(),
            runtime_command_digest: w.runtime_command_digest.clone(),
            room_id: w.room_id.clone(),
            thread_root: w.thread_root.clone(),
            source_event_id: w.event_id.clone(),
            matrix_txn_id: format!("hagency_shared_{}", w.id),
            matrix_event_id: event.into(),
            matrix_content_digest: digest(&content)?,
            body: body.clone(),
            formatted_body: content["formatted_body"].as_str().map(str::to_owned),
            body_digest: sha(body),
            sent_at_ms: saved.sent_at_ms,
        };
        let record = SharedDeliveryRecord {
            receipt_digest: digest(&receipt)?,
            receipt,
        };
        registry
            .record_shared_reply_delivery(
                b.cleanup(registry, command)
                    .await?,
                w.clone(),
                record,
            )
            .await
            .map_err(store)?;
    }
    wire.body_digest = Some(sha(&saved.body));
    wire.body = Some(saved.body);
    Ok(wire)
}
