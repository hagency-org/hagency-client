//! Token-free native commands routed through the existing authenticated service.
mod agent_crypto;
mod configuration;
mod ledger;
mod remote_reviews;
pub use agent_crypto::{
    AgentCryptoAction, AgentCryptoAuthority, AgentCryptoContext, AgentCryptoIdentity,
    AgentCryptoPreparation, AgentCryptoTransport, DecryptedAgentCiphertext, DecryptedAgentEvent,
};
pub use ledger::LedgerAction;
pub use remote_reviews::RemoteApprovalAction;
mod history;
mod speech;
mod usage;
use super::*;
use crate::owner_host::OwnerHost;
pub use hagency_agent_local::recovery::{
    Disposition as RecoveryDisposition, Input as RecoveryInput,
};
pub use hagency_agent_local::usage::{Group as UsageGroup, Query as UsageQuery};
pub use history::HistoryAction;
use salvo::test::{ResponseExt, TestClient};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex as StdMutex, Weak},
};
use tokio::sync::Mutex;
pub use usage::UsageAction;

/// The SDK remains the sole owner of OAuth refresh and revocation.
pub struct MatrixAccessToken {
    pub access_token: String,
    pub client_id: String,
}
impl std::fmt::Debug for MatrixAccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatrixAccessToken")
            .field("access_token", &"[redacted]")
            .field("client_id", &self.client_id)
            .finish()
    }
}
pub(in crate::console) fn sdk_source_failure(error: NativeError) -> &'static str {
    match error.code.as_str() {
        "matrix_account_changed" => "matrix_account_changed",
        "matrix_authorization_unavailable" => "matrix_authorization_unavailable",
        "matrix_oauth_sign_in_required" => "matrix_oauth_sign_in_required",
        "unsupported_hagency_issuer" => "unsupported_hagency_issuer",
        "matrix_account_mismatch" => "matrix_account_mismatch",
        "unsupported_hagency_server" => "unsupported_hagency_server",
        "unsupported_hagency_protocol" => "unsupported_hagency_protocol",
        "unsupported_hagency_capabilities" => "unsupported_hagency_capabilities",
        "invalid_hagency_metadata" => "invalid_hagency_metadata",
        "hagency_server_unavailable" => "hagency_server_unavailable",
        "hagency_server_rejected" => "hagency_server_rejected",
        _ => "matrix_authorization_required",
    }
}
/// One explicit owner message, armed once by the local recovery ledger.
#[derive(Clone)]
pub struct ContinuationMessage {
    pub owner: String,
    pub room: String,
    pub thread: String,
    pub agent_mxid: String,
    pub transaction_id: String,
    pub body: String,
    pub require_encrypted: bool,
}
pub use hagency_agent_local::attachments::Attachment as AgentAttachment;
pub const MAX_AGENT_FILE_BYTES: usize = hagency_agent_local::received_files::MAX_BYTES;
pub trait MatrixTokenSource: Send + Sync {
    /// Observe or explicitly sign the exact prepared Agent identity using the
    /// current owner's actual Matrix SDK. The boolean is presentation only.
    fn agent_crypto_owner_trust(
        &self,
        _identity: AgentCryptoIdentity,
        _authority: AgentCryptoAuthority,
        verify: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, NativeError>> + Send + '_>>
    {
        Box::pin(async move {
            if verify {
                Err(failure(503, "agent_crypto_owner_verification_unavailable"))
            } else {
                Ok(false)
            }
        })
    }
    fn supports_agent_crypto(&self) -> bool {
        false
    }
    fn prepare_agent_crypto(&self, _context: AgentCryptoContext) -> AgentCryptoPreparation<'_> {
        Box::pin(async { Err(failure(503, "agent_crypto_transport_unavailable")) })
    }
    fn list_remote_requests<'a>(
        &'a self,
        _room: &'a str,
        _cursor: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        hagency_agent_local::remote_approval::review::RequestPage,
                        NativeError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async { Err(failure(503, "remote_approval_transport_unavailable")) })
    }
    fn inspect_remote_request<'a>(
        &'a self,
        _room: &'a str,
        _event: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        hagency_agent_local::remote_approval::review::ObservedRequest,
                        NativeError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async { Err(failure(503, "remote_approval_transport_unavailable")) })
    }
    fn publish_remote_verdict(
        &self,
        _permit: hagency_agent_local::remote_approval::review::VerdictSendPermit,
        _live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + '_>>
    {
        Box::pin(async { Err(failure(503, "remote_approval_transport_unavailable")) })
    }
    fn supports_remote_approvals(&self) -> bool {
        false
    }
    fn prepare_remote_approval<'a>(
        &'a self,
        _room: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + 'a>>
    {
        Box::pin(async { Err(failure(503, "remote_approval_transport_unavailable")) })
    }
    fn publish_remote_approval(
        &self,
        _permit: hagency_agent_local::remote_approval::SendPermit,
        _live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + '_>>
    {
        Box::pin(async { Err(failure(503, "remote_approval_transport_unavailable")) })
    }
    fn observe_remote_approval<'a>(
        &'a self,
        _publication: &'a hagency_agent_local::remote_approval::Publication,
        _request_event: &'a str,
        _verdict_event: &'a str,
        _live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        hagency_agent_local::remote_approval::ObservedVerdict,
                        NativeError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async { Err(failure(503, "remote_approval_transport_unavailable")) })
    }
    fn find_remote_approval<'a>(
        &'a self,
        _publication: &'a hagency_agent_local::remote_approval::Publication,
        _request_event: &'a str,
        _live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        hagency_agent_local::remote_approval::ObservedVerdict,
                        NativeError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async { Err(failure(503, "remote_approval_transport_unavailable")) })
    }
    fn supports_agent_file_receive(&self) -> bool {
        false
    }
    fn fetch_agent_attachment(
        &self,
        _attachment: AgentAttachment,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u8>, NativeError>> + Send + '_>,
    > {
        Box::pin(async { Err(failure(503, "agent_file_transport_unavailable")) })
    }

    fn send_continuation(
        &self,
        _message: ContinuationMessage,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + '_>>
    {
        Box::pin(async { Err(failure(503, "continuation_transport_unavailable")) })
    }

    /// Return the current SDK token, checking its account epoch before and after
    /// awaiting. Implementations must not call this native service recursively.
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    >;
    /// Ask the SDK refresh owner for a fresh grant before a short authorization
    /// window expires. No refresh credential crosses this interface.
    fn refresh_access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        self.access_token()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentAction {
    Pause,
    Resume,
    Retire,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BindingAction {
    SetThreadAutoReply(bool),
    Status,
    Pause,
    Resume,
    Leave,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProviderAction {
    UseLocal,
    Disconnect,
    Models,
    Status,
    Login,
    Cancel,
    Logout,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LocalAction {
    AgentModel,
    SaveAgentModel,
    AgentPolicy,
    SaveAgentPolicy,
    ResetAgentPolicy,
    Policy,
    Model,
    SavePolicy,
    ResetPolicy,
    SaveModel,
    Runtime,
    RuntimeStart,
    RuntimeStop,
    StartAgentService,
    StopAgentService,
    AgentServiceStatus,
    Approvals,
    Decision { approval_id: String },
    PublishRemoteApproval { approval_id: String },
    ObserveRemoteApproval { approval_id: String },
    RemoteApprovalStatus { approval_id: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectCreationPolicy {
    pub default_allow: bool,
    pub allow: std::collections::BTreeSet<String>,
    pub deny: std::collections::BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoomCreationPolicy {
    InheritProject {
        deny: std::collections::BTreeSet<String>,
    },
    AllowList {
        allow: std::collections::BTreeSet<String>,
        deny: std::collections::BTreeSet<String>,
    },
    Disabled,
}
impl<'de> Deserialize<'de> for RoomCreationPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
        enum Strict {
            InheritProject {
                deny: std::collections::BTreeSet<String>,
            },
            AllowList {
                allow: std::collections::BTreeSet<String>,
                deny: std::collections::BTreeSet<String>,
            },
            Disabled {},
        }
        Ok(match Strict::deserialize(d)? {
            Strict::InheritProject { deny } => Self::InheritProject { deny },
            Strict::AllowList { allow, deny } => Self::AllowList { allow, deny },
            Strict::Disabled {} => Self::Disabled,
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    AgentCrypto {
        action: AgentCryptoAction,
    },
    RemoteApprovals {
        action: RemoteApprovalAction,
    },
    AgentHistory {
        action: HistoryAction,
    },
    AgentUsage {
        action: UsageAction,
    },
    Ledger {
        action: LedgerAction,
    },
    ProjectBoard {
        project_id: String,
        action: hagency_core::project_board::BoardAction,
    },
    AgentConfigurationStatus {
        agent_id: String,
    },
    ExecutionEnvironment {
        agent_id: String,
    },
    AgentConfiguration {
        agent_id: String,
        expected_generation: i64,
        idempotency_key: String,
        command: Box<Command>,
    },
    AgentCommands,
    CheckAgentCommand {
        id: String,
    },
    DismissAgentCommand {
        id: String,
    },
    ResumeAgentCommand {
        id: String,
    },
    ScopeServiceState {
        project_id: String,
        room_id: Option<String>,
    },
    SetProjectCreationPolicy {
        project_id: String,
        expected_revision: i64,
        policy: ProjectCreationPolicy,
    },
    SetRoomCreationPolicy {
        project_id: String,
        room_id: String,
        expected_revision: i64,
        policy: RoomCreationPolicy,
    },
    SetScopeServicePause {
        project_id: String,
        room_id: Option<String>,
        paused: bool,
    },
    Devices,
    AgentDetails {
        agent_id: String,
    },
    AssignAgentToCurrentDevice {
        agent_id: String,
        expected_generation: i64,
    },
    AgentOwnerDirectStatus {
        agent_id: String,
    },
    EnsureAgentOwnerDirect {
        agent_id: String,
    },
    AgentOwnerDirect {
        agent_id: String,
        room_id: String,
    },

    LoginStatus,
    BeginLogin,
    Logout,
    Projects,
    ProjectRooms {
        project_id: String,
    },
    SpaceCandidates {
        cursor: Option<String>,
    },
    RoomRoster {
        project_id: String,
        room_id: String,
    },
    AdoptProject {
        space_id: String,
    },
    AdoptRoom {
        project_id: String,
        room_id: String,
    },
    CreateMatrix {
        input: Value,
    },
    Creations,
    Creation {
        id: String,
    },
    ResumeCreation {
        id: String,
        room_id: Option<String>,
    },
    Agents,
    DiscoverAgents,
    SetAgentVisibility {
        agent_id: String,
        visibility: String,
    },
    Bindings {
        agent_id: String,
    },
    CreateAgent {
        input: Value,
    },
    BindAgent {
        agent_id: String,
        input: Value,
    },
    Agent {
        agent_id: String,
        action: AgentAction,
    },
    Binding {
        agent_id: String,
        binding_id: String,
        action: BindingAction,
    },
    Local {
        agent_id: String,
        action: LocalAction,
        input: Option<Value>,
        binding_id: Option<String>,
        requester: Option<String>,
    },
    PrepareAgentWorkspace {
        agent_id: String,
    },
    Provider {
        action: ProviderAction,
    },
    RuntimeProvider {
        runtime: String,
        action: ProviderAction,
    },
}
impl Command {
    /// Pure inspection and exact existing approvals only; never launch, recover,
    /// reconfigure, log in or create work while waiting for an update.
    pub fn continues_running_task(&self) -> bool {
        matches!(
            self,
            Self::AgentCrypto {
                action: AgentCryptoAction::Status { .. } | AgentCryptoAction::Stop { .. }
            } | Self::Projects
                | Self::RemoteApprovals { .. }
                | Self::ProjectRooms { .. }
                | Self::Agents
                | Self::Bindings { .. }
                | Self::AgentDetails { .. }
                | Self::Devices
                | Self::AgentOwnerDirectStatus { .. }
                | Self::Local {
                    action: LocalAction::Approvals
                        | LocalAction::Runtime
                        | LocalAction::AgentServiceStatus
                        | LocalAction::AgentModel
                        | LocalAction::AgentPolicy
                        | LocalAction::Policy
                        | LocalAction::Model
                        | LocalAction::RemoteApprovalStatus { .. },
                    input: None,
                    ..
                }
                | Self::Local {
                    action: LocalAction::Decision { .. }
                        | LocalAction::ObserveRemoteApproval { .. },
                    ..
                }
        )
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Identity {
    pub origin: String,
    pub issuer: String,
    pub subject: String,
    pub owner: String,
}
#[derive(Debug, thiserror::Error)]
#[error("{code}")]
pub struct NativeError {
    pub status: u16,
    pub code: String,
}
fn failure(status: u16, code: &str) -> NativeError {
    NativeError {
        status,
        code: code.into(),
    }
}
fn key(id: &str) -> Result<(), NativeError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(failure(400, "invalid_arguments"));
    }
    Ok(())
}
fn segment(id: &str) -> Result<String, NativeError> {
    if id.len() > 255
        || !id.starts_with('!')
        || !super::matrix_identifiers::room_id(id)
        || id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || ['/', '\\', '?', '#', '%'].contains(&c))
    {
        return Err(failure(400, "invalid_arguments"));
    }
    Ok(percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC).to_string())
}
fn project_scope(project: &str, room: Option<&str>) -> Result<String, NativeError> {
    key(project)?;
    match room {
        Some(room) => Ok(format!("{project}/rooms/{}", segment(room)?)),
        None => Ok(project.into()),
    }
}
fn request(command: Command) -> Result<(&'static str, String, Option<Value>), NativeError> {
    let root = "/console/api";
    let mut body = None;
    let (method, path) = match command {
        Command::AgentConfigurationStatus { .. }
        | Command::ExecutionEnvironment { .. }
        | Command::AgentConfiguration { .. }
        | Command::Ledger { .. }
        | Command::RemoteApprovals { .. }
        | Command::AgentCrypto { .. }
        | Command::AgentUsage { .. }
        | Command::AgentHistory { .. } => {
            return Err(failure(400, "configuration_operation_unsupported"));
        }
        Command::LoginStatus => ("GET", "/console/server-login".into()),
        Command::Logout => ("POST", "/console/server-login/logout".into()),
        Command::BeginLogin => return Err(failure(400, "login_requires_native_context")),
        Command::ProjectBoard { project_id, action } => {
            key(&project_id)?;
            body = Some(
                serde_json::to_value(action).map_err(|_| failure(400, "invalid_board_payload"))?,
            );
            (
                "POST",
                format!("{root}/owner-projects/{project_id}/board-command"),
            )
        }
        Command::Projects => ("GET", format!("{root}/owner-projects")),
        Command::AgentCommands => ("GET", format!("{root}/owned-agents/commands")),
        Command::CheckAgentCommand { id } => {
            key(&id)?;
            ("GET", format!("{root}/owned-agents/commands/{id}"))
        }
        Command::DismissAgentCommand { id } => {
            key(&id)?;
            ("POST", format!("{root}/owned-agents/commands/{id}/dismiss"))
        }
        Command::ResumeAgentCommand { id } => {
            key(&id)?;
            ("POST", format!("{root}/owned-agents/commands/{id}/resume"))
        }
        Command::ScopeServiceState {
            project_id,
            room_id,
        } => {
            let scope = project_scope(&project_id, room_id.as_deref())?;
            (
                "GET",
                format!("{root}/owner-projects/{scope}/service-state"),
            )
        }
        Command::SetProjectCreationPolicy {
            project_id,
            expected_revision,
            policy,
        } => {
            key(&project_id)?;
            if expected_revision < 0 {
                return Err(failure(400, "invalid_arguments"));
            }
            body = Some(json!({"expectedRevision":expected_revision,"policy":policy}));
            (
                "PUT",
                format!("{root}/owner-projects/{project_id}/creation-policy"),
            )
        }
        Command::SetRoomCreationPolicy {
            project_id,
            room_id,
            expected_revision,
            policy,
        } => {
            let scope = project_scope(&project_id, Some(&room_id))?;
            if expected_revision < 0 {
                return Err(failure(400, "invalid_arguments"));
            }
            body = Some(json!({"expectedRevision":expected_revision,"policy":policy}));
            (
                "PUT",
                format!("{root}/owner-projects/{scope}/creation-policy"),
            )
        }
        Command::SetScopeServicePause {
            project_id,
            room_id,
            paused,
        } => {
            let scope = project_scope(&project_id, room_id.as_deref())?;
            let suffix = if paused {
                "pause-service"
            } else {
                "clear-service-pause"
            };
            ("POST", format!("{root}/owner-projects/{scope}/{suffix}"))
        }
        Command::Devices => ("GET", format!("{root}/owned-agents/devices")),
        Command::AgentDetails { agent_id } => {
            key(&agent_id)?;
            ("GET", format!("{root}/owned-agents/{agent_id}"))
        }
        Command::AssignAgentToCurrentDevice {
            agent_id,
            expected_generation,
        } => {
            key(&agent_id)?;
            if expected_generation < 0 {
                return Err(NativeError {
                    status: 400,
                    code: "invalid_arguments".into(),
                });
            }
            body = Some(json!({"expectedGeneration":expected_generation}));
            (
                "PUT",
                format!("{root}/owned-agents/{agent_id}/execution-device"),
            )
        }
        Command::AgentOwnerDirectStatus { agent_id } => {
            key(&agent_id)?;
            (
                "GET",
                format!("{root}/owned-agents/{agent_id}/owner-direct"),
            )
        }
        Command::EnsureAgentOwnerDirect { agent_id } => {
            key(&agent_id)?;
            (
                "POST",
                format!("{root}/owned-agents/{agent_id}/owner-direct/ensure"),
            )
        }
        Command::AgentOwnerDirect { agent_id, room_id } => {
            key(&agent_id)?;
            segment(&room_id)?;
            body = Some(json!({"roomId":room_id}));
            (
                "POST",
                format!("{root}/owned-agents/{agent_id}/owner-direct"),
            )
        }
        Command::ProjectRooms { project_id } => {
            key(&project_id)?;
            ("GET", format!("{root}/owner-projects/{project_id}/rooms"))
        }
        Command::RoomRoster {
            project_id,
            room_id,
        } => {
            key(&project_id)?;
            (
                "GET",
                format!(
                    "{root}/owner-projects/{project_id}/rooms/{}/agents",
                    segment(&room_id)?
                ),
            )
        }
        Command::SpaceCandidates { cursor } => {
            let mut p = format!("{root}/owner-projects/space-candidates");
            if let Some(c) = cursor {
                p.push_str(&format!("?cursor={}", segment(&c)?));
            }
            ("GET", p)
        }
        Command::AdoptProject { space_id } => {
            segment(&space_id)?;
            body = Some(json!({"spaceId":space_id}));
            ("POST", format!("{root}/owner-projects"))
        }
        Command::AdoptRoom {
            project_id,
            room_id,
        } => {
            key(&project_id)?;
            segment(&room_id)?;
            body = Some(json!({"roomId":room_id}));
            ("POST", format!("{root}/owner-projects/{project_id}/rooms"))
        }
        Command::CreateMatrix { input } => {
            body = Some(input);
            ("POST", format!("{root}/matrix-creations"))
        }
        Command::Creations => ("GET", format!("{root}/matrix-creations")),
        Command::Creation { id } => {
            key(&id)?;
            ("GET", format!("{root}/matrix-creations/{id}"))
        }
        Command::ResumeCreation { id, room_id } => {
            key(&id)?;
            if let Some(r) = &room_id {
                segment(r)?;
            }
            body = Some(json!({"roomId":room_id}));
            ("POST", format!("{root}/matrix-creations/{id}/resume"))
        }
        Command::Agents => ("GET", format!("{root}/owned-agents")),
        Command::DiscoverAgents => ("GET", format!("{root}/owned-agents/discover")),
        Command::SetAgentVisibility {
            agent_id,
            visibility,
        } => {
            key(&agent_id)?;
            if !matches!(visibility.as_str(), "public" | "private") {
                return Err(failure(400, "invalid_arguments"));
            }
            body = Some(json!({"visibility":visibility}));
            ("PUT", format!("{root}/owned-agents/{agent_id}/visibility"))
        }
        Command::CreateAgent { input } => {
            body = Some(input);
            ("POST", format!("{root}/owned-agents"))
        }
        Command::Bindings { agent_id } => {
            key(&agent_id)?;
            ("GET", format!("{root}/owned-agents/{agent_id}/bindings"))
        }
        Command::BindAgent { agent_id, input } => {
            key(&agent_id)?;
            body = Some(input);
            ("POST", format!("{root}/owned-agents/{agent_id}/bindings"))
        }
        Command::Agent { agent_id, action } => {
            key(&agent_id)?;
            let (m, s) = match action {
                AgentAction::Pause => ("POST", "/pause"),
                AgentAction::Resume => ("POST", "/resume"),
                AgentAction::Retire => ("DELETE", ""),
            };
            (m, format!("{root}/owned-agents/{agent_id}{s}"))
        }
        Command::Binding {
            agent_id,
            binding_id,
            action,
        } => {
            key(&agent_id)?;
            key(&binding_id)?;
            let (m, s) = match action {
                BindingAction::SetThreadAutoReply(enabled) => {
                    body = Some(serde_json::json!({"threadAutoReply":enabled}));
                    ("PUT", "/reply-policy")
                }
                BindingAction::Status => ("GET", ""),
                BindingAction::Pause => ("POST", "/pause"),
                BindingAction::Resume => ("POST", "/resume"),
                BindingAction::Leave => ("DELETE", ""),
            };
            (
                m,
                format!("{root}/owned-agents/{agent_id}/bindings/{binding_id}{s}"),
            )
        }
        Command::PrepareAgentWorkspace { agent_id } => {
            key(&agent_id)?;
            (
                "POST",
                format!("{root}/owner-provider/workspaces/{agent_id}"),
            )
        }
        Command::RuntimeProvider { runtime, action } => {
            if !matches!(runtime.as_str(), "claude" | "octos") {
                return Err(failure(400, "invalid_arguments"));
            }
            let (method, suffix) = match action {
                ProviderAction::Status => ("GET", ""),
                ProviderAction::Models => ("GET", "/models"),
                ProviderAction::UseLocal => ("POST", "/use-local"),
                ProviderAction::Disconnect => ("POST", "/disconnect"),
                _ => return Err(failure(400, "local_provider_login_managed_locally")),
            };
            (
                method,
                format!("{root}/owner-provider/runtimes/{runtime}{suffix}"),
            )
        }
        Command::Provider { action } => {
            let (m, s) = match action {
                ProviderAction::UseLocal => ("POST", "/use-local"),
                ProviderAction::Disconnect => ("POST", "/disconnect"),
                ProviderAction::Models => ("GET", "/models"),
                ProviderAction::Status => ("GET", ""),
                ProviderAction::Login => ("POST", "/login"),
                ProviderAction::Cancel => ("POST", "/cancel"),
                ProviderAction::Logout => ("POST", "/logout"),
            };
            (m, format!("{root}/owner-provider{s}"))
        }
        Command::Local {
            agent_id,
            action,
            input,
            binding_id,
            requester,
        } => {
            key(&agent_id)?;
            if matches!(
                action,
                LocalAction::AgentModel
                    | LocalAction::SaveAgentModel
                    | LocalAction::AgentPolicy
                    | LocalAction::SaveAgentPolicy
                    | LocalAction::ResetAgentPolicy
                    | LocalAction::StartAgentService
                    | LocalAction::StopAgentService
                    | LocalAction::AgentServiceStatus
            ) && (binding_id.is_some() || requester.is_some())
            {
                return Err(failure(400, "invalid_arguments"));
            }

            body = input;
            let (m, s) = match action {
                LocalAction::AgentModel => ("GET", "agent-model-profile".into()),
                LocalAction::SaveAgentModel => ("PUT", "agent-model-profile".into()),
                LocalAction::AgentPolicy => ("GET", "agent-policy".into()),
                LocalAction::SaveAgentPolicy => ("PUT", "agent-policy".into()),
                LocalAction::ResetAgentPolicy => ("DELETE", "agent-policy".into()),
                LocalAction::Policy => ("GET", "local-policy".into()),
                LocalAction::Model => ("GET", "model-profile".into()),
                LocalAction::SavePolicy => ("PUT", "local-policy".into()),
                LocalAction::ResetPolicy => ("DELETE", "local-policy".into()),
                LocalAction::SaveModel => ("PUT", "model-profile".into()),
                LocalAction::Runtime => ("GET", "runtime".into()),
                LocalAction::RuntimeStart => ("POST", "runtime/start".into()),
                LocalAction::RuntimeStop => ("POST", "runtime/stop".into()),
                LocalAction::StartAgentService => ("POST", "runtime/service/start".into()),
                LocalAction::StopAgentService => ("POST", "runtime/service/stop".into()),
                LocalAction::AgentServiceStatus => ("GET", "runtime/service".into()),
                LocalAction::Approvals => ("GET", "runtime/approvals".into()),
                LocalAction::PublishRemoteApproval { approval_id } => {
                    key(&approval_id)?;
                    ("POST", format!("runtime/approvals/{approval_id}/remote"))
                }
                LocalAction::ObserveRemoteApproval { approval_id } => {
                    key(&approval_id)?;
                    (
                        "POST",
                        format!("runtime/approvals/{approval_id}/remote-verdict"),
                    )
                }
                LocalAction::RemoteApprovalStatus { approval_id } => {
                    key(&approval_id)?;
                    ("GET", format!("runtime/approvals/{approval_id}/remote"))
                }
                LocalAction::Decision { approval_id } => {
                    key(&approval_id)?;
                    ("POST", format!("runtime/approvals/{approval_id}/decision"))
                }
            };
            let mut p = format!("{root}/owned-agents/{agent_id}/{s}");
            if m == "GET" && ["local-policy", "model-profile"].contains(&s.as_str()) {
                let b = binding_id.ok_or(failure(400, "invalid_arguments"))?;
                key(&b)?;
                let r = requester.ok_or(failure(400, "invalid_arguments"))?;
                let mut url = reqwest::Url::parse("http://local.invalid/").unwrap();
                url.query_pairs_mut()
                    .append_pair("bindingId", &b)
                    .append_pair("requester", &r);
                p.push('?');
                p.push_str(url.query().unwrap());
            }
            (m, p)
        }
    };
    Ok((method, path, body))
}
struct Inner {
    host: OwnerHost,
    service: salvo::Service,
    address: SocketAddr,
    expected_origin: String,
    expected_owner: String,
    cookie: Mutex<String>,
    nonce: Mutex<Option<String>>,
    calls: Mutex<()>,
    matrix_sync: Mutex<()>,
    configuration_stopped: std::sync::atomic::AtomicBool,
    configuration_tasks: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
    review_tasks: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
    crypto: agent_crypto::Registry,
    callback_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    matrix_source: StdMutex<Option<Arc<dyn MatrixTokenSource>>>,
    shutdown: std::sync::OnceLock<crate::shutdown::Task>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.host.console.retire();
        self.crypto.retire();
        if let Ok(tasks) = self.review_tasks.get_mut() {
            for task in tasks.drain(..) {
                task.abort();
            }
        }
        if let Ok(tasks) = self.configuration_tasks.get_mut() {
            for task in tasks.drain(..) {
                task.abort();
            }
        }
        if let Ok(task) = self.callback_task.get_mut()
            && let Some(task) = task.take()
        {
            task.abort();
        }
    }
}
/// A native owner service. Neither credentials nor its private local grants are
/// returned in command responses. Only the OAuth callback listens on loopback.
#[derive(Clone)]
pub struct NativeOwner(Arc<Inner>);
impl NativeOwner {
    pub async fn open(state: &Path, origin: &str, owner: &str) -> Result<Self, NativeError> {
        Self::open_inner(state, origin, owner, true).await
    }
    async fn open_inner(
        state: &Path,
        origin: &str,
        owner: &str,
        start_worker: bool,
    ) -> Result<Self, NativeError> {
        let origin = server_login::origin(origin)
            .map_err(|e| failure(400, e))?
            .to_string();
        if !owner.starts_with('@') || !owner.contains(':') || owner.len() > 255 {
            return Err(failure(400, "invalid_owner_identity"));
        }
        let acceptor = salvo::conn::TcpListener::new("127.0.0.1:0")
            .try_bind()
            .await
            .map_err(|_| failure(503, "native_callback_unavailable"))?;
        let address = acceptor
            .local_addr()
            .map_err(|_| failure(503, "native_callback_unavailable"))?;
        let host = OwnerHost::open_native(state, address)
            .map_err(|_| failure(409, "native_owner_state_unavailable"))?;
        *host
            .console
            .0
            .server_login
            .native_expected
            .lock()
            .map_err(|_| failure(503, "native_state_unavailable"))? =
            Some((origin.clone(), owner.into()));
        let ticket = host
            .owner_access_ticket()
            .map_err(|_| failure(503, "native_state_unavailable"))?;
        let service = salvo::Service::new(host.clone().router());
        let mut grant = TestClient::post(format!("http://{address}/console/session"))
            .add_header("host", address.to_string(), true)
            .add_header("sec-fetch-site", "same-origin", true)
            .add_header("origin", format!("http://{address}"), true)
            .json(&json!({"ticket":ticket}))
            .send(&service)
            .await;
        if !grant.status_code.unwrap_or_default().is_success() {
            return Err(failure(503, "native_access_unavailable"));
        }
        let cookie = grant
            .headers()
            .get_all("set-cookie")
            .iter()
            .find_map(|v| {
                v.to_str()
                    .ok()
                    .filter(|s| s.starts_with("hagency_console="))
                    .map(|s| s.split(';').next().unwrap().to_owned())
            })
            .ok_or(failure(503, "native_access_unavailable"))?;
        let native = Self(Arc::new(Inner {
            host,
            service,
            address,
            expected_origin: origin,
            expected_owner: owner.into(),
            cookie: Mutex::new(cookie),
            nonce: Mutex::new(None),
            calls: Mutex::new(()),
            matrix_sync: Mutex::new(()),
            configuration_stopped: std::sync::atomic::AtomicBool::new(false),
            configuration_tasks: StdMutex::new(Vec::new()),
            review_tasks: StdMutex::new(Vec::new()),
            crypto: Default::default(),
            callback_task: StdMutex::new(None),
            matrix_source: StdMutex::new(None),
            shutdown: std::sync::OnceLock::new(),
        }));
        let router = Router::new()
            .hoop(Callback(Arc::downgrade(&native.0)))
            .push(Router::with_path("console/server-login/callback").get(callback));
        let task = tokio::spawn(async move {
            salvo::Server::new(acceptor).serve(router).await;
        });
        *native.0.callback_task.lock().unwrap() = Some(task);
        // Consume any bootstrap response without exposing a grant to the caller.
        let _ = grant.take_json::<Value>().await;
        if start_worker {
            configuration::start(&native);
        }
        Ok(native)
    }
    pub async fn open_with_matrix(
        state: &Path,
        origin: &str,
        owner: &str,
        source: Arc<dyn MatrixTokenSource>,
    ) -> Result<Self, NativeError> {
        let native = Self::open_inner(state, origin, owner, false).await?;
        *native
            .0
            .matrix_source
            .lock()
            .map_err(|_| failure(503, "native_state_unavailable"))? = Some(source.clone());
        match native
            .0
            .host
            .console
            .0
            .server_login
            .authorize_matrix(
                &native.0.host.console,
                &native.0.expected_origin,
                &native.0.expected_owner,
                source,
            )
            .await
        {
            Ok(cookie) => {
                *native.0.cookie.lock().await = format!("hagency_console={cookie}");
                configuration::start(&native);
                Ok(native)
            }
            Err(e) => {
                native.shutdown().await;
                Err(failure(
                    if e.ends_with("unavailable") { 503 } else { 401 },
                    e,
                ))
            }
        }
    }
    async fn sync_matrix(&self) -> Result<(), NativeError> {
        let _guard = self.0.matrix_sync.lock().await;
        let source = self
            .0
            .matrix_source
            .lock()
            .map_err(|_| failure(503, "native_state_unavailable"))?
            .clone();
        if let Some(source) = source {
            let cookie = self.0.cookie.lock().await.clone();
            let raw = cookie
                .strip_prefix("hagency_console=")
                .ok_or(failure(401, "owner_authorization_required"))?;
            let next = match self
                .0
                .host
                .console
                .0
                .server_login
                .renew_matrix(&self.0.host.console, raw)
                .await
            {
                Ok(next) => next,
                Err(super::Error::Unavailable) => {
                    return Err(failure(503, "matrix_authorization_unavailable"));
                }
                Err(_) => Some(
                    self.0
                        .host
                        .console
                        .0
                        .server_login
                        .authorize_matrix(
                            &self.0.host.console,
                            &self.0.expected_origin,
                            &self.0.expected_owner,
                            source.clone(),
                        )
                        .await
                        .map_err(|e| {
                            failure(if e.ends_with("unavailable") { 503 } else { 401 }, e)
                        })?,
                ),
            };
            if let Some(next) = next {
                *self.0.cookie.lock().await = format!("hagency_console={next}");
            }
        }
        Ok(())
    }
    #[cfg(test)]
    pub(in crate::console) fn test_console(&self) -> &super::Console {
        &self.0.host.console
    }
    pub async fn identity(&self) -> Result<Identity, NativeError> {
        let d = self
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        if d.origin() != self.0.expected_origin || d.owner_mxid() != self.0.expected_owner {
            return Err(failure(401, "matrix_account_mismatch"));
        }
        Ok(Identity {
            origin: d.origin().into(),
            issuer: d.issuer().into(),
            subject: d.subject().into(),
            owner: d.owner_mxid().into(),
        })
    }
    /// In-process execution-host capability; never a native command response.
    /// Retained snapshots share retirement and expiry checks with this owner.
    pub async fn authorized_execution_device(
        &self,
    ) -> Result<super::AuthorizedDevice, NativeError> {
        let device = self
            .0
            .host
            .console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        if device.origin() != self.0.expected_origin || device.owner_mxid() != self.0.expected_owner
        {
            return Err(failure(401, "matrix_account_mismatch"));
        }
        device
            .bearer()
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        Ok(device)
    }
    pub fn retire(&self) {
        self.0.host.console.retire();
        self.0.crypto.retire();
        self.0
            .configuration_stopped
            .store(true, std::sync::atomic::Ordering::Release);
        if let Ok(tasks) = self.0.configuration_tasks.lock() {
            let current = tokio::task::try_id();
            for task in tasks.iter() {
                if current != Some(task.id()) {
                    task.abort();
                }
            }
        }
    }
    pub async fn shutdown(&self) {
        let _ = self.shutdown_checked().await;
    }
    pub async fn shutdown_checked(&self) -> crate::shutdown::Result {
        self.0
            .shutdown
            .get_or_init(|| {
                self.retire();
                let owner = self.clone();
                crate::shutdown::Task::start(async move { owner.finish_shutdown().await })
            })
            .wait()
            .await
    }
    async fn finish_shutdown(&self) -> crate::shutdown::Result {
        // Fence capabilities synchronously before any awaited cleanup, including
        // a late OAuth exchange or an in-flight runtime/provider launch.
        self.retire();
        let crypto_result = self.0.crypto.shutdown().await;
        let (reviews, mut review_result) = match self.0.review_tasks.lock() {
            Ok(mut tasks) => (std::mem::take(&mut *tasks), Ok(())),
            Err(_) => (Vec::new(), Err(crate::shutdown::Error::Worker)),
        };
        for task in reviews {
            if task.await.is_err() {
                review_result = Err(crate::shutdown::Error::Worker);
            }
        }
        let (tasks, mut result) = match self.0.configuration_tasks.lock() {
            Ok(mut tasks) => (std::mem::take(&mut *tasks), Ok(())),
            Err(_) => (Vec::new(), Err(crate::shutdown::Error::Worker)),
        };
        for task in tasks {
            if tokio::task::try_id() != Some(task.id())
                && task.await.is_err_and(|error| !error.is_cancelled())
            {
                result = Err(crate::shutdown::Error::Worker);
            }
        }
        if self.0.host.console.0.authority.revoke_all().is_err() {
            result = Err(crate::shutdown::Error::Revocation);
        }
        let login = &self.0.host.console.0.server_login;
        login.revoke_native_all().await;
        let stopped = self.0.host.console.stop_for_update().await;
        login.flush_native_revocations().await;
        self.retire();
        let task = self.0.callback_task.lock().unwrap().take();
        if let Some(task) = task {
            task.abort();
            if task.await.is_err_and(|error| !error.is_cancelled()) {
                result = Err(crate::shutdown::Error::Worker);
            }
        }
        result
            .and(review_result)
            .and(crypto_result)
            .and(stopped)
            .and(login.persist_revocations_for_update().await)
    }
    /// Recover only explicitly enabled bindings from this exact owner's current
    /// assigned device. Each request re-enters the normal start controller; no
    /// lease, tool permission or unknown model execution is replayed here.
    pub async fn restore_runtime_intents(&self) -> Result<Value, NativeError> {
        let _update_task = crate::update_guard::PROCESS
            .enter_task()
            .ok_or_else(|| failure(409, "update_in_progress"))?;
        let intents = {
            let _guard = self.0.calls.lock().await;
            self.sync_matrix().await?;
            self.0
                .host
                .console
                .0
                .owned_runtime
                .recovery_intents(&self.0.host.console)
                .await
                .map_err(|_| failure(409, "runtime_recovery_state_unavailable"))?
        };
        let mut results = Vec::new();
        for intent in intents {
            let input = intent.start_input();
            let result = self
                .execute(Command::Local {
                    agent_id: intent.agent_id.clone(),
                    action: if intent.binding_id == "*" {
                        LocalAction::StartAgentService
                    } else {
                        LocalAction::RuntimeStart
                    },
                    input: Some(if intent.binding_id == "*" {
                        let mut input = input;
                        input.as_object_mut().unwrap().remove("bindingId");
                        input
                    } else {
                        input
                    }),
                    binding_id: if intent.binding_id == "*" {
                        None
                    } else {
                        Some(intent.binding_id.clone())
                    },
                    requester: None,
                })
                .await;
            results.push(match result {
                Ok(value) => json!({"agentId":intent.agent_id,"bindingId":intent.binding_id,"restored":true,"runtime":value["runtime"]}),
                Err(error) => json!({"agentId":intent.agent_id,"bindingId":intent.binding_id,"restored":false,"error":error.code}),
            });
        }
        Ok(json!({"recoveries":results}))
    }
    pub async fn execute(&self, command: Command) -> Result<Value, NativeError> {
        let _update_task = if command.continues_running_task() {
            crate::update_guard::PROCESS.continue_task()
        } else {
            crate::update_guard::PROCESS.enter_task()
        }
        .ok_or_else(|| failure(409, "update_in_progress"))?;
        if configuration::is_wrapper(&command) {
            return configuration::remote(self, command).await;
        }
        let _guard = self.0.calls.lock().await;
        self.execute_locked(command).await
    }
    async fn execute_locked(&self, command: Command) -> Result<Value, NativeError> {
        let matrix_managed = self
            .0
            .matrix_source
            .lock()
            .map_err(|_| failure(503, "native_state_unavailable"))?
            .is_some();
        if matrix_managed && matches!(command, Command::Logout) {
            self.shutdown().await;
            return Ok(
                json!({"state":"signed_out","deviceAuthorized":false,"transportOnline":false}),
            );
        }
        if matrix_managed && matches!(command, Command::BeginLogin) {
            return Err(failure(400, "sdk_owns_matrix_login"));
        }
        if matrix_managed && !matches!(command, Command::Logout) {
            self.sync_matrix().await?;
        }
        if matches!(command, Command::BeginLogin) {
            let ticket = self
                .0
                .host
                .console
                .owner_access_ticket()
                .map_err(|_| failure(503, "native_access_unavailable"))?;
            let bootstrap = self
                .call(
                    "POST",
                    "/console/session",
                    Some(json!({"ticket":ticket})),
                    "",
                )
                .await;
            if !bootstrap.status_code.unwrap_or(StatusCode::OK).is_success() {
                return Err(failure(503, "native_access_unavailable"));
            }
            let cookie = bootstrap
                .headers()
                .get_all("set-cookie")
                .iter()
                .find_map(|v| {
                    v.to_str()
                        .ok()
                        .filter(|s| s.starts_with("hagency_console="))
                        .map(|s| s.split(';').next().unwrap().to_owned())
                })
                .ok_or(failure(503, "native_access_unavailable"))?;
            *self.0.cookie.lock().await = cookie.clone();
            let mut response = self
                .call(
                    "POST",
                    "/console/server-login/start",
                    Some(json!({"server":self.0.expected_origin,"name":"Hagency"})),
                    &cookie,
                )
                .await;
            let nonce = response
                .headers()
                .get_all("set-cookie")
                .iter()
                .find_map(|v| {
                    v.to_str()
                        .ok()
                        .filter(|s| s.starts_with("hagency_server_login="))
                        .map(|s| s.split(';').next().unwrap().to_owned())
                });
            let value = read_response(&mut response).await?;
            *self.0.nonce.lock().await = nonce;
            return Ok(value);
        }
        if !matrix_managed && !matches!(command, Command::LoginStatus | Command::Logout) {
            self.identity().await?;
        }
        if let Command::Ledger { action } = command {
            self.check_matrix_source().await?;
            let result = ledger::execute(self, action).await;
            self.check_matrix_source().await?;
            return result;
        }
        if let Command::RemoteApprovals { action } = command {
            self.check_matrix_source().await?;
            let result = remote_reviews::execute(self, action).await;
            self.check_matrix_source().await?;
            return result;
        }
        if let Command::AgentCrypto { action } = command {
            self.check_matrix_source().await?;
            let result = agent_crypto::execute(self, action).await;
            self.check_matrix_source().await?;
            return result;
        }
        if let Command::AgentUsage { action } = command {
            self.check_matrix_source().await?;
            let result = usage::execute(self, action).await;
            self.check_matrix_source().await?;
            return result;
        }
        if let Command::AgentHistory { action } = command {
            self.check_matrix_source().await?;
            let result = history::execute(self, action).await;
            self.check_matrix_source().await?;
            return result;
        }
        let crypto_command = command.clone();
        let (method, path, body) = request(command)?;
        let cookie = self.0.cookie.lock().await.clone();
        let mut response = self.call(method, &path, body, &cookie).await;
        if let Some(cookie) = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .find_map(|v| {
                v.to_str()
                    .ok()
                    .filter(|s| s.starts_with("hagency_console="))
                    .map(|s| s.split(';').next().unwrap().to_owned())
            })
        {
            *self.0.cookie.lock().await = cookie;
        }
        let mut result = read_response(&mut response).await;
        if let Ok(value) = &mut result {
            agent_crypto::automatic(self, &crypto_command, value).await;
        }
        if matrix_managed && path != "/console/server-login/logout" {
            self.check_matrix_source().await?;
        }
        result
    }
    async fn check_matrix_source(&self) -> Result<(), NativeError> {
        let managed = self
            .0
            .matrix_source
            .lock()
            .map_err(|_| failure(503, "native_state_unavailable"))?
            .is_some();
        if !managed {
            return Ok(());
        }
        let cookie = self.0.cookie.lock().await.clone();
        let raw = cookie
            .strip_prefix("hagency_console=")
            .ok_or_else(|| failure(401, "owner_authorization_required"))?;
        if let Err(code) = self
            .0
            .host
            .console
            .0
            .server_login
            .check_matrix_source(raw)
            .await
        {
            if code == "matrix_authorization_unavailable" {
                tracing::warn!(
                    stage = "sdk_post_request",
                    code,
                    "Matrix authorization temporarily unavailable; preserving recovery state"
                );
            } else {
                self.shutdown().await;
            }
            return Err(failure(
                if code.ends_with("unavailable") {
                    503
                } else {
                    401
                },
                code,
            ));
        }
        Ok(())
    }
    async fn call(&self, method: &str, path: &str, body: Option<Value>, cookie: &str) -> Response {
        let client = match method {
            "POST" => TestClient::post(format!("http://{}{path}", self.0.address)),
            "PUT" => TestClient::put(format!("http://{}{path}", self.0.address)),
            "DELETE" => TestClient::delete(format!("http://{}{path}", self.0.address)),
            _ => TestClient::get(format!("http://{}{path}", self.0.address)),
        };
        let client = client
            .add_header("host", self.0.address.to_string(), true)
            .add_header("sec-fetch-site", "same-origin", true)
            .add_header("origin", format!("http://{}", self.0.address), true)
            .add_header("cookie", cookie, true);
        match body {
            Some(body) => client.json(&body).send(&self.0.service).await,
            None => client.send(&self.0.service).await,
        }
    }
}
async fn read_response(response: &mut Response) -> Result<Value, NativeError> {
    let status = response.status_code.unwrap_or(StatusCode::OK).as_u16();
    let value = response
        .take_json::<Value>()
        .await
        .map_err(|_| failure(502, "invalid_native_response"))?;
    if status >= 400 {
        return Err(failure(
            status,
            value["code"].as_str().unwrap_or("native_operation_failed"),
        ));
    }
    Ok(value)
}
#[derive(Clone)]
struct Callback(Weak<Inner>);
#[handler]
impl Callback {
    async fn handle(&self, depot: &mut Depot) {
        depot.insert_typed(self.clone());
    }
}
#[handler]
async fn callback(req: &mut Request, depot: &Depot, res: &mut Response) {
    let result = async {
        let native = NativeOwner(
            depot
                .get_typed::<Callback>()
                .map_err(|_| failure(503, "native_state_unavailable"))?
                .0
                .upgrade()
                .ok_or(failure(410, "native_owner_stopped"))?,
        );
        let _guard = native.0.calls.lock().await;
        if native
            .0
            .matrix_source
            .lock()
            .map_err(|_| failure(503, "native_state_unavailable"))?
            .is_some()
        {
            return Err(failure(400, "sdk_owns_matrix_login"));
        }
        let query = req
            .uri()
            .query()
            .filter(|q| q.len() <= 8192)
            .ok_or(failure(400, "invalid_oauth_state"))?;
        let nonce = native
            .0
            .nonce
            .lock()
            .await
            .clone()
            .ok_or(failure(400, "invalid_oauth_state"))?;
        let mut response = native
            .call(
                "GET",
                &format!("/console/server-login/callback?{query}"),
                None,
                &nonce,
            )
            .await;
        let cookie = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .find_map(|v| {
                v.to_str()
                    .ok()
                    .filter(|s| s.starts_with("hagency_console="))
                    .map(|s| s.split(';').next().unwrap().to_owned())
            })
            .ok_or(failure(401, "owner_authorization_required"))?;
        *native.0.cookie.lock().await = cookie;
        native.0.nonce.lock().await.take();
        native.identity().await?;
        response.take_string().await.ok();
        Ok::<_, NativeError>(())
    }
    .await;
    res.add_header("cache-control", "no-store", true).unwrap();
    res.add_header("content-security-policy", "default-src 'none'", true)
        .unwrap();
    match result {
        Ok(()) => res.render("Hagency authorization completed. Return to Hagency."),
        Err(e) => {
            res.status_code(StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_REQUEST));
            res.render(
                "Authorization failed. Return to Hagency and retry with the same Matrix account.",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_desktop_listener_exposes_only_oauth_callback() {
        let root = tempfile::tempdir().unwrap();
        let owner = NativeOwner::open_inner(
            &root.path().join("state"),
            "http://127.0.0.1:9",
            "@owner:example.test",
            false,
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        for path in [
            "/",
            "/console/",
            "/console/login/",
            "/console/projects/",
            "/console/agents-owned/",
            "/console/api/owned-agents",
            "/console/api/owner-provider",
            "/health",
            "/ready",
        ] {
            let response = client
                .get(format!("http://{}{path}", owner.0.address))
                .send()
                .await
                .unwrap();
            // Salvo returns 405 for a matched prefix without an endpoint and
            // 404 for an unmatched path. Neither is a management route.
            assert!(
                matches!(response.status().as_u16(), 404 | 405),
                "unexpected public endpoint: {path} ({})",
                response.status()
            );
        }
        let response = client
            .get(format!(
                "http://{}/console/server-login/callback",
                owner.0.address
            ))
            .send()
            .await
            .unwrap();
        assert!(
            matches!(response.status().as_u16(), 400 | 401 | 403),
            "an unauthenticated callback must be refused by the callback handler"
        );
        owner.shutdown().await;
    }
    #[test]
    fn external_provider_commands_allow_only_fixed_runtime_routes_and_local_actions() {
        for runtime in ["claude", "octos"] {
            for (action, method, suffix) in [
                (ProviderAction::Status, "GET", ""),
                (ProviderAction::Models, "GET", "/models"),
                (ProviderAction::UseLocal, "POST", "/use-local"),
                (ProviderAction::Disconnect, "POST", "/disconnect"),
            ] {
                let (actual, path, body) = request(Command::RuntimeProvider {
                    runtime: runtime.into(),
                    action,
                })
                .unwrap();
                assert_eq!(actual, method);
                assert_eq!(
                    path,
                    format!("/console/api/owner-provider/runtimes/{runtime}{suffix}")
                );
                assert!(body.is_none());
            }
            for action in [
                ProviderAction::Login,
                ProviderAction::Logout,
                ProviderAction::Cancel,
            ] {
                assert!(
                    request(Command::RuntimeProvider {
                        runtime: runtime.into(),
                        action
                    })
                    .is_err()
                );
            }
        }
        for runtime in ["../claude", "codex", "remote"] {
            assert!(
                request(Command::RuntimeProvider {
                    runtime: runtime.into(),
                    action: ProviderAction::Models
                })
                .is_err()
            );
        }
    }
    #[test]
    fn agent_audience_commands_keep_catalog_read_only_and_validate_changes() {
        let (method, path, body) = request(Command::DiscoverAgents).unwrap();
        assert_eq!(
            (method, path.as_str(), body),
            ("GET", "/console/api/owned-agents/discover", None)
        );
        for visibility in ["private", "public"] {
            let (method, path, body) = request(Command::SetAgentVisibility {
                agent_id: "a".into(),
                visibility: visibility.into(),
            })
            .unwrap();
            assert_eq!(method, "PUT");
            assert_eq!(path, "/console/api/owned-agents/a/visibility");
            assert_eq!(body, Some(serde_json::json!({"visibility":visibility})));
        }
        assert!(
            request(Command::SetAgentVisibility {
                agent_id: "a".into(),
                visibility: "team".into()
            })
            .is_err()
        );
    }
    #[test]
    fn reply_policy_command_uses_exact_binding_and_boolean_body() {
        for enabled in [false, true] {
            let (method, path, body) = request(Command::Binding {
                agent_id: "agt_1".into(),
                binding_id: "bnd_1".into(),
                action: BindingAction::SetThreadAutoReply(enabled),
            })
            .unwrap();
            assert_eq!(method, "PUT");
            assert_eq!(
                path,
                "/console/api/owned-agents/agt_1/bindings/bnd_1/reply-policy"
            );
            assert_eq!(body, Some(serde_json::json!({"threadAutoReply":enabled})));
        }
    }
    #[test]
    fn native_commands_cannot_select_an_arbitrary_path_or_foreign_binding_route() {
        assert!(
            request(Command::PrepareAgentWorkspace {
                agent_id: "../other".into()
            })
            .is_err()
        );
        let (method, path, body) = request(Command::PrepareAgentWorkspace {
            agent_id: "agt_owned".into(),
        })
        .unwrap();
        assert_eq!(method, "POST");
        assert_eq!(path, "/console/api/owner-provider/workspaces/agt_owned");
        assert!(body.is_none());
        let (method, path, body) = request(Command::Provider {
            action: ProviderAction::Models,
        })
        .unwrap();
        assert_eq!(method, "GET");
        assert_eq!(path, "/console/api/owner-provider/models");
        assert!(body.is_none());
        assert!(
            request(Command::ResumeAgentCommand {
                id: "../other".into()
            })
            .is_err()
        );
        assert!(
            request(Command::Local {
                agent_id: "a".into(),
                action: LocalAction::AgentPolicy,
                input: None,
                binding_id: Some("b".into()),
                requester: None
            })
            .is_err()
        );
        let (_, path, _) = request(Command::Local {
            agent_id: "a".into(),
            action: LocalAction::AgentPolicy,
            input: None,
            binding_id: None,
            requester: None,
        })
        .unwrap();
        assert_eq!(path, "/console/api/owned-agents/a/agent-policy");

        for id in [
            "",
            "../other",
            "agt?token=x",
            "agt/retire",
            "https://evil.test",
        ] {
            assert!(
                request(Command::Bindings {
                    agent_id: id.into()
                })
                .is_err()
            );
        }
        assert!(
            request(Command::SpaceCandidates {
                cursor: Some("!s%3Atest".into())
            })
            .is_err()
        );
        assert_eq!(
            request(Command::Binding {
                agent_id: "agt_1".into(),
                binding_id: "bnd_1".into(),
                action: BindingAction::Leave
            })
            .unwrap()
            .0,
            "DELETE"
        );
        assert!(
            request(Command::Local {
                agent_id: "agt_1".into(),
                action: LocalAction::Policy,
                input: None,
                binding_id: None,
                requester: None
            })
            .is_err()
        );
        let (_, path, _) = request(Command::Local {
            agent_id: "agt_1".into(),
            action: LocalAction::Policy,
            input: None,
            binding_id: Some("bnd_1".into()),
            requester: Some("@user:test&url=https://evil.test".into()),
        })
        .unwrap();
        assert!(path.starts_with("/console/api/owned-agents/agt_1/local-policy?"));
        assert!(!path.contains("&url="));
    }
    #[test]
    fn update_wait_allows_exact_approvals_and_inspection_but_no_new_execution() {
        let local = |action, input| Command::Local {
            agent_id: "agent-a".into(),
            action,
            input,
            binding_id: None,
            requester: None,
        };
        for command in [
            local(LocalAction::Approvals, None),
            local(
                LocalAction::Decision {
                    approval_id: "original".into(),
                },
                None,
            ),
            local(LocalAction::Runtime, None),
            Command::Agents,
        ] {
            assert!(command.continues_running_task());
        }
        for command in [
            local(LocalAction::Approvals, Some(json!({}))),
            local(LocalAction::RuntimeStart, None),
            local(LocalAction::SaveAgentModel, None),
            local(LocalAction::StartAgentService, None),
            Command::BeginLogin,
            Command::EnsureAgentOwnerDirect {
                agent_id: "agent-a".into(),
            },
            Command::Provider {
                action: ProviderAction::Models,
            },
        ] {
            assert!(!command.continues_running_task());
        }
    }
    #[tokio::test]
    async fn shutdown_from_configuration_task_joins_the_aborted_initiator_without_deadlock() {
        let root = tempfile::tempdir().unwrap();
        let owner = NativeOwner::open(
            &root.path().join("state"),
            "https://example.test/",
            "@owner:example.test",
        )
        .await
        .unwrap();
        let (begin, ready) = tokio::sync::oneshot::channel();
        let initiator = owner.clone();
        let task = tokio::spawn(async move {
            ready.await.unwrap();
            let _ = initiator.shutdown_checked().await;
        });
        let abort = task.abort_handle();
        owner.0.configuration_tasks.lock().unwrap().push(task);
        begin.send(()).unwrap();
        // Wait until the configuration task has initiated the retained cleanup.
        while owner.0.shutdown.get().is_none() {
            tokio::task::yield_now().await;
        }
        tokio::time::timeout(Duration::from_secs(5), owner.shutdown_checked())
            .await
            .unwrap()
            .unwrap();
        assert!(abort.is_finished());
    }
    #[tokio::test]
    async fn shutdown_worker_panic_remains_a_failure_on_repeated_shutdown() {
        let root = tempfile::tempdir().unwrap();
        let owner = NativeOwner::open(
            &root.path().join("state"),
            "https://example.test/",
            "@owner:example.test",
        )
        .await
        .unwrap();
        let task = tokio::spawn(async { panic!("configuration worker panic") });
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
        owner.0.configuration_tasks.lock().unwrap().push(task);
        assert_eq!(
            owner.shutdown_checked().await,
            Err(crate::shutdown::Error::Worker)
        );
        assert_eq!(
            owner.shutdown_checked().await,
            Err(crate::shutdown::Error::Worker)
        );
    }
    #[tokio::test]
    async fn native_host_needs_no_web_assets_and_restart_never_restores_remote_authority() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let owner = NativeOwner::open(&state, "https://example.test/", "@owner:example.test")
            .await
            .unwrap();
        assert_eq!(
            owner.execute(Command::Projects).await.unwrap_err().code,
            "owner_authorization_required"
        );
        assert!(owner.execute(Command::LoginStatus).await.is_ok());
        owner.shutdown().await;
        assert!(owner.execute(Command::BeginLogin).await.is_err());
        drop(owner);
        let restored = NativeOwner::open(&state, "https://example.test/", "@owner:example.test")
            .await
            .unwrap();
        assert_eq!(
            restored.execute(Command::Agents).await.unwrap_err().code,
            "owner_authorization_required"
        );
        restored.shutdown().await;
    }
}

#[cfg(test)]
mod owner_scope_command_tests {
    use super::*;
    #[test]
    fn native_scope_admin_commands_are_closed_and_do_not_claim_admin_identity() {
        let policy = ProjectCreationPolicy {
            default_allow: true,
            allow: Default::default(),
            deny: Default::default(),
        };
        let (method, path, body) = request(Command::SetProjectCreationPolicy {
            project_id: "p".into(),
            expected_revision: 4,
            policy,
        })
        .unwrap();
        assert_eq!(
            (method, path.as_str()),
            ("PUT", "/console/api/owner-projects/p/creation-policy")
        );
        assert_eq!(body.unwrap()["expectedRevision"], 4);
        let (method, path, body) = request(Command::SetScopeServicePause {
            project_id: "p".into(),
            room_id: Some("!r:test".into()),
            paused: true,
        })
        .unwrap();
        assert_eq!(method, "POST");
        assert!(path.ends_with("/pause-service"));
        assert!(path.contains("%21r%3Atest"));
        assert!(body.is_none());
        assert!(
            request(Command::ScopeServiceState {
                project_id: "../p".into(),
                room_id: None
            })
            .is_err()
        );
        assert!(
            request(Command::SetRoomCreationPolicy {
                project_id: "p".into(),
                room_id: "!r:test".into(),
                expected_revision: -1,
                policy: RoomCreationPolicy::Disabled
            })
            .is_err()
        );
    }
    #[test]
    fn policy_dtos_reject_unknown_fields_and_disabled_payload() {
        assert!(
            serde_json::from_value::<RoomCreationPolicy>(json!({"mode":"disabled","allow":[]}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<ProjectCreationPolicy>(
                json!({"defaultAllow":true,"allow":[],"deny":[],"owner":"override"})
            )
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(RoomCreationPolicy::Disabled).unwrap(),
            json!({"mode":"disabled"})
        );
    }
    #[test]
    fn agent_resource_commands_need_no_synthetic_room_and_reject_room_scope() {
        let (_, path, _) = request(Command::Local {
            agent_id: "a".into(),
            action: LocalAction::AgentPolicy,
            input: None,
            binding_id: None,
            requester: None,
        })
        .unwrap();
        assert_eq!(path, "/console/api/owned-agents/a/agent-policy");
        assert!(
            request(Command::Local {
                agent_id: "a".into(),
                action: LocalAction::SaveAgentModel,
                input: Some(json!({})),
                binding_id: Some("b".into()),
                requester: None
            })
            .is_err()
        );
        assert!(request(Command::ResumeAgentCommand { id: "../x".into() }).is_err());
        assert_eq!(
            request(Command::AgentCommands).unwrap().1,
            "/console/api/owned-agents/commands"
        );
    }
    #[test]
    fn device_and_assignment_commands_expose_no_bearer_or_arbitrary_path() {
        assert_eq!(
            request(Command::Devices).unwrap().1,
            "/console/api/owned-agents/devices"
        );
        let (method, path, body) = request(Command::AssignAgentToCurrentDevice {
            agent_id: "a".into(),
            expected_generation: 3,
        })
        .unwrap();
        assert_eq!(
            (method, path.as_str()),
            ("PUT", "/console/api/owned-agents/a/execution-device")
        );
        assert_eq!(body, Some(json!({"expectedGeneration":3})));
        assert!(
            request(Command::AssignAgentToCurrentDevice {
                agent_id: "a".into(),
                expected_generation: -1
            })
            .is_err()
        );
        assert!(
            request(Command::AgentDetails {
                agent_id: "../x".into()
            })
            .is_err()
        );
        assert_eq!(
            request(Command::AgentDetails {
                agent_id: "a".into()
            })
            .unwrap()
            .1,
            "/console/api/owned-agents/a"
        );
        assert!(
            request(Command::AgentOwnerDirect {
                agent_id: "a".into(),
                room_id: "!r:test/escape".into()
            })
            .is_err()
        );
    }
}

#[cfg(test)]
mod version12_route_tests {
    use super::*;
    #[test]
    fn version12_native_room_routes_encode_the_original_hash_without_weakening_path_checks() {
        let room = "!xBwLsfb3tVTO0jgpqkLGnxHIT4qWA6yILBlXdpHMSmY";
        let (method, path, body) = request(Command::ScopeServiceState {
            project_id: "project".into(),
            room_id: Some(room.into()),
        })
        .unwrap();
        assert_eq!(method, "GET");
        assert!(body.is_none());
        assert_eq!(
            percent_encoding::percent_decode_str(
                path.split("/rooms/")
                    .nth(1)
                    .unwrap()
                    .split('/')
                    .next()
                    .unwrap()
            )
            .decode_utf8()
            .unwrap(),
            room
        );
        for invalid in [
            "!bad/path:test",
            "!bad%2fpath:test",
            "!bad?query:test",
            "!bad#fragment:test",
            "!truncated",
        ] {
            assert!(
                request(Command::RoomRoster {
                    project_id: "project".into(),
                    room_id: invalid.into()
                })
                .is_err()
            );
        }
    }
}
