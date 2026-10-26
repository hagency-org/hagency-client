//! Native management reuses the current SDK OAuth identity. The SDK alone owns
//! OAuth login, refresh and logout; the owner engine never starts another login.
use super::model::{BindingAction, Command, LocalAction, ProviderAction};
mod continuation;
mod agent_files;
use ::hagency::native_owner::{MatrixAccessToken, MatrixTokenSource, NativeError, NativeOwner};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex, Weak},
};

/// Shared Room candidates require an explicit public audience from the server.
pub(crate) fn can_add_to_shared_room(agent: &Value) -> bool {
    agent["state"] == "active" && agent["visibility"] == "public"
}

/// A UI waiting deadline may detach only these reads. The request itself must
/// keep running: even a first read bootstraps owner authorization and a device.
/// Never extend this list to actions that create, resume, save or run anything.
pub(super) fn management_read(command: &Command) -> bool {
    if let Command::ProjectBoard {action,..}=command {return action.is_read();}
    matches!(command,
        Command::Projects | Command::ProjectRooms {..} | Command::SpaceCandidates {..}
        | Command::RoomRoster {..} | Command::Agents | Command::DiscoverAgents | Command::Bindings {..}
        | Command::Devices | Command::AgentDetails {..} | Command::AgentConfigurationStatus {..}
        | Command::AgentOwnerDirectStatus {..} | Command::ScopeServiceState {..}
        | Command::LoginStatus | Command::AgentCommands | Command::CheckAgentCommand {..} | Command::Creations | Command::Creation {..}
        | Command::Binding {action: BindingAction::Status, ..}
        | Command::Provider {action: ProviderAction::Status | ProviderAction::Models}
        | Command::RuntimeProvider {action: ProviderAction::Status | ProviderAction::Models, ..}
        | Command::Local {action: LocalAction::AgentModel | LocalAction::AgentPolicy
            | LocalAction::Policy | LocalAction::Model | LocalAction::Runtime
            | LocalAction::AgentServiceStatus | LocalAction::Approvals, input: None, ..}
    )
}

/// The Matrix SDK owns OAuth rotation and persistence. This helper never reads
/// a refresh token, and checks the caller's identity boundary on both sides.
pub(crate) async fn refresh_sdk_oauth(
    client: &matrix_sdk::Client,
    live_check: impl Fn() -> Result<(), NativeError> + Send + Sync,
) -> Result<(), NativeError> {
    live_check()?;
    tokio::time::timeout(std::time::Duration::from_secs(8), client.oauth().refresh_access_token())
        .await
        .map_err(|_| source_error("matrix_authorization_unavailable"))?
        .map_err(|error| {
            let code = crate::session_auth::refresh_failure_code(&error);
            makepad_widgets::warning!("Matrix OAuth refresh failed: {code}");
            source_error(code)
        })?;
    makepad_widgets::log!("Matrix OAuth refresh completed");
    live_check()?;
    Ok(())
}

/// Only engine-defined static codes may enter startup diagnostics. Neither a
/// provider response nor arbitrary source error text is logged.
fn recovery_error_code(code: &str) -> &'static str {
    match code {
        "runtime_profile_unavailable" => "runtime_profile_unavailable",
        "runtime_stopped" => "runtime_stopped",
        "agent_runtime_already_active" => "agent_runtime_already_active",
        "agent_execution_device_required" => "agent_execution_device_required",
        "ledger_recovery_required" => "ledger_recovery_required",
        "owner_authorization_required" => "owner_authorization_required",
        "owner_scope_required" => "owner_scope_required",
        "sign_in_required" => "sign_in_required",
        "invalid_arguments" => "invalid_arguments",
        "invalid_server_response" => "invalid_server_response",
        "invalid_runtime_start" => "invalid_runtime_start",
        "codex_strict_quota_unavailable" => "codex_strict_quota_unavailable",
        "provider_profile_mismatch" => "provider_profile_mismatch",
        "provider_authorization_lost" => "provider_authorization_lost",
        "provider_unavailable" => "provider_unavailable",
        "local_state_unavailable" => "local_state_unavailable",
        "device_transport_unavailable" => "device_transport_unavailable",
        "matrix_account_changed" => "matrix_account_changed",
        "matrix_authorization_unavailable" => "matrix_authorization_unavailable",
        "matrix_authorization_required" => "matrix_authorization_required",
        "matrix_oauth_sign_in_required" => "matrix_oauth_sign_in_required",
        "runtime_recovery_state_unavailable" => "runtime_recovery_state_unavailable",
        "server_unavailable" => "server_unavailable",
        _ => "runtime_recovery_unrecognized_error",
    }
}
fn log_runtime_recovery(result: Result<Value, NativeError>) {
    match result {
        Ok(value) => {
            if let Some(items)=value["recoveries"].as_array() {
                makepad_widgets::log!("Agent startup recovery: {} eligible bindings",items.len());
                for (index,item) in items.iter().enumerate() {
                    if item["restored"]==false {
                        let code=recovery_error_code(item["error"].as_str().unwrap_or(""));
                        makepad_widgets::warning!("Agent startup recovery item {index} failed: {code}");
                    } else {
                        makepad_widgets::log!("Agent startup recovery item {index}: start accepted; readiness not yet confirmed");
                    }
                }
            }
        }
        Err(error) => {
            let code=recovery_error_code(&error.code);
            makepad_widgets::warning!("Agent startup recovery unavailable: {code}");
        }
    }
}

struct SdkTokens(Weak<Inner>);
fn source_error(code: &str) -> NativeError {
    NativeError {
        status: if code == "matrix_authorization_unavailable" {503} else {401},
        code: code.into(),
    }
}
impl MatrixTokenSource for SdkTokens {
    fn supports_agent_file_receive(&self)->bool { true }
    fn fetch_agent_attachment(&self, attachment: ::hagency::native_owner::AgentAttachment) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<Vec<u8>,NativeError>> + Send + '_>> {
        Box::pin(async move {
            self.access_token().await?;
            let service=Service(self.0.upgrade().ok_or_else(||source_error("matrix_account_changed"))?);
            let client=crate::sliding_sync::get_client().ok_or_else(||source_error("matrix_sign_in_required"))?;
            agent_files::fetch(&client,&service.0.origin,&service.0.owner,&attachment,||service.check().map_err(|e|source_error(&e))).await
        })
    }

    fn send_continuation(&self, message: ::hagency::native_owner::ContinuationMessage) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<String,NativeError>> + Send + '_>> {
        Box::pin(async move {
            self.access_token().await?;
            let service=Service(self.0.upgrade().ok_or_else(||source_error("matrix_account_changed"))?);
            service.check().map_err(|e|source_error(&e))?;
            let client=crate::sliding_sync::get_client().ok_or_else(||source_error("matrix_sign_in_required"))?;
            continuation::send(&client,&service.0.origin,&service.0.owner,message,||service.check().map_err(|e|source_error(&e))).await
        })
    }

    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Box::pin(async move {
            let service = Service(
                self.0
                    .upgrade()
                    .ok_or_else(|| source_error("matrix_account_changed"))?,
            );
            service.check().map_err(|e| source_error(&e))?;
            let client = crate::sliding_sync::get_client()
                .ok_or_else(|| source_error("matrix_sign_in_required"))?;
            let oauth = client.oauth();
            if oauth.client_id().is_none() {
                return Err(source_error("matrix_oauth_sign_in_required"));
            }
            let metadata = tokio::time::timeout(
                std::time::Duration::from_secs(8),
                oauth.cached_server_metadata(),
            )
            .await
            .map_err(|_| source_error("matrix_authorization_unavailable"))?
            .map_err(|_| source_error("matrix_authorization_unavailable"))?;
            service.check().map_err(|e| source_error(&e))?;
            if metadata.issuer.as_str() != format!("{}auth/", service.0.origin) {
                return Err(source_error("unsupported_hagency_issuer"));
            }
            // This actual SDK request refreshes expired OAuth credentials. Do not
            // call the owner service here: its session lock may be awaiting us.
            let identity = tokio::time::timeout(std::time::Duration::from_secs(8), client.whoami())
                .await
                .map_err(|_| source_error("matrix_authorization_unavailable"))?
                .map_err(|error| {
                    source_error(
                        if crate::session_auth::http_requires_login(&error)
                        {
                            "matrix_authorization_required"
                        } else {
                            "matrix_authorization_unavailable"
                        },
                    )
                })?;
            service.check().map_err(|e| source_error(&e))?;
            if identity.user_id.as_str() != service.0.owner
                || identity.device_id.as_deref() != client.device_id()
            {
                return Err(source_error("matrix_account_mismatch"));
            }
            Ok(MatrixAccessToken {
                access_token: client
                    .access_token()
                    .ok_or_else(|| source_error("matrix_authorization_required"))?,
                client_id: oauth
                    .client_id()
                    .ok_or_else(|| source_error("matrix_oauth_sign_in_required"))?
                    .as_str()
                    .into(),
            })
        })
    }
    fn refresh_access_token(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>> {
        Box::pin(async move {
            let service = Service(self.0.upgrade().ok_or_else(|| source_error("matrix_account_changed"))?);
            service.check().map_err(|e| source_error(&e))?;
            let client = crate::sliding_sync::get_client().ok_or_else(|| source_error("matrix_sign_in_required"))?;
            if client.oauth().client_id().is_none() {
                return Err(source_error("matrix_oauth_sign_in_required"));
            }
            refresh_sdk_oauth(&client, || service.check().map_err(|e| source_error(&e))).await?;
            // Fresh SDK whoami and issuer/device checks prove the rotated token.
            // A late account transition cannot authorize its old snapshot.
            self.access_token().await
        })
    }

}

static CURRENT: LazyLock<Mutex<Option<Service>>> = LazyLock::new(|| Mutex::new(None));
// An update must join an account transition that already took CURRENT, rather
// than mistake its empty slot for a completed owner shutdown.
static STOP_CURRENT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static RETIREMENTS: LazyLock<Mutex<::hagency::shutdown::Registry>> = LazyLock::new(|| Mutex::new(::hagency::shutdown::Registry::default()));
struct Inner {
    origin: String,
    owner: String,
    epoch: u64,
    state: PathBuf,
    native: tokio::sync::Mutex<Option<NativeOwner>>,
    retired: std::sync::atomic::AtomicBool,
    matrix_checked: tokio::sync::Mutex<Option<std::time::Instant>>,
    shutdown: std::sync::OnceLock<::hagency::shutdown::Task>,
}
#[derive(Clone)]
pub struct Service(Arc<Inner>);
impl Service {
    pub fn for_current() -> Result<Self, String> {
        if crate::logout::logout_state_machine::is_logout_in_progress() {
            return Err("matrix_account_changing".into());
        }
        let epoch = crate::account_session::epoch();
        let client = crate::sliding_sync::get_client().ok_or("matrix_sign_in_required")?;
        let owner = client
            .user_id()
            .ok_or("matrix_sign_in_required")?
            .to_string();
        let origin = client.homeserver().to_string();
        if epoch != crate::account_session::epoch() || crate::sliding_sync::account_changing() {
            return Err("matrix_account_changing".into());
        }
        let mut current = CURRENT.lock().map_err(|_| "native_owner_unavailable")?;
        if let Some(service) = current.as_ref()
            && service.0.origin == origin
            && service.0.owner == owner
            && service.0.epoch == epoch
            && !service.0.retired.load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(service.clone());
        }
        // Looking up an existing pinned owner is read-only (also used by the
        // approval UI). Creation/retirement still requires fresh work admission.
        let _update_task = ::hagency::update_guard::PROCESS.enter_task().ok_or("update_in_progress")?;
        if current.is_some() {
            let retirement = ::hagency::update_guard::PROCESS.continue_task()
                .ok_or("update_in_progress")?;
            let old = current.take().unwrap();
            old.0
                .retired
                .store(true, std::sync::atomic::Ordering::Release);
            crate::sliding_sync::backend_runtime().spawn(async move {
                let _retirement = retirement;
                old.shutdown().await;
            });
        }
        let digest = blake3::hash(
            serde_json::to_string(&(&origin, &owner))
                .map_err(|_| "invalid_account_identity")?
                .as_bytes(),
        )
        .to_hex()
        .to_string();
        let state = crate::app_data_dir().join("native-owner").join(digest);
        let service = Self(Arc::new(Inner {
            origin,
            owner,
            epoch,
            state,
            native: tokio::sync::Mutex::new(None),
            retired: std::sync::atomic::AtomicBool::new(false),
            matrix_checked: tokio::sync::Mutex::new(None),
            shutdown: std::sync::OnceLock::new(),
        }));
        *current = Some(service.clone());
        let startup = service.clone();
        crate::sliding_sync::backend_runtime().spawn(async move {
            let _ = startup.execute(Command::LoginStatus).await;
        });
        Ok(service)
    }
    /// Stable request pin: account epoch plus homeserver and exact Matrix ID.
    /// The engine independently pins issuer/subject in every ledger/provider path.
    pub fn identity(&self) -> String {
        serde_json::to_string(&(self.0.epoch, &self.0.origin, &self.0.owner)).unwrap_or_default()
    }
    fn check(&self) -> Result<(), String> {
        if self.0.retired.load(std::sync::atomic::Ordering::Acquire)
            || self.0.epoch != crate::account_session::epoch()
            || crate::sliding_sync::account_changing()
            || crate::logout::logout_state_machine::is_logout_in_progress()
        {
            return Err("matrix_account_changed".into());
        }
        let client = crate::sliding_sync::get_client().ok_or("matrix_sign_in_required")?;
        if client.homeserver().as_str() != self.0.origin
            || client
                .user_id()
                .is_none_or(|id| id.as_str() != self.0.owner)
        {
            return Err("matrix_account_changed".into());
        }
        Ok(())
    }
    async fn verify_matrix(&self, fresh: bool) -> Result<(), String> {
        self.check()?;
        let mut checked = self.0.matrix_checked.lock().await;
        self.check()?;
        if !fresh && checked.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(30)) {
            return Ok(());
        }
        let client = crate::sliding_sync::get_client().ok_or("matrix_sign_in_required")?;
        let response = tokio::time::timeout(std::time::Duration::from_secs(8), client.whoami())
            .await
            .map_err(|_| "matrix_authorization_unavailable")?;
        self.check()?;
        match response {
            Ok(identity) if identity.user_id.as_str() == self.0.owner => {
                *checked = Some(std::time::Instant::now());
                Ok(())
            }
            Ok(_) => {
                drop(checked);
                self.shutdown().await;
                Err("matrix_account_mismatch".into())
            }
            Err(error) => {
                *checked = None;
                if crate::session_auth::http_requires_login(&error)
                {
                    drop(checked);
                    self.shutdown().await;
                    Err("matrix_authorization_required".into())
                } else {
                    Err("matrix_authorization_unavailable".into())
                }
            }
        }
    }
    /// Reads the selected Space/Room through this exact SDK account. Never
    /// joins a Room or grants access based on parent Space membership.
    pub async fn room_members(&self, room_id: String, space: bool) -> Result<Value, String> {
        self.verify_matrix(false).await?;
        let id = matrix_sdk::ruma::OwnedRoomId::try_from(room_id.as_str())
            .map_err(|_| "invalid_room_id")?;
        let client = crate::sliding_sync::get_client().ok_or("matrix_sign_in_required")?;
        // Pin the captured SDK client itself: account switching may happen
        // between verification and acquisition of the global client.
        if client.homeserver().as_str() != self.0.origin
            || client.user_id().is_none_or(|id| id.as_str() != self.0.owner)
        {
            return Err("matrix_account_changed".into());
        }
        self.check()?;
        let room = client.get_room(&id).ok_or("room_join_required")?;
        if room.state() != matrix_sdk::RoomState::Joined {
            return Err("room_join_required".into());
        }
        if room.is_space() != space {
            return Err("room_kind_mismatch".into());
        }
        let members = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            room.members(matrix_sdk::RoomMemberships::JOIN | matrix_sdk::RoomMemberships::INVITE),
        )
        .await
        .map_err(|_| "members_unavailable")?
        .map_err(|_| "members_unavailable")?;
        self.check()?;
        if room.state() != matrix_sdk::RoomState::Joined {
            return Err("room_join_required".into());
        }
        let can_invite = members.iter().any(|m| {
            m.user_id().as_str() == self.0.owner
                && m.membership() == &matrix_sdk::ruma::events::room::member::MembershipState::Join
                && m.can_invite()
        });
        let mut members = members.iter().map(|m| {
            let power = match m.power_level() {
                matrix_sdk::ruma::events::room::power_levels::UserPowerLevel::Int(v) => v.to_string(),
                _ => "Creator".into(),
            };
            serde_json::json!({"userId":m.user_id().as_str(),"name":m.display_name().unwrap_or(m.user_id().as_str()),
                "membership":m.membership().as_str(),"role":format!("{:?}",m.suggested_role_for_power_level()),"power":power})
        }).collect::<Vec<_>>();
        members.sort_by_key(|m| {
            (
                m["name"].as_str().unwrap_or("").to_lowercase(),
                m["userId"].as_str().unwrap_or("").to_owned(),
            )
        });
        self.check()?;
        Ok(
            serde_json::json!({"roomId":room_id,"space":space,"canInvite":can_invite,"members":members}),
        )
    }
    /// Resolve only an existing, visible Project/Room association. This never
    /// adopts a Matrix Room and never substitutes an SDK invite for a binding.
    pub async fn agents_for_registered_room(&self, room_id: &str) -> Result<Value, String> {
        self.check()?;
        let client = crate::sliding_sync::get_client().ok_or("matrix_sign_in_required")?;
        let room = client.get_room(matrix_sdk::ruma::RoomId::parse(room_id).map_err(|_| "invalid_room_id")?.as_ref()).ok_or("room_join_required")?;
        if room.state() != matrix_sdk::RoomState::Joined { return Err("room_join_required".into()); }
        let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let roster = self.execute(Command::Agents).await?;
            let candidates: Vec<Value> = roster["agents"].as_array().ok_or("room_discovery_unavailable")?.iter().filter(|agent|can_add_to_shared_room(agent)).cloned().collect();
            let projects = self.execute(Command::Projects).await?;
            let projects = projects["projects"].as_array().ok_or("room_discovery_unavailable")?;
            if projects.len() > 128 { return Err("room_discovery_incomplete".into()); }
            for project in projects {
                let Some(id) = project["id"].as_str() else {continue;};
                let rooms = self.execute(Command::ProjectRooms {project_id:id.into()}).await?;
                if rooms["rooms"].as_array().is_some_and(|rooms| rooms.iter().any(|room| room["roomId"] == room_id && room["active"] == true)) {
                    self.check()?;
                    return Ok(serde_json::json!({"roomId":room_id,"projectId":id,"agents":candidates}));
                }
            }
            Err("room_project_registration_required".into())
        }).await.map_err(|_| "room_discovery_unavailable")?;
        self.check()?;
        result
    }
    /// Read this owner's existing Room services without adopting or starting any.
    pub async fn room_agent_services(&self, room_id: &str) -> Result<Value, String> {
        self.check()?;
        let client = crate::sliding_sync::get_client().ok_or("matrix_sign_in_required")?;
        let room = client.get_room(matrix_sdk::ruma::RoomId::parse(room_id).map_err(|_| "invalid_room_id")?.as_ref()).ok_or("room_join_required")?;
        if room.state() != matrix_sdk::RoomState::Joined {return Err("room_join_required".into());}
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let roster = self.execute(Command::Agents).await?;
            let devices = self.execute(Command::Devices).await?;
            let agents = roster["agents"].as_array().ok_or("room_discovery_unavailable")?;
            if agents.len() > 128 {return Err("room_discovery_incomplete".into());}
            let mut services = Vec::new();
            for agent in agents {
                if agent["state"] != "active" {continue;}
                let Some(id) = agent["id"].as_str() else {continue;};
                let bindings = self.execute(Command::Bindings {agent_id:id.into()}).await?;
                let Some(binding) = bindings["bindings"].as_array().and_then(|rows| rows.iter().find(|b|b["roomId"] == room_id && matches!(b["state"].as_str(),Some("active"|"pending"|"suspended")))) else {continue;};
                if binding["scopeKind"] != "owner_direct" && !can_add_to_shared_room(agent) {continue;}
                let Some(binding_id) = binding["id"].as_str() else {continue;};
                let here = agent["executionDeviceId"].as_str().is_some_and(|device| devices["currentDeviceId"].as_str() == Some(device));
                let runtime = if here {
                    self.execute(Command::Local {agent_id:id.into(),action:LocalAction::AgentServiceStatus,input:None,binding_id:None,requester:None}).await
                } else {Ok(Value::Null)};
                services.push(serde_json::json!({"agentId":id,"bindingId":binding_id,"bindingState":binding["state"],"ownerServicePaused":binding["ownerServicePaused"],"service":runtime.as_ref().ok().and_then(|v|v.get("service")),"displayName":agent["displayName"],"thisDevice":here,"runtime":runtime.as_ref().ok().and_then(|v|v.get("runtime")),"statusAvailable":runtime.is_ok()}));
            }
            Ok(serde_json::json!({"roomId":room_id,"services":services}))
        }).await.map_err(|_|"room_discovery_unavailable")?;
        self.check()?;
        result
    }
    pub async fn execute(&self, command: Command) -> Result<Value, String> {
        let continuation = command.continues_running_task();
        let _update_task = if continuation { ::hagency::update_guard::PROCESS.continue_task() }
            else { ::hagency::update_guard::PROCESS.enter_task() }.ok_or("update_in_progress")?;
        self.check()?;
        if matches!(command, Command::BeginLogin) {
            return Err("matrix_oauth_sign_in_required".into());
        }
        // Logout may always stop this pinned owner even when Matrix is offline.
        if !matches!(command, Command::Logout) {
            self.verify_matrix(false).await?;
        }
        let native = {
            let mut native = self.0.native.lock().await;
            self.check()?;
            if native.is_none() {
                // A continuation must never bootstrap a new owner behind a fence.
                let _bootstrap = ::hagency::update_guard::PROCESS.enter_task().ok_or("update_in_progress")?;
                let owner = ::hagency::native_owner::open_with_matrix(
                    &self.0.state, &self.0.origin, &self.0.owner,
                    Arc::new(SdkTokens(Arc::downgrade(&self.0))),
                ).await.map_err(|e| {
                    let code = recovery_error_code(&e.code);
                    makepad_widgets::warning!("Native owner initialization failed: {code}");
                    e.code
                })?;
                // Preserve even a partially initialized owner's cleanup receipt.
                *native = Some(owner.clone());
                if let Err(error) = self.check() {
                    owner.shutdown().await;
                    return Err(error);
                }
                // Saved explicit consent is not authority: restoration invokes
                // every fresh device/binding/provider/history startup gate.
                // A failure leaves intent for review, never a blind takeover.
                log_runtime_recovery(owner.restore_runtime_intents().await);
                if let Err(error) = self.check() {
                    owner.shutdown().await;
                    return Err(error);
                }
                *native = Some(owner);
            }
            native.as_ref().unwrap().clone()
        };
        self.check()?;
        let result = native.execute(command).await.map_err(|e| e.code);
        if let Err(e) = self.check() {
            native.shutdown().await;
            return Err(e);
        }
        result
    }
    /// Dictation uses the same selected Codex account as Agent services.
    pub async fn transcribe_speech(&self, pcm: &[u8], locale: &str, context: &str, probe: bool) -> Result<String, String> {
        // Bootstrap the owner boundary without launching a possibly unselected
        // dedicated provider. NativeOwner checks the saved local choice first.
        self.execute(Command::LoginStatus).await?;
        self.check()?;
        let native = self.0.native.lock().await.as_ref().cloned().ok_or("native_owner_unavailable")?;
        let result = native.transcribe_speech(pcm, locale, context, probe).await.map_err(|error| {
            // This entry point returns static guard/transport codes only.
            makepad_widgets::log!("Local Codex dictation request failed: {}", error.code);
            error.code
        });
        self.check()?;
        result
    }
    pub async fn stream_speech(&self, audio: hagency::console::speech::Audio, locale: &str, context: &str,
        updates: hagency::console::speech::Updates) -> Result<String, String> {
        self.execute(Command::LoginStatus).await?;
        self.check()?;
        let native = self.0.native.lock().await.as_ref().cloned().ok_or("native_owner_unavailable")?;
        let service = self.clone();
        let guarded: hagency::console::speech::Updates = std::sync::Arc::new(move |event| {
            if service.check().is_ok() {updates(event);}
        });
        let result = native.stream_speech(audio, locale, context, guarded).await.map_err(|error| error.code);
        self.check()?;
        result
    }
    async fn shutdown(&self) {
        let _ = self.begin_shutdown().wait().await;
    }
    fn begin_shutdown(&self) -> ::hagency::shutdown::Task {
        self.0.shutdown.get_or_init(|| {
            self.0.retired.store(true, std::sync::atomic::Ordering::Release);
            let service = self.clone();
            let task = ::hagency::shutdown::Task::start(async move {
                let native = service.0.native.lock().await.as_ref().cloned();
                if let Some(native) = native { native.shutdown_checked().await } else { Ok(()) }
            });
            RETIREMENTS.lock().expect("retirement registry poisoned").track(task.clone());
            task
        }).clone()
    }
    /// Account transitions must call this before clearing SDK/UI state. It
    /// revokes owner authority and drains runtime/provider work before return.
    pub async fn stop_current() {
        let _ = Self::stop_current_checked().await;
    }
    pub async fn stop_current_checked() -> ::hagency::shutdown::Result {
        let _stopping = STOP_CURRENT.lock().await;
        let service = CURRENT.lock().map_err(|_| ::hagency::shutdown::Error::Worker)?.take();
        if let Some(service) = service {
            service.begin_shutdown();
        }
        let operations = {
            let mut retained = RETIREMENTS.lock().map_err(|_| ::hagency::shutdown::Error::Worker)?;
            retained.outstanding()
        };
        let mut result = Ok(());
        for operation in operations { result = result.and(operation.wait().await); }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_room_picker_excludes_private_unknown_and_inactive_agents() {
        use serde_json::json;
        for agent in [json!({}),json!({"state":"active"}),json!({"state":"active","visibility":"private"}),json!({"state":"suspended","visibility":"public"}),json!({"state":"retired","visibility":"public"})] {
            assert!(!can_add_to_shared_room(&agent));
        }
        assert!(can_add_to_shared_room(&json!({"state":"active","visibility":"public"})));
    }
    #[test]
    fn startup_recovery_diagnostic_never_logs_provider_or_source_text() {
        assert_eq!(recovery_error_code("runtime_profile_unavailable"),"runtime_profile_unavailable");
        assert_eq!(recovery_error_code("owner_authorization_required"),"owner_authorization_required");
        assert_eq!(recovery_error_code("/Users/example/.codex/auth.json bearer secret"),"runtime_recovery_unrecognized_error");
        assert_eq!(recovery_error_code("provider_error: access_token=secret"),"runtime_recovery_unrecognized_error");
    }
    #[test]
    fn waiting_deadline_accepts_reads_but_never_detaches_actions_or_recovery() {
        for command in [Command::AgentDetails {agent_id:"a".into()}, Command::Agents, Command::Projects, Command::AgentCommands,
            Command::Creations, Command::Creation {id: "original".into()},
            Command::Provider {action: ProviderAction::Models},
            Command::Binding {agent_id:"a".into(),binding_id:"b".into(),action:BindingAction::Status},
            Command::Local {agent_id:"a".into(),action:LocalAction::Runtime,input:None,binding_id:None,requester:None}] {
            assert!(management_read(&command), "{command:?}");
        }
        for command in [Command::AssignAgentToCurrentDevice {agent_id:"a".into(),expected_generation:4}, Command::CreateAgent {input:serde_json::json!({})},
            Command::BindAgent {agent_id:"a".into(),input:serde_json::json!({})},
            Command::CreateMatrix {input:serde_json::json!({})},
            Command::ResumeCreation {id:"original".into(),room_id:None},
            Command::ResumeAgentCommand {id:"original".into()},
            Command::EnsureAgentOwnerDirect {agent_id:"a".into()},
            Command::PrepareAgentWorkspace {agent_id:"a".into()},
            Command::Provider {action:ProviderAction::UseLocal}, Command::Logout,
            Command::Local {agent_id:"a".into(),action:LocalAction::RuntimeStart,input:None,binding_id:None,requester:None},
            Command::Local {agent_id:"a".into(),action:LocalAction::RuntimeStop,input:None,binding_id:None,requester:None},
            Command::Local {agent_id:"a".into(),action:LocalAction::SaveAgentPolicy,input:None,binding_id:None,requester:None},
            Command::Local {agent_id:"a".into(),action:LocalAction::Decision {approval_id:"p".into()},input:None,binding_id:None,requester:None},
            Command::Local {agent_id:"a".into(),action:LocalAction::Runtime,input:Some(serde_json::json!({})),binding_id:None,requester:None}] {
            assert!(!management_read(&command), "{command:?}");
        }
    }
}
