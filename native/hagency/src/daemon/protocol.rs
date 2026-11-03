//! Versioned local control protocol (ADR 0012, API version 1).
//!
//! A frame is a 4-byte big-endian payload length followed by one UTF-8 JSON
//! object. Field names are camelCase and enum values are snake_case, except
//! the command `type`, which uses the documented camelCase verbs. Nothing in
//! this module grants authority: the listener authenticates the peer and the
//! dispatcher enforces owner scope, revision and idempotency.
use hagency_metering::dashboard::{DashboardSnapshot, HostingState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Local daemon API version. Independent of, but currently equal to, the
/// dashboard DTO version carried inside snapshots.
pub const API_VERSION: u32 = 1;
/// Largest accepted frame payload in either direction.
pub const MAX_FRAME: usize = 64 * 1024;
pub const MAX_REPLY_FRAME: usize = 4 * 1024 * 1024;
/// Request identifiers are client-chosen, retried verbatim after an
/// uncertain outcome, and never reused for another command.
pub const MAX_REQUEST_ID: usize = 64;
const MAX_OWNER_ID: usize = 255;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub request_id: String,
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    pub command: Command,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Command {
    /// Version/owner discovery. Read-only; requires no owner scope.
    Hello,
    /// Current dashboard snapshot for the scoped owner.
    Snapshot {
        #[serde(default)]
        period: Option<Period>,
        #[serde(default, rename="usageFilter", skip_serializing_if="Option::is_none")]
        usage_filter: Option<hagency_metering::dashboard::UsageFilter>,
    },
    /// Stop admitting new work; running work continues to completion.
    PauseHosting,
    /// Reopen admission, restarting hosting if it was stopped.
    ResumeHosting,
    /// Gracefully stop hosting: fence admission, drain workers and release
    /// the owner root. Confirmed as stopped only after the drain completes.
    StopHosting,
    /// Request fresh provider plan observations.
    RefreshProviders,
    Bridge { call: super::bridge::Call },
    #[cfg(unix)]
    Enrollment {call:super::enrollment_rpc::Call},
}

impl Command {
    pub fn is_mutation(&self) -> bool {
        if matches!(self, Self::Bridge { .. }) { return true; }
        #[cfg(unix)]
        if matches!(self, Self::Enrollment { .. }) { return true; }
        matches!(
            self,
            Self::PauseHosting | Self::ResumeHosting | Self::StopHosting | Self::RefreshProviders
        )
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello => "hello",
            Self::Snapshot { .. } => "snapshot",
            Self::PauseHosting => "pauseHosting",
            Self::ResumeHosting => "resumeHosting",
            Self::StopHosting => "stopHosting",
            Self::RefreshProviders => "refreshProviders",
            Self::Bridge { .. } => "bridge",
            #[cfg(unix)]
            Self::Enrollment{..}=>"enrollment",
        }
    }
}

/// Reporting period shared with the telemetry pipeline. Calendar views never
/// imply that a provider reports matching allowance windows.
pub use hagency_metering::dashboard::period::UsagePeriod as Period;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reply {
    pub version: u32,
    /// Echo of the request ID, or empty when the request could not be parsed.
    pub request_id: String,
    /// Durable control revision. Compare `expectedRevision` against this.
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

/// Mutation result. `Accepted` is not completion: the snapshot reports the
/// transition (for example `draining`) until the backend confirms it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeState {
    Accepted,
    Completed,
    Unchanged,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandOutcome {
    pub state: OutcomeState,
    pub hosting: HostingState,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    pub api_version: u32,
    pub dashboard_version: u32,
    /// Adopted owner, if any. A controller signed into a different account
    /// must not display this daemon's data.
    pub owner_id: Option<String>,
    pub server: Option<String>,
    pub hosting: HostingState,
}

impl Reply {
    pub fn ok(request_id: &str, revision: u64, result: Value) -> Self {
        Self {
            version: API_VERSION,
            request_id: request_id.into(),
            revision,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(request_id: &str, revision: u64, error: DaemonError) -> Self {
        Self {
            version: API_VERSION,
            request_id: request_id.into(),
            revision,
            result: None,
            error: Some(error.body()),
        }
    }
    /// Typed accessors for controller connectors.
    pub fn snapshot(&self) -> Result<DashboardSnapshot, ErrorBody> {
        self.typed()
    }
    pub fn outcome(&self) -> Result<CommandOutcome, ErrorBody> {
        self.typed()
    }
    pub fn hello(&self) -> Result<Hello, ErrorBody> {
        self.typed()
    }
    fn typed<T: serde::de::DeserializeOwned>(&self) -> Result<T, ErrorBody> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let value = self.result.clone().ok_or_else(|| invalid_reply())?;
        serde_json::from_value(value).map_err(|_| invalid_reply())
    }
}

fn invalid_reply() -> ErrorBody {
    DaemonError::new("invalid_reply", "The daemon reply was not understood.", false).body()
}

/// Structured, static error. Messages are fixed English text and never
/// contain provider output, paths, tokens or other source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonError {
    pub code: &'static str,
    pub message: &'static str,
    pub retryable: bool,
}

impl DaemonError {
    pub const fn new(code: &'static str, message: &'static str, retryable: bool) -> Self {
        Self {
            code,
            message,
            retryable,
        }
    }
    pub fn body(&self) -> ErrorBody {
        ErrorBody {
            code: self.code.into(),
            message: self.message.into(),
            retryable: self.retryable,
        }
    }
}

pub mod errors {
    use super::DaemonError;
    pub const UNSUPPORTED_VERSION: DaemonError = DaemonError::new(
        "unsupported_version",
        "This controller and the owner daemon use different API versions. Update the older one.",
        false,
    );
    pub const INVALID_REQUEST: DaemonError =
        DaemonError::new("invalid_request", "The request was malformed.", false);
    pub const FRAME_TOO_LARGE: DaemonError =
        DaemonError::new("frame_too_large", "The request exceeded the size limit.", false);
    pub const OWNER_SCOPE_REQUIRED: DaemonError = DaemonError::new(
        "owner_scope_required",
        "This request must name the signed-in owner.",
        false,
    );
    pub const OWNER_SCOPE_MISMATCH: DaemonError = DaemonError::new(
        "owner_scope_mismatch",
        "The daemon hosts a different owner account.",
        false,
    );
    pub const OWNER_NOT_ADOPTED: DaemonError = DaemonError::new(
        "owner_not_adopted",
        "No owner account has been enrolled with this daemon.",
        false,
    );
    pub const REVISION_CONFLICT: DaemonError = DaemonError::new(
        "revision_conflict",
        "Hosting changed since this view was loaded. Refresh and try again.",
        false,
    );
    pub const REQUEST_ID_REUSED: DaemonError = DaemonError::new(
        "request_id_reused",
        "This request ID was already used for a different command.",
        false,
    );
    pub const IN_PROGRESS: DaemonError = DaemonError::new(
        "in_progress",
        "This command is still being applied. Retry with the same request ID.",
        true,
    );
    pub const BUSY: DaemonError = DaemonError::new(
        "busy",
        "Another hosting change is in progress. Try again shortly.",
        true,
    );
    pub const NOT_SUPPORTED: DaemonError = DaemonError::new(
        "not_supported",
        "This daemon build does not support that operation.",
        false,
    );
    pub const HOSTING_STOPPED: DaemonError = DaemonError::new(
        "hosting_stopped",
        "Hosting is stopped. Resume hosting first.",
        false,
    );
    pub const DAEMON_STOPPING: DaemonError = DaemonError::new(
        "daemon_stopping",
        "The owner daemon is shutting down.",
        true,
    );
    pub const STATE_UNAVAILABLE: DaemonError = DaemonError::new(
        "state_unavailable",
        "The daemon could not record this change. Nothing was applied.",
        true,
    );
    pub const PROVIDER_REFRESH_UNAVAILABLE: DaemonError = DaemonError::new(
        "provider_refresh_unavailable",
        "Provider plan collection is not available in this daemon yet.",
        false,
    );
}

pub fn valid_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_REQUEST_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn valid_owner_id(id: &str) -> bool {
    id.len() <= MAX_OWNER_ID
        && id.starts_with('@')
        && id.contains(':')
        && !id.chars().any(char::is_control)
}

impl Request {
    pub fn new(request_id: impl Into<String>, owner_id: Option<String>, command: Command) -> Self {
        Self {
            version: API_VERSION,
            request_id: request_id.into(),
            owner_id,
            expected_revision: None,
            command,
        }
    }
    /// Shape validation independent of daemon state.
    pub fn validate(&self) -> Result<(), DaemonError> {
        if self.version != API_VERSION {
            return Err(errors::UNSUPPORTED_VERSION);
        }
        if !valid_request_id(&self.request_id)
            || self.owner_id.as_deref().is_some_and(|o| !valid_owner_id(o))
        {
            return Err(errors::INVALID_REQUEST);
        }
        if let Command::Snapshot {
            period: Some(Period::Range { start_ms, end_ms }), ..
        } = &self.command
            && start_ms >= end_ms
        {
            return Err(errors::INVALID_REQUEST);
        }
        Ok(())
    }
    /// Stable identity of the command a request ID is bound to. Retrying with
    /// the same ID must carry exactly this owner, revision and command.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let canonical = serde_json::to_vec(&(
            self.owner_id.as_deref(),
            self.expected_revision,
            &self.command,
        ))
        .unwrap_or_default();
        format!("{:x}", Sha256::digest(canonical))
    }
}

/// Parse a request payload, returning the best-effort request ID for errors.
pub fn decode_request(payload: &[u8]) -> Result<Request, (String, DaemonError)> {
    let value: Value =
        serde_json::from_slice(payload).map_err(|_| (String::new(), errors::INVALID_REQUEST))?;
    let id = value["requestId"]
        .as_str()
        .filter(|id| valid_request_id(id))
        .unwrap_or_default()
        .to_owned();
    if value["version"].as_u64().is_some_and(|v| v != u64::from(API_VERSION)) {
        return Err((id, errors::UNSUPPORTED_VERSION));
    }
    let request: Request =
        serde_json::from_value(value).map_err(|_| (id.clone(), errors::INVALID_REQUEST))?;
    request.validate().map_err(|e| (id, e))?;
    Ok(request)
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("connection closed")]
    Closed,
    #[error("frame too large")]
    TooLarge,
    #[error("io")]
    Io(#[from] std::io::Error),
}

/// Read one frame. A clean EOF before the header is `Closed`; an oversized
/// header is rejected before allocating its payload.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, FrameError> { read_bounded(reader,MAX_FRAME).await }
pub async fn read_reply<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, FrameError> { read_bounded(reader,MAX_REPLY_FRAME).await }
async fn read_bounded<R: AsyncRead + Unpin>(reader: &mut R,limit:usize) -> Result<Vec<u8>, FrameError> {
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(e.into()),
    }
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > limit {
        return Err(FrameError::TooLarge);
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
) -> Result<(), FrameError> {
    write_bounded(writer,payload,MAX_FRAME).await
}
pub async fn write_reply<W:AsyncWrite+Unpin>(writer:&mut W,payload:&[u8])->Result<(),FrameError>{write_bounded(writer,payload,MAX_REPLY_FRAME).await}
async fn write_bounded<W:AsyncWrite+Unpin>(writer:&mut W,payload:&[u8],limit:usize)->Result<(),FrameError>{
    if payload.is_empty() || payload.len() > limit {
        return Err(FrameError::TooLarge);
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_names_follow_the_shared_contract() {
        let request = Request {
            expected_revision: Some(4),
            ..Request::new(
                "r-1",
                Some("@o:s".into()),
                Command::Snapshot {
                    period: Some(Period::Range {
                        start_ms: 1,
                        end_ms: 2,
                    }),
                    usage_filter:None,
                },
            )
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            value,
            json!({"version":1,"requestId":"r-1","ownerId":"@o:s","expectedRevision":4,
                "command":{"type":"snapshot","period":{"kind":"range","startMs":1,"endMs":2}}})
        );
        for (command, name) in [
            (Command::PauseHosting, "pauseHosting"),
            (Command::ResumeHosting, "resumeHosting"),
            (Command::StopHosting, "stopHosting"),
            (Command::RefreshProviders, "refreshProviders"),
            (Command::Hello, "hello"),
        ] {
            assert_eq!(serde_json::to_value(&command).unwrap()["type"], name);
            assert_eq!(command.name(), name);
        }
        let reply = Reply::err("r-1", 9, errors::REVISION_CONFLICT);
        assert_eq!(
            serde_json::to_value(&reply).unwrap(),
            json!({"version":1,"requestId":"r-1","revision":9,"error":{"code":"revision_conflict",
                "message":errors::REVISION_CONFLICT.message,"retryable":false}})
        );
        let outcome = CommandOutcome {
            state: OutcomeState::Accepted,
            hosting: HostingState::Draining,
            revision: 3,
            detail: None,
        };
        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({"state":"accepted","hosting":"draining","revision":3})
        );
    }

    #[test]
    fn decoding_rejects_bad_shapes_and_reports_version_mismatch() {
        let ok = br#"{"version":1,"requestId":"a_b-1","command":{"type":"hello"}}"#;
        assert_eq!(decode_request(ok).unwrap().command, Command::Hello);
        let (id, e) =
            decode_request(br#"{"version":2,"requestId":"x","command":{"type":"hello"}}"#)
                .unwrap_err();
        assert_eq!((id.as_str(), e), ("x", errors::UNSUPPORTED_VERSION));
        for bad in [
            &br#"not json"#[..],
            br#"{"version":1,"requestId":"","command":{"type":"hello"}}"#,
            br#"{"version":1,"requestId":"has space","command":{"type":"hello"}}"#,
            br#"{"version":1,"requestId":"a","command":{"type":"launchMissiles"}}"#,
            br#"{"version":1,"requestId":"a","command":{"type":"hello"},"extra":1}"#,
            br#"{"version":1,"requestId":"a","ownerId":"nobody","command":{"type":"hello"}}"#,
            br#"{"version":1,"requestId":"a","command":{"type":"snapshot","period":{"kind":"range","startMs":5,"endMs":5}}}"#,
        ] {
            assert_eq!(decode_request(bad).unwrap_err().1, errors::INVALID_REQUEST);
        }
        let long = "x".repeat(MAX_REQUEST_ID + 1);
        let raw = format!(r#"{{"version":1,"requestId":"{long}","command":{{"type":"hello"}}}}"#);
        assert_eq!(decode_request(raw.as_bytes()).unwrap_err().1, errors::INVALID_REQUEST);
    }

    #[test]
    fn fingerprint_binds_owner_revision_and_command() {
        let base = Request::new("same", Some("@o:s".into()), Command::PauseHosting);
        let mut other = base.clone();
        assert_eq!(base.fingerprint(), other.fingerprint());
        other.command = Command::StopHosting;
        assert_ne!(base.fingerprint(), other.fingerprint());
        let mut other = base.clone();
        other.expected_revision = Some(1);
        assert_ne!(base.fingerprint(), other.fingerprint());
        let mut other = base.clone();
        other.owner_id = Some("@p:s".into());
        assert_ne!(base.fingerprint(), other.fingerprint());
    }

    #[tokio::test]
    async fn framing_round_trips_and_bounds_before_allocation() {
        let (mut a, mut b) = tokio::io::duplex(MAX_FRAME * 2);
        write_frame(&mut a, b"{}").await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), b"{}");
        a.write_all(&((MAX_FRAME as u32) + 1).to_be_bytes())
            .await
            .unwrap();
        assert!(matches!(read_frame(&mut b).await, Err(FrameError::TooLarge)));
        assert!(matches!(
            write_frame(&mut a, &vec![b' '; MAX_FRAME + 1]).await,
            Err(FrameError::TooLarge)
        ));
        a.write_all(&0u32.to_be_bytes()).await.unwrap();
        assert!(matches!(read_frame(&mut b).await, Err(FrameError::TooLarge)));
        drop(a);
        assert!(matches!(read_frame(&mut b).await, Err(FrameError::Closed)));
    }

    #[test]
    fn typed_reply_accessors_surface_structured_errors() {
        let reply = Reply::err("r", 1, errors::BUSY);
        let error = reply.outcome().unwrap_err();
        assert_eq!(error.code, "busy");
        assert!(error.retryable);
        let reply = Reply::ok("r", 1, json!({"unexpected":true}));
        assert_eq!(reply.outcome().unwrap_err().code, "invalid_reply");
    }
}
