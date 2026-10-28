//! Host-only device transport. This module performs no model/tool execution and
//! never ACKs automatically: callers must durably persist events before ACK.
use super::{AuthorizedDevice, Console};
use hagency_agent_local::file_delivery::{
    self, Delivery as FileDelivery, Manifest as FileManifest, Target as FileTarget,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub mod ciphertexts;
pub mod encrypted;
pub use ciphertexts::Ciphertext;

const MAX_WIRE_BYTES: usize = 131_072;
const MAX_TEXT_BYTES: usize = 65_536;
// Up to 100 events with 64KiB text each, worst-case six-byte JSON escaping.
const MAX_RESPONSE_BYTES: usize = 40 * 1024 * 1024;

/// Immutable host-only profile/device pin. No bearer or browser token is stored.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct DeviceIdentity {
    pub(super) origin: String,
    pub(super) issuer: String,
    pub(super) subject: String,
    pub(super) owner: String,
    pub(super) user: String,
    pub(super) id: String,
    pub(super) generation: u64,
}
impl From<&AuthorizedDevice> for DeviceIdentity {
    fn from(device: &AuthorizedDevice) -> Self {
        Self {
            origin: device.origin().into(),
            issuer: device.issuer().into(),
            subject: device.subject().into(),
            owner: device.owner_mxid().into(),
            user: device.user_id().into(),
            id: device.device_id().into(),
            generation: device.generation(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceError {
    pub status: u16,
    pub code: String,
}
impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code)
    }
}
impl std::error::Error for DeviceError {}
fn error(status: u16, code: &str) -> DeviceError {
    DeviceError {
        status,
        code: code.into(),
    }
}
fn id(value: &str) -> Result<(), DeviceError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Err(error(400, "invalid_execution_arguments"))
    } else {
        Ok(())
    }
}
fn matrix(value: &str, prefix: char) -> Result<(), DeviceError> {
    if (prefix == '!' && super::matrix_identifiers::room_id(value))
        || (prefix != '!'
            && value.starts_with(prefix)
            && value.len() <= 255
            && value[1..]
                .split_once(':')
                .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
            && !value.chars().any(|c| c.is_control() || c.is_whitespace()))
    {
        Ok(())
    } else {
        Err(error(502, "invalid_execution_response"))
    }
}
fn now_ms() -> Result<i64, DeviceError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .map_err(|_| error(503, "invalid_local_clock"))
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeaseRef {
    pub agent_id: String,
    pub epoch: i64,
}
impl LeaseRef {
    fn validate(&self) -> Result<(), DeviceError> {
        id(&self.agent_id)?;
        if self.epoch < 1 {
            Err(error(400, "invalid_lease_epoch"))
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    pub agent_id: String,
    pub owner_user_id: String,
    pub device_id: String,
    pub device_generation: i64,
    pub epoch: i64,
    pub expires_at_ms: i64,
}
impl Lease {
    pub fn reference(&self) -> LeaseRef {
        LeaseRef {
            agent_id: self.agent_id.clone(),
            epoch: self.epoch,
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    pub id: String,
    pub binding_id: String,
    pub agent_id: String,
    pub event_id: String,
    pub room_id: String,
    pub requester_mxid: String,
    pub thread_root: String,
    pub body: String,
    pub state: String,
    pub binding_generation: i64,
    pub dispatch_epoch: Option<i64>,
    pub dispatch_device_id: Option<String>,
    pub execution_id: Option<String>,
    pub outcome: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionStart {
    pub dispatch: Dispatch,
    pub newly_started: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyReceipt {
    pub id: String,
    pub owner_event_id: String,
    pub agent_id: String,
    pub binding_id: String,
    pub owner_user_id: String,
    pub room_id: String,
    pub puppet_mxid: String,
    pub binding_generation: i64,
    pub dispatch_epoch: i64,
    pub delivery_epoch: i64,
    pub requester_mxid: String,
    pub thread_root: String,
    pub body: String,
    pub payload_digest: String,
    pub matrix_txn_id: String,
    pub state: String,
    pub matrix_event_id: Option<String>,
}
/// Taking over another live device lease requires a deliberate owner action.
#[derive(Clone, Copy)]
pub enum Takeover {
    Never,
    OwnerRequested,
}
#[derive(Clone, Copy)]
pub enum Outcome {
    Rejected,
    Failed,
    Unknown,
}
impl Outcome {
    fn wire(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}
/// Text already durably persisted for an original execution. These local
/// expectations never become client-supplied membership facts on the wire.
pub struct KnownReply {
    pub dispatch_id: String,
    pub execution_id: String,
    pub body: String,
    pub binding_id: String,
    pub room_id: String,
    pub requester_mxid: String,
    pub thread_root: String,
    pub binding_generation: i64,
    pub original_epoch: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistorySnapshot {
    pub count: u64,
    pub digest: String,
}
impl HistorySnapshot {
    fn validate(&self) -> Result<(), DeviceError> {
        if self.digest.len() != 64
            || !self
                .digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(error(502, "invalid_execution_history"));
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionHistory {
    pub agent_id: String,
    pub snapshot: HistorySnapshot,
    pub executions: Vec<hagency_agent_local::inbox::HistoryExecution>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessingReceipt {
    pub id: String,
    pub dispatch_id: String,
    pub execution_id: String,
    pub state: String,
    pub matrix_event_id: Option<String>,
}
impl ProcessingReceipt {
    fn validate(&self, dispatch_id: &str, execution_id: &str) -> Result<(), DeviceError> {
        id(&self.id).map_err(|_| error(502, "invalid_execution_response"))?;
        if self.dispatch_id != dispatch_id
            || self.execution_id != execution_id
            || !["pending", "sending", "sent", "unknown", "cancelled"]
                .contains(&self.state.as_str())
            || (self.state == "sent") != self.matrix_event_id.is_some()
            || self.matrix_event_id.as_ref().is_some_and(|event| {
                !event.starts_with('$') || event.len() > 1024 || event.chars().any(char::is_control)
            })
        {
            return Err(error(502, "execution_scope_mismatch"));
        }
        Ok(())
    }
}
/// Private host response. Never include descriptors in model tool output.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionAttachments {
    pub dispatch_id: String,
    pub execution_id: String,
    pub attachments: Vec<hagency_agent_local::attachments::Attachment>,
}
impl ExecutionAttachments {
    fn validate(&self, dispatch: &str, execution: &str) -> Result<(), DeviceError> {
        if self.dispatch_id != dispatch
            || self.execution_id != execution
            || self.attachments.len() > 1
        {
            return Err(error(502, "execution_scope_mismatch"));
        }
        for attachment in &self.attachments {
            attachment
                .validate()
                .map_err(|_| error(502, "invalid_execution_response"))?;
        }
        Ok(())
    }
}
/// Local original metadata used to validate every later response. Only target
/// is serialized; this value never grants file-byte or send ownership.
#[derive(Clone)]
pub struct FileExpectation {
    pub target: FileTarget,
    pub request_id: String,
    pub file: FileManifest,
}
impl FileExpectation {
    fn validate(&self) -> Result<(), DeviceError> {
        self.target
            .validate()
            .map_err(|_| error(400, "invalid_file_target"))?;
        id(&self.request_id)?;
        self.file
            .validate()
            .map_err(|_| error(400, "invalid_file_manifest"))
    }
    fn check(&self, delivery: &FileDelivery) -> Result<(), DeviceError> {
        delivery
            .validate(
                &self.target.dispatch_id,
                &self.target.execution_id,
                &self.request_id,
                &self.file,
            )
            .map_err(|_| error(502, "invalid_file_delivery_response"))?;
        if delivery.id != self.target.file_id {
            return Err(error(502, "execution_scope_mismatch"));
        }
        Ok(())
    }
}
pub enum DeviceOperation {
    PrepareFile {
        lease: LeaseRef,
        dispatch_id: String,
        execution_id: String,
        request_id: String,
        file: FileManifest,
    },
    StageFileChunk {
        lease: LeaseRef,
        expected: FileExpectation,
        offset: u64,
        bytes: Vec<u8>,
    },
    QueueFile {
        lease: LeaseRef,
        expected: FileExpectation,
    },
    /// Historical status only: no previous lease is supplied or renewed.
    FileDelivery {
        expected: FileExpectation,
    },
    /// Read by original request ID when no preparation response/file ID survived.
    LookupFile {
        request: file_delivery::FileLookup,
        file: FileManifest,
    },
    History {
        agent_id: String,
        cursor: Option<String>,
        snapshot: Option<HistorySnapshot>,
    },
    Acquire {
        agent_id: String,
        ttl_ms: i64,
        takeover: Takeover,
        history_snapshot: HistorySnapshot,
    },
    Renew {
        lease: LeaseRef,
        ttl_ms: i64,
    },
    Release {
        lease: LeaseRef,
    },
    PrepareCiphertext {
        lease: LeaseRef,
        expected: Ciphertext,
        projection: encrypted::Projection,
    },
    AckCiphertext {
        lease: LeaseRef,
        expected: Ciphertext,
        prepared: Box<encrypted::Admission>,
    },
    CiphertextAdmission {
        lease: LeaseRef,
        expected: Ciphertext,
    },
    PollCiphertexts {
        lease: LeaseRef,
        binding_id: String,
        limit: usize,
    },
    ReadCiphertext {
        lease: LeaseRef,
        inbox_id: String,
    },
    Poll {
        lease: LeaseRef,
        binding_id: String,
        limit: usize,
    },
    /// Explicit opt-in for a host with working attachment tools. Ordinary Poll
    /// stays text-only, including when a newer Server supports file admission.
    PollFiles {
        lease: LeaseRef,
        binding_id: String,
        limit: usize,
    },
    Attachments {
        lease: LeaseRef,
        dispatch_id: String,
        execution_id: String,
    },
    Ack {
        lease: LeaseRef,
        dispatch_id: String,
    },
    Start {
        lease: LeaseRef,
        dispatch_id: String,
        execution_id: String,
    },
    Processing {
        lease: LeaseRef,
        dispatch_id: String,
        execution_id: String,
    },
    AuthorizeTool {
        lease: LeaseRef,
        dispatch_id: String,
        execution_id: String,
    },
    Finish {
        lease: LeaseRef,
        dispatch_id: String,
        execution_id: String,
        outcome: Outcome,
    },
    ReconcileKnownReply {
        lease: LeaseRef,
        known: KnownReply,
    },
    Reply {
        lease: LeaseRef,
        dispatch_id: String,
        execution_id: String,
        body: String,
    },
}
pub enum DeviceResponse {
    FileObservation(file_delivery::FileObservation),
    FileDelivery(FileDelivery),
    History(ExecutionHistory),
    Lease(Lease),
    Released,
    Events(Vec<Dispatch>),
    Ciphertexts(Vec<Ciphertext>),
    EncryptedAdmission(encrypted::Admission),
    Ciphertext(Ciphertext),
    Attachments(ExecutionAttachments),
    Acknowledged,
    Started(ExecutionStart),
    Processing(ProcessingReceipt),
    ToolAuthorized(Dispatch),
    Finished,
    ReplyQueued(ReplyReceipt),
}
impl DeviceOperation {
    fn lease(&self) -> Option<&LeaseRef> {
        match self {
            Self::Acquire { .. }
            | Self::History { .. }
            | Self::FileDelivery { .. }
            | Self::LookupFile { .. } => None,
            Self::PrepareFile { lease, .. }
            | Self::StageFileChunk { lease, .. }
            | Self::QueueFile { lease, .. }
            | Self::Renew { lease, .. }
            | Self::Release { lease }
            | Self::PrepareCiphertext { lease, .. }
            | Self::AckCiphertext { lease, .. }
            | Self::CiphertextAdmission { lease, .. }
            | Self::PollCiphertexts { lease, .. }
            | Self::ReadCiphertext { lease, .. }
            | Self::Poll { lease, .. }
            | Self::PollFiles { lease, .. }
            | Self::Attachments { lease, .. }
            | Self::Ack { lease, .. }
            | Self::Start { lease, .. }
            | Self::Processing { lease, .. }
            | Self::AuthorizeTool { lease, .. }
            | Self::Finish { lease, .. }
            | Self::Reply { lease, .. }
            | Self::ReconcileKnownReply { lease, .. } => Some(lease),
        }
    }
    fn request(&self) -> Result<(&'static str, Vec<u8>), DeviceError> {
        if let Some(lease) = self.lease() {
            lease.validate()?;
        }
        let (path, body) = match self {
            Self::PrepareCiphertext {
                lease,
                expected,
                projection,
            } => {
                expected.validate(lease)?;
                projection.validate()?;
                (
                    "ciphertexts/prepare",
                    json!({"lease":lease,"inboxId":expected.id,"sourceDigest":expected.source_digest,"projection":projection}),
                )
            }
            Self::AckCiphertext {
                lease, expected, ..
            }
            | Self::CiphertextAdmission { lease, expected } => {
                expected.validate(lease)?;
                (
                    if matches!(self, Self::AckCiphertext { .. }) {
                        "ciphertexts/ack"
                    } else {
                        "ciphertexts/admission"
                    },
                    json!({"lease":lease,"inboxId":expected.id,"sourceDigest":expected.source_digest}),
                )
            }

            Self::PollCiphertexts {
                lease,
                binding_id,
                limit,
            } => {
                id(binding_id)?;
                if *limit == 0 || *limit > 4 {
                    return Err(error(400, "invalid_poll_limit"));
                }
                (
                    "ciphertexts/poll",
                    json!({"lease":lease,"bindingId":binding_id,"limit":limit}),
                )
            }
            Self::ReadCiphertext { lease, inbox_id } => {
                id(inbox_id)?;
                (
                    "ciphertexts/read",
                    json!({"lease":lease,"inboxId":inbox_id}),
                )
            }

            Self::PrepareFile {
                lease,
                dispatch_id,
                execution_id,
                request_id,
                file,
            } => {
                id(dispatch_id)?;
                id(execution_id)?;
                id(request_id)?;
                file.validate()
                    .map_err(|_| error(400, "invalid_file_manifest"))?;
                (
                    "files/prepare",
                    json!({"lease":lease,"dispatchId":dispatch_id,"executionId":execution_id,"requestId":request_id,"file":file}),
                )
            }
            Self::StageFileChunk {
                lease,
                expected,
                offset,
                bytes,
            } => {
                use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
                expected.validate()?;
                if bytes.is_empty()
                    || bytes.len() > file_delivery::CHUNK_BYTES
                    || offset.saturating_add(bytes.len() as u64) > expected.file.bytes
                {
                    return Err(error(400, "invalid_file_chunk"));
                }
                (
                    "files/chunk",
                    json!({"lease":lease,"target":expected.target,"offset":offset,"data":STANDARD_NO_PAD.encode(bytes)}),
                )
            }
            Self::QueueFile { lease, expected } => {
                expected.validate()?;
                (
                    "files/queue",
                    json!({"lease":lease,"target":expected.target}),
                )
            }
            Self::FileDelivery { expected } => {
                expected.validate()?;
                (
                    "files/status",
                    serde_json::to_value(&expected.target)
                        .map_err(|_| error(400, "invalid_file_target"))?,
                )
            }
            Self::LookupFile { request, file } => {
                request
                    .validate()
                    .and_then(|_| file.validate())
                    .map_err(|_| error(400, "invalid_file_lookup"))?;
                (
                    "files/lookup",
                    serde_json::to_value(request).map_err(|_| error(400, "invalid_file_lookup"))?,
                )
            }
            Self::History {
                agent_id,
                cursor,
                snapshot,
            } => {
                id(agent_id)?;
                if let Some(cursor) = cursor {
                    id(cursor)?;
                }
                if cursor.is_some() != snapshot.is_some() {
                    return Err(error(400, "invalid_execution_history"));
                }
                if let Some(snapshot) = snapshot {
                    snapshot.validate()?;
                }
                (
                    "history",
                    json!({"agentId":agent_id,"cursor":cursor,"snapshot":snapshot}),
                )
            }
            Self::Acquire {
                agent_id,
                ttl_ms,
                takeover,
                history_snapshot,
            } => {
                history_snapshot.validate()?;
                id(agent_id)?;
                ttl(*ttl_ms)?;
                (
                    "leases/acquire",
                    json!({"agentId":agent_id,"ttlMs":ttl_ms,"takeover":matches!(takeover,Takeover::OwnerRequested),"historySnapshot":history_snapshot}),
                )
            }
            Self::Renew { lease, ttl_ms } => {
                ttl(*ttl_ms)?;
                ("leases/renew", json!({"lease":lease,"ttlMs":ttl_ms}))
            }
            Self::Release { lease } => ("leases/release", json!({"lease":lease})),
            Self::Poll {
                lease,
                binding_id,
                limit,
            }
            | Self::PollFiles {
                lease,
                binding_id,
                limit,
            } => {
                id(binding_id)?;
                if !(1..=100).contains(limit) {
                    return Err(error(400, "invalid_poll_limit"));
                }
                let mut body = json!({"lease":lease,"bindingId":binding_id,"limit":limit});
                if matches!(self, Self::PollFiles { .. }) {
                    body["acceptFiles"] = true.into();
                }
                ("events/poll", body)
            }
            Self::Attachments {
                lease,
                dispatch_id,
                execution_id,
            } => {
                id(dispatch_id)?;
                id(execution_id)?;
                (
                    "files/attachments",
                    json!({"lease":lease,"dispatchId":dispatch_id,"executionId":execution_id}),
                )
            }
            Self::Ack { lease, dispatch_id } => {
                id(dispatch_id)?;
                (
                    "events/ack",
                    json!({"lease":lease,"dispatchId":dispatch_id}),
                )
            }
            Self::Start {
                lease,
                dispatch_id,
                execution_id,
            } => {
                id(dispatch_id)?;
                id(execution_id)?;
                (
                    "events/start",
                    json!({"lease":lease,"dispatchId":dispatch_id,"executionId":execution_id}),
                )
            }
            Self::Processing {
                lease,
                dispatch_id,
                execution_id,
            } => {
                id(dispatch_id)?;
                id(execution_id)?;
                (
                    "events/processing",
                    json!({"lease":lease,"dispatchId":dispatch_id,"executionId":execution_id}),
                )
            }
            Self::AuthorizeTool {
                lease,
                dispatch_id,
                execution_id,
            } => {
                id(dispatch_id)?;
                id(execution_id)?;
                (
                    "events/authorize-tool",
                    json!({"lease":lease,"dispatchId":dispatch_id,"executionId":execution_id}),
                )
            }
            Self::Finish {
                lease,
                dispatch_id,
                execution_id,
                outcome,
            } => {
                id(dispatch_id)?;
                id(execution_id)?;
                (
                    "events/finish",
                    json!({"lease":lease,"dispatchId":dispatch_id,"executionId":execution_id,"outcome":outcome.wire()}),
                )
            }
            Self::ReconcileKnownReply { lease, known } => {
                for value in [&known.dispatch_id, &known.execution_id, &known.binding_id] {
                    id(value)?;
                }
                matrix(&known.room_id, '!')
                    .map_err(|_| error(400, "invalid_execution_arguments"))?;
                matrix(&known.requester_mxid, '@')
                    .map_err(|_| error(400, "invalid_execution_arguments"))?;
                if known.original_epoch < 1
                    || known.original_epoch > lease.epoch
                    || known.binding_generation < 1
                    || !(known.thread_root.starts_with('$') || known.thread_root == known.room_id)
                    || known.thread_root.is_empty()
                    || known.thread_root.len() > 1024
                    || known.thread_root.chars().any(char::is_control)
                {
                    return Err(error(400, "invalid_execution_arguments"));
                }
                if known.body.is_empty() || known.body.len() > MAX_TEXT_BYTES {
                    return Err(error(400, "reply_too_large"));
                }
                (
                    "replies/reconcile-known",
                    json!({"lease":lease,"reply":{"dispatchId":known.dispatch_id,"executionId":known.execution_id,"body":known.body}}),
                )
            }
            Self::Reply {
                lease,
                dispatch_id,
                execution_id,
                body,
            } => {
                id(dispatch_id)?;
                id(execution_id)?;
                if body.is_empty() || body.len() > MAX_TEXT_BYTES {
                    return Err(error(400, "reply_too_large"));
                }
                (
                    "replies",
                    json!({"lease":lease,"reply":{"dispatchId":dispatch_id,"executionId":execution_id,"body":body}}),
                )
            }
        };
        let bytes =
            serde_json::to_vec(&body).map_err(|_| error(400, "invalid_execution_arguments"))?;
        if bytes.len() > MAX_WIRE_BYTES {
            return Err(error(400, "execution_wire_body_too_large"));
        }
        Ok((path, bytes))
    }
}
fn ttl(ttl: i64) -> Result<(), DeviceError> {
    if (1000..=60_000).contains(&ttl) {
        Ok(())
    } else {
        Err(error(400, "invalid_lease_ttl"))
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Result<T, DeviceError> {
    serde_json::from_value(value[key].clone()).map_err(|_| error(502, "invalid_execution_response"))
}
fn check_dispatch(
    dispatch: &Dispatch,
    reference: &LeaseRef,
    device: &AuthorizedDevice,
) -> Result<(), DeviceError> {
    for value in [&dispatch.id, &dispatch.binding_id, &dispatch.agent_id] {
        id(value).map_err(|_| error(502, "invalid_execution_response"))?;
    }
    matrix(&dispatch.room_id, '!')?;
    matrix(&dispatch.requester_mxid, '@')?;
    if dispatch.agent_id != reference.agent_id
        || dispatch.binding_generation < 1
        || dispatch.dispatch_epoch != Some(reference.epoch)
        || dispatch.dispatch_device_id.as_deref() != Some(device.device_id())
        || dispatch.body.len() > MAX_TEXT_BYTES
        || dispatch.event_id.is_empty()
        || dispatch.event_id.len() > 1024
        || dispatch.event_id.chars().any(char::is_control)
        || !(dispatch.thread_root.starts_with('$') || dispatch.thread_root == dispatch.room_id)
        || dispatch.thread_root.is_empty()
        || dispatch.thread_root.len() > 1024
        || dispatch.thread_root.chars().any(char::is_control)
    {
        return Err(error(502, "execution_scope_mismatch"));
    }
    Ok(())
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubmitDigest<'a> {
    dispatch_id: &'a str,
    execution_id: &'a str,
    body: &'a str,
}
fn payload_digest(
    dispatch_id: &str,
    execution_id: &str,
    body: &str,
) -> Result<String, DeviceError> {
    let encoded = serde_json::to_vec(&SubmitDigest {
        dispatch_id,
        execution_id,
        body,
    })
    .map_err(|_| error(400, "invalid_execution_arguments"))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}
fn check_reply(
    reply: &ReplyReceipt,
    lease: &LeaseRef,
    device: &AuthorizedDevice,
    dispatch_id: &str,
    execution_id: &str,
    body: &str,
) -> Result<(), DeviceError> {
    if reply.owner_event_id != dispatch_id
        || reply.agent_id != lease.agent_id
        || reply.owner_user_id != device.user_id()
        || reply.binding_generation < 1
        || reply.delivery_epoch < 1
        || reply.body != body
        || reply.payload_digest != payload_digest(dispatch_id, execution_id, body)?
        || reply.matrix_txn_id != format!("hagency_{:x}", Sha256::digest(dispatch_id.as_bytes()))
        || !["pending", "unknown", "sending", "sent", "cancelled"].contains(&reply.state.as_str())
        || !(reply.thread_root.starts_with('$') || reply.thread_root == reply.room_id)
        || reply.thread_root.is_empty()
        || reply.thread_root.len() > 1024
        || reply.thread_root.chars().any(char::is_control)
    {
        return Err(error(502, "execution_scope_mismatch"));
    }
    matrix(&reply.room_id, '!')?;
    matrix(&reply.puppet_mxid, '@')?;
    matrix(&reply.requester_mxid, '@')?;
    id(&reply.id).map_err(|_| error(502, "invalid_execution_response"))?;
    id(&reply.binding_id).map_err(|_| error(502, "invalid_execution_response"))?;
    if reply
        .matrix_event_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 1024 || id.chars().any(char::is_control))
    {
        return Err(error(502, "execution_scope_mismatch"));
    }
    // Durable acceptance and historical sent receipts do not permit model replay.
    Ok(())
}
fn parse_response(
    operation: &DeviceOperation,
    value: Value,
    device: &AuthorizedDevice,
) -> Result<DeviceResponse, DeviceError> {
    let invalid = || error(502, "invalid_execution_response");
    match operation {
        DeviceOperation::LookupFile { request, file } => {
            let observation: file_delivery::FileObservation = decode(&value, "observation")?;
            observation
                .validate(request, file)
                .map_err(|_| error(502, "invalid_file_observation_response"))?;
            Ok(DeviceResponse::FileObservation(observation))
        }
        DeviceOperation::PrepareFile {
            dispatch_id,
            execution_id,
            request_id,
            file,
            ..
        } => {
            let delivery: FileDelivery = decode(&value, "delivery")?;
            delivery
                .validate(dispatch_id, execution_id, request_id, file)
                .map_err(|_| error(502, "invalid_file_delivery_response"))?;
            Ok(DeviceResponse::FileDelivery(delivery))
        }
        DeviceOperation::StageFileChunk {
            expected,
            offset,
            bytes,
            ..
        } => {
            let delivery: FileDelivery = decode(&value, "delivery")?;
            expected.check(&delivery)?;
            if delivery.state != "staging"
                || delivery.received_bytes < offset.saturating_add(bytes.len() as u64)
            {
                return Err(error(502, "invalid_file_delivery_response"));
            }
            Ok(DeviceResponse::FileDelivery(delivery))
        }
        DeviceOperation::QueueFile { expected, .. }
        | DeviceOperation::FileDelivery { expected } => {
            let delivery: FileDelivery = decode(&value, "delivery")?;
            expected.check(&delivery)?;
            if matches!(operation, DeviceOperation::QueueFile { .. }) && delivery.state == "staging"
            {
                return Err(error(502, "invalid_file_delivery_response"));
            }
            Ok(DeviceResponse::FileDelivery(delivery))
        }
        DeviceOperation::History {
            agent_id,
            cursor,
            snapshot,
        } => {
            let history: ExecutionHistory = decode(&value, "history")?;
            history.snapshot.validate()?;
            if history.agent_id != *agent_id
                || history.executions.len() > 128
                || snapshot.as_ref().is_some_and(|s| s != &history.snapshot)
            {
                return Err(invalid());
            }
            let mut previous = cursor.as_deref();
            for h in &history.executions {
                for v in [
                    &h.dispatch_id,
                    &h.execution_id,
                    &h.binding_id,
                    &h.dispatch_device_id,
                ] {
                    id(v).map_err(|_| invalid())?;
                }
                matrix(&h.room_id, '!')?;
                matrix(&h.requester_mxid, '@')?;
                if h.agent_id != *agent_id
                    || h.dispatch_epoch < 1
                    || h.binding_generation < 1
                    || h.immutable_digest.len() != 64
                    || !h
                        .immutable_digest
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                    || previous.is_some_and(|p| p >= h.dispatch_id.as_str())
                    || !h.event_id.starts_with('$')
                    || h.event_id.len() > 1024
                    || !(h.thread_root.starts_with('$') || h.thread_root == h.room_id)
                    || h.thread_root.is_empty()
                    || h.thread_root.len() > 1024
                    || h.thread_root.chars().any(char::is_control)
                {
                    return Err(invalid());
                }
                previous = Some(&h.dispatch_id);
            }
            if let Some(next) = &history.next_cursor
                && (history.executions.is_empty() || Some(next.as_str()) != previous)
            {
                return Err(invalid());
            }
            Ok(DeviceResponse::History(history))
        }
        DeviceOperation::Acquire { agent_id, .. }
        | DeviceOperation::Renew {
            lease: LeaseRef { agent_id, .. },
            ..
        } => {
            let lease: Lease = decode(&value, "lease")?;
            let remaining = lease
                .expires_at_ms
                .checked_sub(now_ms()?)
                .filter(|n| *n > 0 && *n <= 31_000)
                .ok_or_else(invalid)?;
            if &lease.agent_id != agent_id
                || lease.owner_user_id != device.user_id()
                || lease.device_id != device.device_id()
                || lease.device_generation
                    != i64::try_from(device.generation()).map_err(|_| invalid())?
                || lease.epoch < 1
                || std::time::Instant::now() + Duration::from_millis(remaining as u64)
                    > device.valid_until() + Duration::from_secs(1)
                || operation
                    .lease()
                    .is_some_and(|reference| reference.epoch != lease.epoch)
            {
                return Err(error(502, "execution_scope_mismatch"));
            }
            Ok(DeviceResponse::Lease(lease))
        }
        DeviceOperation::PrepareCiphertext {
            lease,
            expected,
            projection,
        } => {
            let result: encrypted::Admission = decode(&value, "admission")?;
            result.check(expected, lease, device, Some(projection))?;
            Ok(DeviceResponse::EncryptedAdmission(result))
        }
        DeviceOperation::AckCiphertext {
            lease,
            expected,
            prepared,
        } => {
            let result: encrypted::Admission = decode(&value, "admission")?;
            result.check(expected, lease, device, None)?;
            if !result.same_input(prepared) || result.dispatch.state != "encrypted_acknowledged" {
                return Err(invalid());
            }
            Ok(DeviceResponse::EncryptedAdmission(result))
        }
        DeviceOperation::CiphertextAdmission { lease, expected } => {
            let result: encrypted::Admission = decode(&value, "admission")?;
            result.check(expected, lease, device, None)?;
            Ok(DeviceResponse::EncryptedAdmission(result))
        }
        DeviceOperation::PollCiphertexts {
            lease,
            binding_id,
            limit,
        } => {
            let events: Vec<Ciphertext> = decode(&value, "ciphertexts")?;
            if events.len() > *limit {
                return Err(invalid());
            }
            let mut ids = std::collections::BTreeSet::new();
            for event in &events {
                event.validate(lease)?;
                if event.binding_id != *binding_id || !ids.insert(&event.id) {
                    return Err(invalid());
                }
            }
            Ok(DeviceResponse::Ciphertexts(events))
        }
        DeviceOperation::ReadCiphertext { lease, inbox_id } => {
            let event: Ciphertext = decode(&value, "ciphertext")?;
            event.validate(lease)?;
            if event.id != *inbox_id {
                return Err(invalid());
            }
            Ok(DeviceResponse::Ciphertext(event))
        }
        DeviceOperation::Poll {
            lease,
            binding_id,
            limit,
        }
        | DeviceOperation::PollFiles {
            lease,
            binding_id,
            limit,
        } => {
            let events: Vec<Dispatch> = decode(&value, "events")?;
            if events.len() > *limit {
                return Err(invalid());
            }
            let mut ids = std::collections::BTreeSet::new();
            for event in &events {
                check_dispatch(event, lease, device)?;
                if event.binding_id != *binding_id
                    || !["offered", "acknowledged"].contains(&event.state.as_str())
                    || !ids.insert(&event.id)
                {
                    return Err(invalid());
                }
            }
            Ok(DeviceResponse::Events(events))
        }
        DeviceOperation::Attachments {
            dispatch_id,
            execution_id,
            ..
        } => {
            let attachments: ExecutionAttachments =
                serde_json::from_value(value).map_err(|_| invalid())?;
            attachments.validate(dispatch_id, execution_id)?;
            Ok(DeviceResponse::Attachments(attachments))
        }
        DeviceOperation::Start {
            lease,
            dispatch_id,
            execution_id,
        } => {
            let start: ExecutionStart = decode(&value, "execution")?;
            check_dispatch(&start.dispatch, lease, device)?;
            if &start.dispatch.id != dispatch_id
                || start.dispatch.execution_id.as_deref() != Some(execution_id.as_str())
                || start.dispatch.state != "running"
            {
                return Err(error(502, "execution_scope_mismatch"));
            }
            Ok(DeviceResponse::Started(start))
        }
        DeviceOperation::Processing {
            dispatch_id,
            execution_id,
            ..
        } => {
            let receipt: ProcessingReceipt = decode(&value, "processing")?;
            receipt.validate(dispatch_id, execution_id)?;
            Ok(DeviceResponse::Processing(receipt))
        }
        DeviceOperation::AuthorizeTool {
            lease,
            dispatch_id,
            execution_id,
        } => {
            let dispatch: Dispatch = decode(&value, "dispatch")?;
            check_dispatch(&dispatch, lease, device)?;
            if dispatch.id != *dispatch_id
                || dispatch.execution_id.as_deref() != Some(execution_id.as_str())
                || dispatch.state != "running"
            {
                return Err(error(502, "execution_scope_mismatch"));
            }
            Ok(DeviceResponse::ToolAuthorized(dispatch))
        }
        DeviceOperation::Reply {
            lease,
            dispatch_id,
            execution_id,
            body,
        } => {
            let reply: ReplyReceipt = decode(&value, "reply")?;
            check_reply(&reply, lease, device, dispatch_id, execution_id, body)?;
            if reply.dispatch_epoch != lease.epoch || reply.delivery_epoch != lease.epoch {
                return Err(error(502, "execution_scope_mismatch"));
            }
            Ok(DeviceResponse::ReplyQueued(reply))
        }
        DeviceOperation::ReconcileKnownReply { lease, known } => {
            let reply: ReplyReceipt = decode(&value, "reply")?;
            check_reply(
                &reply,
                lease,
                device,
                &known.dispatch_id,
                &known.execution_id,
                &known.body,
            )?;
            if reply.dispatch_epoch != known.original_epoch
                || reply.binding_id != known.binding_id
                || reply.room_id != known.room_id
                || reply.requester_mxid != known.requester_mxid
                || reply.thread_root != known.thread_root
                || reply.binding_generation != known.binding_generation
                || (reply.state != "sent" && reply.delivery_epoch != lease.epoch)
                || (reply.state == "sent"
                    && (reply.delivery_epoch < 1
                        || reply.delivery_epoch > lease.epoch
                        || reply.matrix_event_id.as_deref().is_none_or(str::is_empty)))
            {
                return Err(error(502, "execution_scope_mismatch"));
            }
            Ok(DeviceResponse::ReplyQueued(reply))
        }
        DeviceOperation::Release { .. } if value["released"] == true => {
            Ok(DeviceResponse::Released)
        }
        DeviceOperation::Ack { .. } if value["acknowledged"] == true => {
            Ok(DeviceResponse::Acknowledged)
        }
        DeviceOperation::Finish { .. } if value["finished"] == true => Ok(DeviceResponse::Finished),
        _ => Err(invalid()),
    }
}
impl Console {
    /// Obtain a fresh owner/device snapshot for each operation. No credential,
    /// arbitrary URL or arbitrary remote path is accepted from a caller.
    pub async fn execution_api(
        &self,
        operation: DeviceOperation,
    ) -> Result<DeviceResponse, DeviceError> {
        self.execution_api_inner(operation, None, None).await
    }
    pub(super) async fn execution_api_pinned(
        &self,
        operation: DeviceOperation,
        expected: &DeviceIdentity,
    ) -> Result<DeviceResponse, DeviceError> {
        self.execution_api_inner(operation, Some(expected), None)
            .await
    }
    pub(super) async fn execution_api_pinned_live(
        &self,
        operation: DeviceOperation,
        expected: &DeviceIdentity,
        live: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<DeviceResponse, DeviceError> {
        self.execution_api_inner(operation, Some(expected), Some(live))
            .await
    }
    async fn execution_api_inner(
        &self,
        operation: DeviceOperation,
        expected: Option<&DeviceIdentity>,
        live: Option<std::sync::Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> Result<DeviceResponse, DeviceError> {
        let (path, body) = operation.request()?;
        if live.as_ref().is_some_and(|f| !f()) {
            return Err(error(409, "agent_crypto_scope_retired"));
        }
        let device = self.authorized_device().await.map_err(|e| {
            if matches!(e, super::Error::Unavailable) {
                error(503, "server_unavailable")
            } else {
                error(401, "device_authorization_required")
            }
        })?;
        if expected.is_some_and(|pin| pin != &DeviceIdentity::from(&device)) {
            return Err(error(401, "device_authorization_required"));
        }
        let server =
            Url::parse(device.origin()).map_err(|_| error(503, "invalid_server_binding"))?;
        let client = super::server_login::http().map_err(|_| error(503, "server_unavailable"))?;
        let bearer = device
            .bearer()
            .map_err(|_| error(401, "device_authorization_required"))?;
        let request = client
            .post(
                server
                    .join(&format!("/hagency/api/v1/execution/{path}"))
                    .unwrap(),
            )
            .bearer_auth(bearer)
            .header("content-type", "application/json")
            .body(body);
        if live.as_ref().is_some_and(|f| !f()) {
            return Err(error(409, "agent_crypto_scope_retired"));
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| error(503, "server_unavailable"))?;
        let status = response.status().as_u16();
        let mut raw = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| error(503, "server_unavailable"))?
        {
            if raw.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(error(502, "execution_response_too_large"));
            }
            raw.extend_from_slice(&chunk);
        }
        // A response from an already-started request cannot resurrect local
        // authorization revoked while that request was in flight.
        device
            .bearer()
            .map_err(|_| error(401, "device_authorization_required"))?;
        let current = self.authorized_device().await.map_err(|e| {
            if matches!(e, super::Error::Unavailable) {
                error(503, "server_unavailable")
            } else {
                error(401, "device_authorization_required")
            }
        })?;
        current
            .bearer()
            .map_err(|_| error(401, "device_authorization_required"))?;
        if DeviceIdentity::from(&current) != DeviceIdentity::from(&device) {
            return Err(error(401, "device_authorization_required"));
        }
        let value: Value =
            serde_json::from_slice(&raw).map_err(|_| error(502, "invalid_execution_response"))?;
        if !(200..300).contains(&status) {
            let code = value["code"]
                .as_str()
                .filter(|s| {
                    !s.is_empty()
                        && s.len() <= 80
                        && s.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                })
                .unwrap_or("execution_request_failed");
            return Err(error(status, code));
        }
        parse_response(&operation, value, &device)
    }
}

#[cfg(test)]
mod processing_tests {
    use super::*;
    #[test]
    fn processing_route_is_closed_and_receipt_pins_execution() {
        let op = DeviceOperation::Processing {
            lease: LeaseRef {
                agent_id: "agent-a".into(),
                epoch: 1,
            },
            dispatch_id: "dispatch-a".into(),
            execution_id: "execution-a".into(),
        };
        let (route, bytes) = op.request().unwrap();
        assert_eq!(route, "events/processing");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            json!({"lease":{"agentId":"agent-a","epoch":1},"dispatchId":"dispatch-a","executionId":"execution-a"})
        );
        let mut receipt: ProcessingReceipt=serde_json::from_value(json!({"id":"processing-a","dispatchId":"dispatch-a","executionId":"execution-a","state":"pending","matrixEventId":null})).unwrap();
        receipt.validate("dispatch-a", "execution-a").unwrap();
        assert!(receipt.validate("dispatch-other", "execution-a").is_err());
        assert!(receipt.validate("dispatch-a", "execution-other").is_err());
        receipt.state = "sent".into();
        assert!(receipt.validate("dispatch-a", "execution-a").is_err());
        receipt.matrix_event_id = Some("$eyes:test".into());
        receipt.validate("dispatch-a", "execution-a").unwrap();
        receipt.matrix_event_id = Some("!room:test".into());
        assert!(receipt.validate("dispatch-a", "execution-a").is_err());
        receipt.state = "arbitrary".into();
        assert!(receipt.validate("dispatch-a", "execution-a").is_err());
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    fn lease() -> LeaseRef {
        LeaseRef {
            agent_id: "agent-a".into(),
            epoch: 1,
        }
    }
    #[test]
    fn legacy_poll_does_not_opt_in_and_attachment_lookup_cannot_choose_a_url() {
        let legacy = DeviceOperation::Poll {
            lease: lease(),
            binding_id: "binding-a".into(),
            limit: 1,
        };
        let capable = DeviceOperation::PollFiles {
            lease: lease(),
            binding_id: "binding-a".into(),
            limit: 1,
        };
        let (route, bytes) = legacy.request().unwrap();
        assert_eq!(route, "events/poll");
        let mut expected: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(expected.get("acceptFiles").is_none());
        expected["acceptFiles"] = true.into();
        assert_eq!(
            serde_json::from_slice::<Value>(&capable.request().unwrap().1).unwrap(),
            expected
        );
        let lookup = DeviceOperation::Attachments {
            lease: lease(),
            dispatch_id: "dispatch-a".into(),
            execution_id: "execution-a".into(),
        };
        let (route, bytes) = lookup.request().unwrap();
        assert_eq!(route, "files/attachments");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            json!({"lease":{"agentId":"agent-a","epoch":1},"dispatchId":"dispatch-a","executionId":"execution-a"})
        );
    }
    #[test]
    fn private_attachment_response_pins_request_and_rejects_mutated_source() {
        let attachment = hagency_agent_local::attachments::Attachment::from_content(
            &json!({"msgtype":"m.file","body":"report.txt","url":"mxc://media.test/opaque"}),
        )
        .unwrap();
        let value = json!({"dispatchId":"dispatch-a","executionId":"execution-a","attachments":[attachment]});
        let receipt: ExecutionAttachments = serde_json::from_value(value.clone()).unwrap();
        receipt.validate("dispatch-a", "execution-a").unwrap();
        assert!(receipt.validate("dispatch-b", "execution-a").is_err());
        assert!(receipt.validate("dispatch-a", "execution-b").is_err());
        assert!(!format!("{receipt:?}").contains("mxc://"));
        let mut changed = value.clone();
        changed["attachments"][0]["source"]["url"] = json!("mxc://other.test/replaced");
        assert!(
            serde_json::from_value::<ExecutionAttachments>(changed)
                .unwrap()
                .validate("dispatch-a", "execution-a")
                .is_err()
        );
        let mut extra = value.clone();
        extra["roomId"] = json!("!other:test");
        assert!(serde_json::from_value::<ExecutionAttachments>(extra).is_err());
        let mut many = value;
        many["attachments"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(attachment).unwrap());
        assert!(
            serde_json::from_value::<ExecutionAttachments>(many)
                .unwrap()
                .validate("dispatch-a", "execution-a")
                .is_err()
        );
    }
}

#[cfg(test)]
mod file_delivery_tests {
    use super::*;
    fn expected() -> FileExpectation {
        FileExpectation {
            target: FileTarget {
                dispatch_id: "dispatch-a".into(),
                execution_id: "execution-a".into(),
                file_id: "file-a".into(),
            },
            request_id: "request-a".into(),
            file: FileManifest {
                filename: "report.bin".into(),
                caption: None,
                bytes: file_delivery::MAX_BYTES as u64,
                sha256: "0".repeat(64),
            },
        }
    }
    #[test]
    fn file_wire_is_bounded_and_status_never_carries_a_lease_or_destination() {
        let lease = LeaseRef {
            agent_id: "agent-a".into(),
            epoch: 1,
        };
        let original = expected();
        let chunk = DeviceOperation::StageFileChunk {
            lease: lease.clone(),
            expected: original.clone(),
            offset: 0,
            bytes: vec![255; file_delivery::CHUNK_BYTES],
        };
        let (route, bytes) = chunk.request().unwrap();
        assert_eq!(route, "files/chunk");
        assert!(bytes.len() < MAX_WIRE_BYTES);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert_eq!(
            value["target"],
            serde_json::to_value(&original.target).unwrap()
        );
        use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
        assert_eq!(
            STANDARD_NO_PAD
                .decode(value["data"].as_str().unwrap())
                .unwrap(),
            vec![255; file_delivery::CHUNK_BYTES]
        );
        for (offset, length) in [
            (0, 0),
            (0, file_delivery::CHUNK_BYTES + 1),
            (file_delivery::MAX_BYTES as u64, 1),
            (u64::MAX, 1),
        ] {
            assert!(
                DeviceOperation::StageFileChunk {
                    lease: lease.clone(),
                    expected: original.clone(),
                    offset,
                    bytes: vec![0; length]
                }
                .request()
                .is_err()
            );
        }
        let status = DeviceOperation::FileDelivery {
            expected: original.clone(),
        };
        assert!(status.lease().is_none());
        let (route, bytes) = status.request().unwrap();
        assert_eq!(route, "files/status");
        let lookup = DeviceOperation::LookupFile {
            request: file_delivery::FileLookup {
                dispatch_id: original.target.dispatch_id.clone(),
                execution_id: original.target.execution_id.clone(),
                request_id: original.request_id.clone(),
            },
            file: original.file.clone(),
        };
        assert!(lookup.lease().is_none());
        let (lookup_route, lookup_bytes) = lookup.request().unwrap();
        assert_eq!(lookup_route, "files/lookup");
        assert_eq!(
            serde_json::from_slice::<Value>(&lookup_bytes).unwrap(),
            json!({"dispatchId":original.target.dispatch_id,"executionId":original.target.execution_id,"requestId":original.request_id})
        );

        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            serde_json::to_value(original.target).unwrap()
        );
    }
}
