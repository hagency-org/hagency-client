//! Native owner management. Server identities and local execution policy are
//! separate controls; no web console, transfer or administrator resource gate.
use makepad_widgets::*;
use serde_json::{Value, json};
use super::{backend::Service, model::*, approvals::Notices};
use std::{cell::Cell, collections::HashSet, time::Instant};
use crate::sliding_sync::spawn_async_task;
use super::board::ProjectBoardWidgetExt;
use crate::shared::{feedback::{InlineNoticeWidgetExt,InlineNoticeWidgetRefExt}, popup_list::PopupKind};
mod ledger;
mod usage;
mod history;
mod file_history;
mod recovery;
mod crypto;
mod project_navigation;
mod model_resources;
pub mod approval_discovery;

#[derive(Clone, Debug)]
pub enum HagencyAction {
    OpenProjects,
    NewProject,
    NewRoom {project_id:String},
    ProjectBoard {project_id: String, board_id: String},
    ProjectDetails {
        project_id: String,
    },
    ProjectSection {
        project_id: String,
        section: ProjectDetailSection,
    },
    OpenAgents,
    AgentRun {agent_id:String,binding_id:String},
    ReviewApproval(approval_discovery::ApprovalTarget),
    NewAgent {
        project_id: Option<String>,
    },
    Close,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectDetailSection {
    Board,
    Access,
    Overview,
    Rooms,
    Members,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Page {
    #[default]
    Projects,
    Agents,
}
#[derive(Clone, Copy, Debug)]
enum Target {
    Stopped,RecoveryReview,RecoveryDecision,
    History,
    HistoryDetail,
    Files,
    FileDetail,
    Usage,
    Ledger,
    Journal,
    Projects,
    ProjectHome,
    BoardCreated,
    ProjectArchived,
    NavigationDefaultChanged,
    Rooms,
    Spaces,
    Agents,
    Bindings,
    VisibilityChanged,
    BindingChanged,
    ReplyPolicyChanged,
    RoomRoster,
    RoomCapabilities,
    ScopeState,
    AgentJournal,
    AgentResumed,
    AgentCommandChecked,
    AgentCommandDismissed,
    AgentDetails,
    AgentAssigned,
    Devices,
    OwnerDirect,
    OwnerDirectCreated,
    ProviderModels,
    ConfigurationStatus,
    ExecutionEnvironment,
    WorkspacePrepared,
    ScopeChanged,
    Policy,
    ModelSaved,
    Creation,
    Provider,
    Runtime,
    ListService,
    ServiceRoomChanged,
    ApprovalDecision,
    RemoteApproval,
    RemoteApprovalDecision,
    RemoteInbox,
    RemoteVerdict,
    Crypto,
    Members,
    Changed,
    ProjectAdopted,
    RoomAdopted,
    AgentCreated,
    AgentBound,
}
#[derive(Debug)]
struct Completed {
    identity: String,
    request: u64,
    target: Target,
    configuration_fence: Option<(String, i64, String)>,
    configuration_digest: Option<String>,
    result: Result<Value, String>,
}
#[derive(Debug)]
struct ConfigurationCompleted {identity:String,request:u64,agent:String,generation:i64,device:String,result:Result<Value,String>}
#[derive(Debug)]
struct ApprovalsCompleted { identity:String, request:u64, agent:String, binding:String, room:String, result:Result<Value,String> }

type ServiceRowTargets = std::collections::HashMap<WidgetUid,(String,String)>;
type AgentCardTargets = std::collections::HashMap<WidgetUid,String>;
type ConfigurationKeys = std::collections::HashMap<String, String>;
type RuntimeStates = std::collections::HashMap<String,Result<Value,String>>;
type ServiceErrors = std::collections::HashMap<String,String>;
#[derive(Debug)]
struct RuntimeListCompleted { identity:String, request:u64, states:Vec<(String,Result<Value,String>)> }
pub(crate) fn runtime_list_label(runtime:Option<&Result<Value,String>>,agent:&str)->&'static str {
    let Some(runtime)=runtime else {return "Checking local processing…";};
    let Ok(value)=runtime else {return "Local processing could not be verified. Refresh to retry.";};
    if service_needs_model(value) {return "No model is configured on this device. Open Model & resources, connect an account and save a model before starting.";}
    if value["service"]["enabled"] == false && value["runtime"].is_null() {return "Not running on this device. Open Service to configure and start.";}
    if value["service"]["enabled"] == true && (value["service"]["watcherRunning"] == false || !string(&value["service"],"lastError").is_empty()) {return "Service needs attention. Refresh status or stop and start again.";}
    if !array(&value["service"],"problems").is_empty() {return "Some Rooms could not start. The service will retry automatically.";}
    if value["service"]["enabled"] == true && value["runtime"].is_null() {return if value["service"]["phase"] == "starting" {"Starting service. Discovering connected Rooms…"} else {"Service enabled. Waiting for an available Room."};}
    let r=&value["runtime"];
    if r.is_null() {return "Not running on this device. Open Service to configure and start.";}
    if string(r,"agentId")!=agent {return "Local processing could not be verified. Refresh to retry.";}
    let bindings=array(r,"bindings");
    let phases=if bindings.is_empty() {vec![string(r,"phase")]} else {bindings.iter().map(|b|string(b,"phase")).collect::<Vec<_>>()};
    if string(r,"phase")=="waiting_for_lease" || phases.contains(&"waiting_for_lease") {"Waiting for the previous runtime lease to be released…"}
    else if phases.iter().any(|p|matches!(*p,"executing"|"executing_chat_only"|"executing_host_files")) {"Processing requests on this device."}
    else if phases.iter().any(|p|matches!(*p,"online"|"online_chat_only")) {if value["service"]["enabled"] == false {"Running in previously selected Rooms. Stop and start to enable automatic Room discovery."} else {"Running in all connected Rooms on this device. New Rooms are included automatically; configured budgets and policy permissions still apply."}}
    else if phases.iter().any(|p|matches!(*p,"awaiting_approval"|"waiting_owner")) {"Waiting for your approval before continuing."}
    else if phases.iter().any(|p|matches!(*p,"ledger_recovery_required"|"reconciliation_only")) {"Restore this owner's complete ledger before new requests. Only known original replies may be recovered."}
    else if !string(r,"lastError").is_empty() || bindings.iter().any(|b|!string(b,"lastError").is_empty()) {"The runtime stopped with an error. Check the details below."}
    else if phases.contains(&"checking_provider") {"Checking this device's local runtime connection…"}
    else if phases.contains(&"starting") {"Starting service. Discovering connected Rooms…"}
    else if phases.iter().all(|p|matches!(*p,"stopped"|"")) {"Not running on this device. Open Service to configure and start."}
    else {"The runtime has not confirmed it is ready. Refresh its status."}
}
fn service_needs_model(value:&Value)->bool {
    !service_enabled(value) && value["service"]["modelConfigured"] == false
}
fn runtime_diagnostic(code: &str) -> String {
    // Use fixed explanations for runtime failures; other machine codes go
    // through the same validation as management errors.
    let message = match code {
        "agent_model_required" => "No model is configured on this device. Open Model & resources, connect an account and save a model before starting.",
        "runtime_profile_unavailable" => "The saved local model, workspace or account is unavailable. Open Model & resources, reconnect the account and save an available model.",
        "codex_programming_rules_unsupported" => "Programming cannot inherit Codex execution rules. Use a separate Codex sign-in and a workspace without .codex/rules.",
        "codex_provider_configuration_rejected" => "The local provider configuration failed validation. Open Service logs to identify the stage; check the provider version and configuration.",
        "codex_web_search_unavailable" => "The local provider does not support the selected web search settings. Check its version and Model & resources.",
        "codex_protocol_mismatch" => "The local provider returned an incompatible protocol response. Check its version and Service logs.",
        "codex_provider_unavailable" => "The local provider could not start or complete its handshake. Check its installation and Service logs.",
        "codex_provider_timeout" => "The local provider did not respond in time. Check Service logs and retry.",
        "provider_profile_mismatch" | "provider_authorization_lost" => "The connected account changed or expired. Reconnect it in Model & resources and save the model again.",
        "owner_authorization_required" => "Your account authorization expired. Sign in again to continue.",
        "matrix_authorization_unavailable" | "server_unavailable" => "Cannot verify this account right now. Check the server connection and retry.",
        "agent_runtime_already_active" => "A service is already active for this Agent. Refresh its status before starting again.",
        "runtime_stopped" | "execution_not_running" => "This service has stopped. Refresh its status, check Model & resources and start again.",
        "agent_lease_busy" => "The previous runtime lease is still active. Wait briefly and retry.",
        "authorization_or_lease_lost" => "Runtime authorization or its lease was lost. Stop the service, check your account and device assignment, then start again.",
        "device_transport_unavailable" => "The runtime could not reach the server. Check the connection; the service will retry automatically.",
        "agent_process_closing" => "The previous runtime is still stopping. The service will retry automatically.",
        "agent_execution_device_required" => "Assign this Agent to this device before starting it here.",
        "ledger_recovery_required" => "Restore this owner's complete ledger before new requests. Only known original replies may be recovered.",
        "room_worker_stopped" | "runtime_start_failed" => "A Room worker stopped before becoming ready. The service will retry automatically.",
        _ => return visible_error(code),
    };
    format!("{} ({code})", crate::i18n::tr(message))
}
pub(crate) fn runtime_service_description(runtime: Option<&Result<Value,String>>, agent: &str) -> String {
    let mut text=crate::i18n::tr(runtime_list_label(runtime,agent)).to_owned();
    let Some(runtime)=runtime else {return text;};
    let value=match runtime {Ok(value)=>value,Err(code)=>{text.push_str(&format!("\n{}",runtime_diagnostic(code)));return text;}};
    let mut diagnostics=Vec::new();
    let mut add=|code:&str| {if !code.is_empty() {let message=runtime_diagnostic(code);if !diagnostics.contains(&message) {diagnostics.push(message);}}};
    add(string(&value["service"],"lastError"));
    // A mismatched runtime must never supply another Agent's diagnostics.
    if string(&value["runtime"],"agentId")==agent {
        add(string(&value["runtime"],"lastError"));
        for binding in array(&value["runtime"],"bindings") {add(string(&binding,"lastError"));}
    }
    for problem in array(&value["service"],"problems") {
        let mut message=runtime_diagnostic(string(&problem,"error"));
        if let Some(seconds)=problem["retryInSeconds"].as_u64() {message.push_str(&format!(" · {} {seconds}s",crate::i18n::tr("Retry in")));}
        let base=runtime_diagnostic(string(&problem,"error"));
        diagnostics.retain(|entry|entry != &base);
        if !diagnostics.contains(&message) {diagnostics.push(message);}
    }
    for message in diagnostics {text.push_str(&format!("\n{message}"));}
    text
}
fn remote_presence_label(status:Option<&Result<Value,String>>,agent:&Value)->&'static str {
    let Some(Ok(value))=status else {return "Device status unknown";};
    if value["agentId"]==agent["id"] && value["agentGeneration"]==agent["generation"] && value["executionDeviceId"]==agent["executionDeviceId"]
        && value["deviceGeneration"].is_null() && value["online"]==false {return "Device unavailable · no active device session";}
    if !configuration_matches(value,agent) {return "Device status unknown";}
    if value["online"]==true {"Device online · Agent readiness is separate"}
    else if value["lastSeenAtMs"].is_null() {"No device heartbeat observed"}
    else {"Device offline · no recent heartbeat"}
}
fn service_log_text(value:&Value)->String {
    let mut lines=Vec::new();
    for log in array(&value["service"],"logs").iter().rev().take(100) {
        let at=log["atMs"].as_i64().and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis).map(|t|t.format("%Y-%m-%d %H:%M:%S UTC").to_string()).unwrap_or_default();
        let phase=match string(log,"phase") {"starting"=>"Starting service…","serving"=>"Service running","retrying"=>"Waiting to retry","disconnected"=>"Disconnected","stopped"=>"Stopped",_=>"Status updated"};
        let mut line=format!("{at} · {}",crate::i18n::tr(phase));
        let binding=string(log,"bindingId");
        if !binding.is_empty() {line.push_str(&format!(" · Room {}",binding.chars().take(128).collect::<String>()));}
        let code=string(log,"error");
        if !code.is_empty() {line.push_str(&format!("\n{}",runtime_diagnostic(code)));}
        if let Some(seconds)=log["retryInSeconds"].as_u64() {line.push_str(&format!(" · {} {seconds}s",crate::i18n::tr("Retry in")));}
        lines.push(line);
    }
    if lines.is_empty() {crate::i18n::tr("No Service events recorded in this session yet. Refresh status after starting the service.").to_owned()} else {lines.join("\n\n")}
}
fn service_enabled(value:&Value)->bool {
    value["service"]["enabled"] == true || {
        let runtime=&value["runtime"];
        matches!(string(runtime,"phase"),"online"|"online_chat_only"|"executing"|"executing_chat_only"|"executing_host_files"|"awaiting_approval"|"waiting_owner"|"waiting_for_lease")
    }
}
const MANAGEMENT_READ_WAIT: std::time::Duration = std::time::Duration::from_secs(45);
struct ReadWait {
    identity: String,
    request: u64,
    deadline: std::time::Instant,
}
fn nonce() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn current_completion(active: bool, identity: &str, request: u64, done: &Completed) -> bool {
    active && done.identity == identity && done.request == request
}
// This whitelist is deliberately narrower than local management. Credentials and
// Room/requester policy remain managed only on the execution device.
fn remote_configuration_command(command: &Command) -> bool {
    matches!(command,
        Command::Local {action: LocalAction::AgentModel | LocalAction::AgentPolicy
            | LocalAction::SaveAgentModel | LocalAction::SaveAgentPolicy,
            binding_id: None, requester: None, ..}
        | Command::PrepareAgentWorkspace {..}
        | Command::Provider {action: ProviderAction::Status | ProviderAction::Models | ProviderAction::UseLocal}
        | Command::RuntimeProvider {action: ProviderAction::Status | ProviderAction::Models | ProviderAction::UseLocal, ..})
}
fn configuration_outcome_unconfirmed(error: &str) -> bool {
    matches!(error,"invalid_configuration_response"|"configuration_outcome_unknown"|"configuration_request_unknown"|"configuration_request_timeout"|"server_unavailable"|"device_transport_unavailable"|"matrix_authorization_unavailable")
}
fn configuration_connection_lost(error: &str) -> bool {
    matches!(error,"configuration_device_offline"|"configuration_device_unavailable"|"execution_device_offline"|"configuration_target_changed"|"server_unavailable"|"device_transport_unavailable"|"matrix_authorization_unavailable"|"device_authorization_required")
}
fn configuration_matches(configuration: &Value, agent: &Value) -> bool {
    configuration["agentId"] == agent["id"] && configuration["agentGeneration"] == agent["generation"]
        && configuration["executionDeviceId"] == agent["executionDeviceId"]
        && configuration["deviceGeneration"].as_i64().is_some_and(|generation| generation > 0)
}
fn policy_matches(policy: &Value, agent: &str, binding: &str, requester: &str) -> bool {
    string(policy, "agentId") == agent
        && string(policy, "bindingId") == binding
        && string(policy, "requester") == requester
}
fn editor_matches(
    policy: &Value,
    agent: &str,
    binding: Option<&str>,
    requester: &str,
    layer: usize,
) -> bool {
    if layer == 0 {
        string(policy, "agentId") == agent && policy["scope"] == "agent"
    } else {
        binding.is_some_and(|binding| policy_matches(policy, agent, binding, requester))
    }
}
fn members_match(target: &Option<(String, bool)>, value: &Value) -> bool {
    target.as_ref().is_some_and(|(room, space)| {
        string(value, "roomId") == room && value["space"].as_bool() == Some(*space)
    })
}
fn agent_command_label(command:&Value)->String {
    if string(command,"operation") == "agent.create" {
        crate::i18n::format("Create {name}",&[("name",string(&command["input"],"displayName").into())])
    } else {crate::i18n::format("Add Room {room}",&[("room",string(&command["input"],"roomId").into())])}
}
fn agent_command_details(command:&Value)->String {
    match string(command,"state") {
        "rejected" => crate::i18n::tr(user_error(string(command,"errorCode"))).into(),
        "pending" => crate::i18n::tr("The server is still setting this up. Check again in a moment. You can create other Agents meanwhile.").into(),
        _ => crate::i18n::tr("We could not confirm the result. Check status first, or retry this same request safely. You can create other Agents meanwhile.").into(),
    }
}
// Only these static native error codes are safe to expose as diagnostics.
// Never render a provider's raw JSON, stderr, filesystem paths, or credentials.
fn provider_error_message(error: &str) -> Option<&'static str> {
    Some(match error {
        "claude_binary_unavailable" => "Claude Code is not installed or could not be started. Check the local installation.",
        "octos_binary_unavailable" => "Octos is not installed or could not be started. Check the local installation.",
        "claude_account_unavailable" | "local_provider_not_signed_in" => "Check your local Claude sign-in, then connect the account again.",
        "local_provider_home_unavailable" => "The local runtime's account directory is unavailable. Check its local setup.",
        "octos_profiles_unavailable" => "No usable Octos profiles were found. Configure a local Octos profile, then refresh.",
        "provider_authorization_lost" => "The local runtime's account or configuration is unavailable. Check Model & resources and reconnect.",
        "local_provider_login_managed_locally" => "Sign in through the local runtime, then connect it here.",
        "codex_binary_unavailable" => "Codex is not installed or could not be started. Check the local Codex installation.",
        "local_codex_home_unavailable" | "provider_directory_unavailable" | "provider_directory_not_canonical" => "Codex's local account or workspace directory is unavailable. Check its local permissions and retry.",
        "local_codex_configuration_unsupported" | "provider_file_credentials_or_config_rejected" => "This Codex configuration cannot be safely used by Hagency. Your existing account has not been changed. Check the configuration or use a separate Codex account.",
        "provider_profile_mismatch" | "provider_choice_unavailable" => "The saved local account selection is unavailable or does not match this profile. Reconnect the intended account and retry.",
        "local_codex_account_not_signed_in" => "Codex is not signed in. Sign in through your local Codex, then check the account again.",
        "provider_keyring_unavailable" => "Codex cannot access the system credential store. Unlock it or check its permissions, then retry.",
        "codex_model_catalog_unavailable" => "Codex could not load its model list. Check the account and connection, then refresh the models.",
        "codex_provider_timeout" | "codex_provider_unavailable" => "Codex did not respond. Check that it can run locally, then retry.",
        "codex_protocol_mismatch" => "This Codex version returned an unsupported response. Check the installed Codex version and retry.",
        "codex_provider_request_rejected" => "Codex rejected this request. Check the local account and configuration before retrying.",
        "provider_login_callback_unavailable" | "provider_authorization_url_rejected" | "provider_login_method_rejected" => "The separate Codex sign-in could not be started safely. Retry or connect your signed-in local Codex account.",
        "shared_codex_login_is_managed_locally" | "shared_codex_logout_is_managed_locally" => "This account is managed by your local Codex. Use Disconnect here to keep your everyday Codex signed in.",
        "profile_or_local_state_unavailable" => "The saved model or workspace is unavailable. Check this Agent's local resources and save them before starting.",
        _ => return None,
    })
}
fn visible_error(error: &str) -> String {
    let message = crate::i18n::tr(user_error(error));
    // Match the owner API's bounded machine-code contract. Never render raw
    // response bodies, provider stderr, paths or credential-bearing details.
    if !error.is_empty() && error.len() <= 80
        && error.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
        format!("{message} ({error})")
    } else {
        message.to_owned()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentDeviceState { Here, Other, Unassigned, Unavailable, Checking }
fn agent_device_state(agent: &Value, current: &str, devices: &[Value], loaded: bool) -> AgentDeviceState {
    if !loaded || current.is_empty() || agent["generation"].as_i64().is_none_or(|generation|generation <= 0) {return AgentDeviceState::Checking;}
    let Some(device) = agent["executionDeviceId"].as_str().filter(|id|!id.is_empty()) else {return AgentDeviceState::Unassigned;};
    let Some(record) = devices.iter().find(|d|string(d,"id") == device) else {return AgentDeviceState::Unavailable;};
    if record["revoked"] != false {return AgentDeviceState::Unavailable;}
    if device == current {AgentDeviceState::Here} else {AgentDeviceState::Other}
}
fn execution_device_name(devices: &[Value], id: &str) -> String {
    let name = devices.iter().find(|d| string(d,"id") == id).map(|d|string(d,"name").trim()).unwrap_or("");
    let ambiguous = matches!(name, "" | "Hagency" | "Hagency Desktop")
        || devices.iter().filter(|d| string(d,"name").trim() == name).count() > 1;
    let short_id = id.chars().rev().take(8).collect::<String>().chars().rev().collect::<String>();
    let display = name.chars().take(36).collect::<String>();
    let display = if name.chars().count() > 36 {format!("{display}…")} else {display};
    if name.is_empty() {short_id} else if ambiguous {format!("{display} · {short_id}")} else {display}
}
fn render_device_badge(cx: &mut Cx, badge: &WidgetRef, state: AgentDeviceState, text: &str) {
    let mut badge = badge.clone();
    badge.label(cx, ids!(device_label)).set_text(cx, text);
    match state {
        AgentDeviceState::Here => {
            script_apply_eval!(cx, badge, {draw_bg +: {color: mod.widgets.RINX_SUCCESS_BG}});
            let mut label = badge.label(cx, ids!(device_label));
            script_apply_eval!(cx, label, {draw_text +: {color: mod.widgets.RINX_SUCCESS_FG}});
        }
        AgentDeviceState::Other => {
            script_apply_eval!(cx, badge, {draw_bg +: {color: mod.widgets.RINX_SELECTED}});
            let mut label = badge.label(cx, ids!(device_label));
            script_apply_eval!(cx, label, {draw_text +: {color: mod.widgets.RINX_ACCENT}});
        }
        AgentDeviceState::Unassigned | AgentDeviceState::Unavailable => {
            script_apply_eval!(cx, badge, {draw_bg +: {color: mod.widgets.RINX_WARNING_BG}});
            let mut label = badge.label(cx, ids!(device_label));
            script_apply_eval!(cx, label, {draw_text +: {color: mod.widgets.RINX_WARNING_FG}});
        }
        AgentDeviceState::Checking => {
            script_apply_eval!(cx, badge, {draw_bg +: {color: mod.widgets.RINX_HOVER}});
            let mut label = badge.label(cx, ids!(device_label));
            script_apply_eval!(cx, label, {draw_text +: {color: mod.widgets.RINX_MUTED}});
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct ProjectRoomNavigation {
    pub identity: String,
    pub project_id: String,
}
impl ProjectRoomNavigation {
    pub(crate) fn current(&self) -> bool {
        Service::for_current().is_ok_and(|service| service.identity() == self.identity)
    }
}
fn navigate_agent_chat(cx: &mut Cx, room: matrix_sdk::ruma::OwnedRoomId, owner_direct: bool, project: Option<(&str, &str)>) {
    if !owner_direct && let Some((identity, project_id)) = project {
        cx.action(ProjectRoomNavigation {identity: identity.into(), project_id: project_id.into()});
        cx.action(crate::home::project_tree::ProjectSelectionChanged {identity:identity.into(),project_id:project_id.into(),target:crate::home::project_tree::ProjectSelection::Room(room.clone())});
    }
    let name = crate::sliding_sync::get_client().and_then(|client| client.get_room(&room))
        .and_then(|sdk_room| sdk_room.cached_display_name().or_else(|| sdk_room.name().map(matrix_sdk::RoomDisplayName::Named)))
        .map(|name| crate::utils::RoomNameId::new(name, room.clone()))
        .or_else(|| if cx.has_global::<crate::home::rooms_list::RoomsListRef>() {
            cx.global::<crate::home::rooms_list::RoomsListRef>().get_room_name(&room)
        } else {None})
        .unwrap_or_else(|| crate::utils::RoomNameId::empty(room));
    if owner_direct {crate::home::mobile::navigate_direct(cx,name);return;}
    cx.action(crate::app::AppStateAction::NavigateToRoom {
        room_to_close: None,
        destination_room: crate::room::BasicRoomDetails::RoomId(name),
    });
}
fn audience_label(agent: &Value) -> &'static str {
    match agent["visibility"].as_str() {
        Some("public") => "Public service",
        Some("private") => "Private · owner chat only",
        _ => "Service audience unavailable. Update the server and refresh.",
    }
}
fn project_empty_message(loaded:bool,loading:bool)->&'static str {
    crate::i18n::tr(if loading {"Loading projects…"}else if !loaded {"Projects could not be loaded. Refresh to retry."}else{"No Projects yet. Create a Project to start a shared workspace."})
}
fn agent_empty_message(loaded: bool, loading: bool) -> &'static str {
    if loading { crate::i18n::tr("Loading your Agents…") }
    else if !loaded { crate::i18n::tr("Your Agent list could not be loaded. Refresh to retry.") }
    else { crate::i18n::tr("No Agents yet. Create an Agent, then configure this device and add it to Rooms.") }
}
fn user_error(error: &str) -> &str {
    if let Some(message) = provider_error_message(error) { return message; }
    match error {
        "remote_approval_waiting" => "Waiting for a verified decision.",
        "remote_approval_incomplete" => "Decision history is incomplete. No approval was applied.",
        "remote_approval_transport_unavailable" => "Remote approvals require the current Matrix client.",
        "remote_approval_unavailable" | "remote_approval_unverified" => "This remote request could not be verified. Refresh its original request before deciding.",
        "remote_approval_busy" => "Private approvals are busy. Wait for the original operations to finish.",
        "remote_approval_expired" => "This approval request has expired.",
        "recovery_stop_unconfirmed" => "No observed provider stop is recorded. Continuation is unavailable.",
        "recovery_runtime_busy" => "Stop this Agent's services before inspecting or deciding a stopped request.",
        "recovery_review_changed" => "The recovery review expired or the files changed. Inspect the original request and files again.",
        "continuation_route_unavailable" => "The Agent or original Room binding is no longer available for continuation.",
        "continuation_transport_unavailable" => "Continuation requires the current signed-in Matrix client.",
        "invalid_history_query" => "The approval history selection is no longer valid. Refresh the Agent history.",
        "file_history_upgrade_required" => "This Server needs the original file history upgrade before receipts can be refreshed.",
        "file_history_changed" => "Original file evidence conflicts with this receipt. Refresh the list; the last observed status is preserved.",
        "invalid_file_history_query" => "Select an original file delivery from this Agent history.",
        "file_history_unavailable" => "File receipts could not be refreshed. The last observed status is preserved.",
        "owner_history_unavailable" => "Approval history could not be read from this device's ledger. Check local storage and retry.",
        "invalid_usage_query" => "Select the scope and enter valid UTC dates (YYYY-MM-DD). Daily reports support up to 366 days; monthly reports support up to 10 years.",
        "usage_report_unavailable" => "Usage could not be read from this device's ledger. Check local storage and retry.",
        "ledger_runtime_busy" => "Stop all Agent services on this device before restoring the owner ledger.",
        "ledger_restore_requires_empty_ledger" => "This device already has owner ledger records. Restore cannot overwrite usage or requests; use an empty ledger on a recovery device.",
        "ledger_backup_changed" => "The backup changed after inspection. Check the backup again before restoring.",
        "ledger_backup_owner_mismatch" => "This backup belongs to a different account or server profile.",
        "ledger_backup_schema_unsupported" => "This backup uses an unsupported ledger schema. Use a compatible client version.",
        "invalid_ledger_backup" => "The backup is damaged or incomplete. Choose a valid owner ledger backup.",
        "invalid_backup_path" | "backup_destination_unavailable" => "Choose a readable backup or enter a new absolute export path. Existing files are never overwritten.",
        "ledger_backup_unavailable" => "The ledger backup operation could not finish. Check available disk space and file permissions.",
        "invalid_agent_name" => "The Agent name must contain 1–64 characters and no control characters.",
        "invalid_arguments" => "The request contains invalid fields. Check the name and selected options; if they are valid, check that the client and server versions are compatible.",
        "idempotency_key_reused" => "This request identifier was already used with different settings. Refresh Agents and check the original request before creating again.",
        "agent_command_state_unavailable" | "local_state_unavailable" | "native_state_unavailable" => "Could not read or save local request state. Check local storage permissions and available disk space, then retry the original request.",
        "invalid_server_response" | "invalid_native_response" => "The server or local service returned an unexpected response. Check that the client and server versions are compatible, then refresh the original request's status.",
        "appservice_startup_not_ready" | "appservice_roundtrip_unconfirmed" => "The server's Agent service is not ready. Check its appservice registration, Matrix connection and startup logs, then retry the original request.",
        "storage_unavailable" => "The server could not access its database. Check the server logs and database connection, then refresh the original request's status.",
        "domain_unavailable" => "Agent management is unavailable on this server. Check its Agent service configuration and restart the server if needed.",
        "server_request_failed" | "native_operation_failed" => "The service rejected the operation without a usable error code. Check the service logs and refresh the original request's status.",
        "agent_model_required" => "No model is configured on this device. Open Model & resources, connect an account and save a model before starting.",
        "runtime_profile_unavailable" => "The saved local model, workspace or account is unavailable. Open Model & resources, reconnect the account and save an available model.",
        "provider_profile_mismatch" | "provider_authorization_lost" => "The connected account changed or expired. Reconnect it in Model & resources and save the model again.",
        "owner_authorization_required" => "Your account authorization expired. Sign in again to continue.",
        "configuration_unsupported" | "unsupported_hagency_capabilities" => "This server does not support execution device configuration. Upgrade the server and execution device client, then refresh.",
        "server_unavailable" | "configuration_local_state_unavailable" => "Could not reach execution device configuration. Check your connection and refresh.",
        "configuration_authority_changed" | "configuration_authority_invalidated" => "Account or device authorization changed. Sign in again and reopen this Agent.",
        "configuration_device_offline" | "execution_device_offline" | "configuration_device_unavailable" => "Execution device offline. Cached settings are read-only.",
        "invalid_configuration_response" | "configuration_outcome_unknown" | "configuration_request_unknown" | "configuration_request_expired" | "configuration_request_timeout" => "Application result is unknown. Refresh the execution device settings before retrying; the original request is retained.",
        "configuration_request_busy" | "configuration_runtime_busy" | "configuration_queue_full" => "The execution device is busy. Stop its active service there or wait for the current configuration request.",
        "configuration_generation_conflict" | "configuration_assignment_changed" | "configuration_target_changed" => "Execution device assignment changed. Reopen this Agent.",
        "configuration_model_unavailable" | "configuration_provider_unavailable" => "The selected model, profile or reasoning effort is unavailable on the execution device. Refresh its environment and choose an available option.",
        "invalid_model_profile" | "invalid_local_model" | "workspace_not_canonical" | "workspace_root_invalid" => "The model profile or workspace is invalid on the execution device. Choose its catalog entry and an existing absolute directory, or leave the workspace empty.",
        "invalid_local_policy" => "The execution device rejected the workspace or policy settings. Custom workspaces must be existing absolute directories; check token allowances and exact tool and directory rules.",
        "policy_revision_conflict" | "local_revision_conflict" => "The Agent policy changed. Reload the execution device settings before saving again.",
        "agent_lease_busy"=>"The previous runtime lease is still active. Wait briefly and retry.",
        "room_join_required" => {
            "Join this Space or Room first. Discussion Rooms have independent membership."
        }
        "agent_command_recovery_required" => "A matching request already exists. Open My Agents and check that request; other Agent names can still be created.",
        "agent_command_outcome_unknown" => "This request may have reached the server. Check its status or retry the same request before dismissing it.",
        "management_request_unavailable" => "This management read is still waiting. You can refresh to retry; any late response from this view will be ignored.",
        "private_agent_owner_direct_only" => "Private Agents can only chat with you. Publish this Agent before adding a shared Room.",
        "leave_shared_rooms_before_private" => "Leave every shared Room before switching to private. Your private chat remains available.",
        "members_unavailable" => "Could not load members. Check your connection and retry.",
        "forbidden" | "creation_not_allowed" | "project_forbidden" => {
            "You do not have permission for this action in this Project."
        }
        "project_not_found" | "room_not_found" | "room_kind_mismatch" => {
            "This Project or Room is no longer available. Refresh the list."
        }
        "device_authorization_required" | "local_access_required" => {
            "Your account authorization expired. Sign in again to continue."
        }
        _ => "This action could not be completed. Refresh and retry.",
    }
}
fn member_summary(member: &Value) -> String {
    let state = if string(member, "membership") == "invite" {
        "Invited"
    } else {
        "Joined"
    };
    let role = match string(member, "role") {
        "Creator" => "Creator",
        "Administrator" => "Administrator",
        "Moderator" => "Moderator",
        _ => "Member",
    };
    format!("{} · {}", crate::i18n::tr(state), crate::i18n::tr(role))
}
fn mutation_refresh(target: Target) -> Option<Command> {
    match target {
        Target::ProjectAdopted | Target::RoomAdopted => Some(Command::Projects),
        Target::AgentCreated | Target::AgentBound => Some(Command::Agents),
        _ => None,
    }
}
fn recovery_command(input: Value, phase: &str, room: String) -> Command {
    if phase == "unrecorded" {
        Command::CreateMatrix { input }
    } else {
        Command::ResumeCreation {
            id: string(&input, "commandId").into(),
            room_id: (!room.is_empty()).then_some(room),
        }
    }
}
// PortalList requests indices beyond its logical range to fill its viewport.
// Only the real empty-state index carries text; all other entries are blank.
fn card_template(count: usize, index: usize) -> LiveId {
    if index < count {
        id!(Card)
    } else if count == 0 && index == 0 {
        id!(Empty)
    } else {
        id!(Filler)
    }
}
fn array(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    mod.widgets.ICON_PROJECT_ARCHIVE = crate_resource("self://resources/icons/archive_box.svg")
    let Text = RinxLabel {width: Fill height: Fit flow: Flow.Right{wrap: true} draw_text +: {color: mod.widgets.RINX_INK text_style: theme.font_regular{font_size: mod.widgets.RINX_BODY_SIZE}}}
    let Input = RobrixTextInput {
        width: Fill height: mod.widgets.RINX_CONTROL_HEIGHT margin: 0 padding: Inset{left: 12 right: 12 top: 8 bottom: 8}
        draw_text.text_style: theme.font_regular{font_size: mod.widgets.RINX_UI_SIZE}
    }
    let Select = RobrixSettingsDropDown {
        width: Fill height: mod.widgets.RINX_CONTROL_HEIGHT margin: 0
        popup_menu_position: PopupMenuPosition.BelowInput
        draw_text.text_style: theme.font_regular{font_size: mod.widgets.RINX_UI_SIZE}
        popup_menu: RobrixSettingsPopupMenu {
            width: 360
            menu_item: RobrixSettingsPopupMenuItem {
                height: mod.widgets.RINX_CONTROL_HEIGHT
                draw_text.text_style: theme.font_regular{font_size: mod.widgets.RINX_UI_SIZE}
            }
        }
    }
    let Action = RobrixNeutralIconButton {
        height: RINX_BUTTON_HEIGHT padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
        icon_walk: Walk{width: 0 height: 0} spacing: 0
    }
    let Primary = RobrixIconButton {
        height: RINX_BUTTON_HEIGHT padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
        icon_walk: Walk{width: 0 height: 0} spacing: 0
    }
    let Danger = RobrixNegativeIconButton {
        height: RINX_BUTTON_HEIGHT padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
        icon_walk: Walk{width: 0 height: 0} spacing: 0
    }
    let Section = RoundedView {width: Fill height: Fit flow: Down padding: 16 spacing: 10
        draw_bg +: {color: mod.widgets.RINX_SURFACE border_color: mod.widgets.RINX_BORDER border_size: 1 border_radius: 8}
    }
    let DeviceBadge = RoundedView {width: Fit height: 24 align: Align{y:0.5} padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
        draw_bg +: {color: mod.widgets.RINX_HOVER border_radius: 8}
        device_label := RinxLabel {width: Fit height: Fit padding: 0 draw_text +: {color: mod.widgets.RINX_MUTED text_style: theme.font_regular{font_size: mod.widgets.RINX_META_SIZE}}}
    }
    let OverviewMetric = RinxCard {width:Fill height:100 padding:16 flow:Down spacing:10
        caption := RinxHint {width:Fill max_lines:1 text_overflow:Ellipsis}
        value := Text {text:"—" draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_DISPLAY_SIZE}}
    }
    let Cards = PortalList {width: Fill height: Fill
        Card := RoundedView {width: Fill height: Fit flow: Right padding: 16 spacing: 16 margin: Inset{right: 20 bottom: 12} align: Align{y: 0.5}
            draw_bg +: {color: mod.widgets.RINX_SURFACE border_color: mod.widgets.RINX_BORDER border_size: 1 border_radius: 8}
            content := View {width: Fill height: Fit flow: Down spacing: 6
                title := Text {draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                badge_row := View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 6
                    audience_badge := DeviceBadge {visible: false}
                    device_badge := DeviceBadge {visible: false}
                    runtime_badge := RinxGhostButton {
                        visible: false width: Fit height: 24 padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
                        spacing: 0 icon_walk: Walk{width: 0 height: 0}
                        draw_bg +: {color: RINX_HOVER color_hover: RINX_PRESSED border_radius: 8}
                        draw_text +: {color: RINX_MUTED text_style: theme.font_regular{font_size: RINX_META_SIZE}}
                    }
                }
                summary := Text {draw_text +: {color: mod.widgets.RINX_MUTED text_style: theme.font_regular{font_size: mod.widgets.RINX_BODY_SIZE}}}
            }
            service_control := RobrixNeutralIconButton {
                height: RINX_BUTTON_HEIGHT padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
                visible: false spacing: 6 icon_walk: Walk{width: 16 height: 16} draw_icon.svg: ICON_SERVICE_PLAY text: "Start"
            }
            open := Action {text: #(crate::i18n::tr("View details")) i18n_text: "View details"}
        }
        Filler := View {width: Fill height: 64}
        Empty := View {width: Fill height: 64 padding: 16 margin: Inset{right: 20}
            empty_message := Text {text: #(crate::i18n::tr("No Projects yet. Create a Project to start a shared workspace.")) i18n_text: "No Projects yet. Create a Project to start a shared workspace."}
        }
    }
    mod.widgets.ApprovalInboxRows = #(approval_discovery::ApprovalRows::register_widget(vm)) {
        ..mod.widgets.PortalList
        selectable:false keep_invisible:false
    }
    mod.widgets.GlobalApprovalInbox = #(approval_discovery::GlobalApprovalInbox::register_widget(vm)) {
        width: Fill height: Fit flow: Down
        open_approval_inbox := RinxGhostButton {width:Fill height:40 align:Align{x:0 y:0.5} padding:Inset{left:12 right:8}
            text:#(crate::i18n::tr("Approvals")) i18n_text:"Approvals"
            draw_icon.svg:crate_resource("self://resources/icons/bell.svg") icon_walk:Walk{width:18 height:18} spacing:10}
        inbox_popup := Modal {
            content := SmallModal {width:580 height:560 flow:Down spacing:10
                inbox_header := View {width:Fill height:32 flow:Right spacing:6 align:Align{y:0.5}
                    ModalTitle {width:Fill margin:0 align:Align{x:0 y:0.5} text:#(crate::i18n::tr("Pending approvals")) i18n_text:"Pending approvals"}
                    inbox_refresh := RinxIconButton {width:32 height:32 draw_icon.svg:crate_resource("self://resources/icons/article_refresh.svg")}
                    inbox_close := RinxIconButton {width:32 height:32 draw_icon.svg:ICON_CLOSE}
                }
                inbox_status := RinxLabel {width:Fill height:Fit max_lines:3 flow:Flow.Right{wrap:true}}
                inbox_rows := mod.widgets.ApprovalInboxRows {width:Fill height:Fill
                    Approval := View {width:Fill height:102 flow:Down padding:8 spacing:6
                        approval_summary := RinxLabel {width:Fill height:Fit max_lines:2 text_overflow:Ellipsis}
                        review_approval := RinxGhostButton {width:Fill height:32 text:#(crate::i18n::tr("Review exact request")) i18n_text:"Review exact request"}
                    }
                    Empty := RinxLabel {width:Fill height:50 text:#(crate::i18n::tr("No pending approvals on this device.")) i18n_text:"No pending approvals on this device."}
                }
                inbox_pagination := View {visible:false width:Fill height:Fit flow:Right spacing:6 align:Align{x:1 y:0.5}
                    inbox_previous := RinxGhostButton {height:32 text:#(crate::i18n::tr("Previous page")) i18n_text:"Previous page"}
                    inbox_next := RinxGhostButton {height:32 text:#(crate::i18n::tr("Next page")) i18n_text:"Next page"}
                }
            }
        }
    }
    mod.widgets.HagencyPanel = #(HagencyPanel::register_widget(vm)) {
        ..mod.widgets.SolidView
        width: Fill height: Fill flow: Down padding: Inset{top: 8 left: 12 right: 0 bottom: 0} spacing: 6 draw_bg.color: mod.widgets.RINX_CONTENT_BG
        panel_header := View {width:Fill height:Fit margin:Inset{right:20} spacing:6 align:Align{y:0.5}
            title_row := View {width:Fill height:Fit spacing:8 align:Align{y:0.5}
            back := RinxIconButton {visible:false width:32 height:32 draw_icon.svg:ICON_ARROW_BACK}
            title_group := View {width:Fill height:32 spacing:8 align:Align{y:0.5}
            heading := Text {width:Fit{max:FitBound.Rel{base:Base.Line factor:0.9}} max_lines:1 text_overflow:Ellipsis text: #(crate::i18n::tr("Projects")) i18n_text: "Projects" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
            loading_status := LoadingSpinner {visible: false width: 18 height: 18 draw_bg +: {color: mod.widgets.RINX_MUTED stroke_width: 2.0}}
            }
            new_item := Primary {text: #(crate::i18n::tr("New Project")) i18n_text: "New Project"}
            show_archived_projects := RinxCheckBox {text: #(crate::i18n::tr("Archived projects")) i18n_text: "Archived projects"}
            refresh := Action {text: #(crate::i18n::tr("Refresh")) i18n_text: "Refresh"}
            ledger_toggle := Action {visible: false text: #(crate::i18n::tr("Ledger backups")) i18n_text: "Ledger backups"}
            }
            project_tabs := View {visible:false width: Fit height: Fit flow: Flow.Right{wrap: true} spacing: 4
                        overview_tab := Action {height:32 padding:Inset{left:10 right:10}text: #(crate::i18n::tr("Overview")) i18n_text: "Overview"}
                        board_tab := Action {height:32 padding:Inset{left:10 right:10}text: #(crate::i18n::tr("Boards")) i18n_text: "Boards"}
                        rooms_tab := Action {height:32 padding:Inset{left:10 right:10}text: #(crate::i18n::tr("Rooms")) i18n_text: "Rooms"}
                        members_tab := Action {height:32 padding:Inset{left:10 right:10}text: #(crate::i18n::tr("Members")) i18n_text: "Members"}
                        permissions_tab := Action {height:32 padding:Inset{left:10 right:10}text: #(crate::i18n::tr("Access")) i18n_text: "Access"}
                        archive_project := RinxIconButton {visible:false width:32 height:32 draw_icon.svg:crate_resource("self://resources/icons/archive_box.svg")}
                    }
        }
        owner := Text {visible: false}
        status_holder := InlineNotice {margin: Inset{right: 20}}
        login := Action {visible: false text: #(crate::i18n::tr("Sign in again")) i18n_text: "Sign in again"}
        lists_pane := View {width: Fill height: Fill margin: Inset{bottom: 16} flow: Down spacing: 8
                ledger_panel := Section {visible: false width: Fill height: Fit flow: Down spacing: 8 margin: Inset{right: 20}
                    Text {text: #(crate::i18n::tr("Owner ledger backup")) i18n_text: "Owner ledger backup" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                    Text {text: #(crate::i18n::tr("Includes Agent settings, usage, pending requests and saved replies on this device. Keep backups private. Provider and Matrix login files are excluded.")) i18n_text: "Includes Agent settings, usage, pending requests and saved replies on this device. Keep backups private. Provider and Matrix login files are excluded."}
                    ledger_path := Input {empty_text: #(crate::i18n::tr("Full backup file path on this device"))}
                    View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                        ledger_choose := Action {text: #(crate::i18n::tr("Choose backup")) i18n_text: "Choose backup"}
                        ledger_export := Action {text: #(crate::i18n::tr("Export new backup")) i18n_text: "Export new backup"}
                        ledger_inspect := Action {text: #(crate::i18n::tr("Check backup")) i18n_text: "Check backup"}
                    }
                    ledger_summary := Text {}
                    Text {text: #(crate::i18n::tr("Stop all Agent services before restoring. Restore requires an empty local ledger and cannot overwrite recorded usage or requests. Restored services stay stopped.")) i18n_text: "Stop all Agent services before restoring. Restore requires an empty local ledger and cannot overwrite recorded usage or requests. Restored services stay stopped." draw_text.color: mod.widgets.RINX_MUTED}
                    ledger_confirm := RinxCheckBox {text: #(crate::i18n::tr("Restore the checked backup into this owner's empty ledger")) i18n_text: "Restore the checked backup into this owner's empty ledger"}
                    ledger_restore := Primary {text: #(crate::i18n::tr("Restore checked backup")) i18n_text: "Restore checked backup"}
                }
                agent_recovery := View {visible: false width: Fill height: Fit flow: Down margin: Inset{right: 20}
                    agent_request_list := FlatList {width: Fill height: Fit flow: Down spacing: 8
                        scroll_bars +: {show_scroll_x: false show_scroll_y: false}
                        Request := View {width: Fill height: Fit flow: Down spacing: 6
                            request_notice := InlineNotice {padding: 10 spacing: 4
                                header +: {flow: Flow.Right{wrap: true} spacing: 8
                                    title +: {width: Fit}
                                    request_actions := View {width: Fit height: Fit flow: Flow.Right{wrap: true} spacing: 8
                                        check_request := Action {text: #(crate::i18n::tr("Check status")) i18n_text: "Check status"}
                                        retry_request := Primary {text: #(crate::i18n::tr("Retry request")) i18n_text: "Retry request"}
                                        dismiss_request := Action {text: #(crate::i18n::tr("Dismiss failed request")) i18n_text: "Dismiss failed request"}
                                    }
                                }
                            }
                        }
                    }
                }
            project_list_holder := View {width: Fill height: Fill project_list := Cards {Card +: {
                padding: Inset{left:16 right:16 top:10 bottom:10} spacing:12 margin:Inset{right:20 bottom:8}
                draw_bg.border_radius:mod.widgets.RINX_CONTROL_RADIUS
                content +: {title +: {max_lines:1 text_overflow:Ellipsis draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_BODY_SIZE}}}
            }}}
            agent_list_holder := View {visible: false width: Fill height: Fill flow: Down spacing: 8
                View {visible: #(cfg!(feature = "agent_chat")) width: Fill height: Fit flow: Flow.Right{wrap: true} padding: Inset{right: 20}
                    agent_approval_chats := Action {text: #(crate::i18n::tr("Agent approval chats")) i18n_text: "Agent approval chats"}
                }
                agent_list := Cards {}
            }
        }
        details_scroll := ScrollYView {visible: false width: Fill height: Fill flow: Down padding: Inset{right: 20 bottom: 16} spacing: 12
            projects_page := View {width: Fill height: Fit flow: Down spacing: 10
                View {visible: false width: 0 height: 0 projects := Select {labels: [#(crate::i18n::tr("Select a Project"))]}}
                project_detail := View {visible: false width: Fill height: Fit flow: Down spacing: 6
                    board_section := View {visible: false width: Fill height: Fill flow: Down spacing: 8
                        board := ProjectBoard {}
                    }
                    overview_section := View {width:Fill height:Fit flow:Down spacing:20 padding:Inset{top:12 bottom:20}
                        project_details := RinxHint {visible:false}
                        overview_archived := RinxHint {visible:false text:#(crate::i18n::tr("Archived Project · boards are read only")) i18n_text:"Archived Project · boards are read only"}
                        View {width:Fill height:Fit flow:Right spacing:12
                            metric_tasks := OverviewMetric {caption.text:#(crate::i18n::tr("Tasks")) caption.i18n_text:"Tasks"}
                            metric_done := OverviewMetric {caption.text:#(crate::i18n::tr("Completed")) caption.i18n_text:"Completed" value.draw_text.color:mod.widgets.RINX_ACCENT}
                            metric_overdue := OverviewMetric {caption.text:#(crate::i18n::tr("Overdue")) caption.i18n_text:"Overdue"}
                        }
                        View {width:Fill height:Fit flow:Right spacing:12
                            metric_boards := OverviewMetric {caption.text:#(crate::i18n::tr("Boards")) caption.i18n_text:"Boards"}
                            metric_members := OverviewMetric {caption.text:#(crate::i18n::tr("Members")) caption.i18n_text:"Members"}
                            metric_progress := OverviewMetric {caption.text:#(crate::i18n::tr("Completion")) caption.i18n_text:"Completion"}
                        }
                        overview_boards_section := RinxCard {width:Fill height:Fit flow:Down spacing:12 padding:16
                            Text {text:#(crate::i18n::tr("Boards")) i18n_text:"Boards" draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_BODY_SIZE}}
                            overview_boards := FlatList {width:Fill height:Fit flow:Flow.Right{wrap:true} spacing:8
                                scroll_bars +: {show_scroll_x:false show_scroll_y:false}
                                Board := View {width:Fit height:32 spacing:6 align:Align{y:0.5} open_board := RinxGhostButton {height:32 draw_icon.svg:ICON_SQUARES} default_badge := mod.widgets.RinxDefaultBadge {visible:false label +: {i18n_text:"default"}}}
                            }
                        }
                        RinxCard {width:Fill height:Fit flow:Down spacing:14 padding:16
                            Text {text:#(crate::i18n::tr("Recent activity")) i18n_text:"Recent activity" draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_BODY_SIZE}}
                            overview_activity := RinxHint {text:#(crate::i18n::tr("Loading…")) i18n_text:"Loading…"}
                            overview_activity_list := FlatList {width:Fill height:Fit flow:Down spacing:0
                                scroll_bars +: {show_scroll_x:false show_scroll_y:false}
                                Activity := View {width:Fill height:Fit flow:Right spacing:12 padding:Inset{top:12 bottom:12}
                                    RoundedView {width:7 height:7 margin:Inset{top:6} draw_bg +: {color:mod.widgets.RINX_ACCENT border_radius:4}}
                                    View {width:Fill height:Fit flow:Down spacing:7
                                        View {width:Fill height:Fit flow:Right spacing:10
                                            activity_title := Text {width:Fill max_lines:2 text_overflow:Ellipsis}
                                            activity_time := RinxHint {width:Fit}
                                        }
                                        activity_context := RinxHint {width:Fill max_lines:2 text_overflow:Ellipsis}
                                    }
                                }
                            }
                        }
                    }
                    rooms_section := View {visible:false width:Fill height:Fit flow:Down spacing:12
                        rooms_empty_status := Text {text:#(crate::i18n::tr("Loading…"))}
                        room_management := View {visible:false width:Fill height:Fit flow:Down
                        View {width:Fill height:32 spacing:6 align:Align{y:0.5}
                            rooms := Select {width:220 height:32 labels:[#(crate::i18n::tr("Select a Room"))]}
                            new_room := RinxIconButton {width:32 height:32 draw_icon.svg:ICON_ADD}
                        }
                        rooms_hint := Text {width:Fill height:Fit flow:Flow.Right{wrap:true} text:#(crate::i18n::tr("No Rooms yet. Add a Room to start a conversation.")) i18n_text:"No Rooms yet. Add a Room to start a conversation." draw_text.color:mod.widgets.RINX_MUTED}
                        room_details := Text {}
                        View {width:Fill height:Fit flow:Flow.Right{wrap:true} spacing:8
                            room_members := Action {text:#(crate::i18n::tr("Room members")) i18n_text:"Room members"}
                            open_room := Action {text:#(crate::i18n::tr("Open Room")) i18n_text:"Open Room"}
                            room_permissions := Action {text:#(crate::i18n::tr("Room Agent permissions")) i18n_text:"Room Agent permissions"}
                            room_agents := Action {text:#(crate::i18n::tr("Manage my Agents")) i18n_text:"Manage my Agents"}
                        }
                    }
                    }
                    permissions_section := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                        permission_heading := Text {text: #(crate::i18n::tr("Access")) i18n_text: "Access"}
                        permission_status := Text {}
                        permission_editor := View {visible: false width: Fill height: Fit flow: Down spacing: 8
                            Text {text: #(crate::i18n::tr("Access rule")) i18n_text: "Access rule"}
                            creation_mode := Select {labels: [#(crate::i18n::tr("Allow Project members by default")) #(crate::i18n::tr("Only specified Project members"))]}
                            Text {text: #(crate::i18n::tr("Allowed members")) i18n_text: "Allowed members"}
                            creation_allow := Input {empty_text: #(crate::i18n::tr("Allowed Matrix user IDs, comma separated")) i18n_empty_text: "Allowed Matrix user IDs, comma separated"}
                            Text {text: #(crate::i18n::tr("Blocked members")) i18n_text: "Blocked members"}
                            creation_deny := Input {empty_text: #(crate::i18n::tr("Denied Matrix user IDs, comma separated")) i18n_empty_text: "Denied Matrix user IDs, comma separated"}
                            save_creation_policy := Primary {text: #(crate::i18n::tr("Save Agent access permissions")) i18n_text: "Save Agent access permissions"}
                            Hr {width: Fill height: 1}
                            Text {text: #(crate::i18n::tr("Service availability")) i18n_text: "Service availability"}
                            scope_pause := RinxCheckBox {text: #(crate::i18n::tr("Pause Agent services in this Project or Room")) i18n_text: "Pause Agent services in this Project or Room"}
                            save_scope_pause := Primary {text: #(crate::i18n::tr("Apply service pause")) i18n_text: "Apply service pause"}
                            Text {text: #(crate::i18n::tr("Access permissions and service pause are separate. Resuming services does not change access permissions.")) i18n_text: "Access permissions and service pause are separate. Resuming services does not change access permissions." draw_text.color: mod.widgets.RINX_MUTED}
                        }
                    }
                    members_section := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                        View {width: Fill height: 40 flow: Right spacing: 8 align: Align{y: 0.5}
                            members_heading := Text {width: Fill text: #(crate::i18n::tr("Members")) i18n_text: "Members"}
                            refresh_members := RinxIconButton {draw_icon.svg: crate_resource("self://resources/icons/article_refresh.svg")}
                            invite_member := RinxIconButton {draw_icon.svg: ICON_ADD_USER visible: false enabled: false}
                        }
                        members_note := Text {visible: false}
                        members_status := Text {}
                        members_list := PortalList {width: Fill height: Fill
                            Member := View {width: Fill height: 72 flow: Right padding: 12 spacing: 12 align: Align{y: 0.5}
                                indent := View {visible: false width: 24 height: 1}
                                avatar := Avatar {width: 36 height: 36}
                                View {width: Fill height: Fit flow: Down spacing: 4
                                    name := Text {max_lines: 1 text_overflow: Ellipsis}
                                    identity := Text {max_lines: 1 text_overflow: Ellipsis draw_text.color: mod.widgets.RINX_MUTED}
                                }
                                agent_badge := DeviceBadge {visible: false device_label.text: #(crate::i18n::tr("Agent")) device_label.i18n_text: "Agent"}
                                role_badge := DeviceBadge {}
                                membership := Text {visible: false draw_text.color: mod.widgets.RINX_MUTED}
                            }
                            Filler := View {width: Fill height: 64}
                        }
                    }
                }
                project_form := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                    creation_heading := Text {visible: false text: #(crate::i18n::tr("Create Project")) i18n_text: "Create Project" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                    creation_note := Text {text: #(crate::i18n::tr("Choose a new Space or connect one you already belong to.")) i18n_text: "Choose a new Space or connect one you already belong to." draw_text.color: mod.widgets.RINX_MUTED}
                    creation_source_holder := View {width: Fill height: Fit flow: Down spacing: 4
                        creation_source_label := Text {text: #(crate::i18n::tr("Create or connect")) i18n_text: "Create or connect"}
                        creation_source := Select {labels: [#(crate::i18n::tr("Create a new Space (default)"))]}
                    }
                    creation_name_holder := View {width: Fill height: Fit flow: Down spacing: 4 Text {text: #(crate::i18n::tr("Name")) i18n_text: "Name"} creation_name := Input {empty_text: "Project name"}}
                    candidates_holder := View {visible: false width: Fill height: Fit flow: Down spacing: 4 Text {text: #(crate::i18n::tr("Space")) i18n_text: "Space"}
 candidates := Select {labels: [#(crate::i18n::tr("Select a joined Space"))]}}
                    candidates_more := Action {visible: false text: #(crate::i18n::tr("Refresh / load more Spaces")) i18n_text: "Refresh / load more Spaces"}
                    existing_room_holder := View {visible: false width: Fill height: Fit flow: Down spacing: 4 Text {text: #(crate::i18n::tr("Room ID")) i18n_text: "Room ID"}
 existing_room := Input {empty_text: "!existing-room:server"}}
                    creation_encryption := View {visible:false width:Fill height:Fit flow:Down spacing:4
                        creation_encrypted := RinxCheckBox {text:#(crate::i18n::tr("Encrypted room")) i18n_text:"Encrypted room"}
                        Text {text:#(crate::i18n::tr("Encryption protects new messages and cannot be turned off. Earlier messages stay unencrypted.")) i18n_text:"Encryption protects new messages and cannot be turned off. Earlier messages stay unencrypted." draw_text.color:mod.widgets.RINX_MUTED}
                    }
                    create_project_room := Primary {text: #(crate::i18n::tr("Create Project")) i18n_text: "Create Project"}
                    creation_result := Text {}
                    recovery := View {visible: false width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("An earlier creation has not been confirmed. Resume the original request to avoid a duplicate.")) i18n_text: "An earlier creation has not been confirmed. Resume the original request to avoid a duplicate."}
                        found_room := Input {empty_text: #(crate::i18n::tr("Created Matrix ID, if known")) i18n_empty_text: "Created Matrix ID, if known"}
                        resume_creation := Action {text: #(crate::i18n::tr("Resume original request")) i18n_text: "Resume original request"}
                    }
                }
            }
            agents_page := View {visible: false width: Fill height: Fit flow: Down spacing: 10
                View {visible: false width: 0 height: 0 agents := Select {labels: [#(crate::i18n::tr("Select my Agent"))]}}
                agent_create := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                    agent_creation_heading := Text {visible: false text: #(crate::i18n::tr("Create Agent")) i18n_text: "Create Agent" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                    Text {text: #(crate::i18n::tr("Give your Agent a name. After creation, connect a model to start chatting. Adding shared Rooms is optional.")) i18n_text: "Give your Agent a name. After creation, connect a model to start chatting. Adding shared Rooms is optional." draw_text.color: mod.widgets.RINX_MUTED}
                    agent_binding_target := View {visible: false width: Fill height: Fit flow: Down spacing: 8
                        agent_project := Select {labels: [#(crate::i18n::tr("Select a Project"))]}
                        agent_room := Select {labels: [#(crate::i18n::tr("Select a visible registered Room"))]}
                    }
                    agent_name_holder := View {width: Fill height: Fit flow: Down spacing: 4 Text {text: #(crate::i18n::tr("Agent name")) i18n_text: "Agent name"}
 agent_name := Input {empty_text: #(crate::i18n::tr("Agent display name")) i18n_empty_text: "Agent display name"}}
                    creation_audience := View {width: Fill height: Fit flow: Down spacing: 6
                        Text {text: #(crate::i18n::tr("Service audience")) i18n_text: "Service audience"}
                        new_agent_visibility := Select {labels: [#(crate::i18n::tr("Private · owner chat only")) #(crate::i18n::tr("Public service"))] i18n_labels: ["Private · owner chat only" "Public service"]}
                        Text {text: #(crate::i18n::tr("Private Agents can only chat with you. Public Agents can serve members in shared Rooms.")) i18n_text: "Private Agents can only chat with you. Public Agents can serve members in shared Rooms." draw_text.color: mod.widgets.RINX_MUTED}
                    }
                    View {width: Fill height: Fit flow: Right spacing: 10 align: Align{y: 0.5}
                        create_agent := Primary {text: #(crate::i18n::tr("Create Agent")) i18n_text: "Create Agent"}
                        agent_create_spinner := LoadingSpinner {visible: false width: 18 height: 18 draw_bg.color: RINX_MUTED}
                    }
                    bind_agent := Primary {text: #(crate::i18n::tr("Bind selected existing Agent to Room")) i18n_text: "Bind selected existing Agent to Room"}
                }
                agent_detail := View {visible: false width: Fill height: Fit flow: Down spacing: 10
                    View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                        setup_tab := Action {text: #(crate::i18n::tr("Agent")) i18n_text: "Agent"}
                        model_tab := Action {text: #(crate::i18n::tr("Model & resources")) i18n_text: "Model & resources"}
                        policy_tab := Action {text: #(crate::i18n::tr("Limits")) i18n_text: "Limits"}
                        runtime_tab := Action {text: #(crate::i18n::tr("Service")) i18n_text: "Service"}
                        usage_tab := Action {text: #(crate::i18n::tr("Usage & alerts")) i18n_text: "Usage & alerts"}
                        recovery_tab := Action {text: #(crate::i18n::tr("Recovery")) i18n_text: "Recovery"}
                        files_tab := Action {text: #(crate::i18n::tr("File delivery")) i18n_text: "File delivery"}
                        history_tab := Action {text: #(crate::i18n::tr("Approval history")) i18n_text: "Approval history"}
                        remote_review_tab := Action {text: #(crate::i18n::tr("Remote approvals")) i18n_text: "Remote approvals"}
                    }
                    remote_review_section := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("Requests from your other devices")) i18n_text: "Requests from your other devices"}
                        Text {text: #(crate::i18n::tr("Choose your private approval Room and review the complete original action. Your decision applies once; the execution device still checks the live task and policies.")) i18n_text: "Choose your private approval Room and review the complete original action. Your decision applies once; the execution device still checks the live task and policies." draw_text.color: mod.widgets.RINX_MUTED}
                        remote_review_room := Select {labels: [#(crate::i18n::tr("Select a private approval Room"))]}
                        View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                            remote_review_refresh := Action {text: #(crate::i18n::tr("Refresh requests")) i18n_text: "Refresh requests"}
                            remote_review_older := Action {text: #(crate::i18n::tr("Older requests")) i18n_text: "Older requests"}
                        }
                        remote_review_requests := Select {labels: [#(crate::i18n::tr("Select a remote approval request"))]}
                        remote_review_details := Text {}
                        remote_review_copy := Action {text: #(crate::i18n::tr("Copy complete proposal")) i18n_text: "Copy complete proposal"}
                        remote_review_status := Text {draw_text.color: mod.widgets.RINX_MUTED}
                        View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                            remote_review_allow := Action {text: #(crate::i18n::tr("Approve exact request once")) i18n_text: "Approve exact request once"}
                            remote_review_deny := Danger {text: #(crate::i18n::tr("Reject request")) i18n_text: "Reject request"}
                            remote_review_receipt := Action {text: #(crate::i18n::tr("Refresh decision delivery")) i18n_text: "Refresh decision delivery"}
                        }
                    }
                    recovery_section := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("Inspect a stopped request")) i18n_text: "Inspect a stopped request"}
                        Text {text: #(crate::i18n::tr("Original unknown results stay unknown. Your notes are separate evidence. Continuing sends a new request as you to the original Room and thread; it does not replay the original execution.")) i18n_text: "Original unknown results stay unknown. Your notes are separate evidence. Continuing sends a new request as you to the original Room and thread; it does not replay the original execution."}
                        recovery_records := Select {labels: [#(crate::i18n::tr("Select a stopped request"))]}
                        View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                            recovery_refresh := Action {text: #(crate::i18n::tr("Newest records")) i18n_text: "Newest records"}
                            recovery_older := Action {text: #(crate::i18n::tr("Older records")) i18n_text: "Older records"}
                        }
                        Text {text: #(crate::i18n::tr("Files to inspect, relative to the recorded workspace (one per line, up to 32 files)")) i18n_text: "Files to inspect, relative to the recorded workspace (one per line, up to 32 files)"}
                        recovery_paths := Input {height: 86 is_multiline: true}
                        recovery_inspect := Action {text: #(crate::i18n::tr("Inspect original request and current files")) i18n_text: "Inspect original request and current files"}
                        recovery_details := Text {}
                        recovery_copy := Action {text: #(crate::i18n::tr("Copy original input")) i18n_text: "Copy original input"}
                        Text {text: #(crate::i18n::tr("Owner review note (required)")) i18n_text: "Owner review note (required)"}
                        recovery_note := Input {height: 86 is_multiline: true}
                        Text {text: #(crate::i18n::tr("New continuation instruction")) i18n_text: "New continuation instruction"}
                        recovery_instruction := Input {height: 100 is_multiline: true}
                        View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                            recovery_keep := Action {text: #(crate::i18n::tr("Keep blocked")) i18n_text: "Keep blocked"}
                            recovery_accept := Action {text: #(crate::i18n::tr("Record owner confirmation")) i18n_text: "Record owner confirmation"}
                            recovery_continue := Action {text: #(crate::i18n::tr("Review new continuation")) i18n_text: "Review new continuation"}
                        }
                        recovery_confirmation := View {visible: false width: Fill height: Fit flow: Down spacing: 8
                            recovery_confirm_details := Text {}
                            Text {text: #(crate::i18n::tr("Confirm this exact disposition. A continuation may incur new model usage when the Agent service runs. Missing delivery receipts remain unknown and are never resent by this action.")) i18n_text: "Confirm this exact disposition. A continuation may incur new model usage when the Agent service runs. Missing delivery receipts remain unknown and are never resent by this action."}
                            View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                                recovery_confirm := Action {text: #(crate::i18n::tr("Confirm disposition")) i18n_text: "Confirm disposition"}
                                recovery_cancel := Action {text: #(crate::i18n::tr("Cancel")) i18n_text: "Cancel"}
                            }
                        }
                    }
                    files_section := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("File delivery")) i18n_text: "File delivery"}
                        Text {text: #(crate::i18n::tr("Only an original Matrix event receipt proves delivery. Refresh reads the original request; it never uploads, resends or restarts the model.")) i18n_text: "Only an original Matrix event receipt proves delivery. Refresh reads the original request; it never uploads, resends or restarts the model." draw_text.color: mod.widgets.RINX_MUTED}
                        file_records := Select {labels: [#(crate::i18n::tr("Select a file delivery"))]}
                        View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                            file_newest := Action {text: #(crate::i18n::tr("Newest records")) i18n_text: "Newest records"}
                            file_older := Action {text: #(crate::i18n::tr("Older records")) i18n_text: "Older records"}
                            file_refresh := Action {text: #(crate::i18n::tr("Refresh delivery receipt")) i18n_text: "Refresh delivery receipt"}
                        }
                        file_details := Text {}
                    }
                    history_section := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("Approval evidence on this device")) i18n_text: "Approval evidence on this device" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                        Text {text: #(crate::i18n::tr("A recorded owner decision is not proof of execution. Permission application, provider submission and matching tool receipts are shown separately. Missing receipts remain unknown; history never resubmits an approval.")) i18n_text: "A recorded owner decision is not proof of execution. Permission application, provider submission and matching tool receipts are shown separately. Missing receipts remain unknown; history never resubmits an approval." draw_text.color: mod.widgets.RINX_MUTED}
                        history_records := Select {labels: [#(crate::i18n::tr("Select an approval record"))]}
                        View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                            history_refresh := Action {text: #(crate::i18n::tr("Newest records")) i18n_text: "Newest records"}
                            history_older := Action {text: #(crate::i18n::tr("Older records")) i18n_text: "Older records"}
                        }
                        history_details := Text {}
                        history_proposal := Text {}
                        Text {text: #(crate::i18n::tr("The proposal preview shows up to 16,384 characters. Copy the complete original proposal to inspect longer commands or file changes.")) i18n_text: "The proposal preview shows up to 16,384 characters. Copy the complete original proposal to inspect longer commands or file changes." draw_text.color: mod.widgets.RINX_MUTED}
                        history_copy := Action {text: #(crate::i18n::tr("Copy complete proposal")) i18n_text: "Copy complete proposal"}
                    }
                    usage_section := View {visible: false width: Fill height: Fit flow: Down spacing: 10
                        Section {width: Fill height: Fit flow: Down spacing: 8
                            Text {text: #(crate::i18n::tr("Usage recorded on this device")) i18n_text: "Usage recorded on this device" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                            Text {text: #(crate::i18n::tr("UTC dates use the call start time. Cached input is included in input; reasoning is included in output. Older calls without a recorded time are listed separately.")) i18n_text: "UTC dates use the call start time. Cached input is included in input; reasoning is included in output. Older calls without a recorded time are listed separately." draw_text.color: mod.widgets.RINX_MUTED}
                            usage_scope := Select {labels: [#(crate::i18n::tr("Entire Agent")) #(crate::i18n::tr("Selected Room")) #(crate::i18n::tr("Selected requester in Room"))]}
                            usage_room_holder := View {visible: false width: Fill height: Fit usage_room := Select {labels: [#(crate::i18n::tr("Select a Room binding"))]}}
                            usage_requester_holder := View {visible: false width: Fill height: Fit usage_requester := Input {empty_text: "@user:server"}}
                            usage_group := Select {labels: [#(crate::i18n::tr("Daily")) #(crate::i18n::tr("Monthly"))]}
                            Text {text: #(crate::i18n::tr("From UTC date, inclusive")) i18n_text: "From UTC date, inclusive"}
                            usage_from := Input {empty_text: "YYYY-MM-DD"}
                            Text {text: #(crate::i18n::tr("Until UTC date, exclusive")) i18n_text: "Until UTC date, exclusive"}
                            usage_until := Input {empty_text: "YYYY-MM-DD"}
                            usage_refresh := Action {text: #(crate::i18n::tr("Load usage report")) i18n_text: "Load usage report"}
                            usage_totals := Text {}
                            usage_undated := Text {draw_text.color: mod.widgets.RINX_MUTED}
                            usage_rows := Text {}
                        }
                        Section {width: Fill height: Fit flow: Down spacing: 8
                            Text {text: #(crate::i18n::tr("Budget alerts")) i18n_text: "Budget alerts" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                            Text {text: #(crate::i18n::tr("Alerts record when spent plus reserved tokens reach 80% or 100% of a configured budget. Acknowledging keeps usage and the original alert. Alerts include all dates in the selected scope.")) i18n_text: "Alerts record when spent plus reserved tokens reach 80% or 100% of a configured budget. Acknowledging keeps usage and the original alert. Alerts include all dates in the selected scope." draw_text.color: mod.widgets.RINX_MUTED}
                            usage_alerts := Select {labels: [#(crate::i18n::tr("Select a budget alert"))]}
                            usage_alert_details := Text {}
                            usage_ack := Action {text: #(crate::i18n::tr("Acknowledge alert")) i18n_text: "Acknowledge alert"}
                            View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                                usage_latest := Action {text: #(crate::i18n::tr("Newest alerts")) i18n_text: "Newest alerts"}
                                usage_older := Action {text: #(crate::i18n::tr("Older alerts")) i18n_text: "Older alerts"}
                            }
                        }
                    }
                    configuration_notice := Text {visible: false draw_text.color: mod.widgets.RINX_MUTED}
                    configuration_refresh := Action {visible: false text: #(crate::i18n::tr("Refresh execution device environment")) i18n_text: "Refresh execution device environment"}
                    local_device_notice := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                        local_device_message := Text {}
                        assignment_confirm := RinxCheckBox {text: #(crate::i18n::tr("Switching devices stops the previous lease. Its configuration is not copied. Restore this owner's complete ledger and configure resources before running here.")) i18n_text: "Switching devices stops the previous lease. Its configuration is not copied. Restore this owner's complete ledger and configure resources before running here."}
                        assignment_destination := Text {}
                        assign_current_device := Primary {text: #(crate::i18n::tr("Switch execution to this device")) i18n_text: "Switch execution to this device"}
                    }
                    setup_section := View {width: Fill height: Fit flow: Down spacing: 10
                    identity_section := Section {width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("Identity")) i18n_text: "Identity" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                        View {width: Fill height: Fit spacing: 6
                            owner_badge := DeviceBadge {device_label +: {text: #(crate::i18n::tr("Mine")) i18n_text: "Mine"}}
                            detail_audience_badge := DeviceBadge {}
                        }
                        Text {text: #(crate::i18n::tr("Service audience")) i18n_text: "Service audience"}
                        audience_editor := View {width: Fill height: Fit flow: Down spacing: 6
                            agent_visibility := Select {labels: [#(crate::i18n::tr("Private · owner chat only")) #(crate::i18n::tr("Public service"))] i18n_labels: ["Private · owner chat only" "Public service"]}
                            save_agent_visibility := Action {text: #(crate::i18n::tr("Save service audience")) i18n_text: "Save service audience"}
                        }
                        Text {text: #(crate::i18n::tr("Leave every shared Room before switching to private. Your private chat remains available.")) i18n_text: "Leave every shared Room before switching to private. Your private chat remains available." draw_text.color: mod.widgets.RINX_MUTED}
                        agent_details := Text {max_lines: 3 text_overflow: Ellipsis draw_text +: {color: mod.widgets.RINX_MUTED text_style: theme.font_regular{font_size: mod.widgets.RINX_BODY_SIZE}}}
                        Text {text: #(crate::i18n::tr("Execution device")) i18n_text: "Execution device"}
                        execution_badge := DeviceBadge {}
                        execution_device_details := Text {}
                        owner_direct_status := Text {}
                        View {width: Fill height: 40 spacing: 8
                            ensure_owner_direct := Action {text: #(crate::i18n::tr("Set up / retry private chat")) i18n_text: "Set up / retry private chat"}
                            open_owner_direct := Action {text: #(crate::i18n::tr("Open private chat")) i18n_text: "Open private chat" enabled: false}
                        }
                    }
                    View {width: Fill height: 40 spacing: 8
                        pause_agent := Action {text: #(crate::i18n::tr("Pause Agent")) i18n_text: "Pause Agent"}
                        resume_agent := Action {text: #(crate::i18n::tr("Resume Agent")) i18n_text: "Resume Agent"}
                        another_binding := Action {text: #(crate::i18n::tr("Add Room binding")) i18n_text: "Add Room binding"}
                    }
                    }
                    retirement_section := Section {visible: false width: Fill height: Fit padding: 12 spacing: 6
                    Text {text: #(crate::i18n::tr("Dangerous actions")) i18n_text: "Dangerous actions" draw_text.color: mod.widgets.RINX_MUTED}
                    retire_confirm := RinxCheckBox {text: #(crate::i18n::tr("I confirm retirement stops every Room; this identity remains mine")) i18n_text: "I confirm retirement stops every Room; this identity remains mine"}
                    retire_agent := Danger {text: #(crate::i18n::tr("Retire Agent permanently")) i18n_text: "Retire Agent permanently"}
                    }
                    policy_section := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("Usage limits")) i18n_text: "Usage limits" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                        Text {text: #(crate::i18n::tr("No token limit is required. Set optional limits for this Agent, one Room, or one member in a Room. All applicable limits are checked together.")) i18n_text: "No token limit is required. Set optional limits for this Agent, one Room, or one member in a Room. All applicable limits are checked together." draw_text.color: mod.widgets.RINX_MUTED}
                        Text {text: #(crate::i18n::tr("Apply to")) i18n_text: "Apply to"}
                        policy_layer := Select {labels: [#(crate::i18n::tr("Entire Agent")) #(crate::i18n::tr("One Room")) #(crate::i18n::tr("One member in a Room"))] i18n_labels: ["Entire Agent" "One Room" "One member in a Room"]}
                        policy_room_holder := View {visible: false width: Fill height: Fit flow: Down spacing: 4
                            Text {text: #(crate::i18n::tr("Room")) i18n_text: "Room"}
                            policy_binding := Select {labels: [#(crate::i18n::tr("Select a Room binding"))]}
                        }
                        policy_target := Text {draw_text.color: mod.widgets.RINX_MUTED}
                        requester_holder := View {width: Fill height: Fit flow: Down spacing: 8
                            Text {text: #(crate::i18n::tr("Member")) i18n_text: "Member"}
                            requester := Input {empty_text: "@requester:server"}
                            load_policy := Action {text: #(crate::i18n::tr("Load member settings")) i18n_text: "Load member settings"}
                        }
                        policy_usage := Text {draw_text.color: mod.widgets.RINX_MUTED}
                        Text {text: #(crate::i18n::tr("Token allowance")) i18n_text: "Token allowance"}
                        limit_mode := Select {labels: [#(crate::i18n::tr("No token limit")) #(crate::i18n::tr("Limited tokens"))] i18n_labels: ["No token limit" "Limited tokens"]}
                        token_amount_holder := View {visible: false width: Fill height: Fit flow: Down spacing: 8
                            Text {text: #(crate::i18n::tr("Tokens")) i18n_text: "Tokens"}
                            token_limit := Input {empty_text: #(crate::i18n::tr("Nonnegative token quota")) i18n_empty_text: "Nonnegative token quota"}
                        }
                        period_holder := View {visible: false width: Fill height: Fit flow: Down spacing: 4
                            Text {text: #(crate::i18n::tr("Reset allowance")) i18n_text: "Reset allowance"}
                            period := Select {labels: [#(crate::i18n::tr("Never (cumulative)")) #(crate::i18n::tr("Daily (UTC)")) #(crate::i18n::tr("Monthly (UTC)"))] i18n_labels: ["Never (cumulative)" "Daily (UTC)" "Monthly (UTC)"]}
                        }
                        zero_limit_note := Text {visible: false text: #(crate::i18n::tr("Use zero tokens to stop usage in this scope.")) i18n_text: "Use zero tokens to stop usage in this scope." draw_text.color: mod.widgets.RINX_MUTED}
                        Hr {width: Fill height: 1}
                        Text {text: #(crate::i18n::tr("Request permissions")) i18n_text: "Request permissions" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_SECTION_SIZE}}
                        Text {text: #(crate::i18n::tr("Request handling")) i18n_text: "Request handling"}
                        requests := Select {labels: [#(crate::i18n::tr("Allow requests")) #(crate::i18n::tr("Deny requests")) #(crate::i18n::tr("Ask owner per request"))] i18n_labels: ["Allow requests" "Deny requests" "Ask owner per request"]}
                        Text {text: #(crate::i18n::tr("High-risk tools")) i18n_text: "High-risk tools"}
                        tool_risk := Select {labels: [#(crate::i18n::tr("Deny high-risk tools")) #(crate::i18n::tr("Ask owner per tool")) #(crate::i18n::tr("Allow only exact tools/directories"))] i18n_labels: ["Deny high-risk tools" "Ask owner per tool" "Allow only exact tools/directories"]}
                        allowed_tools_holder := View {visible: false width: Fill height: Fit flow: Down spacing: 4 Text {text: #(crate::i18n::tr("Allowed tools")) i18n_text: "Allowed tools"}
 allowed_tools := Input {empty_text: #(crate::i18n::tr("Exact tool names, comma separated")) i18n_empty_text: "Exact tool names, comma separated"}}
                        allowed_directories_holder := View {visible: false width: Fill height: Fit flow: Down spacing: 4 Text {text: #(crate::i18n::tr("Allowed directories")) i18n_text: "Allowed directories"}
 allowed_directories := Input {empty_text: #(crate::i18n::tr("Exact absolute directories, comma separated")) i18n_empty_text: "Exact absolute directories, comma separated"}}
                        Text {text: #(crate::i18n::tr("Saving keeps recorded usage and pending reservations. Any applicable deny rule still blocks the request.")) i18n_text: "Saving keeps recorded usage and pending reservations. Any applicable deny rule still blocks the request."}
                        save_policy := Primary {text: #(crate::i18n::tr("Save settings")) i18n_text: "Save settings"}
                        reset_policy := Action {text: #(crate::i18n::tr("Reset this scope")) i18n_text: "Reset this scope"}
                    }
                    model_section := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                        model_setup_notice := InlineNotice {}
                        Text {text: #(crate::i18n::tr("Model & resources")) i18n_text: "Model & resources" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                        Text {text: #(crate::i18n::tr("Model settings apply to this Agent across its Rooms.")) i18n_text: "Model settings apply to this Agent across its Rooms." draw_text.color: mod.widgets.RINX_MUTED}
                        Text {text: #(crate::i18n::tr("Local agent runtime")) i18n_text: "Local agent runtime"}
                        runtime_kind := Select {labels: ["Codex" "Claude Code" "Octos"]}
                        runtime_provider_note := Text {draw_text.color: mod.widgets.RINX_MUTED}
                        RinxCard {width: Fill height: Fit flow: Down padding: 12 spacing: 8
                            View {width: Fill height: Fit spacing: 8 align: Align{y: 0.5}
                                Text {width: Fill text: #(crate::i18n::tr("Local account")) i18n_text: "Local account" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_BODY_SIZE}}
                                provider_spinner := LoadingSpinner {visible: false width: 18 height: 18 draw_bg.color: mod.widgets.RINX_ACCENT}
                                provider_check := RinxIconButton {width: 32 height: 32 draw_icon.svg: crate_resource("self://resources/icons/article_refresh.svg")}
                            }
                            provider_status := Text {width: Fill height: Fit flow: Flow.Right{wrap: true} text: #(crate::i18n::tr("Checking local account…"))}
                            provider_controls := View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                                use_local_provider := Primary {visible: false text: #(crate::i18n::tr("Connect my local account")) i18n_text: "Connect my local account"}
                                provider_open := Action {visible: false text: #(crate::i18n::tr("Open sign-in page")) i18n_text: "Open sign-in page"}
                                provider_cancel := Action {visible: false text: #(crate::i18n::tr("Cancel sign-in")) i18n_text: "Cancel sign-in"}
                            }
                        }
                        model_label := Text {text: #(crate::i18n::tr("Local model")) i18n_text: "Local model"}
                        View {width: Fill height: Fit spacing: 8 align: Align{y: 0.5}
                            model := Select {width: Fill labels: [#(crate::i18n::tr("Load available models"))]}
                            refresh_models := RinxIconButton {width: 36 height: 36 draw_icon.svg: crate_resource("self://resources/icons/article_refresh.svg")}
                        }
                        model_note := Text {}
                        effort_holder := View {width: Fill height: Fit flow: Down spacing: 8
                            effort_label := Text {text: #(crate::i18n::tr("Reasoning effort")) i18n_text: "Reasoning effort"}
                            effort := Select {labels: [#(crate::i18n::tr("Checking model choices…"))]}
                            effort_note := Text {draw_text.color: mod.widgets.RINX_MUTED}
                        }
                        web_search_holder := View {width: Fill height: Fit flow: Down spacing: 4
                            web_search := RinxCheckBox {text: #(crate::i18n::tr("Live web search")) i18n_text: "Live web search"}
                            Text {text: #(crate::i18n::tr("Enabled by default. Save and restart the service to apply changes.")) i18n_text: "Enabled by default. Save and restart the service to apply changes." draw_text.color: mod.widgets.RINX_MUTED}
                        }
                        advanced_toggle := Action {text: #(crate::i18n::tr("Advanced settings")) i18n_text: "Advanced settings" spacing: 6 icon_walk: Walk{width: 12 height: 12} draw_icon.svg: ICON_CHEVRON_RIGHT}
                        advanced_settings := View {visible: false width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("Local account")) i18n_text: "Local account"}
                        View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                            provider_login := Action {visible: false text: #(crate::i18n::tr("Sign in with a separate Codex account")) i18n_text: "Sign in with a separate Codex account"}
                            disconnect_provider := Action {visible: false text: #(crate::i18n::tr("Disconnect local account")) i18n_text: "Disconnect local account"}
                            provider_logout := Action {visible: false text: #(crate::i18n::tr("Sign out of separate Codex account")) i18n_text: "Sign out of separate Codex account"}
                        }
                        Text {text: #(crate::i18n::tr("Disconnecting here keeps your local runtime signed in.")) i18n_text: "Disconnecting here keeps your local runtime signed in." draw_text.color: mod.widgets.RINX_MUTED}
                        workspace_label := Text {text: #(crate::i18n::tr("Agent workspace on this device")) i18n_text: "Agent workspace on this device"}
                        workspace := Input {empty_text: #(crate::i18n::tr("Automatic: private workspace for this Agent")) i18n_empty_text: "Automatic: private workspace for this Agent"}
                        Text {text: #(crate::i18n::tr("Leave empty to use this Agent's private workspace. Custom directories must already exist.")) i18n_text: "Leave empty to use this Agent's private workspace. Custom directories must already exist." draw_text.color: mod.widgets.RINX_MUTED}
                        prepare_workspace := Action {text: #(crate::i18n::tr("Prepare this Agent's private workspace")) i18n_text: "Prepare this Agent's private workspace"}
                        }
                        programming_holder := View {width: Fill height: Fit flow: Down spacing: 8
                            programming := RinxCheckBox {text: #(crate::i18n::tr("Enable Codex programming")) i18n_text: "Enable Codex programming"}
                            Text {text: #(crate::i18n::tr("Run commands and edit existing files in this Agent's workspace. Rooms using this Agent share that workspace. Tool policies and exact owner approvals still apply.")) i18n_text: "Run commands and edit existing files in this Agent's workspace. Rooms using this Agent share that workspace. Tool policies and exact owner approvals still apply." draw_text.color: mod.widgets.RINX_MUTED}
                            mcp_content := RinxCard {width: Fill height: Fit flow: Down padding: 12 spacing: 8
                                mcp_header := View {width: Fill height: Fit flow: Right spacing: 8 align: Align{y:0.5}
                                    Text {width: Fill text: #(crate::i18n::tr("MCP servers")) i18n_text: "MCP servers" draw_text.text_style: theme.font_bold{font_size:mod.widgets.RINX_BODY_SIZE}}
                                    mcp_add := Action {text: #(crate::i18n::tr("Add server")) i18n_text: "Add server"}
                                }
                                Text {text: #(crate::i18n::tr("Give this Agent access to tools from an MCP server. These settings are saved with this Agent; personal Codex servers are not imported.")) i18n_text: "Give this Agent access to tools from an MCP server. These settings are saved with this Agent; personal Codex servers are not imported." draw_text.color: mod.widgets.RINX_MUTED}
                                mcp_empty := Text {text: #(crate::i18n::tr("No MCP servers configured.")) i18n_text: "No MCP servers configured." draw_text.color: mod.widgets.RINX_MUTED}
                                mcp_list := FlatList {width: Fill height: Fit flow: Down spacing: 8
                                    scroll_bars +: {show_scroll_x:false show_scroll_y:false}
                                    Server := View {width: Fill height: Fit flow: Down spacing: 4
                                        View {width: Fill height: Fit spacing: 8 align: Align{y:0.5}
                                            name := Text {width: Fill}
                                            edit := Action {text:#(crate::i18n::tr("Edit")) i18n_text:"Edit"}
                                            remove := Action {text:#(crate::i18n::tr("Remove")) i18n_text:"Remove"}
                                        }
                                        details := Text {width: Fill height: Fit flow: Flow.Right{wrap:true} draw_text.color:mod.widgets.RINX_MUTED}
                                    }
                                }
                                mcp_form := View {visible:false width:Fill height:Fit flow:Down spacing:8
                                    Text {text:#(crate::i18n::tr("Server name")) i18n_text:"Server name"}
                                    mcp_name := Input {empty_text:"docs"}
                                    Text {text:#(crate::i18n::tr("Use 1–64 letters, digits, hyphens or underscores.")) i18n_text:"Use 1–64 letters, digits, hyphens or underscores." draw_text.color:mod.widgets.RINX_MUTED}
                                    Text {text:#(crate::i18n::tr("Transport")) i18n_text:"Transport"}
                                    mcp_transport := Select {labels:["HTTP" "Stdio"]}
                                    mcp_http := View {width:Fill height:Fit flow:Down spacing:6
                                        Text {text:#(crate::i18n::tr("Server URL")) i18n_text:"Server URL"}
                                        mcp_url := Input {empty_text:"https://example.com/mcp"}
                                    }
                                    mcp_stdio := View {visible:false width:Fill height:Fit flow:Down spacing:6
                                        Text {text:#(crate::i18n::tr("Executable (absolute path)")) i18n_text:"Executable (absolute path)"}
                                        mcp_command := Input {empty_text:"/absolute/path/to/mcp-server"}
                                        Text {text:#(crate::i18n::tr("Arguments (JSON string array)")) i18n_text:"Arguments (JSON string array)"}
                                        mcp_args := Input {empty_text:"[\"--port\", \"3000\"]"}
                                    }
                                    View {width:Fill height:Fit flow:Flow.Right{wrap:true} spacing:8
                                        mcp_apply := Action {text:#(crate::i18n::tr("Apply server")) i18n_text:"Apply server"}
                                        mcp_cancel := Action {text:#(crate::i18n::tr("Cancel")) i18n_text:"Cancel"}
                                    }
                                }
                                mcp_json_toggle := Action {text:#(crate::i18n::tr("Edit JSON")) i18n_text:"Edit JSON"}
                                mcp_json_panel := View {visible:false width:Fill height:Fit flow:Down spacing:8
                                    Text {text:#(crate::i18n::tr("Use a server-name object, without an mcpServers wrapper. HTTP needs transport and url; stdio needs transport, an existing executable's canonical absolute path and an optional args array. Up to 16 servers. Headers and environment variables are not supported.")) i18n_text:"Use a server-name object, without an mcpServers wrapper. HTTP needs transport and url; stdio needs transport, an existing executable's canonical absolute path and an optional args array. Up to 16 servers. Headers and environment variables are not supported." draw_text.color:mod.widgets.RINX_MUTED}
                                    Text {text:"{\n  \"docs\": {\"transport\": \"http\", \"url\": \"https://example.com/mcp\"},\n  \"local\": {\"transport\": \"stdio\", \"command\": \"/absolute/path/to/mcp-server\", \"args\": []}\n}"}
                                    mcp_servers := Input {height:160 is_multiline:true submit_on_enter:false empty_text:"{}"}
                                    View {width:Fill height:Fit flow:Flow.Right{wrap:true} spacing:8
                                        mcp_validate := Action {text:#(crate::i18n::tr("Validate JSON")) i18n_text:"Validate JSON"}
                                        mcp_format := Action {text:#(crate::i18n::tr("Format JSON")) i18n_text:"Format JSON"}
                                    }
                                }
                                mcp_feedback := InlineNotice {}
                                Text {text:#(crate::i18n::tr("Validation checks configuration and local paths only; it does not start servers or test connections. Apply changes with Save local model profile, then restart the Agent service.")) i18n_text:"Validation checks configuration and local paths only; it does not start servers or test connections. Apply changes with Save local model profile, then restart the Agent service." draw_text.color:mod.widgets.RINX_MUTED}
                            }
                        }
                        credential := Text {visible: false}
                        View {width: Fill height: Fit flow: Right spacing: 10 align: Align{y:0.5}
                            save_model := Primary {text: #(crate::i18n::tr("Save local model profile")) i18n_text: "Save local model profile"}
                            model_save_spinner := LoadingSpinner {visible: false width: 18 height: 18 draw_bg +: {color: mod.widgets.RINX_ACCENT stroke_width: 2.0}}
                        }
                        model_save_feedback := InlineNotice {}
                        continue_to_service := Primary {visible: false text: #(crate::i18n::tr("Continue to Service")) i18n_text: "Continue to Service"}
                        Text {text: #(crate::i18n::tr("Restart the Agent service to apply saved model settings.")) i18n_text: "Restart the Agent service to apply saved model settings." draw_text.color: mod.widgets.RINX_MUTED}
                    }
                    runtime_section := View {visible: false width: Fill height: Fit flow: Down spacing: 12
                        Section {width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("Agent service")) i18n_text: "Agent service" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                        runtime_chat_target := Text {draw_text.color: mod.widgets.RINX_MUTED}
                        runtime_status := Text {}
                        service_logs_toggle := Action {text: #(crate::i18n::tr("View Service logs")) i18n_text: "View Service logs"}
                        service_logs_panel := Section {visible: false width: Fill height: Fit flow: Down spacing: 8
                            Text {text: #(crate::i18n::tr("Recent Service logs")) i18n_text: "Recent Service logs" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                            Text {text: #(crate::i18n::tr("Up to 100 events from this service session. Times are UTC. Provider output and credentials are excluded.")) i18n_text: "Up to 100 events from this service session. Times are UTC. Provider output and credentials are excluded."}
                            service_log_text := Text {}
                            copy_service_logs := Action {text: #(crate::i18n::tr("Copy logs")) i18n_text: "Copy logs"}
                        }
                        runtime_start_options := View {width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("Estimated token accounting is used by default. Reserve tokens before each request; actual usage may exceed the estimate. Missing usage is recorded as zero and the reservation is released automatically.")) i18n_text: "Estimated token accounting is used by default. Reserve tokens before each request; actual usage may exceed the estimate. Missing usage is recorded as zero and the reservation is released automatically."}
                        start_requirements := Text {draw_text.color: mod.widgets.RINX_MUTED}
                        Text {text: #(crate::i18n::tr("Reservation per request (tokens)")) i18n_text: "Reservation per request (tokens)"}
                        reservation := Input {empty_text: #(crate::i18n::tr("Positive token count")) i18n_empty_text: "Positive token count"}
                        service_advanced_toggle := Action {text: #(crate::i18n::tr("Advanced service options")) i18n_text: "Advanced service options" spacing: 6 icon_walk: Walk{width: 12 height: 12} draw_icon.svg: ICON_CHEVRON_RIGHT}
                        service_advanced := View {visible: false width: Fill height: Fit flow: Down spacing: 8
                        takeover := RinxCheckBox {text: #(crate::i18n::tr("Explicitly take over this Agent's device lease")) i18n_text: "Explicitly take over this Agent's device lease"}
                        host_files := RinxCheckBox {visible: false text: #(crate::i18n::tr("Enable this Room's scoped host files (list/read/create)")) i18n_text: "Enable this Room's scoped host files (list/read/create)"}
                        }
                        }
                        runtime_model_settings := Text {draw_text.color: mod.widgets.RINX_MUTED}
                        runtime_restart_note := Text {visible: false text: #(crate::i18n::tr("Stop responding before changing start options, then start again.")) i18n_text: "Stop responding before changing start options, then start again." draw_text.color: mod.widgets.RINX_MUTED}
                        View {width: Fill height: 40 spacing: 8
                            service_toggle := Primary {spacing: 6 icon_walk: Walk{width: 16 height: 16} draw_icon.svg: ICON_SERVICE_PLAY text: #(crate::i18n::tr("Checking…"))}
                            runtime_check := Action {text: #(crate::i18n::tr("Refresh status")) i18n_text: "Refresh status"}
                        }
                        }
                        Section {width: Fill height: Fit flow: Down spacing: 8
                            Text {text: #(crate::i18n::tr("Agent encryption identity")) i18n_text: "Agent encryption identity" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                            Text {text: #(crate::i18n::tr("Verified owner private Rooms support encrypted tasks, replies and files. Shared requester trust and cross-host key recovery remain unavailable.")) i18n_text: "Verified owner private Rooms support encrypted tasks, replies and files. Shared requester trust and cross-host key recovery remain unavailable." draw_text.color: mod.widgets.RINX_MUTED}
                            crypto_status := Text {}
                            crypto_identity := Text {}
                            crypto_confirm := RinxCheckBox {text: #(crate::i18n::tr("I checked the complete master key and device fingerprints for this Agent.")) i18n_text: "I checked the complete master key and device fingerprints for this Agent."}
                            View {width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                                crypto_refresh := Action {text: #(crate::i18n::tr("Refresh identity")) i18n_text: "Refresh identity"}
                                crypto_prepare := Action {text: #(crate::i18n::tr("Initialize identity")) i18n_text: "Initialize identity"}
                                crypto_stop := Action {text: #(crate::i18n::tr("Stop identity session")) i18n_text: "Stop identity session"}
                                crypto_copy := Action {text: #(crate::i18n::tr("Copy public identity")) i18n_text: "Copy public identity"}
                                crypto_verify := Action {text: #(crate::i18n::tr("Verify exact Agent identity")) i18n_text: "Verify exact Agent identity"}
                            }
                        }
                        Section {width: Fill height: Fit flow: Down spacing: 8
                            Text {text: #(crate::i18n::tr("Connected Rooms")) i18n_text: "Connected Rooms" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                            Text {text: #(crate::i18n::tr("Pause only where needed. Requests in a paused Room receive a notice.")) i18n_text: "Pause only where needed. Requests in a paused Room receive a notice." draw_text.color: mod.widgets.RINX_MUTED}
                            service_rooms := PortalList {width: Fill height: 260
                                Room := View {width: Fill height: Fit flow: Right padding: Inset{top: 8 bottom: 8} spacing: 8 align: Align{y: 0.5}
                                    View {width: Fill height: Fit flow: Down spacing: 4
                                        room_name := Text {draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_HEADING_SIZE}}
                                        room_status := Text {draw_text.color: mod.widgets.RINX_MUTED}
                                    }
                                    open_room_chat := Action {text: #(crate::i18n::tr("Open chat")) i18n_text: "Open chat"}
                                    room_toggle := RobrixNeutralIconButton {
                                        height: RINX_BUTTON_HEIGHT padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
                                        text: "Pause" spacing: 6 icon_walk: Walk{width: 16 height: 16} draw_icon.svg: ICON_SERVICE_PAUSE
                                    }
                                }
                                Empty := View {width: Fill height: 48 Text {text: #(crate::i18n::tr("No connected Rooms yet.")) i18n_text: "No connected Rooms yet."}}
                                Filler := View {width: Fill height: 32}
                            }
                        }
                        Section {width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("Pending approvals")) i18n_text: "Pending approvals" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_TITLE_SIZE}}
                        Text {text: #(crate::i18n::tr("Only requests or tools configured to ask you appear here.")) i18n_text: "Only requests or tools configured to ask you appear here." draw_text.color: mod.widgets.RINX_MUTED}
                        approvals_check := Action {text: #(crate::i18n::tr("Refresh pending requests/tools")) i18n_text: "Refresh pending requests/tools"}
                        approvals := Select {labels: [#(crate::i18n::tr("Select a pending approval"))]}
                        approval_count := Text {draw_text.color: mod.widgets.RINX_MUTED}
                        approval_feedback := Text {}
                        approval_details := Text {}
                        View {width: Fill height: 40 spacing: 8
                            approve := Action {text: #(crate::i18n::tr("Approve exact request once")) i18n_text: "Approve exact request once"}
                            reject := Danger {text: #(crate::i18n::tr("Reject request")) i18n_text: "Reject request"}
                        }
                        Text {text: #(crate::i18n::tr("Remote approval")) i18n_text: "Remote approval" draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_HEADING_SIZE}}
                        Text {text: #(crate::i18n::tr("Use an encrypted Room containing only you. Only your verified devices receive the request key.")) i18n_text: "Use an encrypted Room containing only you. Only your verified devices receive the request key." draw_text.color: mod.widgets.RINX_MUTED}
                        remote_approval_room := Select {labels: [#(crate::i18n::tr("Select a private approval Room"))]}
                        remote_approval_publish := Action {text: #(crate::i18n::tr("Publish privately")) i18n_text: "Publish privately"}
                        remote_approval_refresh := Action {text: #(crate::i18n::tr("Refresh private approval status")) i18n_text: "Refresh private approval status"}
                        remote_approval_details := Text {draw_text.color: mod.widgets.RINX_MUTED}
                        remote_approval_observe := Action {text: #(crate::i18n::tr("Read verified decision")) i18n_text: "Read verified decision"}
                        }
                    }
                    room_controls := Section {width: Fill height: Fit flow: Down spacing: 8
                        room_advanced_toggle := Action {text: #(crate::i18n::tr("Advanced Room settings")) i18n_text: "Advanced Room settings" spacing: 6 icon_walk: Walk{width: 12 height: 12} draw_icon.svg: ICON_CHEVRON_RIGHT}
                        room_advanced_content := View {visible: false width: Fill height: Fit flow: Down spacing: 8
                        Text {text: #(crate::i18n::tr("Choose a chat")) i18n_text: "Choose a chat"}
                        Text {text: #(crate::i18n::tr("Private chat is selected by default. Choose another Room to manage its service separately.")) i18n_text: "Private chat is selected by default. Choose another Room to manage its service separately." draw_text.color: mod.widgets.RINX_MUTED}
                        bindings := Select {labels: [#(crate::i18n::tr("Select a Room binding"))]}
                        binding_details := Text {}
                        reply_policy := View {width: Fill height: Fit flow: Down spacing: 8
                            Text {text: #(crate::i18n::tr("Room reply behavior")) i18n_text: "Room reply behavior"}
                            reply_policy_status := Text {draw_text.color: mod.widgets.RINX_MUTED}
                            reply_policy_toggle := Action {text: #(crate::i18n::tr("Enable automatic thread follow-ups")) i18n_text: "Enable automatic thread follow-ups"}
                        }
                        open_binding_chat := Action {text: #(crate::i18n::tr("Open selected Room chat")) i18n_text: "Open selected Room chat"}
                        room_roster_check := Action {text: #(crate::i18n::tr("Agents in this Room")) i18n_text: "Agents in this Room"}
                        room_roster := Text {}
                        room_service_actions := View {width: Fill height: 40 spacing: 8
                            pause_binding := Action {text: #(crate::i18n::tr("Pause Room")) i18n_text: "Pause Room"}
                            resume_binding := Action {text: #(crate::i18n::tr("Resume Room")) i18n_text: "Resume Room"}
                        }
                        room_danger_heading := Text {text: #(crate::i18n::tr("Dangerous actions")) i18n_text: "Dangerous actions" draw_text.color: mod.widgets.RINX_MUTED}
                        leave_confirm := RinxCheckBox {text: #(crate::i18n::tr("I want this Agent to leave the selected Room")) i18n_text: "I want this Agent to leave the selected Room"}
                        leave_binding := Danger {text: #(crate::i18n::tr("Leave this Room")) i18n_text: "Leave this Room"}
                    }
                    }
                }
            }
        }
    }
}

#[derive(Script, Widget)]
pub struct HagencyPanel {
    #[rust]
    navigation_ready: Cell<bool>,
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    active: bool,
    #[rust] ledger_open: bool,
    #[rust] ledger_preview: Value,
    #[rust] ledger_file: Option<robius_file_picker::LocalFile>,
    #[rust] usage_report: Value,
    #[rust] usage_agent: String,
    #[rust] usage_cursor: Option<String>,
    #[rust] file_page: Value,
    #[rust] file_detail: Value,
    #[rust] file_selected: String,
    #[rust] file_cursor: Option<String>,
    #[rust] history_page: Value,
    #[rust] history_detail: Value,
    #[rust] history_selected: String,
    #[rust] history_cursor: Option<String>,
    #[rust] stopped_page: Value,
    #[rust] recovery_review: Value,
    #[rust] recovery_selected: String,
    #[rust] recovery_cursor: Option<String>,
    #[rust] recovery_pending: Option<(String,RecoveryInput)>,
    #[rust]
    identity: String,
    #[rust]
    request: u64,
    #[rust]
    busy: bool,
    #[rust]
    members: Vec<Value>,
    #[rust]
    members_room: Option<(String, bool)>,
    #[rust]
    page: Page,
    #[rust]
    projects: Vec<Value>,
    #[rust]
    rooms: Vec<Value>,
    #[rust]
    agent_rooms: Vec<Value>,
    #[rust]
    spaces: Vec<Value>,
    #[rust]
    cursor: Option<String>,
    #[rust]
    agents: Vec<Value>,
    #[rust] runtime_list: RuntimeStates,
    #[rust] runtime_list_errors: ServiceErrors,
    #[rust] runtime_list_request:u64,
    #[rust] runtime_list_timer:Timer,
    #[rust] runtime_list_loading:bool,
    #[rust] display_blocked:bool,
    #[rust] detail_scroll_key:String,
    #[rust]
    agents_loaded: bool,
    #[rust] projects_loaded:bool,
    #[rust]
    agents_loading: bool,
    #[rust]
    bindings: Vec<Value>,
    #[rust]
    approvals: Vec<Value>,
    #[rust]
    policy: Value,
    #[rust]
    scope_state: Value,
    #[rust]
    creation: Option<Value>,
    #[rust]
    pending_input: Option<Value>,
    #[rust]
    create_room: bool,
    #[rust]
    wanted_project: Option<String>,
    #[rust]
    wanted_project_section: Option<(String, ProjectDetailSection)>,
    #[rust] wanted_new_room: bool,
    #[rust] wanted_open_rooms: bool,
    #[rust] has_registered_rooms:Option<bool>,
    #[rust] auto_default_project:Option<String>,
    #[rust] board_catalog: Vec<Value>,
    #[rust] selected_board: Option<String>,
    #[rust] wanted_board: Option<String>,
    #[rust] wanted_access: bool,
    #[rust] project_home: Value,
    #[rust] room_can_manage: bool,
    #[rust] board_creation: Option<(String,String)>,
    #[rust] show_archived_projects: bool,
    #[rust] all_projects: Vec<Value>,
    #[rust] header_stacked:Option<bool>,
    #[rust] board_creation_open:bool,
    #[rust]
    credential_ref: String,
    #[rust]
    provider_url: Option<String>,
    #[rust]
    provider_models: Vec<Value>,
    #[rust]
    provider_authenticated: bool,
    #[rust] provider_state:Value,
    #[rust] provider_timer:Timer,
    #[rust] provider_poll_count:u16,
    #[rust] mcp_editing:Option<String>,
    #[rust] mcp_json_open:bool,
    #[rust]
    runtime_efforts: Vec<String>,
    #[rust]
    model_advanced: bool,
    #[rust]
    service_advanced: bool,
    #[rust]
    room_advanced: bool,
    #[rust]
    runtime_room_active: bool,
    #[rust] agent_service_state: Value,
    #[rust] service_logs_open: bool,
    #[rust] runtime_pending_action: Option<bool>,
    #[rust] model_saving: bool,
    #[rust] model_save_message: String,
    #[rust] model_save_failed: bool,
    #[rust] runtime_action_error: String,
    #[rust] service_row_targets: ServiceRowTargets,
    #[rust] agent_card_targets: AgentCardTargets,
    #[rust] list_service_pending_agent: String,
    #[rust]
    runtime_resources_loading: bool,
    #[rust]
    runtime_resources_checked: bool,
    #[rust]
    agent_command: Option<(Value, String)>,
    #[rust]
    agent_pending_commands: Vec<Value>,
    #[rust] agent_request_targets: AgentCardTargets,
    #[rust] agent_creating: bool,
    #[rust]
    agent_journal_loaded: bool,
    #[rust] configuration_status: Value,
    #[rust] configuration_environment: Value,
    #[rust] configuration_timer: Timer,
    #[rust] configuration_discover: bool,
    #[rust] configuration_feedback: String,
    #[rust] configuration_metadata_error: String,
    #[rust] configuration_poll_request: Option<u64>,
    #[rust] configuration_keys: ConfigurationKeys,
    #[rust]
    execution_agent_record: Value,
    #[rust]
    execution_devices: Vec<Value>,
    #[rust]
    current_device_id: String,
    #[rust]
    devices_loaded: bool,
    #[rust] direct_agent: String,
    #[rust] direct_state: Value,
    #[rust] wanted_agent: Option<String>,
    #[rust] wanted_run: Option<(String,String)>,
    #[rust] wanted_approval: Option<approval_discovery::ApprovalTarget>,
    #[rust] wanted_model: Option<String>,
    #[rust] direct_timer: Timer,
    #[rust] direct_retry: Option<(String,String,u8)>,
    #[rust] approval_timer: Timer,
    #[rust] approval_poll: Option<u64>,
    #[rust] approval_started: Option<Instant>,
    #[rust] approval_notices: Notices,
    #[rust] remote_approval: Value,
    #[rust] agent_crypto: Value,
    #[rust] remote_approval_rooms: Vec<Value>,
    #[rust] remote_review_rooms: Vec<Value>,
    #[rust] remote_review_requests: Vec<Value>,
    #[rust] remote_review_next: Option<String>,
    #[rust] remote_review_cursors: HashSet<String>,
    #[rust] approval_checked: bool,
    #[rust] approval_blocked: HashSet<String>,
    #[rust] read_timer: Timer,
    #[rust] read_wait: Option<ReadWait>,
}
impl HagencyPanel {
    fn approval_scope(&self,cx:&mut Cx)->Option<(String,String,String)> {
        let agent=self.agent(cx)?;
        let b=self.selected(cx,ids!(bindings),&self.bindings)?;
        let binding=string(&b,"id").to_owned();let room=string(&b,"roomId").to_owned();
        if binding.is_empty() || room.is_empty() {return None;} Some((agent,binding,room))
    }
    fn clear_approval_state(&mut self,cx:&mut Cx) {
        self.approval_poll=None;self.approval_started=None;self.approval_checked=false;self.approval_notices=Default::default();
        cx.stop_timer(self.approval_timer);
        self.view.label(cx,ids!(approval_count)).set_text(cx,"");
        self.view.label(cx,ids!(approval_feedback)).set_text(cx,"");
        self.remote_approval=Value::Null;
        self.view.label(cx,ids!(remote_approval_details)).set_text(cx,"");
        self.remote_approval_rooms.clear();
        self.labels(cx,ids!(remote_approval_room),"Select a private approval Room",&self.remote_approval_rooms,"name");
    }
    fn approval_view_open(&self,cx:&mut Cx)->bool {
        self.active && self.page==Page::Agents && self.view.view(cx,ids!(agent_detail)).visible()
            && self.view.view(cx,ids!(runtime_section)).visible()
    }
    fn clear_remote_reviews(&mut self,cx:&mut Cx) {
        self.remote_review_requests.clear();self.remote_review_rooms.clear();self.remote_review_next=None;self.remote_review_cursors.clear();
        self.labels(cx,ids!(remote_review_room),"Select a private approval Room",&[],"name");
        self.render_remote_reviews(cx);
    }
    fn render_remote_reviews(&self,cx:&mut Cx) {
        let selected=self.selected(cx,ids!(remote_review_requests),&self.remote_review_requests).map(|v|string(&v,"requestEventId").to_owned());
        self.view.drop_down(cx,ids!(remote_review_requests)).set_labels(cx,std::iter::once(crate::i18n::tr("Select a remote approval request").to_owned()).chain(self.remote_review_requests.iter().map(|r|format!("{} · {}",string(&r["request"]["proposal"],"tool"),string(&r["request"]["proposal"]["scope"],"requester")))).collect());
        let index=selected.and_then(|id|self.remote_review_requests.iter().position(|r|string(r,"requestEventId")==id)).map_or(0,|i|i+1);
        self.view.drop_down(cx,ids!(remote_review_requests)).set_selected_item(cx,index);
        let row=self.selected(cx,ids!(remote_review_requests),&self.remote_review_requests);
        let details=row.as_ref().map(|r|super::approvals::details(&json!({"proposalId":r["request"]["approvalId"],"argsDigest":r["request"]["proposalDigest"],"proposal":r["request"]["proposal"]}),super::approvals::now())).unwrap_or_else(||crate::i18n::tr("Select a remote approval request").into());
        self.view.label(cx,ids!(remote_review_details)).set_text(cx,&details);
        let status=row.as_ref().map(|r|match r["publication"]["phase"].as_str() {Some("sent")=>"Decision delivered. Execution is not yet confirmed.",Some(_)=>"Decision delivery is unconfirmed. Refresh its original status; do not send again.",None if r["request"]["proposal"]["expires"].as_i64().is_none_or(|t|t<=super::approvals::now())=>"This approval request has expired.",None=>"Review the exact action before deciding."}).unwrap_or("");
        self.view.label(cx,ids!(remote_review_status)).set_text(cx,crate::i18n::tr(status));
    }
    fn refresh_remote_review_rooms(&mut self,cx:&mut Cx) {
        if !Service::for_current().is_ok_and(|s|s.identity()==self.identity) {return;}
        let selected=self.selected(cx,ids!(remote_review_room),&self.remote_review_rooms).map(|r|string(&r,"id").to_owned());
        self.remote_review_rooms=crate::sliding_sync::get_client().map(|client|client.joined_rooms().into_iter().filter(|r|!r.is_space() && r.encryption_state().is_encrypted() && r.joined_members_count()==1 && r.invited_members_count()==0).take(128).map(|r|json!({"id":r.room_id().as_str(),"name":r.cached_display_name().map(|n|n.to_string()).or_else(||r.name()).unwrap_or_else(||r.room_id().to_string())})).collect()).unwrap_or_default();
        self.labels(cx,ids!(remote_review_room),"Select a private approval Room",&self.remote_review_rooms,"name");
        let index=selected.and_then(|id|self.remote_review_rooms.iter().position(|r|string(r,"id")==id)).map(|i|i+1).unwrap_or_else(||usize::from(self.remote_review_rooms.len()==1));
        self.view.drop_down(cx,ids!(remote_review_room)).set_selected_item(cx,index);
    }
    fn load_remote_reviews(&mut self,cx:&mut Cx,older:bool) {
        let Some(agent)=self.agent(cx) else {return;};
        let Some(room)=self.selected(cx,ids!(remote_review_room),&self.remote_review_rooms) else {return;};
        let cursor=if older {let Some(c)=self.remote_review_next.clone() else {return;};if !self.remote_review_cursors.insert(c.clone()) {return;}Some(c)} else {self.remote_review_requests.clear();self.remote_review_cursors.clear();None};
        self.run(cx,Target::RemoteInbox,Command::RemoteApprovals {action:RemoteApprovalAction::List {agent,room:string(&room,"id").into(),cursor}});
    }
    fn remote_room(&self,cx:&mut Cx)->Option<String> {
        self.selected(cx,ids!(remote_approval_room),&self.remote_approval_rooms).map(|r|string(&r,"id").to_owned())
    }
    fn refresh_remote_rooms(&mut self,cx:&mut Cx) {
        let selected=self.remote_room(cx);
        // Cached room facts only choose candidates. The SDK adapter always
        // checks fresh private state and signed devices before publication.
        self.remote_approval_rooms=crate::sliding_sync::get_client().map(|client|client.joined_rooms().into_iter()
            .filter(|r|!r.is_space() && r.encryption_state().is_encrypted() && r.joined_members_count()==1 && r.invited_members_count()==0)
            .take(128).map(|r|json!({"id":r.room_id().as_str(),"name":r.cached_display_name().map(|n|n.to_string()).or_else(||r.name()).unwrap_or_else(||r.room_id().to_string())})).collect()).unwrap_or_default();
        if let Some(room)=self.remote_approval["remoteApproval"]["approvalRoom"].as_str()
            && !self.remote_approval_rooms.iter().any(|r|string(r,"id")==room) {
            self.remote_approval_rooms.push(json!({"id":room,"name":room}));
        }
        self.labels(cx,ids!(remote_approval_room),"Select a private approval Room",&self.remote_approval_rooms,"name");
        let index=selected.as_ref().and_then(|id|self.remote_approval_rooms.iter().position(|r|string(r,"id")==id)).map(|i|i+1)
            .unwrap_or_else(||usize::from(selected.is_none() && self.remote_approval_rooms.len()==1));
        self.view.drop_down(cx,ids!(remote_approval_room)).set_selected_item(cx,index);
    }
    fn refresh_approvals(&mut self,cx:&mut Cx) {
        if self.approval_poll.is_some() || self.busy || !self.approval_view_open(cx) || !self.local_agent(cx) {return;}
        let Some((agent,binding,room))=self.approval_scope(cx) else {return;};
        let Ok(service)=Service::for_current() else {return;};
        if service.identity()!=self.identity {return;}
        self.refresh_remote_rooms(cx);
        let request=nonce();self.approval_poll=Some(request);self.approval_started=Some(Instant::now());let identity=self.identity.clone();
        cx.stop_timer(self.approval_timer);self.approval_timer=cx.start_timeout(3.0);
        spawn_async_task(async move {
            // A UI waiting deadline must not cancel owner/device bootstrap work.
            let result=service.execute(Command::Local {
                agent_id:agent.clone(),action:LocalAction::Approvals,input:None,binding_id:Some(binding.clone()),requester:None,
            }).await;
            Cx::post_action(ApprovalsCompleted {identity,request,agent,binding,room,result});
        });
    }
    fn accepts_approval_poll(&self,cx:&mut Cx,done:&ApprovalsCompleted)->bool {
        self.approval_view_open(cx) && self.approval_poll==Some(done.request) && done.identity==self.identity
            && self.approval_scope(cx)==Some((done.agent.clone(),done.binding.clone(),done.room.clone()))
    }
    fn apply_approvals(&mut self,cx:&mut Cx,value:&Value) {
        let selected=self.selected(cx,ids!(approvals),&self.approvals).map(|v|string(&v,"proposalId").to_owned());
        let scope=self.approval_scope(cx);
        self.approvals=array(value,"approvals").into_iter().filter(|row|scope.as_ref().is_some_and(|(a,b,r)|super::approvals::matches(row,a,b,r,super::approvals::now()))).collect();
        self.approval_checked=true;
        let new=self.approval_notices.new_count(&self.approvals);
        if new>0 {
            crate::shared::popup_list::enqueue_popup_notification(
                crate::i18n::format("{count} new approvals in this Room. Review them in Service.",&[("count",new.to_string())]),
                crate::shared::popup_list::PopupKind::Warning,Some(8.0));
        }
        self.view.drop_down(cx,ids!(approvals)).set_labels(cx,std::iter::once(crate::i18n::tr("Select a pending approval").to_owned()).chain(self.approvals.iter().map(|v|format!("{} · {}",string(&v["proposal"],"tool"),string(&v["proposal"]["scope"],"requester")))).collect());
        let index=selected.and_then(|id|self.approvals.iter().position(|v|string(v,"proposalId")==id)).map(|i|i+1).unwrap_or(0);
        self.view.drop_down(cx,ids!(approvals)).set_selected_item(cx,index);
        if let Some(target)=self.wanted_approval.take() {
            let exact=self.approvals.iter().position(|row|target.matches(&self.identity,row,super::approvals::now()));
            self.view.drop_down(cx,ids!(approvals)).set_selected_item(cx,exact.map_or(0,|index|index+1));
            if exact.is_none() {
                self.view.label(cx,ids!(approval_feedback)).set_text(cx,crate::i18n::tr("This exact approval is no longer pending. No decision was submitted."));
            }
        }
        self.view.label(cx,ids!(approval_count)).set_text(cx,&crate::i18n::format("{count} pending approvals in this Room",&[("count",self.approvals.len().to_string())]));
        self.render_approval_details(cx);
        self.sync_navigation(cx);
    }
    fn render_approval_details(&self,cx:&mut Cx) {
        let text=self.selected(cx,ids!(approvals),&self.approvals).map(|p|super::approvals::details(&p,super::approvals::now())).unwrap_or_else(||crate::i18n::tr(if self.approvals.is_empty() {"No pending approvals."} else {"Select a request to review its exact action."}).into());
        self.view.label(cx,ids!(approval_details)).set_text(cx,&text);
        let same=self.selected(cx,ids!(approvals),&self.approvals).is_some_and(|p|p["proposalId"]==self.remote_approval["approvalId"]);
        if !same {self.view.label(cx,ids!(remote_approval_details)).set_text(cx,"");}
    }
    fn reset_detail_scroll(&mut self,cx:&mut Cx) {
        self.detail_scroll_key.clear();self.view.view(cx,ids!(details_scroll)).set_scroll_pos(cx,dvec2(0.0,0.0));
    }
    fn enter_detail(&mut self,cx:&mut Cx,kind:&str,id:&str) {
        self.navigation_ready.set(false);
        let key=format!("{}:{kind}:{id}",self.identity);
        if self.detail_scroll_key!=key {
            self.view.view(cx,ids!(details_scroll)).set_scroll_pos(cx,dvec2(0.0,0.0));self.detail_scroll_key=key;
        }
    }
    fn refresh_runtime_list(&mut self,cx:&mut Cx) {
        if self.runtime_list_loading || !self.active || self.page!=Page::Agents || !self.view.view(cx,ids!(agent_list_holder)).visible() {return;}
        let Ok(service)=Service::for_current() else {return;};
        if service.identity()!=self.identity {return;}
        let agents=self.agents.iter().filter_map(|a| {
            let state=agent_device_state(a,&self.current_device_id,&self.execution_devices,self.devices_loaded);
            matches!(state,AgentDeviceState::Here|AgentDeviceState::Other).then(||(string(a,"id").to_owned(),state==AgentDeviceState::Here))
        }).collect::<Vec<_>>();
        self.runtime_list_request=nonce(); let request=self.runtime_list_request; let identity=self.identity.clone();
        self.runtime_list_loading=true;self.view.redraw(cx);
        cx.stop_timer(self.runtime_list_timer);
        spawn_async_task(async move {
            let states=futures::future::join_all(agents.into_iter().map(|(agent,local)|{let service=service.clone(); async move {
                let command=if local {Command::Local {agent_id:agent.clone(),action:LocalAction::AgentServiceStatus,input:None,binding_id:None,requester:None}} else {Command::AgentConfigurationStatus {agent_id:agent.clone()}};
                let result=tokio::time::timeout(std::time::Duration::from_secs(15),service.execute(command)).await.unwrap_or_else(|_|Err("runtime_status_unavailable".into()));
                let result=result.map(|value|if local {value} else {value["configuration"].clone()});
                (agent,result)
            }})).await;
            Cx::post_action(RuntimeListCompleted {identity,request,states});
        });
    }
    fn local_agent(&self, cx: &mut Cx) -> bool {
        self.agent(cx).as_deref() == Some(string(&self.execution_agent_record, "id"))
            && agent_device_state(&self.execution_agent_record, &self.current_device_id, &self.execution_devices, self.devices_loaded) == AgentDeviceState::Here
    }
    fn remote_agent(&self, cx: &mut Cx) -> bool {
        self.agent(cx).as_deref() == Some(string(&self.execution_agent_record, "id"))
            && matches!(string(&self.execution_agent_record,"state"),"active"|"suspended"|"creating")
            && agent_device_state(&self.execution_agent_record, &self.current_device_id, &self.execution_devices, self.devices_loaded) == AgentDeviceState::Other
    }
    fn configuration_online(&self) -> bool {
        self.configuration_metadata_error.is_empty() && configuration_matches(&self.configuration_status, &self.execution_agent_record) && self.configuration_status["online"] == true
    }
    fn clear_configuration(&mut self, cx: &mut Cx) {
        self.configuration_status = Value::Null;
        self.configuration_environment = Value::Null;
        self.configuration_discover = false;
        self.configuration_poll_request=None;
        self.configuration_feedback.clear();
        self.configuration_metadata_error.clear();
        cx.stop_timer(self.configuration_timer);
        self.provider_models.clear(); self.runtime_efforts.clear(); self.credential_ref.clear();
        self.provider_authenticated = false; self.provider_url = None; self.provider_state=Value::Null;
        cx.stop_timer(self.provider_timer);self.provider_timer=Timer::default();self.provider_poll_count=0;
        self.mcp_editing=None;self.mcp_json_open=false;self.view.view(cx,ids!(mcp_form)).set_visible(cx,false);
        self.view.inline_notice(cx,ids!(mcp_feedback)).clear(cx);self.policy = Value::Null;
        self.render_models(cx);
        self.view.text_input(cx,ids!(workspace)).set_text(cx, "");
        self.view.label(cx,ids!(provider_status)).set_text(cx, "");
        self.view.label(cx,ids!(policy_usage)).set_text(cx, "");
    }
    fn finish_configuration_intent(&mut self, digest: Option<&String>, result: &Result<Value, String>) {
        if let Some(digest)=digest && result.as_ref().err().is_none_or(|error|!configuration_outcome_unconfirmed(error)) {
            self.configuration_keys.remove(digest);
        }
        match result {
            Ok(_) if digest.is_some()=>self.configuration_feedback=crate::i18n::tr("Settings saved on the execution device.").into(),
            Err(error)=> {
                if digest.is_some() {self.configuration_feedback=visible_error(error);}
                else {self.configuration_metadata_error=visible_error(error);}
                if configuration_connection_lost(error) {
                    self.configuration_status["online"]=false.into();
                    self.configuration_status["cached"]=true.into();
                }
            }
            Ok(_)=>self.configuration_metadata_error.clear(),
        }
    }
    fn load_configuration(&mut self, cx: &mut Cx) {
        let Some(agent_id) = self.agent(cx) else {return;};
        self.configuration_poll_request=None;
        self.configuration_discover = true;
        self.run(cx,Target::ConfigurationStatus,Command::AgentConfigurationStatus {agent_id});
    }
    fn poll_configuration_status(&mut self, cx: &mut Cx) {
        if self.busy || self.configuration_poll_request.is_some() || !self.remote_agent(cx) {self.configuration_timer=cx.start_timeout(3.0);return;}
        let Some(agent)=self.agent(cx) else {return;};
        let Ok(service)=Service::for_current() else {return;};
        if service.identity()!=self.identity {return;}
        let request=nonce();self.configuration_poll_request=Some(request);
        let identity=self.identity.clone();
        let generation=self.execution_agent_record["generation"].as_i64().unwrap_or(0);
        let device=string(&self.execution_agent_record,"executionDeviceId").to_owned();
        spawn_async_task(async move {
            let result=service.execute(Command::AgentConfigurationStatus {agent_id:agent.clone()}).await;
            Cx::post_action(ConfigurationCompleted {identity,request,agent,generation,device,result});
        });
    }
    fn render_configuration(&self, cx: &mut Cx) {
        let remote = self.remote_agent(cx);
        self.view.label(cx,ids!(configuration_notice)).set_visible(cx, remote);
        self.view.button(cx,ids!(configuration_refresh)).set_visible(cx,remote);
        if !remote {self.view.label(cx,ids!(workspace_label)).set_text(cx,crate::i18n::tr("Agent workspace on this device"));return;}
        let state = if !self.configuration_metadata_error.is_empty() {self.configuration_metadata_error.as_str()}
            else if !configuration_matches(&self.configuration_status,&self.execution_agent_record) {crate::i18n::tr("Checking execution device environment…")}
            else if self.configuration_status["online"] == true {crate::i18n::tr("Execution device online")}
            else {crate::i18n::tr("Execution device offline. Cached settings are read-only.")};
        let freshness = if self.configuration_status["cached"] == true {crate::i18n::tr("Cached observation")} else {crate::i18n::tr("Current observation")};
        let timestamp = |field:&str| self.configuration_status[field].as_i64()
            .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
            .map(|at|at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S %:z").to_string())
            .unwrap_or_else(||crate::i18n::tr("Not observed yet").into());
        let text=crate::i18n::format("{device} · {state}\n{freshness}: {observed} · Last heartbeat: {heartbeat}", &[
            ("device",self.device_badge(&self.execution_agent_record)),("state",state.into()),("freshness",freshness.into()),
            ("observed",timestamp("observedAtMs")),("heartbeat",timestamp("lastSeenAtMs"))]);
        let text=if self.configuration_feedback.is_empty() {text} else {format!("{text}\n{}",self.configuration_feedback)};
        self.view.label(cx,ids!(configuration_notice)).set_text(cx,&text);
        self.view.label(cx,ids!(workspace_label)).set_text(cx,&crate::i18n::format("Agent workspace on {device}",&[("device",self.device_badge(&self.execution_agent_record))]));
    }
    fn device_badge(&self, agent: &Value) -> String {
        let state = agent_device_state(agent, &self.current_device_id, &self.execution_devices, self.devices_loaded);
        let label = crate::i18n::tr(match state {
            AgentDeviceState::Here => "This device",
            AgentDeviceState::Other => "Another device",
            AgentDeviceState::Unassigned => "Not assigned",
            AgentDeviceState::Unavailable => "Device unavailable",
            AgentDeviceState::Checking => "Checking execution device…",
        });
        if matches!(state, AgentDeviceState::Here | AgentDeviceState::Other | AgentDeviceState::Unavailable) {
            let name = execution_device_name(&self.execution_devices,string(agent,"executionDeviceId"));
            if !name.is_empty() {
                if state==AgentDeviceState::Other {return format!("{label} · {name} · {}",crate::i18n::tr(remote_presence_label(self.runtime_list.get(string(agent,"id")),agent)));}
                return format!("{label} · {name}");
            }
        }
        label.to_owned()
    }
    fn load_runtime_resources(&mut self, cx: &mut Cx) {
        if !self.local_agent(cx) {self.status(cx, "This Agent is not handled on this device. Assign it here before configuring local resources.");return;}
        self.runtime_resources_loading = true;
        self.runtime_resources_checked = false;
        self.runtime_efforts.clear();
        self.policy = Value::Null;
        self.view.drop_down(cx, ids!(effort)).set_labels(cx, vec!["Checking model choices…".into()]);
        self.view.label(cx, ids!(runtime_status)).set_text(cx, crate::i18n::tr("Checking the saved model and connected local account…"));
        self.local(cx, Target::Policy, LocalAction::AgentModel, None);
    }
    fn cancel_read_wait(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.read_timer);
        self.read_timer = Timer::default();
        self.read_wait = None;
    }
    fn arm_read_wait(&mut self, cx: &mut Cx, command: &Command) {
        self.cancel_read_wait(cx);
        if super::backend::management_read(command) {
            self.read_wait = Some(ReadWait {
                identity: self.identity.clone(), request: self.request,
                deadline: std::time::Instant::now() + MANAGEMENT_READ_WAIT,
            });
            self.read_timer = cx.start_timeout(MANAGEMENT_READ_WAIT.as_secs_f64());
        }
    }
    fn expire_read_wait(&mut self, cx: &mut Cx, now: std::time::Instant) -> bool {
        let Some(wait) = &self.read_wait else { return false; };
        if now < wait.deadline { return false; }
        let current = self.active && self.busy && wait.identity == self.identity && wait.request == self.request;
        self.cancel_read_wait(cx);
        if !current { return false; }
        // Do not cancel the account-scoped task: first-read authorization may
        // already have side effects. Only stop accepting its old UI response.
        self.request = nonce();
        self.agents_loading = false;
        if self.runtime_resources_loading {self.runtime_resources_loading=false;self.runtime_resources_checked=false;}
        self.set_busy(cx, false);
        self.status(cx, crate::i18n::tr(user_error("management_request_unavailable")));
        true
    }
    fn cancel_direct_retry(&mut self, cx:&mut Cx) {
        cx.stop_timer(self.direct_timer);
        self.direct_timer = Timer::default();
        self.direct_retry = None;
    }
    fn schedule_direct_retry(&mut self, cx:&mut Cx) {
        cx.stop_timer(self.direct_timer);
        let safe = self.direct_state["state"] == "pending"
            && matches!(string(&self.direct_state,"phase"),"agent_provisioning"|"waiting_identity"|"server_joining"|"pending")
            && self.direct_state["lastError"].as_str().is_none();
        if !safe || self.direct_agent.is_empty() {self.cancel_direct_retry(cx);return;}
        let attempts = self.direct_retry.as_ref().filter(|(identity,agent,_)| identity == &self.identity && agent == &self.direct_agent).map_or(0,|(_,_,n)|*n);
        if attempts >= 8 {self.cancel_direct_retry(cx);return;}
        self.direct_retry=Some((self.identity.clone(),self.direct_agent.clone(),attempts+1));
        self.direct_timer=cx.start_timeout(2.0_f64.powi(i32::from(attempts.min(3))));
    }
    fn render_models(&mut self, cx:&mut Cx) {
        let codex = self.provider_runtime(cx)=="codex";
        let octos = self.provider_runtime(cx)=="octos";
        self.view.label(cx,ids!(model_label)).set_text(cx,crate::i18n::tr(if octos {"Octos profile"} else {"Local model"}));
        self.view.view(cx,ids!(effort_holder)).set_visible(cx,!octos);
        self.view.view(cx,ids!(web_search_holder)).set_visible(cx,codex);
        let programming_here=codex && self.local_agent(cx);
        self.view.view(cx,ids!(programming_holder)).set_visible(cx,programming_here);
        let configured = string(&self.policy["modelProfile"],"model");
        let previous = self.selected(cx,ids!(model),&self.provider_models).map(|m|string(&m,"model").to_owned());
        let selected = self.provider_models.iter().position(|m| string(m,"model") == configured && (string(m,"credentialRef").is_empty() || string(m,"credentialRef") == string(&self.policy["modelProfile"],"credential_ref")))
            .or_else(|| previous.as_deref().and_then(|id|self.provider_models.iter().position(|m|string(m,"model")==id)))
            .or_else(||self.provider_models.iter().position(|m|m["isDefault"] == true));
        let labels=std::iter::once(crate::i18n::tr(if octos {"Select an Octos profile"} else {"Select a model"}).to_owned()).chain(self.provider_models.iter().map(|m|format!("{}{}",string(m,"displayName"),if m["isDefault"] == true {" (default)"} else {""}))).collect();
        self.view.drop_down(cx,ids!(model)).set_labels(cx,labels);
        self.view.drop_down(cx,ids!(model)).set_selected_item(cx,selected.map_or(0,|i|i+1));
        self.render_efforts(cx);
        self.render_mcp(cx);
    }
    fn render_efforts(&mut self, cx:&mut Cx) {
        let model=self.selected(cx,ids!(model),&self.provider_models);
        self.runtime_efforts=model.as_ref().map(|m|array(m,"supportedReasoningEfforts").iter().filter_map(|r|r["reasoningEffort"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
        let default=model.as_ref().map(|m|string(m,"defaultReasoningEffort")).unwrap_or("");
        let saved = if model.as_ref().is_some_and(|m|m["model"] == self.policy["modelProfile"]["model"]) {string(&self.policy["modelProfile"],"reasoning_effort")} else {""};
        let selected=self.runtime_efforts.iter().position(|s|s==saved).or_else(||self.runtime_efforts.iter().position(|s|s==default)).unwrap_or(0);
        let octos = self.provider_runtime(cx)=="octos";
        self.view.drop_down(cx,ids!(effort)).set_labels(cx,if self.runtime_efforts.is_empty(){vec![crate::i18n::tr("Load this model's reasoning choices").into()]}else{self.runtime_efforts.iter().map(|effort|format!("{}{}",if octos {crate::i18n::tr("Use profile settings")} else {effort},if effort==default {crate::i18n::tr(" (default)")} else {""})).collect()});
        self.view.drop_down(cx,ids!(effort)).set_selected_item(cx,selected);
        self.render_effort_note(cx);
        self.view.label(cx,ids!(model_note)).set_text(cx,model.as_ref().map(|m|string(m,"description")).unwrap_or_else(||crate::i18n::tr("Connect a local runtime and refresh its models or profiles. Availability is confirmed when an actual request runs.")));
    }
    fn render_effort_note(&self, cx:&mut Cx) {
        let effort=self.runtime_efforts.get(self.view.drop_down(cx,ids!(effort)).selected_item());
        let model=self.selected(cx,ids!(model),&self.provider_models);
        let description=model.as_ref().and_then(|m|array(m,"supportedReasoningEfforts").into_iter().find(|e|e["reasoningEffort"].as_str()==effort.map(String::as_str))).map(|e|string(&e,"description").to_owned()).unwrap_or_default();
        self.view.label(cx,ids!(effort_note)).set_text(cx,&description);
        self.view.label(cx,ids!(runtime_model_settings)).set_text(cx,&format!("{} · {}",string(&self.policy["modelProfile"],"model"),effort.map(String::as_str).unwrap_or("")));
    }
    fn render_room_advanced(&self, cx: &mut Cx) {
        self.view.view(cx, ids!(room_advanced_content)).set_visible(cx, self.room_advanced);
        let mut toggle = self.view.button(cx, ids!(room_advanced_toggle));
        if self.room_advanced {script_apply_eval!(cx, toggle, {draw_icon.svg: mod.widgets.ICON_CHEVRON_DOWN});}
        else {script_apply_eval!(cx, toggle, {draw_icon.svg: mod.widgets.ICON_CHEVRON_RIGHT});}
    }
    fn render_service_advanced(&self, cx:&mut Cx) {
        self.view.view(cx,ids!(service_advanced)).set_visible(cx,self.service_advanced);
        let mut toggle=self.view.button(cx,ids!(service_advanced_toggle));
        if self.service_advanced {script_apply_eval!(cx,toggle,{draw_icon.svg: mod.widgets.ICON_CHEVRON_DOWN});}
        else {script_apply_eval!(cx,toggle,{draw_icon.svg: mod.widgets.ICON_CHEVRON_RIGHT});}
    }
    fn render_model_advanced(&self, cx:&mut Cx) {
        self.view.view(cx,ids!(advanced_settings)).set_visible(cx,self.model_advanced);
        self.render_provider_controls(cx);
        let mut toggle=self.view.button(cx,ids!(advanced_toggle));
        if self.model_advanced {script_apply_eval!(cx,toggle,{draw_icon.svg: mod.widgets.ICON_CHEVRON_DOWN});}
        else {script_apply_eval!(cx,toggle,{draw_icon.svg: mod.widgets.ICON_CHEVRON_RIGHT});}
    }
    fn status(&self, cx: &mut Cx, message: &str) {
        let loading = message == "Loading…";
        self.view.widget(cx, ids!(loading_status)).set_visible(cx, loading);
        let notice=self.view.inline_notice(cx,ids!(status_holder));
        if message.is_empty() || loading {notice.clear(cx);}
        else {
            let kind=if message.starts_with("Enter ") || message.starts_with("Choose ") || message.starts_with("Select ") || message.starts_with("Confirm ") {PopupKind::Warning} else {PopupKind::Info};
            notice.show(cx,kind,"Please check",message,"",true);
        }
    }
    fn show_failure(&self,cx:&mut Cx,target:Target,code:&str,message:&str) {
        self.view.widget(cx,ids!(loading_status)).set_visible(cx,false);
        let uncertain=matches!(target,Target::AgentCreated | Target::AgentBound | Target::AgentResumed) && matches!(code,
            "server_unavailable" | "invalid_server_response" | "invalid_native_response" | "native_operation_failed" | "agent_command_outcome_unknown" | "agent_command_state_unavailable");
        let title = if uncertain {"Check the request result"} else {match target {
            Target::AgentCreated => "Could not create Agent",
            Target::AgentBound => "Could not add Room",
            Target::AgentResumed => "Could not retry request",
            Target::AgentCommandChecked => "Could not check request status",
            Target::ModelSaved => "Could not save model",
            _ => "Could not complete this action",
        }};
        let suffix=format!(" ({code})");
        let message=message.strip_suffix(&suffix).unwrap_or(message);
        let details=if !code.is_empty() && code.len()<=80 && code.bytes().all(|b|b.is_ascii_lowercase() || b.is_ascii_digit() || b==b'_') {
            crate::i18n::format("Error code: {code}",&[("code",code.into())])
        } else {String::new()};
        let notice=self.view.inline_notice(cx,ids!(status_holder));
        notice.show(cx,if uncertain {PopupKind::Warning} else {PopupKind::Error},title,message,&details,true);
        if uncertain || code=="agent_command_recovery_required" {notice.set_action(cx,"View requests");}
    }
    fn input(&self, cx: &mut Cx, id: &[LiveId]) -> String {
        self.view.text_input(cx, id).text().trim().to_owned()
    }
    fn labels(&self, cx: &mut Cx, id: &[LiveId], prompt: &str, rows: &[Value], key: &str) {
        self.view.drop_down(cx, id).set_labels(
            cx,
            std::iter::once(crate::i18n::tr(prompt).to_owned())
                .chain(rows.iter().map(|v| {
                    let name = string(v, "name");
                    let name = if name.is_empty() {
                        string(v, "displayName")
                    } else {
                        name
                    };
                    let identity = string(v, key);
                    if name.is_empty() {
                        identity.into()
                    } else {
                        format!("{name} · {identity}")
                    }
                }))
                .collect(),
        );
        self.view.drop_down(cx, id).set_selected_item(cx, 0);
    }
    fn selected(&self, cx: &mut Cx, id: &[LiveId], rows: &[Value]) -> Option<Value> {
        self.view
            .drop_down(cx, id)
            .selected_item()
            .checked_sub(1)
            .and_then(|i| rows.get(i))
            .cloned()
    }
    fn project(&self, cx: &mut Cx) -> Option<String> {
        self.selected(cx, ids!(projects), &self.projects)
            .map(|v| string(&v, "id").into())
    }
    fn agent(&self, cx: &mut Cx) -> Option<String> {
        self.selected(cx, ids!(agents), &self.agents)
            .map(|v| string(&v, "id").into())
    }
    fn binding(&self, cx: &mut Cx) -> Option<String> {
        self.selected(cx, ids!(bindings), &self.bindings)
            .map(|v| string(&v, "id").into())
    }
    fn binding_label(&self, cx:&mut Cx, binding:&Value)->String {
        if string(binding,"scopeKind") == "owner_direct" {return crate::i18n::tr("Private chat with this Agent").into();}
        let id=string(binding,"roomId");
        let room=matrix_sdk::ruma::RoomId::parse(id).ok();
        let name=room.as_ref().and_then(|room| crate::sliding_sync::get_client().and_then(|client|client.get_room(room)))
            .and_then(|room|room.cached_display_name().or_else(||room.name().map(matrix_sdk::RoomDisplayName::Named)))
            .map(|name|name.to_string())
            .or_else(||room.as_ref().and_then(|room| {
                if cx.has_global::<crate::home::rooms_list::RoomsListRef>() {cx.global::<crate::home::rooms_list::RoomsListRef>().get_room_name(room).map(|name|name.to_string())} else {None}
            }));
        name.filter(|name|!name.is_empty()).unwrap_or_else(||id.into())
    }
    fn label_bindings(&self,cx:&mut Cx,id:&[LiveId]) {
        let labels=std::iter::once(crate::i18n::tr("Select a Room").into()).chain(self.bindings.iter().map(|binding|self.binding_label(cx,binding))).collect();
        self.view.drop_down(cx,id).set_labels(cx,labels);
        self.view.drop_down(cx,id).set_selected_item(cx,0);
    }
    fn select_policy_room(&mut self, cx: &mut Cx, index: usize) {
        self.view.drop_down(cx, ids!(bindings)).set_selected_item(cx, index);
        self.view.drop_down(cx, ids!(policy_binding)).set_selected_item(cx, index);
        self.policy = Value::Null;
        self.runtime_room_active=false;self.agent_service_state=Value::Null;self.service_row_targets.clear();self.runtime_pending_action=None;self.runtime_action_error.clear();
        self.view.text_input(cx,ids!(reservation)).set_text(cx,"");
        self.clear_approval_state(cx);
        self.approvals.clear();
        self.labels(cx,ids!(approvals),"Select a pending approval",&self.approvals,"proposalId");
        self.view.label(cx,ids!(approval_details)).set_text(cx,"");
        self.view.label(cx, ids!(runtime_status)).set_text(cx,crate::i18n::tr("Refresh status to check this Room."));
        self.view.label(cx, ids!(policy_usage)).set_text(cx, "");
    }
    fn set_busy(&mut self, cx: &mut Cx, busy: bool) {
        self.navigation_ready.set(false);
        self.busy = busy;
        for id in [
            ids!(back),
            ids!(projects),
            ids!(rooms),
            ids!(agents),
            ids!(bindings),
            ids!(policy_binding),
            ids!(agent_project),
            ids!(agent_room),
            ids!(creation_source),
            ids!(candidates),
            ids!(policy_layer),
            ids!(limit_mode),
            ids!(period),
            ids!(requests),
            ids!(tool_risk),
            ids!(approvals),
            ids!(assignment_confirm),
            ids!(assign_current_device),
            ids!(requester),
            ids!(creation_name),
            ids!(existing_room),
            ids!(agent_name),
            ids!(token_limit),
            ids!(allowed_tools),
            ids!(allowed_directories),
            ids!(model),
            ids!(workspace),
            ids!(prepare_workspace),
            ids!(refresh_models),
            ids!(reservation),
            ids!(effort),
            ids!(found_room),
            ids!(takeover),
            ids!(host_files),
            ids!(web_search),
            ids!(programming),
            ids!(mcp_content),
            ids!(ledger_path), ids!(ledger_confirm), ids!(ledger_choose), ids!(ledger_toggle), ids!(ledger_export), ids!(ledger_inspect), ids!(ledger_restore),
            ids!(usage_scope),ids!(usage_room),ids!(usage_requester),ids!(usage_group),ids!(usage_from),ids!(usage_until),ids!(usage_refresh),ids!(usage_alerts),ids!(usage_ack),ids!(usage_latest),ids!(usage_older),
            ids!(history_records),ids!(history_refresh),ids!(history_older),ids!(history_copy),
            ids!(recovery_copy),ids!(recovery_records),ids!(recovery_refresh),ids!(recovery_older),ids!(recovery_paths),ids!(recovery_inspect),ids!(recovery_note),ids!(recovery_instruction),ids!(recovery_keep),ids!(recovery_accept),ids!(recovery_continue),ids!(recovery_confirm),ids!(recovery_cancel),
        ] {
            self.view.widget(cx, id).set_disabled(cx, busy);
        }
    }
    fn list_page(&self, cx: &mut Cx) {
        self.navigation_ready.set(false);
        self.status(cx, "");
        self.view.label(cx, ids!(heading)).set_text(
            cx,
            crate::i18n::tr(if self.page == Page::Projects {"Projects"} else {"My Agents"}),
        );
        self.view
            .view(cx, ids!(project_list_holder))
            .set_visible(cx, true);
        self.view
            .view(cx, ids!(project_detail))
            .set_visible(cx, false);
        self.view
            .view(cx, ids!(project_form))
            .set_visible(cx, false);
        self.view
            .view(cx, ids!(agent_list_holder))
            .set_visible(cx, true);
        self.view
            .view(cx, ids!(agent_detail))
            .set_visible(cx, false);
        self.view
            .view(cx, ids!(agent_create))
            .set_visible(cx, false);
    }
    fn project_detail(&mut self, cx: &mut Cx, index: usize) {
        let Some(p) = self.projects.get(index).cloned() else {
            return;
        };
        self.enter_detail(cx,"project",string(&p,"id"));
        let mut heading=self.view.label(cx,ids!(heading));script_apply_eval!(cx,heading,{i18n_text:""});
        self.view
            .label(cx, ids!(heading))
            .set_text(cx, string(&p, "name"));
        let requested = self.wanted_project_section.as_ref()
            .filter(|(project, _)| project == string(&p, "id"))
            .map(|(_, section)| *section);
        self.clear_members(cx);
        self.scope_state = Value::Null;
        self.project_home = Value::Null;
        self.render_overview_stats(cx);
        self.view.label(cx,ids!(overview_activity)).set_text(cx,crate::i18n::tr("Loading…"));
        self.view.label(cx,ids!(overview_activity)).set_visible(cx,true);
        self.room_can_manage=false;
        self.board_catalog.clear();
        self.selected_board = None;
        self.board_creation = None;
        self.board_creation_open=false;
        self.view.button(cx,ids!(permissions_tab)).set_visible(cx,false);
        self.view.button(cx,ids!(archive_project)).set_visible(cx,false);
        self.view
            .drop_down(cx, ids!(projects))
            .set_selected_item(cx, index + 1);
        self.project_section(cx, match requested {
            Some(ProjectDetailSection::Access) => ids!(overview_section),
            Some(ProjectDetailSection::Board) => ids!(board_section),
            Some(ProjectDetailSection::Rooms) => ids!(rooms_section),
            Some(ProjectDetailSection::Members) => ids!(members_section),
            _ => ids!(overview_section),
        });

        self.view
            .view(cx, ids!(project_list_holder))
            .set_visible(cx, false);
        self.view
            .view(cx, ids!(project_form))
            .set_visible(cx, false);
        self.view
            .view(cx, ids!(project_detail))
            .set_visible(cx, true);
        let topic=string(&p,"topic");
        self.view.label(cx,ids!(project_details)).set_text(cx,topic);
        self.view.label(cx,ids!(project_details)).set_visible(cx,!topic.is_empty());
        self.rooms.clear();self.has_registered_rooms=None;
        self.labels(cx, ids!(rooms), "Select a Room", &self.rooms, "roomId");
        self.view.label(cx, ids!(room_details)).set_text(cx, "");
        self.run(
            cx,
            Target::Rooms,
            Command::ProjectRooms {
                project_id: string(&p, "id").into(),
            },
        );
    }
    fn begin_agent_creation(&mut self, cx: &mut Cx) {
        self.status(cx, "");
        self.agent_command=None;
        self.agent_creating=false;
        self.view.text_input(cx,ids!(agent_name)).set_text(cx,"");
        self.view.drop_down(cx, ids!(new_agent_visibility)).set_selected_item(cx,0);
        self.view.view(cx, ids!(agent_name_holder)).set_visible(cx,true);
        self.view.button(cx, ids!(create_agent)).set_visible(cx,true);
        self.view.button(cx, ids!(bind_agent)).set_visible(cx,false);
        self.view.view(cx, ids!(agent_list_holder)).set_visible(cx,false);
        self.view.view(cx, ids!(agent_create)).set_visible(cx,true);
    }
    fn agent_detail(&mut self, cx: &mut Cx, index: usize) {
        self.reset_file_history(cx);
        let Some(a) = self.agents.get(index).cloned() else {
            return;
        };
        self.status(cx, "");
        self.wanted_model=None;
        self.service_logs_open=false;
        self.view.view(cx,ids!(service_logs_panel)).set_visible(cx,false);
        self.model_saving=false;
        self.clear_model_save_feedback(cx);
        self.enter_detail(cx,"agent",string(&a,"id"));
        self.view.view(cx,ids!(details_scroll)).set_scroll_pos(cx,dvec2(0.0,0.0));
        self.model_advanced=false;
        self.service_advanced=false;
        self.room_advanced=false;
        self.render_room_advanced(cx);
        self.runtime_room_active=false;self.agent_service_state=Value::Null;self.service_row_targets.clear();self.runtime_pending_action=None;self.runtime_action_error.clear();
        self.render_service_advanced(cx);
        self.render_model_advanced(cx);
        let mut heading=self.view.label(cx,ids!(heading));script_apply_eval!(cx,heading,{i18n_text:""});
        self.view
            .label(cx, ids!(heading))
            .set_text(cx, string(&a, "displayName"));
        self.view
            .drop_down(cx, ids!(agents))
            .set_selected_item(cx, index + 1);
        self.view
            .view(cx, ids!(agent_list_holder))
            .set_visible(cx, false);
        self.view
            .view(cx, ids!(agent_create))
            .set_visible(cx, false);
        self.view.view(cx, ids!(agent_detail)).set_visible(cx, true);
        self.section(cx, ids!(setup_section));
        self.view
            .drop_down(cx, ids!(policy_layer))
            .set_selected_item(cx, 0);
        for id in [
            ids!(retire_confirm),
            ids!(leave_confirm),
            ids!(takeover),
            ids!(host_files),
        ] {
            self.view
                .check_box(cx, id)
                .set_active(cx, false, Animate::No);
        }
        self.policy = Value::Null;
        self.runtime_efforts.clear();
        self.runtime_resources_checked = false;
        self.runtime_resources_loading = false;
        self.clear_configuration(cx);
        self.execution_agent_record = Value::Null;
        self.agent_crypto = Value::Null;
        self.render_crypto(cx);

        if self.direct_agent != string(&a,"id") {self.cancel_direct_retry(cx);}
        self.render_owner_direct(cx);
        self.clear_remote_reviews(cx);
        self.view.view(cx, ids!(execution_badge)).label(cx, ids!(device_label)).set_text(cx, crate::i18n::tr("Checking execution device…"));
        self.view.check_box(cx, ids!(assignment_confirm)).set_active(cx, false, Animate::No);
        self.clear_approval_state(cx);
        self.approvals.clear();
        self.labels(
            cx,
            ids!(approvals),
            "Select a pending approval",
            &[],
            "proposalId",
        );
        self.view.label(cx, ids!(approval_details)).set_text(cx, "");
        self.view
            .label(cx, ids!(runtime_status))
            .set_text(cx, crate::i18n::tr("Select a Room and refresh its runtime status."));
        self.view.label(cx, ids!(room_roster)).set_text(cx, "");
        self.bindings.clear();
        self.view.drop_down(cx, ids!(agent_visibility)).set_selected_item(cx, usize::from(a["visibility"] == "public"));
        self.view.view(cx, ids!(detail_audience_badge)).label(cx, ids!(device_label)).set_text(cx, crate::i18n::tr(audience_label(&a)));
        self.view.label(cx, ids!(agent_details)).set_text(
            cx,
            &format!(
                "{}\n{}",
                string(&a, "state"),
                string(&a, "puppetMxid")
            ),
        );
        self.run(
            cx,
            Target::AgentDetails,
            Command::AgentDetails {agent_id: string(&a, "id").into()},
        );
    }
    fn render_owner_direct(&self,cx:&mut Cx) {
        let exact=self.agent(cx).is_some_and(|a|a == self.direct_agent);
        let state=if exact {string(&self.direct_state,"state")} else {""};
        self.view.label(cx,ids!(owner_direct_status)).set_text(cx,match state {
            "active"=>"Your private chat with this Agent is ready.",
            "pending"=>"Your Agent exists. Private chat setup is still pending; retry its original setup when connected.",
            _=>"Set up a private chat with this Agent. Retrying preserves the original setup request.",
        });
        self.view.button(cx,ids!(open_owner_direct)).set_enabled(cx,exact && state == "active" && !string(&self.direct_state,"roomId").is_empty() && !self.busy);
    }
    fn project_section(&self, cx: &mut Cx, selected: &[LiveId]) {
        let project=self.project(cx);
        if self.active && !self.identity.is_empty() && let Some(project_id)=&project {
            use crate::home::project_tree::{ProjectSelection,ProjectSelectionChanged};
            let target=if selected==[id!(board_section)] {self.selected_board.clone().map(ProjectSelection::Board).unwrap_or(ProjectSelection::Project)}else{ProjectSelection::Project};
            cx.action(ProjectSelectionChanged {identity:self.identity.clone(),project_id:project_id.clone(),target});
        }
        self.view.project_board(cx,ids!(board)).bind_scope(cx,&self.identity,project.as_deref(),self.selected_board.as_deref(),self.active && self.selected_board.is_some() && selected==[id!(board_section)]);
        for id in [
            ids!(board_section),
            ids!(overview_section),
            ids!(rooms_section),
            ids!(members_section),
            ids!(permissions_section),
        ] {
            self.view.view(cx, id).set_visible(cx, id == selected);
        }
        for (tab, section) in [
            (ids!(board_tab), ids!(board_section)),
            (ids!(overview_tab), ids!(overview_section)),
            (ids!(rooms_tab), ids!(rooms_section)),
            (ids!(members_tab), ids!(members_section)),
            (ids!(permissions_tab), ids!(permissions_section)),
        ] {
            let mut button = self.view.button(cx, tab);
            if section == selected {
                script_apply_eval!(cx, button, { draw_bg +: {color: mod.widgets.RINX_ACCENT color_hover: mod.widgets.RINX_ACCENT_HOVER color_down: mod.widgets.RINX_ACCENT_DOWN}
                    draw_text +: {color: mod.widgets.RINX_ON_ACCENT color_hover: mod.widgets.RINX_ON_ACCENT color_down: mod.widgets.RINX_ON_ACCENT} });
            } else {
                script_apply_eval!(cx, button, { draw_bg +: {color: mod.widgets.RINX_SURFACE color_hover: mod.widgets.RINX_HOVER color_down: mod.widgets.RINX_PRESSED}
                    draw_text +: {color: mod.widgets.RINX_INK color_hover: mod.widgets.RINX_INK color_down: mod.widgets.RINX_INK} });
            }
        }
    }
    fn clear_members(&mut self, cx: &mut Cx) {
        self.members.clear();
        self.members_room = None;
        self.view.label(cx, ids!(members_status)).set_text(cx, "");
        self.view.button(cx,ids!(invite_member)).set_visible(cx,false);
        self.view
            .button(cx, ids!(invite_member))
            .set_enabled(cx, false);
        self.view
            .button(cx, ids!(invite_member))
            .set_disabled(cx, true);
    }
    fn show_members(&mut self, cx: &mut Cx, room: String, space: bool) {
        if self.busy || !self.active {
            return;
        }
        let service = match Service::for_current() {
            Ok(service) if service.identity() == self.identity => service,
            _ => {
                self.status(
                    cx,
                    "Your account changed. Reopen Projects for the current account.",
                );
                return;
            }
        };
        self.clear_members(cx);
        self.members_room = Some((room.clone(), space));
        self.project_section(cx, ids!(members_section));
        self.view.label(cx, ids!(members_heading)).set_text(
            cx,
            if space {
                "Project members"
            } else {
                "Discussion Room members"
            },
        );
        self.view.label(cx, ids!(members_note)).set_visible(cx,false);
        self.view.label(cx, ids!(members_note)).set_text(cx, if space {
            "Members of this Project's Space. Each discussion Room has its own members and invitations."
        } else { "Only this discussion Room's members and invitations are shown." });
        self.view
            .label(cx, ids!(members_status))
            .set_text(cx, crate::i18n::tr("Loading members…"));
        self.request = nonce();
        let request = self.request;
        let identity = self.identity.clone();
        self.set_busy(cx, true);
        spawn_async_task(async move {
            let result = service.room_members(room, space).await;
            Cx::post_action(Completed {
                identity,
                request,
                target: Target::Members,
                configuration_fence: None,
                configuration_digest: None,
                result,
            });
        });
    }
    #[cfg(feature = "agent_chat")]
    fn approval_chat_actions(&self, cx: &mut Cx, actions: &Actions) -> bool {
        if self.active && !self.busy && !self.display_blocked && self.page == Page::Agents
            && self.view.view(cx,ids!(agent_list_holder)).visible()
            && self.view.button(cx,ids!(agent_approval_chats)).clicked(actions) {
            cx.action(crate::agent_chat::approval_inbox::ApprovalInboxAction::Open {project: None});
            return true;
        }
        false
    }

    fn sync_navigation(&self, cx: &mut Cx) {
        let creation_locked=self.busy || self.creation.as_ref().is_some_and(|c|c["phase"]!="complete");
        self.view.widget(cx,ids!(creation_encrypted)).set_disabled(cx,creation_locked);
        if creation_locked && let Some(input)=&self.pending_input {
            self.view.check_box(cx,ids!(creation_encrypted)).set_active(cx,input["encrypted"]==true,Animate::No);
        }
        self.render_project_controls(cx);
        self.render_owner_direct(cx);
        let agent = self.selected(cx, ids!(agents), &self.agents);
        let editable = agent.as_ref().is_some_and(|a| matches!(a["visibility"].as_str(),Some("public" | "private")) && matches!(a["state"].as_str(),Some("creating" | "active" | "suspended")));
        let public = editable && agent.as_ref().is_some_and(|a|a["visibility"] == "public");
        self.view.button(cx, ids!(another_binding)).set_enabled(cx, public && !self.busy);
        self.view.view(cx, ids!(audience_editor)).set_visible(cx, editable);
        let requested = if self.view.drop_down(cx, ids!(agent_visibility)).selected_item() == 1 {"public"} else {"private"};
        self.view.button(cx, ids!(save_agent_visibility)).set_enabled(cx, editable && !self.busy && agent.as_ref().is_some_and(|a|a["visibility"] != requested));
        self.view.view(cx, ids!(creation_audience)).set_visible(cx, self.view.button(cx, ids!(create_agent)).visible());
        let selected_project_room = self.selected(cx, ids!(rooms), &self.rooms).is_some();
        for id in [ids!(room_members), ids!(open_room)] {self.view.button(cx, id).set_visible(cx, selected_project_room);}
        self.view.label(cx, ids!(room_details)).set_visible(cx, selected_project_room);
        let binding_form = self.view.button(cx, ids!(bind_agent)).visible();
        self.view.view(cx, ids!(agent_binding_target)).set_visible(cx, binding_form);
        for id in [ids!(create_agent), ids!(bind_agent)] {
            self.view
                .button(cx, id)
                .set_enabled(cx, !self.busy);
        }
        self.view.button(cx,ids!(create_agent)).set_text(cx,crate::i18n::tr(if self.agent_creating {"Creating…"} else {"Create Agent"}));
        self.view.widget(cx,ids!(agent_create_spinner)).set_visible(cx,self.agent_creating);
        self.view.widget(cx,ids!(new_agent_visibility)).set_disabled(cx,self.busy);
        let assigned_here = self.local_agent(cx);
        let remote = self.remote_agent(cx);
        let configurable = assigned_here || (remote && self.configuration_online());
        self.render_configuration(cx);
        self.view.view(cx,ids!(runtime_start_options)).set_visible(cx,!self.runtime_room_active);
        let runtime_known=self.agent_service_state.is_object();
        let mut service_toggle=self.view.button(cx,ids!(service_toggle));
        service_toggle.set_text(cx,crate::i18n::tr(if self.runtime_pending_action==Some(true) {"Starting…"} else if self.runtime_pending_action==Some(false) {"Stopping…"} else if !runtime_known {"Checking…"} else if self.runtime_room_active {"Stop responding"} else {"Start responding"}));
        if self.runtime_room_active {script_apply_eval!(cx,service_toggle,{draw_icon +: {svg: mod.widgets.ICON_SERVICE_STOP}});} else {script_apply_eval!(cx,service_toggle,{draw_icon +: {svg: mod.widgets.ICON_SERVICE_PLAY}});}
        self.view.label(cx,ids!(runtime_restart_note)).set_visible(cx,self.runtime_room_active);
        let approval_selected=self.approval_checked && self.selected(cx,ids!(approvals),&self.approvals).is_some_and(|p| {
            !self.approval_blocked.contains(string(&p,"proposalId")) && self.approval_scope(cx).is_some_and(|(a,b,r)|super::approvals::matches(&p,&a,&b,&r,super::approvals::now()))
        });
        for id in [ids!(approve),ids!(reject)] {let enabled=assigned_here && approval_selected && !self.busy;self.view.button(cx,id).set_enabled(cx,enabled);self.view.widget(cx,id).set_disabled(cx,!enabled);}
        let remote_publication=self.selected(cx,ids!(approvals),&self.approvals).filter(|p|p["proposalId"]==self.remote_approval["approvalId"]).map(|_|&self.remote_approval["remoteApproval"]);
        let published=remote_publication.is_some_and(Value::is_object);
        let ready=assigned_here && approval_selected && !self.busy;
        for (id,enabled) in [(ids!(remote_approval_publish),ready && !published && self.remote_room(cx).is_some()),(ids!(remote_approval_refresh),ready),(ids!(remote_approval_observe),ready && remote_publication.is_some_and(|v|v["phase"]=="sent"))] {
            self.view.button(cx,id).set_enabled(cx,enabled);self.view.widget(cx,id).set_disabled(cx,!enabled);
        }
        self.view.widget(cx,ids!(remote_approval_room)).set_disabled(cx,self.busy || published);
        let reviewed=self.selected(cx,ids!(remote_review_requests),&self.remote_review_requests);
        let actionable=reviewed.as_ref().is_some_and(|r|r["publication"].is_null() && self.agent(cx).as_deref()==r["request"]["proposal"]["scope"]["agent"].as_str() && self.selected(cx,ids!(remote_review_room),&self.remote_review_rooms).is_some_and(|room|room["id"]==r["request"]["approvalRoom"]) && r["request"]["proposal"]["expires"].as_i64().is_some_and(|t|t>super::approvals::now()));
        for (id,enabled) in [(ids!(remote_review_allow),actionable),(ids!(remote_review_deny),actionable),(ids!(remote_review_copy),reviewed.is_some()),(ids!(remote_review_receipt),reviewed.as_ref().is_some_and(|r|r["publication"].is_object())),(ids!(remote_review_refresh),!self.remote_review_rooms.is_empty()),(ids!(remote_review_older),self.remote_review_next.is_some() && self.remote_review_requests.len()<128)] {self.view.button(cx,id).set_enabled(cx,!self.busy && enabled);self.view.widget(cx,id).set_disabled(cx,self.busy || !enabled);}
        self.view.widget(cx,ids!(remote_review_room)).set_disabled(cx,self.busy);
        let badge = self.view.widget(cx, ids!(execution_badge));
        render_device_badge(cx, &badge, agent_device_state(&self.execution_agent_record, &self.current_device_id, &self.execution_devices, self.devices_loaded), &self.device_badge(&self.execution_agent_record));
        let device_id=string(&self.execution_agent_record,"executionDeviceId");
        let device_name=self.execution_devices.iter().find(|d|string(d,"id")==device_id).map(|d|string(d,"name")).unwrap_or("");
        self.view.label(cx,ids!(execution_device_details)).set_text(cx,&crate::i18n::format("Device name: {name}\nDevice ID: {id}",&[("name",device_name.into()),("id",device_id.into())]));
        self.view.label(cx,ids!(execution_device_details)).set_visible(cx,self.devices_loaded && !device_id.is_empty());
        self.view.label(cx,ids!(assignment_destination)).set_text(cx,&crate::i18n::format("This device: {device}",&[("device",execution_device_name(&self.execution_devices,&self.current_device_id))]));
        self.view.view(cx, ids!(local_device_notice)).set_visible(cx, !assigned_here);
        self.view.label(cx, ids!(local_device_message)).set_text(cx, if self.devices_loaded && !self.execution_agent_record.is_null() {
            crate::i18n::tr("Model and Agent limits configure the assigned execution device. Service controls remain on that device.")
        } else {crate::i18n::tr("Checking execution device…")});
        self.sync_crypto(cx);
        let assignable = self.devices_loaded && self.execution_agent_record["id"].as_str() == self.agent(cx).as_deref() && self.execution_agent_record["generation"].as_i64().is_some_and(|generation| generation > 0) && matches!(string(&self.execution_agent_record, "state"), "creating" | "active" | "suspended") && !self.current_device_id.is_empty() && self.execution_devices.iter().any(|d|string(d,"id") == self.current_device_id && d["revoked"] == false);
        self.view.button(cx, ids!(assign_current_device)).set_enabled(cx, assignable && !assigned_here && !self.busy && self.view.check_box(cx, ids!(assignment_confirm)).active(cx));
        self.view.check_box(cx, ids!(assignment_confirm)).set_visible(cx, assignable && !assigned_here);
        self.view.button(cx, ids!(assign_current_device)).set_visible(cx, assignable && !assigned_here);
        for id in [ids!(model_tab), ids!(policy_tab)] {self.view.button(cx, id).set_enabled(cx, assigned_here || remote);}
        self.view.button(cx,ids!(runtime_tab)).set_enabled(cx,assigned_here);
        self.view.drop_down(cx,ids!(policy_layer)).set_disabled(cx,remote || self.busy);
        self.view.button(cx,ids!(reset_policy)).set_visible(cx,!remote);
        self.view.button(cx,ids!(configuration_refresh)).set_enabled(cx,!self.busy);
        if remote {self.view.drop_down(cx,ids!(policy_layer)).set_selected_item(cx,0);}
        for id in [ids!(provider_login),ids!(provider_logout),ids!(provider_cancel),ids!(provider_open),ids!(disconnect_provider)] {
            if remote {self.view.button(cx,id).set_visible(cx,false);}
        }
        for id in [ids!(use_local_provider),ids!(provider_check),ids!(refresh_models),ids!(prepare_workspace)] {
            self.view.button(cx,id).set_enabled(cx,configurable && !self.busy);
        }
        let runtime=self.provider_runtime(cx);
        self.view.drop_down(cx,ids!(runtime_kind)).set_disabled(cx,self.busy || self.runtime_room_active || (remote && !configurable));
        self.view.label(cx,ids!(runtime_provider_note)).set_text(cx,crate::i18n::tr(if remote {"Connect uses the owner's provider on the execution device and affects that owner's other Agents. Sign-in and credentials stay on that device."} else {match runtime {
            "claude"=>"Claude uses its local sign-in. Sign in through Claude Code before connecting.",
            "octos"=>"Octos runs the selected local profile. Provider credentials stay in Octos; reasoning uses the profile's settings.",
            _=>"Codex uses its local sign-in or a separate account.",
        }}));
        let profile=&self.policy["modelProfile"];
        let current_model=self.selected(cx,ids!(model),&self.provider_models);
        let model_ready=profile["model"].as_str().is_some_and(|m|!m.is_empty())
            && current_model.as_ref().is_some_and(|m|m["model"] == profile["model"])
            && !self.credential_ref.is_empty() && string(profile,"credential_ref") == self.selected_credential(cx)
            && (string(profile,"reasoning_effort").is_empty() || self.runtime_efforts.iter().any(|e|e==string(profile,"reasoning_effort")))
            && !self.input(cx,ids!(workspace)).is_empty() && string(profile,"workspace_root") == self.input(cx,ids!(workspace));
        let reason=if self.runtime_resources_loading {"Checking the saved model and connected local account…"} else if self.view.view(cx,ids!(runtime_section)).visible() && !self.runtime_resources_checked {"Model resources could not be verified. Refresh this Service page to retry."} else if !assigned_here {"Assign this Agent to this device in Agent and device."} else if !self.provider_authenticated {"Connect your account in Model & resources."} else if !model_ready || self.runtime_efforts.is_empty() {"Save a model in Model & resources. A private workspace is created automatically."} else {"Ready to start with a positive token reservation."};
        self.view.label(cx,ids!(start_requirements)).set_text(cx,crate::i18n::tr(reason));
        let start_ready=self.provider_authenticated && model_ready && !self.runtime_efforts.is_empty() && !self.runtime_resources_loading && self.runtime_resources_checked;
        self.view.button(cx,ids!(service_toggle)).set_enabled(cx, assigned_here && runtime_known && !self.busy && (self.runtime_room_active || start_ready));
        let selected_agent = self.selected(cx, ids!(agents), &self.agents);
        let can_retire = selected_agent
            .as_ref()
            .is_some_and(|a| !["retiring", "retired"].contains(&string(a, "state")))
            && self.view.check_box(cx, ids!(retire_confirm)).active(cx);
        self.view
            .button(cx, ids!(retire_agent))
            .set_enabled(cx, can_retire && !self.busy);
        let selected_binding = self.selected(cx, ids!(bindings), &self.bindings);
        let state = selected_binding.as_ref().map(|b| string(b, "state"));
        self.view.button(cx,ids!(open_binding_chat)).set_enabled(cx,selected_binding.as_ref().is_some_and(|b|string(b,"state")=="active" && !string(b,"roomId").is_empty()) && !self.busy);
        let project_binding = selected_binding.as_ref().is_some_and(|b| b["projectId"].as_str().is_some_and(|id| !id.is_empty()) && string(b,"scopeKind") != "owner_direct");
        self.view.view(cx, ids!(reply_policy)).set_visible(cx, project_binding);
        let auto_reply = selected_binding.as_ref().is_some_and(|b| b["threadAutoReply"] == true);
        self.view.label(cx, ids!(reply_policy_status)).set_text(cx, crate::i18n::tr(if auto_reply {
            "Mention the Agent to start a thread. Everyone in that thread can then follow up without mentioning it."
        } else {
            "Every message requires an @ mention, including follow-ups in a thread."
        }));
        self.view.button(cx, ids!(reply_policy_toggle)).set_text(cx, crate::i18n::tr(if auto_reply { "Require @ for every message" } else { "Enable automatic thread follow-ups" }));
        self.view.button(cx, ids!(reply_policy_toggle)).set_enabled(cx, project_binding && matches!(state, Some("active" | "suspended")) && !self.busy);
        self.view.button(cx, ids!(room_roster_check)).set_visible(cx, project_binding);
        self.view.button(cx, ids!(room_roster_check)).set_enabled(cx, project_binding && !self.busy);
        if selected_binding.as_ref().is_some_and(|b| string(b,"scopeKind") == "owner_direct") {
            self.view.label(cx, ids!(room_roster)).set_text(cx, crate::i18n::tr("This is your private chat with the Agent. Open its chat information to view members."));
        }
        for (id, enabled) in [
            (ids!(pause_binding), state == Some("active")),
            (ids!(resume_binding), state == Some("suspended")),
            (
                ids!(leave_binding),
                state.is_some_and(|s| s != "left" && s != "leaving")
                    && self.view.check_box(cx, ids!(leave_confirm)).active(cx),
            ),
        ] {
            self.view
                .button(cx, id)
                .set_enabled(cx, enabled && !self.busy);
        }
        let layer = self
            .view
            .drop_down(cx, ids!(policy_layer))
            .selected_item()
            .min(2);
        let binding = self.binding(cx);
        let agent = self.agent(cx);
        let requester = self.input(cx, ids!(requester));
        let exact = agent.as_ref().is_some_and(|agent| {
            editor_matches(&self.policy, agent, binding.as_deref(), &requester, layer)
        });
        for id in [ids!(save_policy), ids!(reset_policy)] {
            self.view
                .button(cx, id)
                .set_enabled(cx, exact && configurable && (!remote || id == ids!(save_policy)) && !self.busy);
        }
        let model_exact = agent.as_ref().is_some_and(|agent| {
            string(&self.policy, "agentId") == agent
                && self.policy["scope"] == "agent"
                && self.policy["modelLoaded"] == true
        });
        let model_selected=self.selected(cx,ids!(model),&self.provider_models).is_some();
        let save_enabled=configurable && model_exact && self.provider_authenticated && !self.credential_ref.is_empty() && model_selected && !self.runtime_efforts.is_empty() && !self.busy && !self.model_saving && !self.view.view(cx,ids!(mcp_form)).visible();
        self.view.button(cx, ids!(save_model)).set_enabled(cx,save_enabled);
        self.view.widget(cx,ids!(save_model)).set_disabled(cx,!save_enabled);
        self.render_model_save(cx);
        self.render_provider_controls(cx);
        let policy_visible = self.view.view(cx, ids!(policy_section)).visible();
        if !assigned_here && ((!remote && (self.view.view(cx, ids!(model_section)).visible() || self.view.view(cx, ids!(policy_section)).visible())) || self.view.view(cx, ids!(runtime_section)).visible()) {self.section(cx, ids!(setup_section));}
        self.view.label(cx, ids!(runtime_chat_target)).set_text(cx, crate::i18n::tr("Automatically serves all connected Rooms, including Rooms added later."));
        let room_scoped = self.view.view(cx, ids!(setup_section)).visible();
        let setup_visible = self.view.view(cx, ids!(setup_section)).visible();
        self.view.view(cx, ids!(retirement_section)).set_visible(cx, setup_visible);
        self.view.view(cx, ids!(room_service_actions)).set_visible(cx, setup_visible);
        for id in [ids!(room_danger_heading), ids!(leave_confirm), ids!(leave_binding), ids!(pause_binding), ids!(resume_binding)] {
            self.view.widget(cx, id).set_visible(cx, setup_visible);
        }
        self.view.view(cx, ids!(policy_room_holder)).set_visible(cx, policy_visible && layer != 0);
        let binding_index = self.view.drop_down(cx, ids!(bindings)).selected_item();
        self.view.drop_down(cx, ids!(policy_binding)).set_selected_item(cx, binding_index);
        let target = if layer == 0 {
            "Agent total applies across all Rooms.".to_owned()
        } else if let Some(binding) = self.selected(cx, ids!(bindings), &self.bindings) {
            let room = self.binding_label(cx,&binding);
            if layer == 2 { format!("{} · {}", room, self.input(cx, ids!(requester))) } else { room.to_owned() }
        } else { "Select a Room binding".to_owned() };
        self.view.label(cx, ids!(policy_target)).set_text(cx, crate::i18n::tr(&target));
        self.view
            .view(cx, ids!(room_controls))
            .set_visible(cx, room_scoped);
        self.view
            .view(cx, ids!(requester_holder))
            .set_visible(cx, policy_visible && layer == 2);
        let list_visible = !self.view.view(cx,ids!(agent_detail)).visible() && !self.view.view(cx,ids!(agent_create)).visible();
        self.view.view(cx,ids!(agent_recovery)).set_visible(cx, self.page == Page::Agents && list_visible && !self.agent_pending_commands.is_empty());
        let detail = match self.page {
            Page::Projects => {
                self.view.view(cx, ids!(project_detail)).visible()
                    || self.view.view(cx, ids!(project_form)).visible()
            }
            Page::Agents => {
                self.view.view(cx, ids!(agent_detail)).visible()
                    || self.view.view(cx, ids!(agent_create)).visible()
            }
        };
        if !detail {
            self.view.label(cx,ids!(heading)).set_text(cx,crate::i18n::tr(if self.page==Page::Projects {"Projects"} else {"My Agents"}));
            self.view.button(cx,ids!(new_item)).set_text(cx,crate::i18n::tr(if self.page==Page::Projects {"New Project"} else {"New Agent"}));
        }
        self.view.view(cx,ids!(lists_pane)).set_visible(cx,!detail && !self.display_blocked);
        self.view.view(cx,ids!(details_scroll)).set_visible(cx,detail && !self.display_blocked);
        self.view.view(cx,ids!(project_list_holder)).set_visible(cx,!detail && self.page==Page::Projects);
        self.view.view(cx,ids!(agent_list_holder)).set_visible(cx,!detail && self.page==Page::Agents);
        self.view.button(cx,ids!(agent_approval_chats)).set_enabled(cx,!self.busy);
        let members=self.page==Page::Projects && detail && (self.view.view(cx,ids!(members_section)).visible() || self.view.view(cx,ids!(board_section)).visible());
        for id in [ids!(projects_page),ids!(project_detail),ids!(members_section)] {
            let mut view=self.view.view(cx,id);
            let height=if members {Size::fill()} else {Size::fit()};
            script_apply_eval!(cx,view,{height:#(height)});
        }
        self.view
            .button(cx, ids!(new_item))
            .set_visible(cx, !detail);
        self.view.button(cx, ids!(refresh)).set_visible(cx, !detail);
        self.view.button(cx, ids!(back)).set_visible(cx, detail);
        self.view.button(cx, ids!(ledger_toggle)).set_visible(cx, !detail && self.page == Page::Agents);
        self.view.view(cx, ids!(ledger_panel)).set_visible(cx, !detail && self.page == Page::Agents && self.ledger_open);
        self.render_ledger(cx);
        self.render_usage_controls(cx);
        self.render_history_controls(cx);
        self.render_file_controls(cx);
        self.render_recovery_controls(cx);
        if self.page == Page::Projects && self.view.view(cx, ids!(project_form)).visible() {
            let title = self.view.label(cx, ids!(creation_heading)).text();
            self.view.label(cx, ids!(heading)).set_text(cx, &title);
        } else if self.page == Page::Agents && self.view.view(cx, ids!(agent_create)).visible() {
            let title = if self.view.button(cx, ids!(bind_agent)).visible() {
                "Add Room to Agent"
            } else {
                "Create Agent"
            };
            self.view
                .label(cx, ids!(agent_creation_heading))
                .set_text(cx, crate::i18n::tr(title));
            self.view.label(cx, ids!(heading)).set_text(cx, crate::i18n::tr(title));
        }
    }
    fn open_card(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        if !self.view.view(cx,ids!(lists_pane)).visible() {return false;}
        let list = self.view.portal_list(
            cx,
            if self.page == Page::Projects {
                ids!(project_list)
            } else {
                ids!(agent_list)
            },
        );
        for (index, row) in list.items_with_actions(actions) {
            if self.page == Page::Agents && !list.was_scrolling() && row.button(cx,ids!(runtime_badge)).clicked(actions) {
                let Some(agent)=self.agents.get(index) else {return true;};
                let id=string(agent,"id");
                if self.agent_card_targets.get(&row.widget_uid()).map(String::as_str)!=Some(id)
                    || agent_device_state(agent,&self.current_device_id,&self.execution_devices,self.devices_loaded)!=AgentDeviceState::Here {return true;}
                let mut description=runtime_service_description(self.runtime_list.get(id),id);
                if let Some(error)=self.runtime_list_errors.get(id) && !description.contains(error.as_str()) {description.push_str(&format!("\n{error}"));}
                let button=row.button(cx,ids!(runtime_badge));
                let kind=if button.text()==crate::i18n::tr("Needs attention") {PopupKind::Warning} else {PopupKind::Info};
                let (fg,bg)=crate::shared::feedback::colors(&crate::theme::snapshot(cx),kind);
                cx.widget_action(button.widget_uid(),TooltipAction::HoverIn {
                    widget_rect:button.area().rect(cx),
                    text:description,
                    options:CalloutTooltipOptions {position:TooltipPosition::Bottom,text_color:fg,bg_color:bg,..Default::default()},
                });
                return true;
            }
            if self.page == Page::Agents && !list.was_scrolling() && row.button(cx,ids!(service_control)).clicked(actions) {
                let Some(agent)=self.agents.get(index).cloned() else {return true;};
                if agent_device_state(&agent,&self.current_device_id,&self.execution_devices,self.devices_loaded)!=AgentDeviceState::Here {return true;}
                let id=string(&agent,"id").to_owned();
                if self.agent_card_targets.get(&row.widget_uid()) != Some(&id) {return true;}
                let Some(Ok(status))=self.runtime_list.get(&id) else {return true;};
                let enabled=service_enabled(status);
                if service_needs_model(status) {
                    self.agent_detail(cx,index);
                    self.wanted_run=None;
                    self.wanted_model=Some(id);
                    self.status(cx,"No model is configured on this device. Open Model & resources, connect an account and save a model before starting.");
                    return true;
                }
                self.runtime_list_errors.remove(&id);
                self.list_service_pending_agent=id.clone();
                self.run(cx,Target::ListService,Command::Local {agent_id:id,action:if enabled {LocalAction::StopAgentService} else {LocalAction::StartAgentService},input:if enabled {None} else {Some(json!({}))},binding_id:None,requester:None});
                return true;
            }
            if !list.was_scrolling() && row.button(cx, ids!(open)).clicked(actions) {
                if self.page == Page::Projects {
                    self.project_detail(cx, index);
                } else {
                    self.agent_detail(cx, index);
                }
                return true;
            }
        }
        false
    }
    fn back_to_list(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        if !self.view.button(cx, ids!(back)).clicked(actions) {
            return false;
        }
        self.cancel_direct_retry(cx);
        self.wanted_project = None;
        self.wanted_project_section = None;
        self.wanted_agent = None;
        self.wanted_model = None;
        self.list_page(cx);
        if self.page == Page::Agents {self.run(cx,Target::AgentJournal,Command::AgentCommands);}
        else {self.refresh_runtime_list(cx);}
        self.status(cx, "");
        true
    }
    fn agent_request_actions(&mut self,cx:&mut Cx,actions:&Actions)->bool {
        if !self.view.view(cx,ids!(agent_recovery)).visible() {return false;}
        let list=self.view.flat_list(cx,ids!(agent_request_list));
        for (_,row) in list.items_with_actions(actions) {
            let Some(id)=self.agent_request_targets.get(&row.widget_uid()).cloned() else {continue;};
            let Some(command)=self.agent_pending_commands.iter().find(|c|string(c,"id")==id) else {continue;};
            let action=if row.button(cx,ids!(check_request)).clicked(actions) {
                Some((Target::AgentCommandChecked,Command::CheckAgentCommand{id}))
            } else if row.button(cx,ids!(retry_request)).clicked(actions) && string(command,"state")!="pending" {
                Some((Target::AgentResumed,Command::ResumeAgentCommand{id}))
            } else if row.button(cx,ids!(dismiss_request)).clicked(actions) && string(command,"state")=="rejected" {
                Some((Target::AgentCommandDismissed,Command::DismissAgentCommand{id}))
            } else {None};
            if let Some((target,command))=action {self.run(cx,target,command);return true;}
        }
        false
    }
    fn section(&self, cx: &mut Cx, selected: &[LiveId]) {
        self.navigation_ready.set(false);
        if !self.view.view(cx,selected).visible() {self.view.view(cx,ids!(details_scroll)).set_scroll_pos(cx,dvec2(0.0,0.0));}
        for id in [
            ids!(setup_section),
            ids!(policy_section),
            ids!(model_section),
            ids!(runtime_section),
            ids!(usage_section),
            ids!(history_section),
            ids!(remote_review_section),
            ids!(files_section),
            ids!(recovery_section),
        ] {
            self.view.view(cx, id).set_visible(cx, id == selected);
        }
        for (tab, section) in [
            (ids!(setup_tab), ids!(setup_section)),
            (ids!(policy_tab), ids!(policy_section)),
            (ids!(model_tab), ids!(model_section)),
            (ids!(runtime_tab), ids!(runtime_section)),
            (ids!(usage_tab), ids!(usage_section)),
            (ids!(history_tab), ids!(history_section)),
            (ids!(remote_review_tab), ids!(remote_review_section)),
            (ids!(files_tab), ids!(files_section)),
            (ids!(recovery_tab), ids!(recovery_section)),
        ] {
            let mut button = self.view.button(cx, tab);
            if section == selected {
                script_apply_eval!(cx, button, { draw_bg +: {color: mod.widgets.RINX_ACCENT color_hover: mod.widgets.RINX_ACCENT color_down: mod.widgets.RINX_ACCENT color_focus: mod.widgets.RINX_ACCENT} draw_text +: {color: mod.widgets.RINX_ON_ACCENT color_hover: mod.widgets.RINX_ON_ACCENT color_down: mod.widgets.RINX_ON_ACCENT color_focus: mod.widgets.RINX_ON_ACCENT} });
            } else {
                script_apply_eval!(cx, button, { draw_bg +: {color: mod.widgets.RINX_SURFACE color_hover: mod.widgets.RINX_HOVER color_down: mod.widgets.RINX_PRESSED color_focus: mod.widgets.RINX_SELECTED} draw_text +: {color: mod.widgets.RINX_INK color_hover: mod.widgets.RINX_INK color_down: mod.widgets.RINX_INK color_focus: mod.widgets.RINX_INK} });
            }
        }
    }

    fn open_room_creation(&mut self,cx:&mut Cx) {
        if self.project_home["canCreateRoom"]!=true || self.project_home["archived"]==true {return;}
        if !self.begin_creation_form(cx,true) {return;}
        self.reset_detail_scroll(cx);
        self.view.view(cx,ids!(project_detail)).set_visible(cx,false);
        self.create_room=true;
        self.view.view(cx,ids!(project_form)).set_visible(cx,true);
        self.view.drop_down(cx,ids!(creation_source)).set_labels(cx,vec![
            crate::i18n::tr("Create a private Room").into(),
            crate::i18n::tr("Connect an existing Room").into(),
        ]);
        self.view.drop_down(cx,ids!(creation_source)).set_selected_item(cx,0);
        self.creation_source(cx);
    }
    fn begin_creation_form(&mut self,cx:&mut Cx,room:bool)->bool {
        let unresolved=self.creation.as_ref().is_some_and(|v|v["phase"] != "complete") || (self.creation.is_none() && self.pending_input.is_some());
        if unresolved {
            self.view.view(cx,ids!(project_form)).set_visible(cx,true);
            self.view.view(cx,ids!(project_list_holder)).set_visible(cx,false);
            self.view.view(cx,ids!(project_detail)).set_visible(cx,false);
            self.view.view(cx,ids!(recovery)).set_visible(cx,true);
            self.status(cx,"Resume the original unfinished creation before starting another one.");
            return false;
        }
        self.creation=None;self.pending_input=None;self.create_room=room;
        self.view.check_box(cx,ids!(creation_encrypted)).set_active(cx,false,Animate::No);
        for id in [ids!(creation_name),ids!(found_room),ids!(existing_room)] {self.view.text_input(cx,id).set_text(cx, "");}
        self.view.label(cx,ids!(creation_result)).set_text(cx, "");
        self.view.view(cx,ids!(recovery)).set_visible(cx,false);
        self.view.button(cx,ids!(create_project_room)).set_enabled(cx,!self.busy);
        true
    }
    fn creation_source(&self, cx: &mut Cx) {
        self.view.view(cx,ids!(creation_source_holder)).set_visible(cx,self.create_room);
        self.view.label(cx,ids!(creation_source_label)).set_visible(cx,self.create_room);
        let existing = self.create_room && self.view.drop_down(cx,ids!(creation_source)).selected_item()==1;
        self.view.view(cx,ids!(creation_encryption)).set_visible(cx,self.create_room && !existing);
        let (title, note) = match (self.create_room, existing) {
            (false, false) => (
                "Create Project",
                "Create a Project with its own members, boards and discussions.",
            ),
            (false, true) => (
                "Connect existing Space",
                "Select a Space you already belong to.",
            ),
            (true, false) => (
                "Create discussion Room",
                "Create a Room inside this Project. It has its own members.",
            ),
            (true, true) => (
                "Connect existing Room",
                "Connect a Room already linked to this Project's Space.",
            ),
        };
        self.view
            .label(cx, ids!(creation_heading))
            .set_text(cx, crate::i18n::tr(title));
        self.view.label(cx, ids!(creation_note)).set_text(cx, crate::i18n::tr(note));
        let mut name = self.view.text_input(cx, ids!(creation_name));
        if self.create_room {
            let placeholder=crate::i18n::tr("Room name"); script_apply_eval!(cx, name, {empty_text: #(placeholder)});
        } else {
            let placeholder=crate::i18n::tr("Project name"); script_apply_eval!(cx, name, {empty_text: #(placeholder)});
        }
        self.view
            .button(cx, ids!(create_project_room))
            .set_text(cx, crate::i18n::tr(title));
        self.view
            .view(cx, ids!(creation_name_holder))
            .set_visible(cx, !existing);
        self.view
            .view(cx, ids!(candidates_holder))
            .set_visible(cx, existing && !self.create_room);
        self.view
            .button(cx, ids!(candidates_more))
            .set_visible(cx, existing && !self.create_room);
        self.view
            .view(cx, ids!(existing_room_holder))
            .set_visible(cx, existing && self.create_room);
    }
    fn load_permissions(&mut self, cx: &mut Cx, room: bool) {
        let Some(project_id) = self.project(cx) else {
            return;
        };
        let room_id = if room {
            let Some(value) = self.selected(cx, ids!(rooms), &self.rooms) else {
                self.status(cx, "Select a discussion Room first.");
                return;
            };
            Some(string(&value, "roomId").to_owned())
        } else {
            None
        };
        if (!room && self.project_home["canManagePolicy"]!=true) || (room && !self.room_can_manage) {return;}
        self.scope_state = Value::Null;
        self.view
            .view(cx, ids!(permission_editor))
            .set_visible(cx, false);
        self.project_section(cx, ids!(permissions_section));
        self.run(
            cx,
            Target::ScopeState,
            Command::ScopeServiceState {
                project_id,
                room_id,
            },
        );
    }
    fn set_page(&mut self, cx: &mut Cx, page: Page) {
        self.navigation_ready.set(false);
        self.status(cx, "");
        self.page = page;
        self.view.label(cx, ids!(heading)).set_text(
            cx,
            crate::i18n::tr(if page == Page::Projects {"Projects"} else {"My Agents"}),
        );
        self.view.button(cx, ids!(new_item)).set_text(
            cx,
            crate::i18n::tr(if page == Page::Projects {"New Project"} else {"New Agent"}),
        );
        self.view
            .view(cx, ids!(projects_page))
            .set_visible(cx, page == Page::Projects);
        self.view
            .view(cx, ids!(agents_page))
            .set_visible(cx, page == Page::Agents);
        self.view.redraw(cx);
    }
    fn provider_runtime(&self, cx:&mut Cx)->&'static str {
        match self.view.drop_down(cx,ids!(runtime_kind)).selected_item(){1=>"claude",2=>"octos",_=>"codex"}
    }
    fn selected_credential(&self, cx:&mut Cx)->String {
        self.selected(cx,ids!(model),&self.provider_models).and_then(|m|m["credentialRef"].as_str().map(str::to_owned)).unwrap_or_else(||self.credential_ref.clone())
    }
    fn run(&mut self, cx: &mut Cx, target: Target, command: Command) {
        let command=match command {
            Command::Provider {action} if self.provider_runtime(cx)!="codex"=>Command::RuntimeProvider {runtime:self.provider_runtime(cx).into(),action},
            other=>other,
        };
        let list_control = matches!(target,Target::ListService) && matches!(&command,Command::Local {agent_id,action:LocalAction::StartAgentService | LocalAction::StopAgentService,binding_id:None,requester:None,..} if self.agents.iter().any(|a|string(a,"id")==agent_id && agent_device_state(a,&self.current_device_id,&self.execution_devices,self.devices_loaded)==AgentDeviceState::Here));
        let remote = self.remote_agent(cx);
        let mut configuration_fence = None;
        let mut configuration_digest = None;
        let command = if matches!(&command, Command::Local {..} | Command::Provider {..} | Command::RuntimeProvider {..} | Command::PrepareAgentWorkspace {..}) && !self.local_agent(cx) && !list_control {
            if !remote || !remote_configuration_command(&command) {self.status(cx, "This action is available only on the execution device.");return;}
            if !self.configuration_online() {self.status(cx,"Execution device offline. Cached settings are read-only.");return;}
            let agent_id=string(&self.execution_agent_record,"id").to_owned();
            let expected_generation=self.execution_agent_record["generation"].as_i64().unwrap_or(0);
            configuration_fence=Some((agent_id.clone(),expected_generation,string(&self.execution_agent_record,"executionDeviceId").into()));
            let mutation=matches!(&command,Command::Local {action:LocalAction::SaveAgentModel|LocalAction::SaveAgentPolicy,..}|Command::PrepareAgentWorkspace {..}|Command::Provider {action:ProviderAction::UseLocal}|Command::RuntimeProvider {action:ProviderAction::UseLocal,..});
            let idempotency_key=if mutation {
                let digest=format!("{}:{agent_id}:{expected_generation}:{}:{}:{command:?}",self.identity,string(&self.execution_agent_record,"executionDeviceId"),self.configuration_status["deviceGeneration"]);
                configuration_digest=Some(digest.clone());
                self.configuration_keys.entry(digest).or_insert_with(||uuid::Uuid::new_v4().to_string()).clone()
            } else {uuid::Uuid::new_v4().to_string()};
            Command::AgentConfiguration {agent_id,expected_generation,idempotency_key,command:Box::new(command)}
        } else {command};
        if matches!(&command,Command::AgentConfigurationStatus {..}|Command::ExecutionEnvironment {..}|Command::AgentCrypto {..}) {
            configuration_fence=Some((string(&self.execution_agent_record,"id").into(),self.execution_agent_record["generation"].as_i64().unwrap_or(0),string(&self.execution_agent_record,"executionDeviceId").into()));
        }
        if self.busy || !self.active {
            return;
        }
        let service = match Service::for_current() {
            Ok(s) if s.identity() == self.identity => s,
            _ => {
                self.status(
                    cx,
                    "Matrix account changed. Reopen this panel for the current account.",
                );
                return;
            }
        };
        if matches!(target, Target::Agents) { self.agents_loading = true; }
        self.request = nonce();
        self.arm_read_wait(cx, &command);
        let request = self.request;
        let identity = self.identity.clone();
        if matches!(target,Target::ModelSaved) {
            self.model_saving=true;
            self.model_save_message.clear();
            self.model_save_failed=false;
        }
        if let Command::Local {action:LocalAction::StartAgentService | LocalAction::StopAgentService,..}=&command {
            if matches!(target,Target::Runtime) {
                self.runtime_pending_action=Some(matches!(&command,Command::Local {action:LocalAction::StartAgentService,..}));
                self.runtime_action_error.clear();
                self.view.label(cx,ids!(runtime_status)).set_text(cx,crate::i18n::tr(if self.runtime_pending_action==Some(true) {"Starting service. Discovering connected Rooms…"} else {"Stopping…"}));
            }
        }
        if configuration_digest.is_some() {self.configuration_feedback=crate::i18n::tr("Applying settings on the execution device…").into();}
        self.set_busy(cx, true);
        if matches!(target,Target::AgentCreated) {self.agent_creating=true;}
        self.sync_navigation(cx);
        let quiet = matches!(target,Target::ModelSaved | Target::ListService | Target::ServiceRoomChanged | Target::ConfigurationStatus) || (matches!(target,Target::Runtime) && self.agent_service_state.is_object());
        if !matches!(target,Target::ConfigurationStatus) {self.status(
            cx,
            if quiet {""} else {match target {
                Target::Policy if remote && configuration_digest.is_some() => "Applying settings on the execution device…",
                Target::Changed => "Saving changes…",
                Target::AgentCreated => "Creating Agent…",
                Target::AgentBound | Target::ProjectAdopted | Target::RoomAdopted => "Connecting…",
                Target::Creation => "Checking creation…",
                _ => "Loading…",
            }},
        );}
        spawn_async_task(async move {
            let result = service.execute(command).await;
            Cx::post_action(Completed {
                identity,
                request,
                target,
                configuration_fence,
                configuration_digest,
                result,
            });
        });
    }
    fn model_save_input(&self, cx: &mut Cx, model: &Value, effort: &str) -> Value {
        let mut input=json!({"profile":{"model":model["model"],"credential_ref":self.selected_credential(cx),"workspace_root":self.input(cx,ids!(workspace)),"reasoning_effort":effort,"web_search":self.view.check_box(cx,ids!(web_search)).active(cx),"programming":{"enabled":self.provider_runtime(cx)=="codex" && self.view.check_box(cx,ids!(programming)).active(cx),"mcp_servers":if self.provider_runtime(cx)=="codex" && self.view.check_box(cx,ids!(programming)).active(cx) {let text=self.input(cx,ids!(mcp_servers)); if text.trim().is_empty() {json!({})}else{serde_json::from_str::<Value>(&text).unwrap_or(Value::Null)}}else{json!({})}}}});
        if !self.local_agent(cx) {input["profile"].as_object_mut().unwrap().remove("programming");}
        input
    }
    fn render_model_save(&self,cx:&mut Cx) {
        self.view.button(cx,ids!(save_model)).set_text(cx,crate::i18n::tr(if self.model_saving {"Saving…"} else {"Save local model profile"}));
        self.view.widget(cx,ids!(model_save_spinner)).set_visible(cx,self.model_saving);
        let feedback=self.view.inline_notice(cx,ids!(model_save_feedback));
        if self.model_save_message.is_empty() {feedback.clear(cx);}
        else {
            let (message,details)=self.model_save_message.strip_suffix(')').and_then(|s|s.rsplit_once(" (")).filter(|(_,code)|code.len()<=80 && code.bytes().all(|b|b.is_ascii_lowercase() || b.is_ascii_digit() || b==b'_')).unwrap_or((&self.model_save_message,""));
            feedback.show(cx,if self.model_save_failed {PopupKind::Error} else {PopupKind::Success},if self.model_save_failed {"Could not save model"} else {"Model saved"},message,details,false);
        }
        let configured=!string(&self.policy["modelProfile"],"model").is_empty();
        let setup=self.view.inline_notice(cx,ids!(model_setup_notice));
        if !configured && self.local_agent(cx) {
            setup.show(cx,PopupKind::Info,if self.provider_authenticated {"Next: choose a model"} else {"Next: connect a model"},if self.provider_authenticated {"Choose a model and save it for this Agent."} else {"Connect an account, then choose a model and save it."},"",false);
        } else {setup.clear(cx);}
        self.view.button(cx,ids!(continue_to_service)).set_visible(cx,configured);
        self.view.button(cx,ids!(continue_to_service)).set_enabled(cx,!self.busy);
    }
    fn clear_model_save_feedback(&mut self,cx:&mut Cx) {
        self.model_save_message.clear();self.model_save_failed=false;self.render_model_save(cx);
    }
    fn model_save_validation_failed(&mut self,cx:&mut Cx,message:&str) {
        self.model_save_message=crate::i18n::tr(message).into();self.model_save_failed=true;
        self.render_model_save(cx);
    }
    fn finish_model_save(&mut self,cx:&mut Cx,result:Result<(),&str>) {
        self.model_saving=false;
        self.model_save_failed=result.is_err();
        self.model_save_message=match result {Ok(())=>crate::i18n::tr("Model profile saved.").into(),Err(error)=>visible_error(error)};
        self.sync_navigation(cx);
    }
    fn render_policy(&mut self, cx: &mut Cx) {
        self.mcp_editing=None;
        self.view.view(cx,ids!(mcp_form)).set_visible(cx,false);
        self.view.inline_notice(cx,ids!(mcp_feedback)).clear(cx);
        self.view.check_box(cx,ids!(web_search)).set_active(cx,self.policy["modelProfile"]["web_search"].as_bool().unwrap_or(true),Animate::No);
        self.view.check_box(cx,ids!(programming)).set_active(cx,self.policy["modelProfile"]["programming"]["enabled"]==true,Animate::No);
        let servers=&self.policy["modelProfile"]["programming"]["mcp_servers"];
        self.view.text_input(cx,ids!(mcp_servers)).set_text(cx,&serde_json::to_string_pretty(&servers.as_object().cloned().unwrap_or_default()).unwrap_or_default());
        self.render_mcp(cx);
        if self.policy["modelLoaded"] == true {
            let profile = &self.policy["modelProfile"];
            let selected=self.provider_models.iter().position(|m|string(m,"model") == string(profile,"model") && (string(m,"credentialRef").is_empty() || string(m,"credentialRef") == string(profile,"credential_ref")));
            let runtime=if string(profile,"credential_ref").starts_with("claude-managed:"){1}else if string(profile,"credential_ref").starts_with("octos-managed:"){2}else{0};
            self.view.drop_down(cx,ids!(runtime_kind)).set_selected_item(cx,runtime);
            self.view.drop_down(cx,ids!(model)).set_selected_item(cx,selected.map_or(0,|i|i+1));
            self.view
                .text_input(cx, ids!(workspace))
                .set_text(cx, string(profile, "workspace_root"));
        }
        let layer = self
            .view
            .drop_down(cx, ids!(policy_layer))
            .selected_item()
            .min(2);
        let p = &self.policy["policies"][layer]["policy"];
        if !p.is_object() {
            return;
        }
        let limit = &p["budget"]["limit"];
        let mode = usize::from(limit.get("Tokens").is_some());
        self.view
            .drop_down(cx, ids!(limit_mode))
            .set_selected_item(cx, mode);
        self.view.view(cx,ids!(token_amount_holder)).set_visible(cx,mode==1);
        self.view.view(cx,ids!(period_holder)).set_visible(cx,mode==1);
        self.view.label(cx,ids!(zero_limit_note)).set_visible(cx,mode==1);
        self.view
            .text_input(cx, ids!(token_limit))
            .set_text(cx, &limit["Tokens"].as_u64().unwrap_or(0).to_string());
        self.view.drop_down(cx, ids!(period)).set_selected_item(
            cx,
            match string(&p["budget"], "period") {
                "UtcDay" => 1,
                "UtcMonth" => 2,
                _ => 0,
            },
        );
        self.view.drop_down(cx, ids!(requests)).set_selected_item(
            cx,
            match string(p, "requests") {
                "Deny" => 1,
                "AskOwner" => 2,
                _ => 0,
            },
        );
        self.view.drop_down(cx, ids!(tool_risk)).set_selected_item(
            cx,
            if p["high_risk"] == "AskOwner" {
                1
            } else if p["high_risk"].is_object() {
                2
            } else {
                0
            },
        );
        for (id, key) in [
            (ids!(allowed_tools), "tools"),
            (ids!(allowed_directories), "directories"),
        ] {
            let values = p["high_risk"]["AllowWithRules"][key]
                .as_array()
                .map(|v| {
                    v.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            self.view.text_input(cx, id).set_text(cx, &values);
        }
        self.view.label(cx, ids!(policy_usage)).set_text(
            cx,
            &crate::i18n::format("Used {spent} tokens · reserved {held}", &[
                ("scope", crate::i18n::tr(["Entire Agent", "Selected Room", "Selected requester in Room"][layer]).into()),
                ("spent", self.policy["usage"][layer]["spent"].to_string()),
                ("held", self.policy["usage"][layer]["held"].to_string()),
                
            ]),
        );
        let rules = p["high_risk"].is_object();
        self.view
            .view(cx, ids!(allowed_tools_holder))
            .set_visible(cx, rules);
        self.view
            .view(cx, ids!(allowed_directories_holder))
            .set_visible(cx, rules);
    }
    fn apply(&mut self, cx: &mut Cx, target: Target, value: &Value) {
        match target {
            Target::Stopped|Target::RecoveryReview|Target::RecoveryDecision=>self.apply_recovery(cx,target,value),
            Target::History|Target::HistoryDetail=>self.apply_history(cx,target,value),
            Target::Files|Target::FileDetail=>self.apply_file_history(cx,target,value),
            Target::Usage => self.apply_usage(cx,value),
            Target::Ledger => self.apply_ledger(cx, value),
            Target::Members => {
                if !members_match(&self.members_room, value) {
                    return;
                }
                self.members = project_navigation::ordered_members(array(value, "members"));
                self.view.button(cx,ids!(invite_member)).set_visible(cx,value["canInvite"]==true);
                self.view.label(cx, ids!(members_status)).set_text(
                    cx,
                    &crate::i18n::format("{count} members and pending invitations", &[("count", self.members.len().to_string())]),
                );
                self.view
                    .button(cx, ids!(invite_member))
                    .set_enabled(cx, value["canInvite"] == true);
                self.view
                    .button(cx, ids!(invite_member))
                    .set_disabled(cx, value["canInvite"] != true);
                if value["agentMetadataUnavailable"]==true {self.view.label(cx,ids!(members_status)).set_text(cx,crate::i18n::tr("Agent ownership could not be loaded. Refresh to retry."));}
                if self.project_home.is_null() {self.load_project_home(cx);}
            }
            Target::Journal => {
                if let Some(row) = array(value, "creations")
                    .into_iter()
                    .find(|r| r["creation"]["phase"] != "complete")
                {
                    self.pending_input = Some(row["input"].clone());
                    self.creation = Some(row["creation"].clone());
                    self.view
                        .view(cx, ids!(project_list_holder))
                        .set_visible(cx, false);
                    self.view.view(cx, ids!(project_form)).set_visible(cx, true);
                    self.view.view(cx, ids!(recovery)).set_visible(cx, true);
                    self.view.label(cx,ids!(creation_result)).set_text(cx, crate::i18n::tr("An original creation is unfinished. Resume it; a new command cannot replace it."));
                }
                self.run(cx, Target::Projects, Command::Projects);
            }
            Target::Projects => {
                self.projects_loaded=true;
                self.all_projects = array(value, "projects");
                self.projects = self.all_projects.iter().filter(|p| self.show_archived_projects || p["archived"]!=true || self.wanted_project.as_deref()==p["id"].as_str()).cloned().collect();
                self.labels(cx, ids!(projects), "Select a Project", &self.projects, "id");
                self.labels(
                    cx,
                    ids!(agent_project),
                    "Select a Project",
                    &self.projects,
                    "id",
                );
                if let Some(wanted) = self.wanted_project.take() {
                    if let Some(index) =
                        self.projects.iter().position(|p| string(p, "id") == wanted)
                    {
                        self.view
                            .drop_down(cx, ids!(projects))
                            .set_selected_item(cx, index + 1);
                        self.view
                            .drop_down(cx, ids!(agent_project))
                            .set_selected_item(cx, index + 1);
                        self.project_detail(cx, index);
                    }
                }
            }
            Target::ProjectHome | Target::BoardCreated => self.apply_project_home(cx,target,value),
            Target::NavigationDefaultChanged => {
                cx.action(crate::home::project_tree::ProjectTreeChanged {identity:self.identity.clone()});
                self.load_project_home(cx);
            }
            Target::ProjectArchived => {
                if let Some(project)=self.project(cx) {self.wanted_project=Some(project);}
                cx.action(crate::home::project_tree::ProjectTreeChanged {identity:self.identity.clone()});
                self.run(cx,Target::Projects,Command::Projects);
            }
            Target::Rooms => {
                if self.page == Page::Agents {
                    self.agent_rooms = array(value, "rooms")
                        .into_iter()
                        .filter(|r| r["active"] == true)
                        .collect();
                    self.labels(
                        cx,
                        ids!(agent_room),
                        "Select a visible registered Room",
                        &self.agent_rooms,
                        "roomId",
                    );
                } else {
                    self.rooms = array(value, "rooms");
                    self.rooms.sort_by_key(|r|(r["isDefault"]!=true,string(r,"name").to_lowercase(),string(r,"roomId").to_owned()));
                    self.has_registered_rooms=value["hasRegisteredRooms"].as_bool();
                    self.labels(cx, ids!(rooms), "Select a Room", &self.rooms, "roomId");
                    if let Some((project_id, section)) = self.wanted_project_section.take()
                        && let Some(project) = self.selected(cx, ids!(projects), &self.projects)
                        && string(&project, "id") == project_id
                    {
                        match section {
                            ProjectDetailSection::Access => {self.wanted_access=true;},
                            ProjectDetailSection::Board => self.project_section(cx,ids!(board_section)),
                            ProjectDetailSection::Overview => {
                                self.project_section(cx, ids!(overview_section))
                            }
                            ProjectDetailSection::Rooms => {
                                self.open_project_rooms(cx);
                            }
                            ProjectDetailSection::Members => {
                                self.show_members(cx, string(&project, "spaceId").into(), true)
                            }
                        }
                    }
                    if !self.busy {self.load_project_home(cx);}
                }
            }
            Target::Spaces => {
                let mut rows = array(value, "spaces")
                    .into_iter()
                    .filter(|s| s["projectId"].is_null())
                    .collect::<Vec<_>>();
                if self.cursor.is_some() {
                    self.spaces.append(&mut rows);
                } else {
                    self.spaces = rows;
                }
                self.cursor = value["nextCursor"].as_str().map(str::to_owned);
                self.labels(
                    cx,
                    ids!(candidates),
                    "Select an unconnected joined Space",
                    &self.spaces,
                    "spaceId",
                );
                if value["incomplete"] == true {
                    self.status(cx, "Some Spaces could not be read. Refresh to retry.");
                }
            }
            Target::Agents => {
                self.agents_loaded = true;
                self.agents_loading = false;
                self.agents = array(value, "agents");
                self.runtime_list.clear(); self.runtime_list_loading=false;
                self.execution_devices = array(value, "devices");
                self.current_device_id = string(value, "currentDeviceId").into();
                self.devices_loaded = value["devices"].is_array() && !self.current_device_id.is_empty();
                self.projects = array(value, "projects");
                self.refresh_runtime_list(cx);
                self.labels(cx, ids!(agents), "Select my Agent", &self.agents, "id");
                self.labels(
                    cx,
                    ids!(agent_project),
                    "Select a Project",
                    &self.projects,
                    "id",
                );
                self.view
                    .text_input(cx, ids!(requester))
                    .set_text(cx, string(value, "ownerMxid"));
                if let Some(wanted)=self.wanted_agent.take() {
                    if let Some(index)=self.agents.iter().position(|a|string(a,"id") == wanted) {
                        let open_model=self.wanted_model.as_deref()==Some(wanted.as_str());
                        self.agent_detail(cx,index);
                        if open_model {self.wanted_model=Some(wanted);}
                        return;
                    }
                    if self.wanted_approval.is_some() {self.reject_discovered_approval(cx);return;}
                }
                if let Some(wanted) = self.wanted_project.take() {
                    if let Some(index) =
                        self.projects.iter().position(|p| string(p, "id") == wanted)
                    {
                        self.view
                            .drop_down(cx, ids!(agent_project))
                            .set_selected_item(cx, index + 1);
                        self.run(
                            cx,
                            Target::Rooms,
                            Command::ProjectRooms { project_id: wanted },
                        );
                    }
                }
            }
            Target::VisibilityChanged => {
                let record = &value["agent"];
                if self.agent(cx).as_deref() != record["id"].as_str() { return; }
                if self.execution_agent_record["id"] == record["id"] { self.execution_agent_record = record.clone(); }
                if let Some(existing) = self.agents.iter_mut().find(|a|a["id"] == record["id"]) { *existing = record.clone(); }
                self.view.view(cx, ids!(detail_audience_badge)).label(cx, ids!(device_label)).set_text(cx, crate::i18n::tr(audience_label(record)));
                self.status(cx, "Service audience saved.");
            }
            Target::Crypto => self.apply_crypto(cx,value),
            Target::AgentAssigned | Target::AgentDetails => {
                let Some(agent) = self.agent(cx) else {return;};
                let record = &value["agent"];
                if string(record, "id") != agent || record["generation"].as_i64().is_none() {return;}
                if self.execution_agent_record != *record {self.agent_crypto=Value::Null;self.render_crypto(cx);}
                self.execution_agent_record = record.clone();
                self.view.drop_down(cx, ids!(agent_visibility)).set_selected_item(cx, usize::from(record["visibility"] == "public"));
                self.view.view(cx, ids!(detail_audience_badge)).label(cx, ids!(device_label)).set_text(cx, crate::i18n::tr(audience_label(record)));
                if let Some(existing) = self.agents.iter_mut().find(|a|string(a,"id") == agent) {*existing = record.clone();}
                if matches!(target, Target::AgentAssigned) {
                    self.runtime_list_errors.remove(&agent);
                    self.policy = Value::Null;
                    self.provider_authenticated = false;
                    self.runtime_resources_checked = false;
                    self.runtime_resources_loading = false;
                    self.view.check_box(cx, ids!(assignment_confirm)).set_active(cx, false, Animate::No);
                    self.section(cx, ids!(setup_section));
                    self.status(cx, "This Agent is now handled here. Configure a local runtime and budgets before starting.");
                }
                if !self.devices_loaded {self.run(cx, Target::Devices, Command::Devices);}
                else {self.run(cx, Target::Bindings, Command::Bindings {agent_id: agent});}
            }
            Target::Devices => {
                self.execution_devices = array(value,"devices");
                self.current_device_id = string(value,"currentDeviceId").into();
                self.devices_loaded = value["devices"].is_array() && !self.current_device_id.is_empty();
                if let Some(agent_id) = self.agent(cx) {self.run(cx,Target::Bindings,Command::Bindings {agent_id});}
            }
            Target::AgentJournal => {
                self.agent_journal_loaded = true;
                self.agent_pending_commands = array(value, "commands").into_iter().filter(|c|matches!(string(c,"state"),"unknown"|"pending"|"rejected") && !string(c,"id").is_empty()).collect();
                self.agent_request_targets.clear();
                self.view
                    .view(cx, ids!(agent_recovery))
                    .set_visible(cx, !self.agent_pending_commands.is_empty());
                self.run(cx, Target::Agents, Command::Agents);
            }
            Target::AgentCommandDismissed => {
                self.run(cx, Target::AgentJournal, Command::AgentCommands);
            }
            Target::AgentCommandChecked => {
                let command=&value["command"];
                let id=string(command,"id");
                if let Some(index)=self.agent_pending_commands.iter().position(|r|string(r,"id")==id) {
                    if matches!(string(command,"state"),"unknown"|"pending"|"rejected") {self.agent_pending_commands[index]=command.clone();}
                    else {self.agent_pending_commands.remove(index);}
                }
                let agent_id=string(&command["creation"]["creation"]["agent"],"id");
                if !agent_id.is_empty() {
                    self.wanted_agent=Some(agent_id.into());self.run(cx,Target::Agents,Command::Agents);
                } else {
                    if string(command,"state")=="rejected" {
                        let code=string(command,"errorCode");self.show_failure(cx,Target::AgentResumed,code,&visible_error(code));
                    } else {self.status(cx,"No confirmed result yet. Retry request uses the same identifier and will not create a duplicate.");}
                }
            }
            Target::ScopeChanged => {
                let room = self.scope_state["roomId"].is_string();
                self.load_permissions(cx, room);
            }
            Target::RoomCapabilities => {
                if value["projectId"].as_str()==self.project(cx).as_deref() && self.selected(cx,ids!(rooms),&self.rooms).is_some_and(|r|r["roomId"]==value["roomId"]) {
                    self.room_can_manage=value["canManagePolicy"]==true;
                }
            }
            Target::ScopeState => {
                if self.project(cx).as_deref() != value["projectId"].as_str() {
                    return;
                }
                if let Some(room) = value["roomId"].as_str() {
                    if self
                        .selected(cx, ids!(rooms), &self.rooms)
                        .as_ref()
                        .map(|r| string(r, "roomId"))
                        != Some(room)
                    {
                        return;
                    }
                }
                self.scope_state = value.clone();
                let room = value["roomId"].is_string();
                self.view.label(cx, ids!(permission_heading)).set_text(
                    cx,
                    if room {
                        "Discussion Room Agent permissions"
                    } else {
                        "Project Agent permissions"
                    },
                );
                self.view.label(cx, ids!(permission_status)).set_text(
                    cx,
                    if value["servicePaused"] == true {
                        "Agent services are paused here."
                    } else {
                        "Agent services are not paused here."
                    },
                );
                self.view
                    .view(cx, ids!(permission_editor))
                    .set_visible(cx, value["canManagePolicy"] == true);
                let policy = if value["creationPolicy"].is_string() {
                    serde_json::from_str::<Value>(string(value, "creationPolicy"))
                        .unwrap_or(Value::Null)
                } else {
                    value["creationPolicy"].clone()
                };
                self.view
                    .view(cx, ids!(permission_editor))
                    .set_visible(cx, value["canManagePolicy"] == true && policy.is_object());
                self.scope_state["policy"] = policy.clone();
                self.view.drop_down(cx, ids!(creation_mode)).set_labels(
                    cx,
                    if room {
                        vec![
                            crate::i18n::tr("Inherit Project permissions").into(),
                            crate::i18n::tr("Only specified Room members").into(),
                            crate::i18n::tr("Disable new Agent connections").into(),
                        ]
                    } else {
                        vec![
                            crate::i18n::tr("Allow Project members by default").into(),
                            crate::i18n::tr("Only specified Project members").into(),
                        ]
                    },
                );
                let selected = if room {
                    match string(&policy, "mode") {
                        "allow_list" => 1,
                        "disabled" => 2,
                        _ => 0,
                    }
                } else {
                    usize::from(policy["defaultAllow"] != true)
                };
                self.view
                    .drop_down(cx, ids!(creation_mode))
                    .set_selected_item(cx, selected);
                for (id, key) in [
                    (ids!(creation_allow), "allow"),
                    (ids!(creation_deny), "deny"),
                ] {
                    let values = policy[key]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    self.view.text_input(cx, id).set_text(cx, &values);
                }
                self.view.check_box(cx, ids!(scope_pause)).set_active(
                    cx,
                    if room {
                        value["roomPaused"] == true
                    } else {
                        value["projectPaused"] == true
                    },
                    Animate::No,
                );
            }
            Target::RoomRoster => {
                let roster = array(value, "agents")
                    .iter()
                    .map(|a| {
                        format!(
                            "{} · {} · {} · {}",
                            string(a, "displayName"),
                            crate::i18n::tr(if a["isMine"] == true {"Mine"} else {"Public service"}),
                            string(a, "puppetMxid"),
                            string(a, "ownerMxid")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.view.label(cx, ids!(room_roster)).set_text(
                    cx,
                    if roster.is_empty() {
                        "No Agents are currently visible in this Room."
                    } else {
                        &roster
                    },
                );
            }
            Target::ListService => {self.refresh_runtime_list(cx);}
            Target::ServiceRoomChanged => {self.local(cx,Target::Runtime,LocalAction::AgentServiceStatus,None);}
            Target::ReplyPolicyChanged => {
                let updated = &value["binding"];
                if let Some(binding) = self.bindings.iter_mut().find(|b| string(b, "id") == string(updated, "id")) {
                    *binding = updated.clone();
                }
                self.status(cx, "Room reply behavior saved.");
            }
            Target::BindingChanged => {
                self.policy = Value::Null;
                if let Some(agent_id) = self.agent(cx) {
                    self.run(cx, Target::Bindings, Command::Bindings { agent_id });
                }
            }
            Target::Bindings => {
                self.clear_approval_state(cx);self.approvals.clear();
                self.bindings = array(value, "bindings");
                self.label_bindings(cx,ids!(bindings));
                self.label_bindings(cx,ids!(policy_binding));
                if self.open_discovered_binding(cx) {return;}
                let initial = self.bindings.iter().position(|b| string(b,"scopeKind") == "owner_direct" && string(b,"state") == "active").or_else(||self.bindings.iter().position(|b| string(b,"state") == "active"));
                self.select_policy_room(cx, initial.map_or(0,|i|i+1));
                if let Some(index)=initial {let label=self.binding_label(cx,&self.bindings[index]);self.view.label(cx,ids!(binding_details)).set_text(cx,&format!("{} · {}",label,string(&self.bindings[index],"state")));}
                if self.wanted_model.as_deref()==self.agent(cx).as_deref() && self.wanted_model.is_some() {
                    self.wanted_model=None;
                    if self.local_agent(cx) {self.section(cx,ids!(model_section));}
                }
                self.view.drop_down(cx, ids!(policy_layer)).set_selected_item(cx, 0);
                if let Some((agent,binding))=self.wanted_run.take()
                    && self.agent(cx).as_deref()==Some(agent.as_str())
                    && let Some(index)=self.bindings.iter().position(|b|string(b,"id")==binding) {
                    self.select_policy_room(cx,index+1);
                    self.section(cx,ids!(runtime_section));
                    if self.local_agent(cx) {self.load_runtime_resources(cx);}
                    return;
                }
                if self.local_agent(cx) {self.local(cx, Target::Policy, LocalAction::AgentPolicy, None);}
                else if self.remote_agent(cx) {self.load_configuration(cx);}
            }
            Target::ConfigurationStatus => {
                let metadata=&value["configuration"];
                if !configuration_matches(metadata,&self.execution_agent_record) {self.clear_configuration(cx);self.status(cx,"Execution device assignment changed. Reopen this Agent.");return;}
                let was_online=configuration_matches(&self.configuration_status,&self.execution_agent_record) && self.configuration_status["online"]==true;
                let device_changed=self.configuration_status["deviceGeneration"].is_number()
                    && self.configuration_status["deviceGeneration"]!=metadata["deviceGeneration"];
                let requested=std::mem::take(&mut self.configuration_discover);
                if device_changed {
                    let feedback=self.configuration_feedback.clone();
                    self.clear_configuration(cx);self.configuration_feedback=feedback;
                }
                self.configuration_status=metadata.clone();
                self.configuration_metadata_error.clear();
                let discover=requested || (metadata["online"]==true && (!was_online || device_changed));
                if discover && self.configuration_online() {
                    if let Some(agent_id)=self.agent(cx) {self.run(cx,Target::ExecutionEnvironment,Command::ExecutionEnvironment {agent_id});}
                } else if discover && metadata["environment"].is_object() {
                    self.apply(cx,Target::ExecutionEnvironment,&metadata["environment"]);
                }
                cx.stop_timer(self.configuration_timer);self.configuration_timer=cx.start_timeout(3.0);
                self.render_configuration(cx);
                self.sync_navigation(cx);
            }
            Target::ExecutionEnvironment => {
                if value["configuration"]["agentId"].as_str()!=self.agent(cx).as_deref() {return;}
                self.configuration_environment=value.clone();
                let v=&value["configuration"];
                self.policy=json!({"agentId":v["agentId"],"scope":"agent","policies":[v["policy"]],"usage":[v["usage"]],"modelProfile":v["modelProfile"],"modelLoaded":true});
                self.render_policy(cx);
                let runtime=self.provider_runtime(cx);
                if let Some(provider)=array(value,"providers").iter().find(|p|string(p,"runtime")==runtime).cloned() {
                    self.apply(cx,Target::ProviderModels,&provider);
                } else {
                    self.provider_models.clear();self.runtime_efforts.clear();self.credential_ref.clear();self.provider_authenticated=false;self.render_models(cx);
                    self.view.label(cx,ids!(provider_status)).set_text(cx,crate::i18n::tr("This runtime is unavailable on the execution device."));
                }
                self.render_configuration(cx);self.sync_navigation(cx);
                // Refresh the server observation time after the execution device
                // publishes a live environment; cached/offline reads stay inert.
                if self.configuration_online() && let Some(agent_id)=self.agent(cx) {
                    self.run(cx,Target::ConfigurationStatus,Command::AgentConfigurationStatus {agent_id});
                }
            }
            Target::Policy | Target::ModelSaved => {
                self.policy = if value["policy"].is_object() {
                    json!({"agentId":value["agentId"],"scope":"agent","policies":[value["policy"]],"usage":[value["usage"]],"modelProfile":value["modelProfile"],"modelLoaded":value.get("modelProfile").is_some()})
                } else {
                    value.clone()
                };
                self.render_policy(cx);
                if matches!(target,Target::ModelSaved) {self.finish_model_save(cx,Ok(()));}
                else if self.view.view(cx,ids!(model_section)).visible() || (self.runtime_resources_loading && self.view.view(cx,ids!(runtime_section)).visible()) {
                    self.run(cx,Target::ProviderModels,Command::Provider {action:ProviderAction::Models});
                }
            }
            Target::Creation => {
                let next = value["creation"].clone();
                self.view.label(cx, ids!(creation_result)).set_text(
                    cx,
                    &format!(
                        "{} · {} · {}",
                        string(&next, "phase"),
                        string(&next, "roomId"),
                        string(&next, "lastError")
                    ),
                );
                self.creation = Some(next.clone());
                self.view
                    .view(cx, ids!(recovery))
                    .set_visible(cx, next["phase"] != "complete");
                if next["phase"] == "complete" {
                    cx.action(crate::home::project_tree::ProjectTreeChanged {
                        identity: self.identity.clone(),
                    });
                    self.pending_input = None;
                    if let Some(project)=self.auto_default_project.take() && self.project(cx).as_ref()==Some(&project) && next["projectId"]==project && next["kind"]=="room" {
                        self.wanted_project_section=Some((project.clone(),ProjectDetailSection::Rooms));
                        self.run(cx,Target::Rooms,Command::ProjectRooms {project_id:project});return;
                    }
                    self.list_page(cx);
                    self.status(cx, "Creation completed. Refresh Projects to see its Rooms.");
                    self.run(cx, Target::Projects, Command::Projects);
                }
            }
            Target::WorkspacePrepared => {
                self.view.text_input(cx,ids!(workspace)).set_text(cx,string(value,"workspace"));
                self.status(cx,"Private workspace is ready. Save the model profile to use it.");
            }
            Target::ProviderModels => {
                let runtime_check = self.runtime_resources_loading && self.view.view(cx,ids!(runtime_section)).visible();
                self.apply(cx,Target::Provider,value);
                self.provider_models=array(value,"models");
                self.provider_authenticated=value["authenticated"] == true;
                self.credential_ref=string(value,"credentialRef").into();
                self.render_models(cx);
                self.runtime_resources_checked = true;
                self.runtime_resources_loading = false;
                if runtime_check {
                    self.local(cx,Target::Runtime,LocalAction::AgentServiceStatus,None);
                }
            }
            Target::Provider => {
                self.credential_ref = string(value, "credentialRef").into();
                self.provider_url = value["authUrl"].as_str().map(str::to_owned);
                self.provider_authenticated = value["authenticated"] == true;
                self.view.label(cx, ids!(credential)).set_text(cx, "");
                self.provider_state = value.clone();
                self.render_provider_controls(cx);
                self.schedule_provider_check(cx);
                if self.remote_agent(cx) {
                    self.provider_url=None;
                    let installation=if value["installed"]==true {crate::i18n::tr("Installed")} else {crate::i18n::tr("Not installed")};
                    let version=value["version"].as_str().unwrap_or(crate::i18n::tr("Version unknown"));
                    let readiness=match string(value,"state") {"ready"=>crate::i18n::tr("Ready"),"unsupported"=>crate::i18n::tr("Unsupported"),"disconnected"=>crate::i18n::tr("Disconnected"),_=>crate::i18n::tr("Unavailable")};
                    let verified=if value["accountVerified"]==true {crate::i18n::tr("Account verified")} else {crate::i18n::tr("Account verification unavailable")};
                    self.view.label(cx,ids!(provider_status)).set_text(cx,&format!("{installation} · {version} · {readiness} · {verified}"));
                    self.view.label(cx,ids!(runtime_provider_note)).set_text(cx,crate::i18n::tr("Connect uses the owner's provider on the execution device and affects that owner's other Agents. Sign-in and credentials stay on that device."));
                    for id in [ids!(provider_login),ids!(provider_logout),ids!(provider_cancel),ids!(provider_open),ids!(disconnect_provider)] {self.view.button(cx,id).set_visible(cx,false);}
                }
                self.render_model_advanced(cx);
            }
            Target::Runtime => {
                cx.stop_timer(self.runtime_list_timer);
                let label=runtime_list_label(Some(&Ok(value.clone())),self.agent(cx).as_deref().unwrap_or(""));
                self.runtime_list_timer=cx.start_timeout(if label.starts_with("Starting") || label.starts_with("Checking") {2.0} else {10.0});
                self.agent_service_state=value.clone();
                self.view.label(cx,ids!(service_log_text)).set_text(cx,&service_log_text(value));
                if self.input(cx,ids!(reservation)).is_empty() {
                    let reservation=value["service"]["reservation"].as_u64().filter(|n|*n>0).unwrap_or(10000);
                    self.view.text_input(cx,ids!(reservation)).set_text(cx,&reservation.to_string());
                }
                let height=(array(&value["service"],"rooms").len() as f64 * 88.0).clamp(48.0,352.0);
                let mut list=self.view.portal_list(cx,ids!(service_rooms));
                script_apply_eval!(cx,list,{height: #(height)});
                self.runtime_room_active=service_enabled(value);
                let current=Ok(value.clone());
                let mut summary=runtime_service_description(Some(&current),self.agent(cx).as_deref().unwrap_or(""));
                if !self.runtime_action_error.is_empty() {summary.push_str(&format!("\n{}",self.runtime_action_error));}
                self.view.label(cx, ids!(runtime_status)).set_text(cx,&summary);
                if value.to_string().contains("ledger_recovery_required") {
                    self.status(cx,"Restore the complete current-schema ledger of this exact Matrix owner, including spent costs and unresolved holds. No new model requests are allowed; known original replies may reconcile. Stop, restore, then start again.");
                }
                self.refresh_approvals(cx);
            }
            Target::RemoteApproval => {
                if !self.selected(cx,ids!(approvals),&self.approvals).is_some_and(|p|p["proposalId"]==value["approvalId"]) {return;}
                self.remote_approval=value.clone();
                self.view.label(cx,ids!(remote_approval_details)).set_text(cx,&super::approvals::remote_details(&value["remoteApproval"]));
                if let Some(room)=value["remoteApproval"]["approvalRoom"].as_str() {
                    if !self.remote_approval_rooms.iter().any(|r|string(r,"id")==room) {self.remote_approval_rooms.push(json!({"id":room,"name":room}));}
                    self.labels(cx,ids!(remote_approval_room),"Select a private approval Room",&self.remote_approval_rooms,"name");
                    if let Some(index)=self.remote_approval_rooms.iter().position(|r|string(r,"id")==room) {self.view.drop_down(cx,ids!(remote_approval_room)).set_selected_item(cx,index+1);}
                }
                self.sync_navigation(cx);
            }
            Target::RemoteInbox => {
                if self.agent(cx).as_deref()!=value["agentId"].as_str() || !self.selected(cx,ids!(remote_review_room),&self.remote_review_rooms).is_some_and(|r|r["id"]==value["roomId"]) {return;}
                for row in array(value,"requests") {
                    if self.remote_review_requests.len()>=128 {break;}
                    if let Some(existing)=self.remote_review_requests.iter_mut().find(|r|r["requestEventId"]==row["requestEventId"]) {*existing=row;}else{self.remote_review_requests.push(row);}
                }
                self.remote_review_next=value["nextBatch"].as_str().map(str::to_owned);self.render_remote_reviews(cx);self.sync_navigation(cx);
            }
            Target::RemoteVerdict => {
                for row in &mut self.remote_review_requests {if row["request"]["approvalId"]==value["approvalId"] {row["publication"]=value["publication"].clone();}}
                self.render_remote_reviews(cx);self.sync_navigation(cx);
            }
            Target::RemoteApprovalDecision => {
                if value["decided"]==true {if let Some(id)=value["approvalId"].as_str() {self.approval_blocked.insert(id.into());}}
                self.apply(cx,Target::ApprovalDecision,value);
            }
            Target::ApprovalDecision => {
                self.view.label(cx,ids!(approval_feedback)).set_text(cx,crate::i18n::tr(if value["decided"]==true {
                    "Decision submitted. Execution is not yet confirmed. Check the Room reply and runtime status."
                } else {"Decision delivery is unconfirmed. Refresh status; do not resubmit this request."}));
                self.refresh_approvals(cx);
            }
            Target::AgentCreated | Target::AgentResumed => {
                cx.action(crate::home::project_tree::ProjectTreeChanged {identity:self.identity.clone()});
                self.agent_command=None;
                self.list_page(cx);
                let id=string(&value["creation"]["agent"],"id");
                if !id.is_empty() {
                    self.direct_agent=id.into();
                    self.direct_state=json!({"state":"pending"});
                    self.wanted_agent=Some(id.into());
                    if value["creation"]["binding"].is_null() {
                        self.wanted_model=Some(id.into());
                        self.run(cx,Target::OwnerDirectCreated,Command::EnsureAgentOwnerDirect {agent_id:id.into()});
                    } else {self.run(cx,Target::AgentJournal,Command::AgentCommands);}
                } else {self.run(cx,Target::AgentJournal,Command::AgentCommands);}
            }
            Target::OwnerDirect | Target::OwnerDirectCreated => {
                self.direct_state=value["ownerDirect"].clone();
                self.schedule_direct_retry(cx);
                self.render_owner_direct(cx);
                if matches!(target,Target::OwnerDirectCreated) {self.run(cx,Target::AgentJournal,Command::AgentCommands);}
            }
            Target::ProjectAdopted
            | Target::RoomAdopted
            | Target::AgentBound => {
                cx.action(crate::home::project_tree::ProjectTreeChanged {
                    identity: self.identity.clone(),
                });
                if matches!(target, Target::RoomAdopted) {
                    self.wanted_project = self.project(cx);
                }
                self.list_page(cx);
                if matches!(target, Target::AgentCreated | Target::AgentBound) {
                    self.agent_command = None;
                }
                let query = if matches!(target, Target::AgentCreated | Target::AgentBound) {
                    Command::AgentCommands
                } else {
                    mutation_refresh(target).expect("confirmed mutation refresh")
                };
                self.run(
                    cx,
                    if matches!(target, Target::AgentCreated | Target::AgentBound) {
                        Target::AgentJournal
                    } else {
                        Target::Projects
                    },
                    query,
                );
            }
            Target::Changed => {
                cx.action(crate::home::project_tree::ProjectTreeChanged {
                    identity: self.identity.clone(),
                });
                self.status(cx, "Saved. Refresh to load the confirmed state.");
            }
        }
        self.view.redraw(cx);
    }
}
impl HagencyPanel {
    fn runtime_feedback(&mut self, cx:&mut Cx, message:&str) {
        self.runtime_action_error=crate::i18n::tr(message).to_owned();
        self.view.label(cx,ids!(runtime_status)).set_text(cx,&self.runtime_action_error);
        self.status(cx,message);
    }
    fn local(&mut self, cx: &mut Cx, target: Target, action: LocalAction, input: Option<Value>) {
        if !self.local_agent(cx) && !self.remote_agent(cx) {self.status(cx, "This action is available only on the execution device.");return;}
        if matches!(
            action,
            LocalAction::AgentPolicy
                | LocalAction::SaveAgentPolicy
                | LocalAction::ResetAgentPolicy
                | LocalAction::AgentModel
                | LocalAction::SaveAgentModel
                | LocalAction::StartAgentService
                | LocalAction::StopAgentService
                | LocalAction::AgentServiceStatus
        ) {
            if let Some(agent_id) = self.agent(cx) {
                self.run(
                    cx,
                    target,
                    Command::Local {
                        agent_id,
                        action,
                        input,
                        binding_id: None,
                        requester: None,
                    },
                );
            }
            return;
        }
        let (Some(agent_id), Some(binding_id)) = (self.agent(cx), self.binding(cx)) else {
            self.status(cx, "Select an owned Agent and Room binding first.");
            return;
        };
        let requester = self.input(cx, ids!(requester));
        self.run(
            cx,
            target,
            Command::Local {
                agent_id,
                action,
                input,
                binding_id: Some(binding_id),
                requester: Some(requester),
            },
        );
    }
    fn create_matrix(&mut self, cx: &mut Cx) {
        if self
            .creation
            .as_ref()
            .is_some_and(|v| v["phase"] != "complete")
        {
            self.status(
                cx,
                "An uncertain original creation must be resumed first. Do not create a duplicate.",
            );
            return;
        }
        let existing = self.create_room && self.view.drop_down(cx,ids!(creation_source)).selected_item()==1;
        if existing {
            if self.create_room {
                if let Some(project_id) = self.project(cx) {
                    let room_id = self.input(cx, ids!(existing_room));
                    self.run(
                        cx,
                        Target::RoomAdopted,
                        Command::AdoptRoom {
                            project_id,
                            room_id,
                        },
                    );
                }
            } else if let Some(space) = self.selected(cx, ids!(candidates), &self.spaces) {
                self.run(
                    cx,
                    Target::ProjectAdopted,
                    Command::AdoptProject {
                        space_id: string(&space, "spaceId").into(),
                    },
                );
            } else {
                self.status(cx, "Choose an unconnected Space candidate first.");
            }
            return;
        }
        let name = self.input(cx, ids!(creation_name));
        if name.is_empty() {
            self.status(cx, "Enter a name.");
            return;
        }
        let mut input = json!({"kind":if self.create_room{"room"}else{"space"},"name":name,"commandId":uuid::Uuid::new_v4().to_string()});
        if self.create_room {
            let Some(project) = self.selected(cx, ids!(projects), &self.projects) else {
                self.status(cx, "Select a Project before adding a Room.");
                return;
            };
            input["encrypted"] = self.view.check_box(cx,ids!(creation_encrypted)).active(cx).into();
            input["projectId"] = project["id"].clone();
            input["spaceId"] = project["spaceId"].clone();
        }
        self.pending_input = Some(input.clone());
        self.creation = Some(json!({"commandId":input["commandId"],"phase":"unknown"}));
        self.run(cx, Target::Creation, Command::CreateMatrix { input });
    }
    fn save_policy(&mut self, cx: &mut Cx) {
        let Some(agent) = self.agent(cx) else {
            return;
        };
        let binding = self.binding(cx);
        let requester = self.input(cx, ids!(requester));
        let index = self
            .view
            .drop_down(cx, ids!(policy_layer))
            .selected_item()
            .min(2);
        if !editor_matches(&self.policy, &agent, binding.as_deref(), &requester, index) {
            self.status(cx, "Load the exact Agent or Room policy before saving.");
            return;
        }
        let Some(revision) = self.policy["policies"][index]["revision"].as_i64() else {
            self.status(cx, "Load the exact policy before editing.");
            return;
        };
        let limit = match self.view.drop_down(cx, ids!(limit_mode)).selected_item() {
            0 => json!("Unlimited"),
            _ => {
                let Ok(tokens) = self.input(cx, ids!(token_limit)).parse::<u64>() else {
                    self.status(cx, "Enter a nonnegative whole token count.");
                    return;
                };
                if tokens > 9_007_199_254_740_991 {
                    self.status(cx, "Token count exceeds the supported exact integer range.");
                    return;
                }
                json!({"Tokens":tokens})
            }
        };
        let period = ["Lifetime", "UtcDay", "UtcMonth"]
            [self.view.drop_down(cx, ids!(period)).selected_item().min(2)];
        let requests = ["Allow", "Deny", "AskOwner"][self
            .view
            .drop_down(cx, ids!(requests))
            .selected_item()
            .min(2)];
        let risk = match self.view.drop_down(cx, ids!(tool_risk)).selected_item() {
            1 => json!("AskOwner"),
            2 => {
                let tools = self
                    .input(cx, ids!(allowed_tools))
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let dirs = self
                    .input(cx, ids!(allowed_directories))
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if tools.is_empty() || dirs.is_empty() {
                    self.status(
                        cx,
                        "Exact allow rules require both tools and absolute directories.",
                    );
                    return;
                }
                json!({"AllowWithRules":{"tools":tools,"directories":dirs}})
            }
            _ => json!("Deny"),
        };
        let policy =
            json!({"budget":{"limit":limit,"period":period},"requests":requests,"high_risk":risk});
        if index == 0 {
            self.local(
                cx,
                Target::Policy,
                LocalAction::SaveAgentPolicy,
                Some(json!({"expectedRevision":revision,"policy":policy})),
            );
        } else if let Some(binding_id) = binding {
            self.local(cx, Target::Policy, LocalAction::SavePolicy, Some(json!({"bindingId":binding_id,"requester":requester,"layer":(["agent","room","requester"][index]),"expectedRevision":revision,"policy":policy})));
        }
    }
}
impl ScriptHook for HagencyPanel {
    fn on_after_apply(&mut self, _vm: &mut ScriptVm, _: &Apply, _: &mut Scope, _: ScriptValue) {
        self.navigation_ready.set(false);
    }
}
impl Widget for HagencyPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if matches!(event, Event::Actions(_) | Event::Startup | Event::LiveEdit | Event::ScriptReapply) {
            self.navigation_ready.set(false);
        }
        if let Event::Actions(actions)=event {for action in actions {
            if let Some(crate::home::project_tree::ArchivedProjectsVisibility(show))=action.downcast_ref() {
                self.show_archived_projects = *show;
                self.view.check_box(cx,ids!(show_archived_projects)).set_active(cx,*show,Animate::No);
                if !self.busy && !self.view.view(cx,ids!(project_detail)).visible() {
                    self.projects=self.all_projects.iter().filter(|p|*show || p["archived"]!=true).cloned().collect();
                    self.labels(cx,ids!(projects),"Select a Project",&self.projects,"id");
                }
            }
        }}
        let board_active=self.selected_board.is_some() && self.active && !self.display_blocked && self.page==Page::Projects && self.view.view(cx,ids!(project_detail)).visible() && self.view.view(cx,ids!(board_section)).visible();
        let project=self.project(cx);
        self.view.project_board(cx,ids!(board)).bind_scope(cx,&self.identity,project.as_deref(),self.selected_board.as_deref(),board_active);
        self.project_toolbar_tooltips(cx,event);
        self.view.handle_event(cx, event, scope);
        if !self.active {
            return;
        }
        let Ok(service) = Service::for_current() else {
            self.cancel_direct_retry(cx);
            self.cancel_read_wait(cx);
            self.active = false;
            self.request = nonce();
            self.view
                .view(cx, ids!(projects_page))
                .set_visible(cx, false);
            self.view.view(cx, ids!(agents_page)).set_visible(cx, false);
            self.display_blocked=true;
            self.status(cx, "Sign in with a Matrix account to use Hagency.");
            return;
        };
        if service.identity() != self.identity {
            self.cancel_direct_retry(cx);
            self.cancel_read_wait(cx);
            self.active = false;
            self.busy = false;
            self.request = nonce();
            self.view
                .view(cx, ids!(projects_page))
                .set_visible(cx, false);
            self.view.view(cx, ids!(agents_page)).set_visible(cx, false);
            self.display_blocked=true;
            self.status(cx,"Account changed. Reopen management for this account; no old operations are adopted.");
            return;
        }
        if self.provider_timer.is_event(event).is_some() {
            self.provider_timer=Timer::default();
            if self.active && self.view.view(cx,ids!(model_section)).visible() && self.local_agent(cx) && self.provider_url.is_some() && !self.provider_authenticated {
                if self.busy {self.schedule_provider_check(cx);}
                else {self.provider_poll_count+=1;self.run(cx,Target::Provider,Command::Provider {action:ProviderAction::Status});}
            }
        }
        if self.approval_timer.is_event(event).is_some() {
            if self.approval_started.is_some_and(|at|at.elapsed().as_secs()>=10) {
                self.approval_checked=false;
                self.view.label(cx,ids!(approval_count)).set_text(cx,crate::i18n::tr("Approvals could not be refreshed. Decisions are disabled until a fresh check succeeds."));
            }
            self.sync_navigation(cx);self.render_approval_details(cx);self.refresh_approvals(cx);
            self.approval_timer=cx.start_timeout(3.0);
        }
        if self.runtime_list_timer.is_event(event).is_some() {
            if self.view.view(cx,ids!(runtime_section)).visible() && self.local_agent(cx) {
                if !self.busy {self.local(cx,Target::Runtime,LocalAction::AgentServiceStatus,None);} else {self.runtime_list_timer=cx.start_timeout(2.0);}
            } else {self.refresh_runtime_list(cx);}
        }
        if self.configuration_timer.is_event(event).is_some() {
            if self.remote_agent(cx) && self.view.view(cx,ids!(agent_detail)).visible() {
                self.poll_configuration_status(cx);
            }
        }
        if self.read_timer.is_event(event).is_some() {
            self.expire_read_wait(cx, std::time::Instant::now());
            return;
        }
        if self.direct_timer.is_event(event).is_some() {
            self.direct_timer = Timer::default();
            if let Some((identity,agent,_))=self.direct_retry.clone() {
                let target = self.agent(cx).as_deref() == Some(agent.as_str()) || self.wanted_agent.as_deref() == Some(agent.as_str());
                if identity != self.identity || self.page != Page::Agents || !target {self.cancel_direct_retry(cx);}
                else if self.busy {self.schedule_direct_retry(cx);}
                else {self.run(cx,Target::OwnerDirect,Command::EnsureAgentOwnerDirect {agent_id:agent});}
            }
            return;
        }
        let Event::Actions(actions) = event else {
            return;
        };
        for action in actions {
            if let Some(done)=action.downcast_ref::<ledger::Selected>() {
                self.accept_ledger_selection(cx,done);
            }
            if let Some(done)=action.downcast_ref::<ConfigurationCompleted>()
                && self.configuration_poll_request==Some(done.request) {
                self.configuration_poll_request=None;
                if self.busy {self.configuration_timer=cx.start_timeout(3.0);continue;}
                if done.identity==self.identity && self.agent(cx).as_deref()==Some(done.agent.as_str())
                    && self.execution_agent_record["generation"].as_i64()==Some(done.generation)
                    && string(&self.execution_agent_record,"executionDeviceId")==done.device {
                    match &done.result {
                        Ok(value)=>self.apply(cx,Target::ConfigurationStatus,value),
                        Err(error)=> {self.finish_configuration_intent(None,&Err(error.clone()));self.sync_navigation(cx);self.configuration_timer=cx.start_timeout(3.0);}
                    }
                }
            }
            if let Some(done)=action.downcast_ref::<ApprovalsCompleted>()
                && self.approval_poll==Some(done.request) {
                let accepted=self.accepts_approval_poll(cx,done);
                self.approval_poll=None;self.approval_started=None;
                if !accepted {self.approval_checked=false;continue;}
                match &done.result {
                    Ok(value)=>self.apply_approvals(cx,value),
                    Err(_)=> {self.approval_checked=false;self.sync_navigation(cx);self.view.label(cx,ids!(approval_count)).set_text(cx,crate::i18n::tr("Approvals could not be refreshed. Decisions are disabled until a fresh check succeeds."));}
                }
                cx.stop_timer(self.approval_timer);self.approval_timer=cx.start_timeout(3.0);
            }
            if let Some(done)=action.downcast_ref::<RuntimeListCompleted>()
                && self.active && self.identity==done.identity && self.runtime_list_request==done.request {
                self.runtime_list_loading=false;
                self.runtime_list=done.states.iter().cloned().collect();
                self.runtime_list_timer=cx.start_timeout(10.0);
                self.view.redraw(cx);
            }
            if let Some(done) = action.downcast_ref::<Completed>()
                && current_completion(self.active, &self.identity, self.request, done)
            {
                self.cancel_read_wait(cx);
                if done.configuration_fence.as_ref().is_some_and(|(agent,generation,device)|self.agent(cx).as_deref()!=Some(agent.as_str()) || self.execution_agent_record["generation"].as_i64()!=Some(*generation) || string(&self.execution_agent_record,"executionDeviceId")!=device) {
                    if matches!(done.target,Target::ModelSaved) {self.model_saving=false;self.clear_model_save_feedback(cx);}
                    self.set_busy(cx,false);self.clear_configuration(cx);continue;
                }
                if matches!(done.target,Target::ListService) {
                    let agent=std::mem::take(&mut self.list_service_pending_agent);
                    if !agent.is_empty() {
                        match &done.result {
                            Err(error)=>{self.runtime_list_errors.insert(agent,runtime_diagnostic(error));},
                            Ok(_)=>{self.runtime_list_errors.remove(&agent);},
                        }
                    }
                }
                if matches!(done.target,Target::Runtime) && self.runtime_pending_action.take().is_some() {
                    self.runtime_action_error=done.result.as_ref().err().map(|error|runtime_diagnostic(error)).unwrap_or_default();
                }
                self.set_busy(cx, false);
                if matches!(done.target,Target::AgentCreated) {self.agent_creating=false;}
                self.sync_navigation(cx);
                if matches!(done.target, Target::Agents) { self.agents_loading = false; }
                if done.configuration_fence.is_some() {self.finish_configuration_intent(done.configuration_digest.as_ref(),&done.result);}
                match &done.result {
                    Ok(value) => {
                        self.view.button(cx, ids!(login)).set_visible(cx, false);
                        if !matches!(done.target,Target::ConfigurationStatus) {self.status(cx, "");}
                        self.apply(cx, done.target, value);
                        if matches!(done.target,Target::Provider) && value["authenticated"] == true && self.view.view(cx,ids!(model_section)).visible() {
                            self.run(cx,Target::ProviderModels,Command::Provider {action:ProviderAction::Models});
                        }
                    }
                    Err(error) => {
                        if matches!(done.target,Target::Provider|Target::ProviderModels) {
                            self.provider_authenticated=false;
                            self.provider_state["checkFailed"]=true.into();
                            cx.stop_timer(self.provider_timer);self.provider_timer=Timer::default();
                            self.render_provider_controls(cx);
                        }
                        if matches!(done.target,Target::ModelSaved) {self.finish_model_save(cx,Err(error));}
                        if done.configuration_fence.is_some() {self.sync_navigation(cx);}
                        if matches!(done.target,Target::RemoteApprovalDecision) && error=="remote_approval_waiting" {
                            self.view.label(cx,ids!(remote_approval_details)).set_text(cx,crate::i18n::tr("Waiting for a verified decision."));
                            self.status(cx,"");continue;
                        }
                        if matches!(done.target,Target::RemoteApproval | Target::RemoteApprovalDecision) {
                            self.view.label(cx,ids!(remote_approval_details)).set_text(cx,crate::i18n::tr("Private approval could not be confirmed. Refresh its original status."));
                            self.refresh_approvals(cx);
                        }
                        if matches!(done.target,Target::ApprovalDecision) {
                            self.view.label(cx,ids!(approval_feedback)).set_text(cx,crate::i18n::tr("Decision delivery is unconfirmed. Refresh status; do not resubmit this request."));
                            self.refresh_approvals(cx);
                        }
                        if self.runtime_resources_loading && matches!(done.target, Target::Policy | Target::ProviderModels) {self.runtime_resources_loading=false;self.runtime_resources_checked=false;}
                        let reauthenticate = matches!(
                            error.as_str(),
                            "matrix_oauth_sign_in_required"
                                | "matrix_authorization_required"
                                | "matrix_sign_in_required"
                                | "sign_in_required"
                        );
                        self.view
                            .button(cx, ids!(login))
                            .set_visible(cx, reauthenticate);
                        let message = if error.contains("ledger_recovery_required") {
                            "Restore this exact owner's complete ledger before running new model requests. Stop, restore, then restart.".to_owned()
                        } else if reauthenticate {
                            "Sign in through Pasion to restore this account. Chats and Agent management use the same login.".to_owned()
                        } else if error == "matrix_authorization_unavailable" {
                            "Cannot verify this account right now. Check the server connection and retry.".to_owned()
                        } else {
                            if matches!(done.target,Target::ListService | Target::Runtime) {runtime_diagnostic(error)} else {visible_error(error)}
                        };
                        if !matches!(done.target,Target::ConfigurationStatus | Target::ModelSaved) {self.show_failure(cx,done.target,error,&message);}
                        if matches!(done.target,Target::ProjectArchived|Target::NavigationDefaultChanged) {self.load_project_home(cx);}
                        if matches!(done.target,Target::Runtime) {
                            let current=Ok(self.agent_service_state.clone());
                            let mut summary=runtime_service_description(Some(&current),self.agent(cx).as_deref().unwrap_or(""));
                            summary.push_str(&format!("\n{}",if self.runtime_action_error.is_empty() {message.as_str()} else {self.runtime_action_error.as_str()}));
                            self.view.label(cx,ids!(runtime_status)).set_text(cx,&summary);
                        }
                        if matches!(done.target,Target::OwnerDirect | Target::OwnerDirectCreated) {
                            self.cancel_direct_retry(cx);
                            self.direct_state["state"]="pending".into();
                            self.render_owner_direct(cx);
                            if matches!(done.target,Target::OwnerDirectCreated) {self.run(cx,Target::AgentJournal,Command::AgentCommands);}
                        }
                        if matches!(done.target, Target::Members) {
                            self.view.label(cx, ids!(members_status)).set_text(cx, if error == "room_join_required" {
                                "Join this Space or Room first to view its members. Joining the Project does not join its Rooms."
                            } else { "Could not load members. Check your connection and retry." });
                        }
                        if matches!(done.target, Target::Creation) && self.pending_input.is_some() {
                            if self.auto_default_project.is_some() {self.view.view(cx,ids!(project_detail)).set_visible(cx,false);self.view.view(cx,ids!(project_form)).set_visible(cx,true);self.create_room=true;self.creation_source(cx);}
                            self.view.view(cx, ids!(recovery)).set_visible(cx, true);
                            self.view.label(cx, ids!(creation_result)).set_text(cx, crate::i18n::tr("Result unconfirmed. Resume this original creation; do not start a replacement."));
                        }
                        if matches!(done.target, Target::Creation)
                            && error.contains("creation_command_not_found")
                        {
                            if let Some(creation) = self.creation.as_mut() {
                                creation["phase"] = "unrecorded".into();
                            }
                            self.view.label(cx, ids!(creation_result)).set_text(
                                cx,
                                "Original command not recorded yet; retry the SAME ID and input.",
                            );
                        }
                    }
                }
            }
        }
        if self.busy {
            return;
        }
        #[cfg(feature = "agent_chat")]
        if self.approval_chat_actions(cx,actions) {return;}
        if self.ledger_actions(cx,actions) {return;}
        if self.usage_actions(cx,actions) {return;}
        if self.history_actions(cx,actions) {return;}
        if self.file_history_actions(cx,actions) {return;}
        if self.recovery_actions(cx,actions) {return;}
        if self.view.inline_notice(cx,ids!(status_holder)).action_clicked(cx,actions) {
            self.list_page(cx);self.run(cx,Target::AgentJournal,Command::AgentCommands);return;
        }
        if self.agent_request_actions(cx,actions) {return;}
        if self.view.view(cx,ids!(agent_create)).visible()
            && (self.view.text_input(cx,ids!(agent_name)).changed(actions).is_some()
                || self.view.drop_down(cx,ids!(new_agent_visibility)).selected(actions).is_some()) {
            self.status(cx,"");self.agent_command=None;return;
        }
        if self.view.button(cx,ids!(continue_to_service)).clicked(actions) {
            self.status(cx,"");self.section(cx,ids!(runtime_section));
            if self.local_agent(cx) {self.load_runtime_resources(cx);}
            return;
        }
        if self.view.view(cx,ids!(runtime_section)).visible() {
            let list=self.view.portal_list(cx,ids!(service_rooms));
            let rooms=array(&self.agent_service_state["service"],"rooms");
            for (_,row) in list.items_with_actions(actions) {
                let Some((agent,binding_id))=self.service_row_targets.get(&row.widget_uid()) else {continue;};
                if self.agent(cx).as_deref()!=Some(agent.as_str()) {continue;}
                let Some(binding)=rooms.iter().find(|b|string(b,"id")==binding_id) else {continue;};
                if row.button(cx,ids!(open_room_chat)).clicked(actions) {
                    if let Ok(room)=matrix_sdk::ruma::OwnedRoomId::try_from(string(binding,"roomId")) {navigate_agent_chat(cx,room,string(binding,"scopeKind")=="owner_direct",None);}
                    return;
                }
                if row.button(cx,ids!(room_toggle)).clicked(actions) && let Some(agent_id)=self.agent(cx) {
                    let paused=binding["ownerServicePaused"] == true;
                    if !paused && binding["state"] != "active" {return;}
                    self.run(cx,Target::ServiceRoomChanged,Command::Binding{agent_id,binding_id:string(binding,"id").into(),action:if paused {BindingAction::Resume} else {BindingAction::Pause}});
                    return;
                }
            }
        }
        if self.back_to_list(cx, actions) {
            return;
        }
        if self.view.button(cx, ids!(new_item)).clicked(actions) {
            self.reset_detail_scroll(cx);
            self.list_page(cx);
            if self.page == Page::Projects {
                if !self.begin_creation_form(cx,false) {return;}
                self.create_room = false;
                self.view.drop_down(cx, ids!(creation_source)).set_labels(
                    cx,
                    vec![
                        crate::i18n::tr("Create a new Space (default)").into(),
                    ],
                );
                self.view
                    .label(cx, ids!(creation_heading))
                    .set_text(cx, crate::i18n::tr("Create Project"));
                self.view
                    .view(cx, ids!(project_list_holder))
                    .set_visible(cx, false);
                self.view.view(cx, ids!(project_form)).set_visible(cx, true);
                self.view
                    .drop_down(cx, ids!(creation_source))
                    .set_selected_item(cx, 0);
                self.creation_source(cx);
            } else {
                self.begin_agent_creation(cx);
            }
            return;
        }
        if self.view.button(cx, ids!(permissions_tab)).clicked(actions)
            || self
                .view
                .button(cx, ids!(room_permissions))
                .clicked(actions)
        {
            let room = self
                .view
                .button(cx, ids!(room_permissions))
                .clicked(actions);
            self.load_permissions(cx, room);
            return;
        }
        if self
            .view
            .button(cx, ids!(save_creation_policy))
            .clicked(actions)
            || self
                .view
                .button(cx, ids!(save_scope_pause))
                .clicked(actions)
        {
            let Some(project_id) = self.project(cx) else {
                return;
            };
            if self.scope_state["canManagePolicy"] != true
                || string(&self.scope_state, "projectId") != project_id
            {
                return;
            }
            let room_id = self.scope_state["roomId"].as_str().map(str::to_owned);
            if let Some(room) = &room_id {
                if self
                    .selected(cx, ids!(rooms), &self.rooms)
                    .as_ref()
                    .map(|r| string(r, "roomId"))
                    != Some(room.as_str())
                {
                    return;
                }
            }
            let command = if self
                .view
                .button(cx, ids!(save_scope_pause))
                .clicked(actions)
            {
                Command::SetScopeServicePause {
                    project_id,
                    room_id,
                    paused: self.view.check_box(cx, ids!(scope_pause)).active(cx),
                }
            } else {
                let Some(expected_revision) = self.scope_state["revision"].as_i64() else {
                    return;
                };
                let users = |text: String| {
                    text.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect::<std::collections::BTreeSet<String>>()
                };
                let allow = users(self.input(cx, ids!(creation_allow)));
                let deny = users(self.input(cx, ids!(creation_deny)));
                let mode = self.view.drop_down(cx, ids!(creation_mode)).selected_item();
                if let Some(room_id) = room_id {
                    let policy = match mode {
                        1 => RoomCreationPolicy::AllowList { allow, deny },
                        2 => RoomCreationPolicy::Disabled,
                        _ => RoomCreationPolicy::InheritProject { deny },
                    };
                    Command::SetRoomCreationPolicy {
                        project_id,
                        room_id,
                        expected_revision,
                        policy,
                    }
                } else {
                    Command::SetProjectCreationPolicy {
                        project_id,
                        expected_revision,
                        policy: ProjectCreationPolicy {
                            default_allow: mode == 0,
                            allow,
                            deny,
                        },
                    }
                }
            };
            self.run(cx, Target::ScopeChanged, command);
            return;
        }
        if self.handle_project_navigation(cx,actions) {return;}
        if self.view.button(cx, ids!(board_tab)).clicked(actions) {
            if !self.view.project_board(cx,ids!(board)).can_switch(cx) {return;}
            self.selected_board=self.board_catalog.first().and_then(|b|b["id"].as_str()).map(str::to_owned);
            self.project_section(cx,ids!(board_section));return;
        }
        if self.view.button(cx, ids!(overview_tab)).clicked(actions) {
            self.project_section(cx, ids!(overview_section));
            self.load_project_home(cx);
            return;
        }
        if self.view.button(cx, ids!(rooms_tab)).clicked(actions) {
            self.refresh_project_rooms(cx);
            return;
        }
        if self.view.button(cx, ids!(members_tab)).clicked(actions) {
            if let Some(p) = self.selected(cx, ids!(projects), &self.projects) {
                self.show_members(cx, string(&p, "spaceId").into(), true);
            }
            return;
        }
        if self.view.button(cx, ids!(room_members)).clicked(actions) {
            if let Some(r) = self.selected(cx, ids!(rooms), &self.rooms) {
                self.show_members(cx, string(&r, "roomId").into(), false);
            } else {
                self.status(cx, "Select a discussion Room first.");
            }
            return;
        }
        if self.view.button(cx, ids!(refresh_members)).clicked(actions) {
            if let Some((room, space)) = self.members_room.clone() {
                self.show_members(cx, room, space);
            }
            return;
        }
        if self.view.button(cx, ids!(invite_member)).clicked(actions) {
            if self
                .view
                .button(cx, ids!(invite_member))
                .borrow()
                .is_some_and(|b| b.enabled())
                && let Some((room, _)) = &self.members_room
                && let Ok(id) = matrix_sdk::ruma::OwnedRoomId::try_from(room.as_str())
            {
                cx.action(crate::home::invite_modal::InviteModalAction::OpenProject(
                    crate::utils::RoomNameId::empty(id),
                ));
            }
            return;
        }
        if self.view.button(cx,ids!(open_room)).clicked(actions) {
            if let Some(row)=self.selected(cx,ids!(rooms),&self.rooms) && let Ok(id)=matrix_sdk::ruma::OwnedRoomId::try_from(string(&row,"roomId")) {
                cx.action(crate::app::AppStateAction::NavigateToRoom {room_to_close:None,destination_room:crate::room::BasicRoomDetails::RoomId(crate::utils::RoomNameId::empty(id))});
            }else{self.status(cx,"Select a discussion Room first.");}
            return;
        }
        if self.open_card(cx, actions) {
            return;
        }
        if self.view.button(cx,ids!(remote_review_tab)).clicked(actions) {
            self.section(cx,ids!(remote_review_section));self.refresh_remote_review_rooms(cx);self.sync_navigation(cx);return;
        }
        for (id, section) in [
            (ids!(setup_tab), ids!(setup_section)),
            (ids!(policy_tab), ids!(policy_section)),
            (ids!(model_tab), ids!(model_section)),
            (ids!(runtime_tab), ids!(runtime_section)),
        ] {
            if self.view.button(cx, id).clicked(actions) {
                self.section(cx, section);
                if section == ids!(runtime_section) {self.load_runtime_resources(cx); return;}
                if section == ids!(policy_section) || section == ids!(model_section) {
                    if self.remote_agent(cx) {
                        self.view.drop_down(cx,ids!(policy_layer)).set_selected_item(cx,0);
                        self.clear_configuration(cx);
                        self.load_configuration(cx);
                        return;
                    }
                    self.policy = Value::Null;
                    let global = section == ids!(policy_section)
                        && self.view.drop_down(cx, ids!(policy_layer)).selected_item() == 0;
                    self.local(
                        cx,
                        Target::Policy,
                        if section == ids!(model_section) {
                            LocalAction::AgentModel
                        } else if global {
                            LocalAction::AgentPolicy
                        } else {
                            LocalAction::Policy
                        },
                        None,
                    );
                }
                return;
            }
        }
        if self.view.button(cx, ids!(save_agent_visibility)).clicked(actions) {
            if let Some(agent_id) = self.agent(cx) {
                if !self.selected(cx, ids!(agents), &self.agents).is_some_and(|a|matches!(a["visibility"].as_str(),Some("public" | "private")) && matches!(a["state"].as_str(),Some("creating" | "active" | "suspended"))) {return;}
                let visibility = if self.view.drop_down(cx, ids!(agent_visibility)).selected_item() == 1 { "public" } else { "private" };
                self.run(cx, Target::VisibilityChanged, Command::SetAgentVisibility {agent_id, visibility: visibility.into()});
            }
            return;
        }
        if self.view.button(cx, ids!(another_binding)).clicked(actions) {
            if !self.selected(cx, ids!(agents), &self.agents).is_some_and(|a|a["visibility"] == "public") {
                self.status(cx, "Private Agents can only chat with you. Publish this Agent before adding a shared Room.");
                return;
            }
            self.view
                .view(cx, ids!(agent_detail))
                .set_visible(cx, false);
            self.view.view(cx, ids!(agent_create)).set_visible(cx, true);
            self.view.button(cx, ids!(bind_agent)).set_visible(cx, true);
            self.view
                .view(cx, ids!(agent_name_holder))
                .set_visible(cx, false);
            self.view
                .button(cx, ids!(create_agent))
                .set_visible(cx, false);
            return;
        }
        if self.view.drop_down(cx,ids!(limit_mode)).selected(actions).is_some() {
            let visible=self.view.drop_down(cx,ids!(limit_mode)).selected_item()==1;
            self.view.view(cx,ids!(token_amount_holder)).set_visible(cx,visible);
            self.view.view(cx,ids!(period_holder)).set_visible(cx,visible);
            self.view.label(cx,ids!(zero_limit_note)).set_visible(cx,visible);
        }
        if self
            .view
            .drop_down(cx, ids!(tool_risk))
            .selected(actions)
            .is_some()
        {
            let rules = self.view.drop_down(cx, ids!(tool_risk)).selected_item() == 2;
            self.view
                .view(cx, ids!(allowed_tools_holder))
                .set_visible(cx, rules);
            self.view
                .view(cx, ids!(allowed_directories_holder))
                .set_visible(cx, rules);
            return;
        }
        if self.view.button(cx, ids!(login)).clicked(actions) {
            cx.action(crate::accounts::AccountAction::Reauthenticate);
            return;
        }
        if self.view.button(cx,ids!(configuration_refresh)).clicked(actions) {self.load_configuration(cx);return;}
        if self.view.button(cx, ids!(refresh)).clicked(actions) {
            if self.page == Page::Projects {
                self.run(cx, Target::Journal, Command::Creations);
            } else {
                self.run(cx, Target::AgentJournal, Command::AgentCommands);
            }
            return;
        }
        if self
            .view
            .drop_down(cx, ids!(projects))
            .selected(actions)
            .is_some()
        {
            if let Some(p) = self.selected(cx, ids!(projects), &self.projects) {
                self.view.label(cx, ids!(project_details)).set_text(
                    cx,
                    string(&p, "topic"),
                );
                self.run(
                    cx,
                    Target::Rooms,
                    Command::ProjectRooms {
                        project_id: string(&p, "id").into(),
                    },
                );
            }
            return;
        }
        if self
            .view
            .drop_down(cx, ids!(rooms))
            .selected(actions)
            .is_some()
        {
            self.room_can_manage=false;
            if let Some(r) = self.selected(cx, ids!(rooms), &self.rooms) {
                self.view.label(cx, ids!(room_details)).set_text(
                    cx,
                    &format!(
                        "{} · {}\n{}",
                        string(&r, "name"),
                        string(&r, "roomId"),
                        string(&r, "topic")
                    ),
                );
                if let Some(project_id)=self.project(cx) {self.run(cx,Target::RoomCapabilities,Command::ScopeServiceState {project_id,room_id:Some(string(&r,"roomId").into())});}
            }
            return;
        }
        if self.view.button(cx, ids!(new_room)).clicked(actions) {self.open_room_creation(cx);return;}
        if self.view.button(cx, ids!(room_agents)).clicked(actions) {
            cx.action(HagencyAction::OpenAgents);
            return;
        }
        if self
            .view
            .drop_down(cx, ids!(creation_source))
            .selected(actions)
            .is_some()
        {
            self.creation_source(cx);
        }
        if self
            .view
            .drop_down(cx, ids!(creation_source))
            .selected(actions)
            == Some(1)
            && !self.create_room
        {
            self.cursor = None;
            self.run(
                cx,
                Target::Spaces,
                Command::SpaceCandidates { cursor: None },
            );
            return;
        }
        if self.view.button(cx, ids!(candidates_more)).clicked(actions) {
            self.run(
                cx,
                Target::Spaces,
                Command::SpaceCandidates {
                    cursor: self.cursor.clone(),
                },
            );
            return;
        }
        if self
            .view
            .button(cx, ids!(create_project_room))
            .clicked(actions)
        {
            self.create_matrix(cx);
            return;
        }
        if self.view.button(cx, ids!(resume_creation)).clicked(actions) {
            if let Some(input) = self.pending_input.clone() {
                let phase = self
                    .creation
                    .as_ref()
                    .map(|v| string(v, "phase"))
                    .unwrap_or("unknown");
                let command = recovery_command(input, phase, self.input(cx, ids!(found_room)));
                self.run(cx, Target::Creation, command);
            } else {
                self.status(cx, "No uncertain creation is recorded for this account.");
            }
            return;
        }
        if self
            .view
            .drop_down(cx, ids!(agent_project))
            .selected(actions)
            .is_some()
        {
            if let Some(p) = self.selected(cx, ids!(agent_project), &self.projects) {
                self.run(
                    cx,
                    Target::Rooms,
                    Command::ProjectRooms {
                        project_id: string(&p, "id").into(),
                    },
                );
            }
            return;
        }
        if self.view.button(cx, ids!(create_agent)).clicked(actions)
            || self.view.button(cx, ids!(bind_agent)).clicked(actions)
        {
            let bind = self.view.button(cx, ids!(bind_agent)).clicked(actions);
            let mut input = if bind {
                let (Some(project), Some(room)) = (self.selected(cx, ids!(agent_project), &self.projects), self.selected(cx, ids!(agent_room), &self.agent_rooms)) else {self.status(cx,"Choose a Project and a visible registered Room."); return;};
                json!({"projectId":project["id"],"roomId":room["roomId"]})
            } else {
                let name = self.input(cx, ids!(agent_name));
                if name.is_empty() || name.chars().count()>64 || name.chars().any(char::is_control) {
                    self.show_failure(cx,Target::AgentCreated,"invalid_agent_name",crate::i18n::tr(user_error("invalid_agent_name")));return;
                }
                json!({"displayName":name,"visibility":if self.view.drop_down(cx, ids!(new_agent_visibility)).selected_item() == 1 {"public"} else {"private"}})
            };
            if self
                .agent_command
                .as_ref()
                .is_none_or(|(original, _)| original != &input)
            {
                self.agent_command = Some((input.clone(), uuid::Uuid::new_v4().to_string()));
            }
            input["idempotencyKey"] = self.agent_command.as_ref().unwrap().1.clone().into();
            if bind {
                if let Some(agent_id) = self.agent(cx) {
                    self.run(
                        cx,
                        Target::AgentBound,
                        Command::BindAgent { agent_id, input },
                    );
                }
            } else {
                self.run(cx, Target::AgentCreated, Command::CreateAgent { input });
            }
            return;
        }
        if self
            .view
            .drop_down(cx, ids!(agents))
            .selected(actions)
            .is_some()
        {
            if let Some(index) = self.view.drop_down(cx,ids!(agents)).selected_item().checked_sub(1) {self.agent_detail(cx,index);}
            return;
        }
        for (id, action) in [
            (ids!(pause_agent), AgentAction::Pause),
            (ids!(resume_agent), AgentAction::Resume),
        ] {
            if self.view.button(cx, id).clicked(actions) {
                if let Some(agent_id) = self.agent(cx) {
                    self.run(cx, Target::Changed, Command::Agent { agent_id, action });
                }
                return;
            }
        }
        if self.view.button(cx, ids!(retire_agent)).clicked(actions) {
            if !self.view.check_box(cx, ids!(retire_confirm)).active(cx) {
                self.status(cx, "Confirm permanent retirement first. The Agent cannot transfer to another owner.");
                return;
            }
            if let Some(agent_id) = self.agent(cx) {
                self.run(
                    cx,
                    Target::Changed,
                    Command::Agent {
                        agent_id,
                        action: AgentAction::Retire,
                    },
                );
            }
            return;
        }
        if self.view.button(cx,ids!(open_binding_chat)).clicked(actions) {
            if let Some(binding)=self.selected(cx,ids!(bindings),&self.bindings)
                && string(&binding,"state") == "active"
                && Service::for_current().is_ok_and(|s|s.identity()==self.identity)
                && let Ok(id)=matrix_sdk::ruma::OwnedRoomId::try_from(string(&binding,"roomId")) {
                navigate_agent_chat(cx, id, string(&binding,"scopeKind") == "owner_direct", Some((&self.identity, string(&binding,"projectId"))));
            }
            return;
        }
        if self
            .view
            .button(cx, ids!(room_roster_check))
            .clicked(actions)
        {
            if let Some(binding) = self.selected(cx, ids!(bindings), &self.bindings) {
                if string(&binding,"scopeKind") == "owner_direct" || string(&binding,"projectId").is_empty() {
                    self.status(cx,"Open the private chat information to view its members.");
                    return;
                }
                self.run(
                    cx,
                    Target::RoomRoster,
                    Command::RoomRoster {
                        project_id: string(&binding, "projectId").into(),
                        room_id: string(&binding, "roomId").into(),
                    },
                );
            }
            return;
        }
        let budget_room_changed = self.view.drop_down(cx, ids!(policy_binding)).selected(actions).is_some();
        if self.view.button(cx, ids!(load_policy)).clicked(actions)
            || budget_room_changed
            || self
                .view
                .drop_down(cx, ids!(bindings))
                .selected(actions)
                .is_some()
        {
            self.wanted_approval=None;
            let index = self.view.drop_down(cx, if budget_room_changed { ids!(policy_binding) } else { ids!(bindings) }).selected_item();
            self.select_policy_room(cx, index);
            for id in [
                ids!(leave_confirm),
                ids!(takeover),
                ids!(host_files),
            ] {
                self.view
                    .check_box(cx, id)
                    .set_active(cx, false, Animate::No);
            }
            self.view
                .label(cx, ids!(runtime_status))
                .set_text(cx, crate::i18n::tr("Refresh this Room's runtime status."));
            self.view.label(cx, ids!(room_roster)).set_text(cx, "");
            if let Some(binding) = self.selected(cx, ids!(bindings), &self.bindings) {
                let room_label = self.binding_label(cx, &binding);
                self.view.label(cx, ids!(binding_details)).set_text(
                    cx,
                    &format!(
                        "{} · {}",
                        room_label,
                        string(&binding, "state")
                    ),
                );
            }
            self.clear_approval_state(cx);
        self.approvals.clear();
            self.labels(
                cx,
                ids!(approvals),
                "Select a pending approval",
                &[],
                "proposalId",
            );
            self.view.label(cx, ids!(approval_details)).set_text(cx, "");
            let global = self.view.view(cx, ids!(policy_section)).visible()
                && self.view.drop_down(cx, ids!(policy_layer)).selected_item() == 0;
            self.local(
                cx,
                Target::Policy,
                if global {
                    LocalAction::AgentPolicy
                } else {
                    LocalAction::Policy
                },
                None,
            );
            return;
        }
        if self
            .view
            .drop_down(cx, ids!(policy_layer))
            .selected(actions)
            .is_some()
        {
            self.policy = Value::Null;
            let action = if self.view.drop_down(cx, ids!(policy_layer)).selected_item() == 0 {
                LocalAction::AgentPolicy
            } else {
                LocalAction::Policy
            };
            self.local(cx, Target::Policy, action, None);
            return;
        }
        if self.view.button(cx, ids!(reset_policy)).clicked(actions) {
            let Some(agent) = self.agent(cx) else {
                return;
            };
            let binding = self.binding(cx);
            let requester = self.input(cx, ids!(requester));
            let index = self
                .view
                .drop_down(cx, ids!(policy_layer))
                .selected_item()
                .min(2);
            if !editor_matches(&self.policy, &agent, binding.as_deref(), &requester, index) {
                self.status(cx, "Load the exact policy before resetting.");
                return;
            }
            if let Some(revision) = self.policy["policies"][index]["revision"].as_i64() {
                if index == 0 {
                    self.local(
                        cx,
                        Target::Policy,
                        LocalAction::ResetAgentPolicy,
                        Some(json!({"expectedRevision":revision})),
                    );
                } else if let Some(binding) = binding {
                    self.local(cx, Target::Policy, LocalAction::ResetPolicy, Some(json!({"bindingId":binding,"requester":requester,"layer":(["agent","room","requester"][index]),"expectedRevision":revision})));
                }
            }
            return;
        }
        if self.view.button(cx, ids!(reply_policy_toggle)).clicked(actions) {
            if let (Some(agent_id), Some(binding)) = (self.agent(cx), self.selected(cx, ids!(bindings), &self.bindings)) {
                if string(&binding, "scopeKind") == "project" && matches!(string(&binding, "state"), "active" | "suspended") {
                    self.run(cx, Target::ReplyPolicyChanged, Command::Binding {
                        agent_id, binding_id: string(&binding, "id").into(),
                        action: BindingAction::SetThreadAutoReply(binding["threadAutoReply"] != true),
                    });
                }
            }
            return;
        }
        for (button, action) in [
            (ids!(pause_binding), BindingAction::Pause),
            (ids!(resume_binding), BindingAction::Resume),
            (ids!(leave_binding), BindingAction::Leave),
        ] {
            if self.view.button(cx, button).clicked(actions) {
                if matches!(action, BindingAction::Leave)
                    && !self.view.check_box(cx, ids!(leave_confirm)).active(cx)
                {
                    self.status(cx, "Confirm leaving the selected Room first. Other Room bindings remain separate.");
                    return;
                }
                if let (Some(agent_id), Some(binding_id)) = (self.agent(cx), self.binding(cx)) {
                    self.run(
                        cx,
                        Target::BindingChanged,
                        Command::Binding {
                            agent_id,
                            binding_id,
                            action,
                        },
                    );
                } else {
                    self.status(cx, "Select a Room binding first.");
                }
                return;
            }
        }
        if self.view.button(cx, ids!(save_policy)).clicked(actions) {
            self.save_policy(cx);
            return;
        }
        for (id, action) in [
            (ids!(provider_check), ProviderAction::Status),
            (ids!(use_local_provider), ProviderAction::UseLocal),
            (ids!(disconnect_provider), ProviderAction::Disconnect),
            (ids!(provider_login), ProviderAction::Login),
            (ids!(provider_cancel), ProviderAction::Cancel),
            (ids!(provider_logout), ProviderAction::Logout),
        ] {
            if self.view.button(cx, id).clicked(actions) {
                self.provider_poll_count=0;
                self.run(cx, Target::Provider, Command::Provider { action });
                return;
            }
        }
        if self.view.button(cx, ids!(provider_open)).clicked(actions) {
            if let Some(url) = &self.provider_url {
                let _ = robius_open::Uri::new(url).open();
            }
            return;
        }
        if self.view.button(cx,ids!(refresh_models)).clicked(actions) {
            self.run(cx,Target::ProviderModels,Command::Provider {action:ProviderAction::Models});return;
        }
        if self.view.button(cx,ids!(prepare_workspace)).clicked(actions) {
            if let Some(agent_id)=self.agent(cx) {self.run(cx,Target::WorkspacePrepared,Command::PrepareAgentWorkspace {agent_id});}return;
        }
        if self.view.button(cx, ids!(room_advanced_toggle)).clicked(actions) {
            self.room_advanced = !self.room_advanced;
            self.render_room_advanced(cx);
            return;
        }
        if self.view.button(cx,ids!(service_logs_toggle)).clicked(actions) {
            self.service_logs_open = !self.service_logs_open;
            self.view.view(cx,ids!(service_logs_panel)).set_visible(cx,self.service_logs_open);
            self.view.label(cx,ids!(service_log_text)).set_text(cx,&service_log_text(&self.agent_service_state));
            return;
        }
        if self.view.button(cx,ids!(copy_service_logs)).clicked(actions) {
            cx.copy_to_clipboard(&service_log_text(&self.agent_service_state));return;
        }
        if self.view.button(cx,ids!(service_advanced_toggle)).clicked(actions) {
            self.service_advanced = !self.service_advanced;self.render_service_advanced(cx);return;
        }
        if self.view.button(cx,ids!(advanced_toggle)).clicked(actions) {
            self.model_advanced = !self.model_advanced;self.render_model_advanced(cx);return;
        }
        if self.view.drop_down(cx,ids!(runtime_kind)).selected(actions).is_some() {
            if self.busy || self.runtime_room_active {return;}
            self.clear_model_save_feedback(cx);
            cx.stop_timer(self.provider_timer);self.provider_timer=Timer::default();self.provider_poll_count=0;
            self.provider_state=Value::Null;self.provider_url=None;
            self.provider_models.clear();self.runtime_efforts.clear();self.credential_ref.clear();self.provider_authenticated=false;self.runtime_resources_checked=false;
            self.render_models(cx);
            self.run(cx,Target::ProviderModels,Command::Provider {action:ProviderAction::Models});return;
        }
        if self.view.text_input(cx,ids!(workspace)).changed(actions).is_some() || self.view.check_box(cx,ids!(web_search)).changed(actions).is_some() || self.view.check_box(cx,ids!(programming)).changed(actions).is_some() || self.view.text_input(cx,ids!(mcp_servers)).changed(actions).is_some() {self.clear_model_save_feedback(cx);self.render_mcp(cx);}
        if !self.busy && self.handle_mcp(cx,actions) {self.sync_navigation(cx);return;}
        if self.view.drop_down(cx,ids!(model)).selected(actions).is_some() {self.clear_model_save_feedback(cx);self.render_efforts(cx);return;}
        if self.view.drop_down(cx,ids!(effort)).selected(actions).is_some() {self.clear_model_save_feedback(cx);self.render_effort_note(cx);return;}
        if self.view.button(cx, ids!(save_model)).clicked(actions) {
            if self.model_saving {return;}
            if self.view.view(cx,ids!(mcp_form)).visible() {self.model_save_validation_failed(cx,"Apply or cancel the server edit before saving.");return;}
            if self.credential_ref.is_empty() {
                self.model_save_validation_failed(cx, "Check your connected local account first.");
                return;
            }
            let Some(agent) = self.agent(cx) else {
                return;
            };
            if string(&self.policy, "agentId") != agent
                || self.policy["scope"] != "agent"
                || self.policy["modelLoaded"] != true
            {
                self.model_save_validation_failed(cx, "Load this Agent's local resources before saving.");
                return;
            }
            let Some(model)=self.selected(cx,ids!(model),&self.provider_models) else {self.model_save_validation_failed(cx,"Select a model or profile from this device's catalog.");return;};
            if !self.provider_authenticated || self.credential_ref.is_empty() {self.model_save_validation_failed(cx,"Connect your local account first.");return;}
            let Some(effort)=self.runtime_efforts.get(self.view.drop_down(cx,ids!(effort)).selected_item()).cloned() else {self.model_save_validation_failed(cx,"Select a supported reasoning effort.");return;};
            let input = self.model_save_input(cx,&model,&effort);
            if self.local_agent(cx) && self.provider_runtime(cx)=="codex" && self.view.check_box(cx,ids!(programming)).active(cx) && !self.validate_mcp(cx) {
                self.model_save_validation_failed(cx,"Fix the MCP configuration before saving.");return;
            }
            if self.local_agent(cx) && !input["profile"]["programming"]["mcp_servers"].is_object() {self.model_save_validation_failed(cx,"MCP servers must be a JSON object.");return;}
            self.local(cx, Target::ModelSaved, LocalAction::SaveAgentModel, Some(input));
            return;
        }
        if self.view.button(cx,ids!(ensure_owner_direct)).clicked(actions) {
            if let Some(agent_id)=self.agent(cx) {
                self.cancel_direct_retry(cx);
                self.direct_agent=agent_id.clone();self.direct_state=json!({"state":"pending"});
                self.run(cx,Target::OwnerDirect,Command::EnsureAgentOwnerDirect {agent_id});
            }
            return;
        }
        if self.view.button(cx,ids!(open_owner_direct)).clicked(actions) {
            if self.agent(cx).as_deref()==Some(self.direct_agent.as_str()) && self.direct_state["state"] == "active" && Service::for_current().is_ok_and(|s|s.identity()==self.identity)
                && let Ok(id)=matrix_sdk::ruma::OwnedRoomId::try_from(string(&self.direct_state,"roomId")) {
                navigate_agent_chat(cx, id, true, None);
            }
            return;
        }
        if self.view.button(cx, ids!(assign_current_device)).clicked(actions) {
            let Some(agent_id) = self.agent(cx) else {return;};
            if !self.devices_loaded || string(&self.execution_agent_record,"id") != agent_id || !self.view.check_box(cx, ids!(assignment_confirm)).active(cx) {return;}
            let Some(expected_generation) = self.execution_agent_record["generation"].as_i64() else {return;};
            self.run(cx, Target::AgentAssigned, Command::AssignAgentToCurrentDevice {agent_id, expected_generation});
            return;
        }
        if self.view.button(cx, ids!(service_toggle)).clicked(actions) {
            if self.view.button(cx,ids!(service_toggle)).borrow().is_none_or(|button|!button.enabled()) {return;}
            if self.runtime_room_active {
                self.local(cx,Target::Runtime,LocalAction::StopAgentService,None);
                return;
            }
            if !self.local_agent(cx) {
                self.runtime_feedback(cx,"Assign this Agent to this device before starting it here."); return;
            }
            let Ok(reservation) = self.input(cx, ids!(reservation)).parse::<u64>() else {
                self.runtime_feedback(cx, "Enter a positive token reservation.");
                return;
            };
            if reservation == 0 {
                self.runtime_feedback(cx,"Enter a positive token reservation.");
                return;
            }
            let selected_model=self.selected(cx,ids!(model),&self.provider_models);
            if selected_model.as_ref().is_none_or(|m|string(m,"model") != string(&self.policy["modelProfile"],"model")) || string(&self.policy["modelProfile"],"model").is_empty() {
                self.runtime_feedback(cx,"Save the selected model or profile before starting this Room.");return;
            }
            let effort=self.runtime_efforts.get(self.view.drop_down(cx,ids!(effort)).selected_item()).cloned();
            let Some(effort)=effort else {self.runtime_feedback(cx,"Load local resources and select a supported reasoning effort before starting.");return;};
            let saved_effort=string(&self.policy["modelProfile"],"reasoning_effort");
            if !saved_effort.is_empty() && saved_effort!=effort {self.runtime_feedback(cx,"Save the selected reasoning effort in model settings before starting.");return;}
            let input = json!({"reservation":reservation,"takeover":self.view.check_box(cx,ids!(takeover)).active(cx),"effort":effort});
            self.local(cx, Target::Runtime, LocalAction::StartAgentService, Some(input));
            return;
        }
        if self.handle_crypto(cx,actions) {return;}
        if self.view.button(cx, ids!(runtime_check)).clicked(actions) {
            self.load_runtime_resources(cx);
            return;
        }
        if self.view.button(cx, ids!(approvals_check)).clicked(actions) {
            self.refresh_approvals(cx);
            return;
        }
        if self.view.drop_down(cx,ids!(remote_review_room)).selected(actions).is_some() {
            self.remote_review_requests.clear();self.remote_review_next=None;self.remote_review_cursors.clear();self.render_remote_reviews(cx);self.sync_navigation(cx);return;
        }
        if self.view.drop_down(cx,ids!(remote_review_requests)).selected(actions).is_some() {self.render_remote_reviews(cx);self.sync_navigation(cx);return;}
        if self.view.button(cx,ids!(remote_review_refresh)).clicked(actions) || self.view.button(cx,ids!(remote_review_older)).clicked(actions) {
            self.load_remote_reviews(cx,self.view.button(cx,ids!(remote_review_older)).clicked(actions));return;
        }
        if self.view.button(cx,ids!(remote_review_copy)).clicked(actions) {
            if let Some(row)=self.selected(cx,ids!(remote_review_requests),&self.remote_review_requests) {cx.copy_to_clipboard(&serde_json::to_string_pretty(&row["request"]).unwrap_or_default());}return;
        }
        if self.view.button(cx,ids!(remote_review_allow)).clicked(actions) || self.view.button(cx,ids!(remote_review_deny)).clicked(actions) || self.view.button(cx,ids!(remote_review_receipt)).clicked(actions) {
            if self.busy {return;}
            let Some(row)=self.selected(cx,ids!(remote_review_requests),&self.remote_review_requests) else {return;};
            let Some(agent)=self.agent(cx) else {return;};
            if row["request"]["proposal"]["scope"]["agent"]!=agent {return;}
            let id=string(&row["request"],"approvalId").to_owned();
            let action=if self.view.button(cx,ids!(remote_review_receipt)).clicked(actions) {RemoteApprovalAction::Status {approval_id:id}} else {
                if !row["publication"].is_null() || row["request"]["proposal"]["expires"].as_i64().is_none_or(|t|t<=super::approvals::now()) || !self.selected(cx,ids!(remote_review_room),&self.remote_review_rooms).is_some_and(|r|r["id"]==row["request"]["approvalRoom"]) {return;}
                let approved=self.view.button(cx,ids!(remote_review_allow)).clicked(actions);
                for r in &mut self.remote_review_requests {if r["request"]["approvalId"]==id {r["publication"]=json!({"phase":"unknown","approved":approved});}}
                self.render_remote_reviews(cx);
                RemoteApprovalAction::Decide {agent,room:string(&row["request"],"approvalRoom").into(),event_id:string(&row,"requestEventId").into(),request_digest:string(&row,"requestDigest").into(),approved}
            };
            self.run(cx,Target::RemoteVerdict,Command::RemoteApprovals {action});return;
        }
        if self
            .view
            .drop_down(cx, ids!(approvals))
            .selected(actions)
            .is_some()
        {
            self.render_approval_details(cx);self.sync_navigation(cx);
            return;
        }
        if self.view.button(cx,ids!(remote_approval_publish)).clicked(actions) || self.view.button(cx,ids!(remote_approval_refresh)).clicked(actions) || self.view.button(cx,ids!(remote_approval_observe)).clicked(actions) {
            let Some(p)=self.selected(cx,ids!(approvals),&self.approvals) else {return;};
            if self.busy || !self.local_agent(cx) || !self.approval_checked || self.approval_blocked.contains(string(&p,"proposalId"))
                || !self.approval_scope(cx).is_some_and(|(a,b,r)|super::approvals::matches(&p,&a,&b,&r,super::approvals::now())) {return;}
            let id=string(&p,"proposalId").to_owned();
            if self.view.button(cx,ids!(remote_approval_publish)).clicked(actions) {
                let Some(room)=self.remote_room(cx) else {return;};
                self.local(cx,Target::RemoteApproval,LocalAction::PublishRemoteApproval {approval_id:id},Some(json!({"argsDigest":p["argsDigest"],"approvalRoom":room})));
            } else if self.view.button(cx,ids!(remote_approval_observe)).clicked(actions) {
                self.local(cx,Target::RemoteApprovalDecision,LocalAction::ObserveRemoteApproval {approval_id:id},Some(json!({"argsDigest":p["argsDigest"]})));
            } else {
                self.local(cx,Target::RemoteApproval,LocalAction::RemoteApprovalStatus {approval_id:id},None);
            }
            return;
        }
        if self.view.button(cx, ids!(approve)).clicked(actions)
            || self.view.button(cx, ids!(reject)).clicked(actions)
        {
            if let Some(p) = self.selected(cx, ids!(approvals), &self.approvals) {
                if self.busy || !self.local_agent(cx) || !self.approval_checked || self.approval_blocked.contains(string(&p,"proposalId"))
                    || !self.approval_scope(cx).is_some_and(|(a,b,r)|super::approvals::matches(&p,&a,&b,&r,super::approvals::now())) {return;}
                let approved = self.view.button(cx, ids!(approve)).clicked(actions);
                self.approval_blocked.insert(string(&p,"proposalId").into());
                self.view.label(cx,ids!(approval_feedback)).set_text(cx,crate::i18n::tr("Submitting decision…"));
                self.local(
                    cx,
                    Target::ApprovalDecision,
                    LocalAction::Decision {
                        approval_id: string(&p, "proposalId").into(),
                    },
                    Some(json!({"argsDigest":p["argsDigest"],"approved":approved})),
                );
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let stacked=cx.turtle().rect().size.x<760.0;
        if self.header_stacked!=Some(stacked) {
            self.header_stacked=Some(stacked);
            let header=self.view.view(cx,ids!(panel_header));
            let tabs=self.view.view(cx,ids!(project_tabs));
            if let Some(mut header)=header.borrow_mut() {header.layout.flow=if stacked {Flow::Down}else{Flow::right()};header.layout.align=Align{x:0.0,y:0.5};}
            if let Some(mut tabs)=tabs.borrow_mut() {tabs.walk.width=if stacked {Size::fill()}else{Size::fit()};}
        }
        if self.page==Page::Projects && self.view.view(cx,ids!(board_section)).visible() {
            let loading=self.busy || self.view.project_board(cx,ids!(board)).is_loading();
            let spinner=self.view.widget(cx,ids!(loading_status));
            if spinner.visible()!=loading {spinner.set_visible(cx,loading);}
        }
        // Navigation styles also touch hidden Service controls. Applying their
        // scripts on every Draw dirties the widget tree and schedules the next
        // frame forever, even when the current page is an unchanged board.
        if !self.navigation_ready.replace(true) { self.sync_navigation(cx); }
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if self.draw_mcp(cx,scope,&step) {continue;}
            let is_members =
                step.widget_uid() == self.view.portal_list(cx, ids!(members_list)).widget_uid();
            if step.widget_uid()==self.view.flat_list(cx,ids!(overview_boards)).widget_uid() {
                let Some(mut list)=step.borrow_mut::<FlatList>() else {continue;};
                for board in &self.board_catalog {
                    let Some(row)=list.item(cx,LiveId::from_str(string(board,"id")),id!(Board)) else {continue;};
                    row.view(cx,ids!(default_badge)).set_visible(cx,board["isDefault"]==true);
                    let name=string(board,"name");row.button(cx,ids!(open_board)).set_text(cx,if name=="Main Board" {crate::i18n::tr(name)}else{name});
                    row.draw_all(cx,scope);
                }
                continue;
            }
            if step.widget_uid()==self.view.flat_list(cx,ids!(overview_activity_list)).widget_uid() {
                let Some(mut list)=step.borrow_mut::<FlatList>() else {continue;};
                for (index,activity) in array(&self.project_home,"activity").iter().enumerate() {
                    let Some(row)=list.item(cx,LiveId(index as u64),id!(Activity)) else {continue;};
                    let kind=crate::i18n::tr(match string(activity,"kind") {"created"=>"Created","edited"=>"Updated","moved"=>"Moved","commented"=>"Commented","columns_updated"=>"Columns updated","archived"=>"Archived","restored"=>"Restored",_=>"Updated"});
                    row.label(cx,ids!(activity_title)).set_text(cx,&format!("{kind} · {}",string(activity,"title")));
                    let actor=string(activity,"actor");let actor=self.project_home["memberNames"][actor].as_str().filter(|n|!n.is_empty()).unwrap_or(actor);
                    row.label(cx,ids!(activity_context)).set_text(cx,&format!("{} · {actor}",string(activity,"board")));
                    let time=activity["occurredAt"].as_i64().and_then(chrono::DateTime::from_timestamp_millis).map(|date|date.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string()).unwrap_or_default();
                    row.label(cx,ids!(activity_time)).set_text(cx,&time);row.draw_all(cx,scope);
                }
                continue;
            }
            if step.widget_uid()==self.view.flat_list(cx,ids!(agent_request_list)).widget_uid() {
                let Some(mut list)=step.borrow_mut::<FlatList>() else {continue;};
                for command in &self.agent_pending_commands {
                    let Some(row)=list.item(cx,LiveId::from_str(string(command,"id")),id!(Request)) else {continue;};
                    self.agent_request_targets.insert(row.widget_uid(),string(command,"id").into());
                    let rejected=string(command,"state")=="rejected";
                    let code=string(command,"errorCode");
                    let details=if code.len()<=80 && code.bytes().all(|b|b.is_ascii_lowercase() || b.is_ascii_digit() || b==b'_') {code} else {""};
                    let title=format!("{} · {}",agent_command_label(command),crate::i18n::tr(if rejected {"Failed"} else if string(command,"state")=="pending" {"Creating…"} else {"Check result"}));
                    row.inline_notice(cx,ids!(request_notice)).show(cx,if rejected {PopupKind::Error} else {PopupKind::Warning},&title,&agent_command_details(command),details,false);
                    let mut check=row.button(cx,ids!(check_request));
                    check.set_visible(cx,!rejected);check.set_enabled(cx,!self.busy);
                    crate::shared::styles::apply_primary_button_style(cx,&mut check);
                    let mut retry=row.button(cx,ids!(retry_request));
                    if rejected {crate::shared::styles::apply_primary_button_style(cx,&mut retry);}
                    else {crate::shared::styles::apply_neutral_button_style(cx,&mut retry);}
                    row.button(cx,ids!(retry_request)).set_visible(cx,string(command,"state")!="pending");
                    row.button(cx,ids!(retry_request)).set_enabled(cx,!self.busy);
                    row.button(cx,ids!(dismiss_request)).set_visible(cx,rejected);
                    row.button(cx,ids!(dismiss_request)).set_enabled(cx,!self.busy);
                    row.draw_all(cx,scope);
                }
                list.items.retain_visible();
                continue;
            }
            let Some(mut list) = step.borrow_mut::<PortalList>() else {
                continue;
            };
            if step.widget_uid() == self.view.portal_list(cx,ids!(service_rooms)).widget_uid() {
                let rooms=array(&self.agent_service_state["service"],"rooms");
                list.set_item_range(cx,0,rooms.len().max(1));
                while let Some(index)=list.next_visible_item(cx) {
                    let Some(binding)=rooms.get(index) else {list.item(cx,index,if index==0 && rooms.is_empty() {id!(Empty)} else {id!(Filler)}).draw_all(cx,scope);continue;};
                    let row=list.item(cx,index,id!(Room));
                    self.service_row_targets.insert(row.widget_uid(),(self.agent(cx).unwrap_or_default(),string(binding,"id").into()));
                    let name=self.binding_label(cx,binding);
                    row.label(cx,ids!(room_name)).set_text(cx,&name);
                    let paused=binding["ownerServicePaused"] == true;
                    let status=if paused {"Paused · requests receive a pause notice"} else if binding["state"] == "pending" {"Joining…"} else if binding["state"] != "active" {"Unavailable · check membership and permissions"} else if !self.runtime_room_active {"Service stopped on this device"} else if array(&self.agent_service_state["service"],"problems").iter().any(|p|string(p,"bindingId")==string(binding,"id")) {"Waiting to retry · check service status"} else {"Enabled · limits and permissions apply"};
                    row.label(cx,ids!(room_status)).set_text(cx,crate::i18n::tr(status));
                    row.button(cx,ids!(room_toggle)).set_text(cx,crate::i18n::tr(if paused {"Resume"} else {"Pause"}));
                    row.button(cx,ids!(room_toggle)).set_enabled(cx,!self.busy && (paused || binding["state"] == "active"));
                    let mut button=row.button(cx,ids!(room_toggle));
                    if paused {script_apply_eval!(cx,button,{draw_icon +: {svg: mod.widgets.ICON_SERVICE_PLAY}});} else {script_apply_eval!(cx,button,{draw_icon +: {svg: mod.widgets.ICON_SERVICE_PAUSE}});}
                    row.draw_all(cx,scope);
                }
                continue;
            }
            if is_members {
                let total = self.members.len();
                if list.first_id() >= total {
                    list.set_first_id_and_scroll(0, 0.0);
                }
                list.set_item_range(cx, 0, total);
                while let Some(index) = list.next_visible_item(cx) {
                    if let Some(member) = self.members.get(index) {
                        let row = list.item(cx, index, id!(Member));
                        row.label(cx, ids!(name)).set_text(cx,if member["service"]==true {crate::i18n::tr("Hagency service")} else {string(member,"name")});
                        row.label(cx, ids!(identity))
                            .set_text(cx, string(member, "userId"));
                        row.label(cx, ids!(membership)).set_text(cx, &member_summary(member));
                        let nested=member["agentOwner"].as_str().is_some_and(|owner|self.members.iter().any(|m|m["userId"]==owner));
                        row.view(cx,ids!(indent)).set_visible(cx,nested);
                        row.view(cx,ids!(agent_badge)).set_visible(cx,member["agentOwner"].is_string());
                        let role=match string(member,"role") {"Creator"=>"Creator","Administrator"=>"Administrator","Moderator"=>"Moderator",_=>"Member"};
                        let invited=member["membership"]=="invite";
                        let role=if member["service"]==true {"Service"}else{role};
                        row.view(cx,ids!(role_badge)).set_visible(cx,invited || role!="Member");
                        let badge=if invited {format!("{} · {}",crate::i18n::tr("Invited"),crate::i18n::tr(role))} else {crate::i18n::tr(role).into()};
                        row.label(cx,ids!(role_badge.device_label)).set_text(cx,&badge);
                        if let Ok(user)=matrix_sdk::ruma::UserId::parse(string(member,"userId")) {
                            use crate::shared::avatar::AvatarWidgetRefExt;
                            row.avatar(cx,ids!(avatar)).show_user_text(cx,&user,string(member,"name"));
                        }
                        row.draw_all(cx, scope);
                    } else {
                        list.item(cx, index, id!(Filler)).draw_all(cx, scope);
                    }
                }
                continue;
            }
            let rows = if self.page == Page::Projects {
                &self.projects
            } else {
                &self.agents
            };
            let total = rows.len().max(1);
            if list.first_id() >= total {
                list.set_first_id_and_scroll(0, 0.0);
            }
            list.set_item_range(cx, 0, total);
            while let Some(index) = list.next_visible_item(cx) {
                let Some(value) = rows.get(index) else {
                    let row = list.item(cx, index, card_template(rows.len(), index));
                    if rows.is_empty() && index == 0 {
                        row.label(cx, ids!(empty_message)).set_text(
                            cx,
                            if self.page == Page::Projects {
                                project_empty_message(self.projects_loaded,self.busy)
                            } else {
                                agent_empty_message(self.agents_loaded, self.agents_loading)
                            },
                        );
                    }
                    row.draw_all(cx, scope);
                    continue;
                };
                let row = list.item(cx, index, id!(Card));
                let title = if self.page == Page::Projects {
                    string(value, "name")
                } else {
                    string(value, "displayName")
                };
                row.label(cx, ids!(title))
                    .set_text(cx, if title.is_empty() { "Untitled" } else { title });
                let mut summary = if self.page == Page::Projects {
                    string(value,"topic").trim().to_owned()
                } else {
                    let identity_state=crate::i18n::tr(if string(value,"state")=="active" {"Server identity active"} else {"Server identity pending"});
                    let local=match agent_device_state(value,&self.current_device_id,&self.execution_devices,self.devices_loaded) {
                        AgentDeviceState::Here=>runtime_service_description(self.runtime_list.get(string(value,"id")),string(value,"id")),
                        AgentDeviceState::Other=>format!("{}\n{}",crate::i18n::tr("Processing is assigned to another device; this device does not run it."),crate::i18n::tr(remote_presence_label(self.runtime_list.get(string(value,"id")),value))),
                        AgentDeviceState::Checking=>crate::i18n::tr("Checking execution device…").to_owned(),
                        _=>crate::i18n::tr("No available execution device. Open Agent settings to assign this device.").to_owned(),
                    };
                    format!("{identity_state} · {}\n{local}", string(value,"puppetMxid"))
                };
                row.view(cx,ids!(badge_row)).set_visible(cx,self.page==Page::Agents || value["archived"]==true);
                let audience = row.view(cx, ids!(audience_badge));
                audience.set_visible(cx, self.page == Page::Agents || value["archived"]==true);
                audience.label(cx, ids!(device_label)).set_text(cx, crate::i18n::tr(if self.page==Page::Agents {audience_label(value)} else {"Archived"}));
                row.view(cx, ids!(device_badge)).set_visible(cx, self.page == Page::Agents);
                if self.page == Page::Agents {
                    let badge = row.widget(cx, ids!(device_badge));
                    let state=agent_device_state(value, &self.current_device_id, &self.execution_devices, self.devices_loaded);
                    render_device_badge(cx, &badge, state, &self.device_badge(value));
                    let runtime=row.button(cx,ids!(runtime_badge));runtime.set_visible(cx,state==AgentDeviceState::Here);
                    let label=runtime_list_label(self.runtime_list.get(string(value,"id")),string(value,"id"));
                    let badge=if label.starts_with("Not running") {"Not running"} else if label.starts_with("Running") || label.starts_with("Processing") {"Running"} else if label.starts_with("Service enabled") {"Enabled"} else if label.starts_with("Starting") {"Starting…"} else if label.starts_with("Checking") {"Checking…"} else if label=="Waiting for the previous runtime lease to be released…" {"Waiting for lease"} else {"Needs attention"};
                    runtime.set_text(cx,crate::i18n::tr(badge));
                }
                self.agent_card_targets.insert(row.widget_uid(),string(value,"id").into());
                let control=row.button(cx,ids!(service_control));
                let here=self.page==Page::Agents && agent_device_state(value,&self.current_device_id,&self.execution_devices,self.devices_loaded)==AgentDeviceState::Here;
                control.set_visible(cx,here);
                let status=self.runtime_list.get(string(value,"id")).and_then(|r|r.as_ref().ok());
                let enabled=status.is_some_and(service_enabled);
                control.set_enabled(cx,here && status.is_some() && !self.busy && (enabled || string(value,"state")=="active"));
                control.set_text(cx,crate::i18n::tr(if self.busy && self.list_service_pending_agent==string(value,"id") {"Working…"} else if enabled {"Stop"} else if status.is_some_and(service_needs_model) {"Configure"} else {"Start"}));
                let mut control=control;
                if enabled {script_apply_eval!(cx,control,{draw_icon +: {svg: mod.widgets.ICON_SERVICE_STOP}});} else if status.is_some_and(service_needs_model) {script_apply_eval!(cx,control,{draw_icon +: {svg: mod.widgets.ICON_SETTINGS}});} else {script_apply_eval!(cx,control,{draw_icon +: {svg: mod.widgets.ICON_SERVICE_PLAY}});}
                if self.page==Page::Agents && here && let Some(error)=self.runtime_list_errors.get(string(value,"id")) {summary.push_str(&format!("\n{error}"));}
                row.label(cx, ids!(summary)).set_text(cx, &summary);
                row.label(cx, ids!(summary))
                    .set_visible(cx, !summary.is_empty());
                row.draw_all(cx, scope);
            }
        }
        DrawStep::done()
    }
}
impl HagencyPanelRef {
    pub fn action(&self, cx: &mut Cx, action: &HagencyAction) {
        let Some(mut panel) = self.borrow_mut() else {
            return;
        };
        if let HagencyAction::ReviewApproval(target)=action
            && !Service::existing_current().is_some_and(|service|service.identity()==target.identity) {return;}
        panel.cancel_direct_retry(cx);
        panel.cancel_read_wait(cx);
        if matches!(action,HagencyAction::AgentRun {..}|HagencyAction::ReviewApproval(..)|HagencyAction::NewAgent {..}|HagencyAction::NewProject) {panel.reset_detail_scroll(cx);}
        panel.wanted_run=None;
        panel.wanted_approval=None;
        panel.wanted_model=None;
        if matches!(action, HagencyAction::Close) {
            panel.active = false;
            panel.request = nonce();
            return;
        }
        let service = match Service::for_current() {
            Ok(s) => s,
            Err(error) => {
                panel.display_blocked=true;
                panel.status(cx, &error);
                return;
            }
        };
        panel.display_blocked=false;
        let identity = service.identity();
        if panel.identity != identity {
            panel.stopped_page=Value::Null;panel.recovery_review=Value::Null;panel.recovery_selected.clear();panel.recovery_cursor=None;panel.recovery_pending=None;
            panel.file_page=Value::Null;panel.file_detail=Value::Null;panel.file_selected.clear();panel.file_cursor=None;panel.history_page=Value::Null;panel.history_detail=Value::Null;panel.history_selected.clear();panel.history_cursor=None;
            panel.usage_report=Value::Null;panel.usage_agent.clear();panel.usage_cursor=None;
            panel.ledger_open=false;panel.ledger_preview=Value::Null;panel.ledger_file=None;
            panel.view.text_input(cx,ids!(ledger_path)).set_text(cx,"");
            panel.view.label(cx,ids!(ledger_summary)).set_text(cx,"");
            panel.view.check_box(cx,ids!(ledger_confirm)).set_active(cx,false,Animate::No);
            panel.model_saving=false;panel.clear_model_save_feedback(cx);
            panel.service_logs_open=false;panel.agent_service_state=Value::Null;
            panel.view.view(cx,ids!(service_logs_panel)).set_visible(cx,false);
            panel.view.label(cx,ids!(service_log_text)).set_text(cx,"");
            panel.clear_members(cx);
            panel.projects.clear();panel.all_projects.clear();panel.projects_loaded=false;panel.board_catalog.clear();panel.selected_board=None;panel.wanted_board=None;panel.wanted_access=false;panel.project_home=Value::Null;panel.room_can_manage=false;panel.board_creation=None;panel.show_archived_projects=false;
            panel.view.check_box(cx,ids!(show_archived_projects)).set_active(cx,false,Animate::No);
            panel.rooms.clear();
            panel.agent_rooms.clear();
            panel.spaces.clear();
            panel.agents.clear(); panel.runtime_list.clear();panel.runtime_list_errors.clear();panel.runtime_list_loading=false;panel.runtime_list_request=nonce();
            panel.devices_loaded = false; panel.current_device_id.clear(); panel.execution_devices.clear(); panel.execution_agent_record = Value::Null; panel.agent_crypto = Value::Null;panel.render_crypto(cx);
            panel.agents_loaded = false;
            panel.agents_loading = false;
            panel.bindings.clear();
            panel.clear_approval_state(cx);panel.clear_remote_reviews(cx);panel.approval_blocked.clear();
            panel.approvals.clear();
            panel.clear_configuration(cx);panel.configuration_keys.clear();
            panel.policy = Value::Null;
            panel.creation = None;
            panel.pending_input = None;
            panel.credential_ref.clear();
            panel.direct_agent.clear();panel.direct_state=Value::Null;panel.wanted_agent=None;
            panel.provider_url = None;
            panel.provider_models.clear();panel.runtime_efforts.clear();panel.provider_authenticated=false;panel.runtime_resources_loading=false;panel.runtime_resources_checked=false;
            panel.agent_command = None;
            for id in [
                ids!(creation_name),
                ids!(existing_room),
                ids!(found_room),
                ids!(agent_name),
                ids!(requester),
                ids!(token_limit),
                ids!(allowed_tools),
                ids!(allowed_directories),
                ids!(model),
                ids!(workspace),
            ] {
                panel.view.text_input(cx, id).set_text(cx, "");
            }
            for id in [
                ids!(credential),
                ids!(approval_details),
                ids!(creation_result),
                ids!(project_details),
                ids!(agent_details),
                ids!(room_details),
                ids!(runtime_status),
                ids!(provider_status),
                ids!(policy_usage),
            ] {
                panel.view.label(cx, id).set_text(cx, "");
            }
            for id in [ids!(takeover), ids!(host_files)] {
                panel
                    .view
                    .check_box(cx, id)
                    .set_active(cx, false, Animate::No);
            }
        }
        panel.wanted_project = None;
        panel.wanted_new_room = false;panel.wanted_open_rooms=false;panel.has_registered_rooms=None;panel.auto_default_project=None;
        panel.wanted_board=None;panel.wanted_access=false;
        panel.wanted_project_section = None;
        panel.list_page(cx);
        panel.identity = identity;
        panel.active = true;
        if matches!(action,HagencyAction::OpenProjects|HagencyAction::NewProject) {
            cx.action(crate::home::project_tree::ProjectSelectionChanged {identity:panel.identity.clone(),project_id:String::new(),target:crate::home::project_tree::ProjectSelection::Project});
        }
        panel.agent_journal_loaded = false;
        panel.agent_pending_commands.clear();
        panel.busy = false;
        panel.request = nonce();
        panel
            .view
            .label(cx, ids!(owner))
            .set_text(cx, crate::i18n::tr("Projects and Agents for your signed-in account."));
        panel
            .view
            .view(cx, ids!(project_form))
            .set_visible(cx, matches!(action, HagencyAction::NewProject));
        match action {
            HagencyAction::OpenProjects
            | HagencyAction::NewProject
            | HagencyAction::NewRoom {..}
            | HagencyAction::ProjectBoard { .. }
            | HagencyAction::ProjectDetails { .. }
            | HagencyAction::ProjectSection { .. } => {
                panel.set_page(cx, Page::Projects);
                if matches!(action, HagencyAction::NewProject) {
                    panel
                        .view
                        .view(cx, ids!(project_list_holder))
                        .set_visible(cx, false);
                    panel
                        .view
                        .view(cx, ids!(project_form))
                        .set_visible(cx, true);
                }
                panel.create_room = false;
                panel.view.drop_down(cx, ids!(creation_source)).set_labels(
                    cx,
                    vec![
                        crate::i18n::tr("Create a new Space (default)").into(),
                    ],
                );
                panel
                    .view
                    .label(cx, ids!(creation_heading))
                    .set_text(cx, crate::i18n::tr("Create Project"));
                panel
                    .view
                    .drop_down(cx, ids!(creation_source))
                    .set_selected_item(cx, 0);
                panel.creation_source(cx);
                if let HagencyAction::ProjectBoard {project_id,board_id}=action {
                    panel.wanted_project=Some(project_id.clone());panel.wanted_board=Some(board_id.clone());
                    panel.wanted_project_section=Some((project_id.clone(),ProjectDetailSection::Board));
                }
                if let HagencyAction::ProjectDetails { project_id } = action {
                    panel.wanted_project = Some(project_id.clone());
                }
                if let HagencyAction::NewRoom {project_id}=action {
                    panel.wanted_project=Some(project_id.clone());panel.wanted_new_room=true;
                }
                if let HagencyAction::ProjectSection {
                    project_id,
                    section,
                } = action
                {
                    panel.wanted_project = Some(project_id.clone());
                    panel.wanted_project_section = Some((project_id.clone(), *section));
                }
                panel.run(cx, Target::Journal, Command::Creations);
            }
            HagencyAction::OpenAgents | HagencyAction::NewAgent { .. } | HagencyAction::AgentRun {..} | HagencyAction::ReviewApproval(..) => {
                panel.set_page(cx, Page::Agents);
                if let HagencyAction::AgentRun {agent_id,binding_id}=action {panel.wanted_agent=Some(agent_id.clone()); panel.wanted_run=Some((agent_id.clone(),binding_id.clone()));}
                if let HagencyAction::ReviewApproval(target)=action {
                    panel.wanted_agent=Some(target.agent.clone());panel.wanted_run=Some((target.agent.clone(),target.binding.clone()));
                    panel.wanted_approval=Some(target.clone());
                }
                if matches!(action, HagencyAction::NewAgent { .. }) {
                    panel.begin_agent_creation(cx);
                }
                if let HagencyAction::NewAgent { project_id } = action {
                    panel.wanted_project = project_id.clone();
                }
                panel.run(cx, Target::AgentJournal, Command::AgentCommands);
            }
            HagencyAction::Close => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(feature = "agent_chat")]
    fn approval_chats_open_account_scope_only_from_the_agent_list() {
        let mut cx = Cx::new(Box::new(|_,_|{}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);
            let value = script_eval!(vm,{mod.widgets.HagencyPanel {}});
            HagencyPanel::script_from_value(vm,value)
        });
        panel.active = true;
        panel.page = Page::Agents;
        panel.sync_navigation(&mut cx);
        let button = panel.view.button(&cx,ids!(agent_approval_chats));
        let click = cx.capture_actions(|cx|cx.widget_action(button.widget_uid(),ButtonAction::Clicked(KeyModifiers::default())));
        let out = cx.capture_actions(|cx|assert!(panel.approval_chat_actions(cx,&click)));
        assert!(out.iter().any(|a|matches!(a.downcast_ref(),Some(crate::agent_chat::approval_inbox::ApprovalInboxAction::Open {project:None}))));
        panel.page = Page::Projects;
        assert!(!panel.approval_chat_actions(&mut cx,&click));
        panel.page = Page::Agents;
        panel.view.view(&cx,ids!(agent_list_holder)).set_visible(&mut cx,false);
        assert!(!panel.approval_chat_actions(&mut cx,&click));
        panel.view.view(&cx,ids!(agent_list_holder)).set_visible(&mut cx,true);
        panel.active = false;
        assert!(!panel.approval_chat_actions(&mut cx,&click));
    }

    #[test]
    fn private_audience_disables_shared_rooms() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm| {makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        assert_eq!(panel.view.drop_down(&cx,ids!(new_agent_visibility)).selected_item(),0);
        panel.agents=vec![json!({"id":"mine","visibility":"private","state":"active"})];
        panel.labels(&mut cx,ids!(agents),"Select my Agent",&panel.agents,"id");
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(another_binding)).borrow().unwrap().enabled());
        panel.apply(&mut cx,Target::VisibilityChanged,&json!({"agent":{"id":"mine","visibility":"public","state":"active"}}));
        panel.sync_navigation(&mut cx);
        assert!(panel.view.button(&cx,ids!(another_binding)).borrow().unwrap().enabled());
        panel.apply(&mut cx,Target::VisibilityChanged,&json!({"agent":{"id":"mine","visibility":"private","state":"active"}}));
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(another_binding)).borrow().unwrap().enabled());
        assert_eq!(audience_label(&json!({})),"Service audience unavailable. Update the server and refresh.");
        panel.agents[0].as_object_mut().unwrap().remove("visibility");
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.view(&cx,ids!(audience_editor)).visible());
        assert!(!panel.view.button(&cx,ids!(save_agent_visibility)).borrow().unwrap().enabled());
        panel.view.drop_down(&cx,ids!(new_agent_visibility)).set_selected_item(&mut cx,1);
        panel.begin_agent_creation(&mut cx);
        assert_eq!(panel.view.drop_down(&cx,ids!(new_agent_visibility)).selected_item(),0);
    }

    #[test]
    fn web_search_default_toggle_save_and_reload_use_the_agent_profile() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm| {makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.policy=json!({"modelLoaded":true,"modelProfile":{"model":"probe","credential_ref":"keychain:probe","workspace_root":"/work"}});
        panel.render_policy(&mut cx);
        assert!(panel.view.check_box(&cx,ids!(web_search)).active(&cx));
        panel.view.check_box(&cx,ids!(web_search)).set_active(&mut cx,false,Animate::No);
        let saved=panel.model_save_input(&mut cx,&json!({"model":"probe"}),"low");
        assert_eq!(saved["profile"]["web_search"],false);
        panel.view.check_box(&cx,ids!(web_search)).set_active(&mut cx,true,Animate::No);
        panel.policy["modelProfile"]=saved["profile"].clone();
        panel.render_policy(&mut cx);
        assert!(!panel.view.check_box(&cx,ids!(web_search)).active(&cx));
    }
    #[test]
    fn programming_settings_save_reload_and_stay_on_the_execution_device() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.agents=vec![json!({"id":"agent-a"})];
        panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        local_device_fixture(&mut panel,"agent-a");
        panel.policy=json!({"modelLoaded":true,"modelProfile":{"model":"probe","credential_ref":"keychain:probe","workspace_root":"/work","programming":{"enabled":true,"mcp_servers":{"docs":{"transport":"http","url":"https://example.test/mcp"}}}}});
        panel.render_policy(&mut cx);panel.render_models(&mut cx);
        assert!(panel.view.view(&cx,ids!(programming_holder)).visible());
        assert!(panel.view.check_box(&cx,ids!(programming)).active(&cx));
        let saved=panel.model_save_input(&mut cx,&json!({"model":"probe"}),"low");
        assert_eq!(saved["profile"]["programming"]["enabled"],true);
        assert_eq!(saved["profile"]["programming"]["mcp_servers"]["docs"]["url"],"https://example.test/mcp");
        panel.view.text_input(&cx,ids!(mcp_servers)).set_text(&mut cx,"invalid json");
        assert!(panel.model_save_input(&mut cx,&json!({"model":"probe"}),"low")["profile"]["programming"]["mcp_servers"].is_null());
        panel.current_device_id="other-device".into();panel.render_models(&mut cx);
        assert!(!panel.view.view(&cx,ids!(programming_holder)).visible());
        assert!(panel.model_save_input(&mut cx,&json!({"model":"probe"}),"low")["profile"].get("programming").is_none());
    }
    #[test]
    fn remote_configuration_retry_window_ends_after_confirmed_save_or_validation_failure() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm| {makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        let digest="account:a:3:model-A".to_owned();
        let first_key=uuid::Uuid::new_v4().to_string();
        panel.configuration_keys.insert(digest.clone(),first_key.clone());
        panel.configuration_status=json!({"online":true,"cached":false});
        panel.finish_configuration_intent(Some(&digest),&Err("configuration_outcome_unknown".into()));
        assert_eq!(panel.configuration_keys[&digest],first_key,"unconfirmed saves reuse the original idempotency key");
        assert!(panel.configuration_status["online"]==true,"unknown application result does not assert device offline");
        panel.finish_configuration_intent(Some(&digest),&Err("invalid_configuration_response".into()));
        assert_eq!(panel.configuration_keys[&digest],first_key,"an invalid post-save response cannot prove that no write occurred");
        panel.finish_configuration_intent(Some(&digest),&Ok(json!({"agentId":"a"})));
        assert!(!panel.configuration_keys.contains_key(&digest));
        let later_key=panel.configuration_keys.entry(digest.clone()).or_insert_with(||uuid::Uuid::new_v4().to_string()).clone();
        assert_ne!(later_key,first_key,"model A → B → A cannot replay the first A save receipt");
        panel.finish_configuration_intent(Some(&digest),&Err("invalid_local_policy".into()));
        assert!(!panel.configuration_keys.contains_key(&digest));
        assert!(panel.configuration_status["online"]==true,"a validation failure leaves the execution device online");
        let validation=panel.configuration_feedback.clone();
        panel.finish_configuration_intent(None,&Ok(json!({"configuration":{"online":true}})));
        assert_eq!(panel.configuration_feedback,validation,"quiet metadata reads preserve visible mutation errors");
        panel.finish_configuration_intent(None,&Err("configuration_device_offline".into()));
        assert!(panel.configuration_status["online"]==false);assert!(panel.configuration_status["cached"]==true);
        assert_eq!(panel.configuration_feedback,validation);
    }
    #[test]
    fn remote_configuration_allowlist_rejects_room_credentials_and_runtime_controls() {
        let local=|action,binding_id,requester|Command::Local {agent_id:"a".into(),action,input:None,binding_id,requester};
        for action in [LocalAction::AgentModel,LocalAction::AgentPolicy,LocalAction::SaveAgentModel,LocalAction::SaveAgentPolicy] {
            assert!(remote_configuration_command(&local(action.clone(),None,None)));
            assert!(!remote_configuration_command(&local(action.clone(),Some("room".into()),None)));
            assert!(!remote_configuration_command(&local(action,None,Some("member".into()))));
        }
        for action in [LocalAction::Policy,LocalAction::Model,LocalAction::SavePolicy,LocalAction::ResetAgentPolicy,LocalAction::ResetPolicy,LocalAction::RuntimeStart,LocalAction::RuntimeStop,LocalAction::Approvals,LocalAction::StartAgentService,LocalAction::StopAgentService] {
            assert!(!remote_configuration_command(&local(action,None,None)));
        }
        for action in [ProviderAction::Status,ProviderAction::Models,ProviderAction::UseLocal] {
            assert!(remote_configuration_command(&Command::RuntimeProvider {runtime:"octos".into(),action}));
        }
        for action in [ProviderAction::Login,ProviderAction::Logout,ProviderAction::Disconnect,ProviderAction::Cancel] {
            assert!(!remote_configuration_command(&Command::Provider {action}));
        }
    }
    #[test]
    fn remote_configuration_metadata_rejects_stale_generation_agent_and_device() {
        let agent=json!({"id":"a","generation":3,"executionDeviceId":"A"});
        let fresh=json!({"agentId":"a","agentGeneration":3,"executionDeviceId":"A","deviceGeneration":2});
        assert!(configuration_matches(&fresh,&agent));
        for (field,wrong) in [("agentId",json!("b")),("agentGeneration",json!(2)),("executionDeviceId",json!("B")),("deviceGeneration",Value::Null)] {
            let mut stale=fresh.clone();stale[field]=wrong;
            assert!(!configuration_matches(&stale,&agent),"{field} must bind the cached snapshot");
        }
    }
    #[test]
    fn cached_remote_environment_uses_target_catalog_and_disables_writes_and_local_controls() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm| {makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.agents=vec![json!({"id":"a","generation":3,"executionDeviceId":"A","state":"active"})];
        panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        panel.execution_agent_record=panel.agents[0].clone();panel.current_device_id="B".into();panel.devices_loaded=true;
        panel.execution_devices=vec![json!({"id":"A","name":"Execution workstation","revoked":false}),json!({"id":"B","revoked":false})];
        panel.section(&mut cx,ids!(model_section));
        panel.configuration_status=json!({"agentId":"a","agentGeneration":3,"executionDeviceId":"A","deviceGeneration":1,"online":false,"cached":true,"observedAtMs":1710000000000i64,"lastSeenAtMs":1710000000001i64});
        panel.provider_models=vec![json!({"model":"B-only"})];panel.credential_ref="B-credential".into();
        let reference="octos-managed:shared-home:A:review";
        let env=json!({"providers":[{"runtime":"octos","installed":true,"version":"1.2.3","versionSupported":true,"state":"ready","authenticated":true,"accountVerified":false,"credentialRef":reference,"shared":true,"models":[{"model":"A-only","credentialRef":reference,"displayName":"A profile","isDefault":true,"defaultReasoningEffort":"none","supportedReasoningEfforts":[{"reasoningEffort":"none"}]}]}],"configuration":{"agentId":"a","policy":{"revision":1,"policy":{"budget":{"limit":"Unlimited","period":"Lifetime"},"requests":"Allow","high_risk":"Deny"}},"usage":{"spent":2,"held":0},"modelProfile":{"model":"A-only","credential_ref":reference,"workspace_root":"/A/workspace","reasoning_effort":"none"}}});
        panel.apply(&mut cx,Target::ExecutionEnvironment,&env);panel.sync_navigation(&mut cx);
        assert_eq!(panel.provider_models.len(),1);assert_eq!(panel.provider_models[0]["model"],"A-only");assert_eq!(panel.selected_credential(&mut cx),reference);
        assert_eq!(panel.view.text_input(&cx,ids!(workspace)).text(),"/A/workspace");
        assert!(panel.view.label(&cx,ids!(workspace_label)).text().contains("Execution workstation"));
        assert!(panel.view.label(&cx,ids!(configuration_notice)).text().contains("Cached observation"));
        assert!(panel.view.label(&cx,ids!(provider_status)).text().contains("1.2.3"));
        for id in [ids!(save_model),ids!(save_policy),ids!(prepare_workspace),ids!(use_local_provider),ids!(runtime_tab)] {
            assert!(!panel.view.button(&cx,id).borrow().unwrap().enabled());
        }
        for id in [ids!(provider_login),ids!(provider_logout),ids!(disconnect_provider),ids!(reset_policy)] {assert!(!panel.view.button(&cx,id).visible());}
        panel.configuration_status["online"]=true.into();panel.configuration_status["cached"]=false.into();panel.sync_navigation(&mut cx);
        assert!(panel.view.button(&cx,ids!(save_model)).borrow().unwrap().enabled());
        assert!(panel.view.button(&cx,ids!(save_policy)).borrow().unwrap().enabled());
        let mut status=panel.configuration_status.clone();status["deviceGeneration"]=2.into();status["online"]=false.into();status["environment"]=Value::Null;
        panel.apply(&mut cx,Target::ConfigurationStatus,&json!({"configuration":status}));
        assert!(panel.provider_models.is_empty(),"a different execution device generation cannot display the old catalog");
        assert!(panel.policy.is_null());assert_eq!(panel.view.text_input(&cx,ids!(workspace)).text(),"");
        panel.clear_configuration(&mut cx);panel.sync_navigation(&mut cx);
        assert!(panel.provider_models.is_empty());assert!(panel.credential_ref.is_empty());assert!(panel.policy.is_null());
        assert!(!panel.view.button(&cx,ids!(save_model)).borrow().unwrap().enabled());
        panel.finish_configuration_intent(None,&Err("unsupported_hagency_capabilities".into()));panel.sync_navigation(&mut cx);
        assert!(panel.view.label(&cx,ids!(configuration_notice)).text().contains("does not support execution device configuration"));
        assert!(!panel.view.label(&cx,ids!(configuration_notice)).text().contains("Checking execution device environment"));
    }
    #[test]
    fn external_runtime_resources_restore_kind_and_exact_octos_profile() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.HagencyPanel {}});
            HagencyPanel::script_from_value(vm, value)
        });
        for (kind, index, reference) in [("claude",1,"claude-managed:shared-home:saved"), ("octos",2,"octos-managed:shared-home:saved:review")] {
            panel.policy=json!({"modelLoaded":true,"modelProfile":{"model":"same","credential_ref":reference,"reasoning_effort":"none"}});
            panel.render_policy(&mut cx);
            assert_eq!(panel.view.drop_down(&cx,ids!(runtime_kind)).selected_item(),index);
            panel.apply(&mut cx,Target::ProviderModels,&json!({"runtime":kind,"authenticated":true,"shared":true,"credentialRef":"provider-level-reference","models":[
                {"model":"same","displayName":"Coding","credentialRef":"other-reference","isDefault":true,"defaultReasoningEffort":"none","supportedReasoningEfforts":[{"reasoningEffort":"none"}]},
                {"model":"same","displayName":"Review","credentialRef":reference,"defaultReasoningEffort":"none","supportedReasoningEfforts":[{"reasoningEffort":"none"}]}
            ]}));
            assert_eq!(panel.view.drop_down(&cx,ids!(model)).selected_item(),2);
            assert_eq!(panel.selected_credential(&mut cx),reference);
            assert_eq!(panel.runtime_efforts,vec!["none"]);
            assert!(!panel.view.button(&cx,ids!(provider_login)).visible());
            assert!(!panel.view.button(&cx,ids!(provider_logout)).visible());
        }
    }
    #[test]
    fn remote_approval_controls_bind_original_proposal_and_keep_unknown_non_retryable() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());let v=script_eval!(vm,{mod.widgets.HagencyPanel {}});let p=HagencyPanel::script_from_value(vm,v);assert!(vm.take_errors().is_empty());p});
        panel.agents=vec![json!({"id":"a","executionDeviceId":"device-here","state":"active","generation":1})];
        local_device_fixture(&mut panel,"a");
        panel.labels(&mut cx,ids!(agents),"Select my Agent",&panel.agents,"id");
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        panel.bindings=vec![json!({"id":"binding","roomId":"!task:test"})];
        panel.labels(&mut cx,ids!(bindings),"Select a Room binding",&panel.bindings,"id");panel.select_policy_room(&mut cx,1);
        panel.apply_approvals(&mut cx,&json!({"approvals":[{"proposalId":"original","argsDigest":"a".repeat(64),"proposal":{"scope":{"agent":"a","binding":"binding","room":"!task:test","requester":"@owner:test"},"tool":"codex.command","arguments":{"command":"echo original"},"expires":crate::hagency::approvals::now()+300}}]}));
        panel.view.drop_down(&cx,ids!(approvals)).set_selected_item(&mut cx,1);
        panel.remote_approval_rooms=vec![json!({"id":"!private:test","name":"Private approvals"})];
        panel.labels(&mut cx,ids!(remote_approval_room),"Select a private approval Room",&panel.remote_approval_rooms,"name");
        panel.view.drop_down(&cx,ids!(remote_approval_room)).set_selected_item(&mut cx,1);
        panel.sync_navigation(&mut cx);
        assert!(panel.view.button(&cx,ids!(remote_approval_publish)).borrow().unwrap().enabled());
        panel.apply(&mut cx,Target::RemoteApproval,&json!({"approvalId":"foreign","remoteApproval":{"phase":"sent","approvalRoom":"!foreign:test","requestEventId":"$foreign"}}));
        assert!(panel.remote_approval.is_null());
        let sent=json!({"approvalId":"original","remoteApproval":{"phase":"sent","approvalRoom":"!private:test","requestEventId":"$original"}});
        panel.apply(&mut cx,Target::RemoteApproval,&sent);
        assert_eq!(panel.view.label(&cx,ids!(remote_approval_details)).text(),crate::i18n::tr("Private approval published. Waiting for a verified decision."));
        assert!(!panel.view.button(&cx,ids!(remote_approval_publish)).borrow().unwrap().enabled());
        assert!(panel.view.button(&cx,ids!(remote_approval_observe)).borrow().unwrap().enabled());
        let mut unknown=sent;unknown["remoteApproval"]["phase"]=json!("unknown");
        panel.apply(&mut cx,Target::RemoteApproval,&unknown);
        assert!(!panel.view.button(&cx,ids!(remote_approval_publish)).borrow().unwrap().enabled());
        assert!(!panel.view.button(&cx,ids!(remote_approval_observe)).borrow().unwrap().enabled());
        assert!(panel.view.button(&cx,ids!(remote_approval_refresh)).borrow().unwrap().enabled());
        assert!(panel.view.button(&cx,ids!(reject)).borrow().unwrap().enabled());
        panel.approvals[0]["proposal"]["expires"]=json!(0);panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(remote_approval_refresh)).borrow().unwrap().enabled());
        panel.clear_approval_state(&mut cx);assert!(panel.remote_approval.is_null());
        assert_eq!(panel.view.drop_down(&cx,ids!(remote_approval_room)).selected_item(),0);
    }

    #[test]
    fn remote_reviewer_ui_shows_complete_action_on_other_device_and_never_rearms_unknown() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());let v=script_eval!(vm,{mod.widgets.HagencyPanel {}});let p=HagencyPanel::script_from_value(vm,v);assert!(vm.take_errors().is_empty());p});
        panel.agents=vec![json!({"id":"a","executionDeviceId":"other-device","state":"active","generation":1})];
        local_device_fixture(&mut panel,"a");panel.execution_agent_record["executionDeviceId"]=json!("other-device");
        panel.labels(&mut cx,ids!(agents),"Select my Agent",&panel.agents,"id");panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        panel.remote_review_rooms=vec![json!({"id":"!private:test","name":"Private approvals"})];
        panel.labels(&mut cx,ids!(remote_review_room),"Select a private approval Room",&panel.remote_review_rooms,"name");panel.view.drop_down(&cx,ids!(remote_review_room)).set_selected_item(&mut cx,1);
        let command=format!("{} exact final bytes","x".repeat(20000));
        let row=json!({"requestEventId":"$original","requestDigest":"a".repeat(64),"request":{"approvalId":"a".repeat(64),"approvalRoom":"!private:test","proposalDigest":"b".repeat(64),"proposal":{"scope":{"agent":"a","binding":"b","room":"!task:test","requester":"@owner:test"},"tool":"codex.command","arguments":{"command":command},"expires":super::super::approvals::now()+300}},"publication":null});
        panel.apply(&mut cx,Target::RemoteInbox,&json!({"agentId":"foreign","roomId":"!private:test","requests":[row.clone()]}));assert!(panel.remote_review_requests.is_empty());
        panel.apply(&mut cx,Target::RemoteInbox,&json!({"agentId":"a","roomId":"!private:test","requests":[row],"nextBatch":"older"}));
        panel.view.drop_down(&cx,ids!(remote_review_requests)).set_selected_item(&mut cx,1);panel.render_remote_reviews(&mut cx);panel.sync_navigation(&mut cx);
        assert!(panel.view.button(&cx,ids!(remote_review_allow)).borrow().unwrap().enabled());
        assert!(panel.view.button(&cx,ids!(remote_review_deny)).borrow().unwrap().enabled());
        assert!(panel.view.label(&cx,ids!(remote_review_details)).text().contains(&command));
        panel.apply(&mut cx,Target::RemoteVerdict,&json!({"approvalId":"a".repeat(64),"publication":{"phase":"unknown","approved":true}}));
        assert!(!panel.view.button(&cx,ids!(remote_review_allow)).borrow().unwrap().enabled());assert!(!panel.view.button(&cx,ids!(remote_review_deny)).borrow().unwrap().enabled());
        assert!(panel.view.button(&cx,ids!(remote_review_receipt)).borrow().unwrap().enabled());assert!(panel.view.button(&cx,ids!(remote_review_copy)).borrow().unwrap().enabled());
        panel.clear_remote_reviews(&mut cx);panel.sync_navigation(&mut cx);assert!(panel.remote_review_requests.is_empty());assert!(!panel.view.button(&cx,ids!(remote_review_allow)).borrow().unwrap().enabled());
    }

    #[test]
    fn approval_widget_filters_rooms_keeps_selection_and_disables_stale_decisions() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());let v=script_eval!(vm,{mod.widgets.HagencyPanel {}});let p=HagencyPanel::script_from_value(vm,v);assert!(vm.take_errors().is_empty());p});
        panel.agents=vec![json!({"id":"a","executionDeviceId":"device-here","state":"active","generation":1})];
        local_device_fixture(&mut panel,"a");
        panel.labels(&mut cx,ids!(agents),"Select my Agent",&panel.agents,"id");
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        panel.bindings=vec![json!({"id":"b1","roomId":"!one:s"}),json!({"id":"b2","roomId":"!two:s"})];
        panel.labels(&mut cx,ids!(bindings),"Select a Room binding",&panel.bindings,"id");
        panel.select_policy_room(&mut cx,1);
        let agent=panel.agent(&mut cx).unwrap();
        let row=|id:&str,b:&str,r:&str|json!({"proposalId":id,"argsDigest":"a".repeat(64),"proposal":{"scope":{"agent":agent,"binding":b,"room":r,"requester":"@owner:s"},"tool":"model.request","arguments":{"inputPreview":"Exact request"},"risk":"model_request","expires":crate::hagency::approvals::now()+300}});
        let data=json!({"approvals":[row("one","b1","!one:s"),row("two","b2","!two:s"),row("wrong-room","b1","!two:s")]});
        panel.apply_approvals(&mut cx,&data);
        assert_eq!(panel.approvals.len(),1);assert_eq!(panel.approvals[0]["proposalId"],"one");
        panel.view.drop_down(&cx,ids!(approvals)).set_selected_item(&mut cx,1);
        panel.apply_approvals(&mut cx,&data);
        assert_eq!(panel.view.drop_down(&cx,ids!(approvals)).selected_item(),1);
        panel.sync_navigation(&mut cx);assert!(panel.view.button(&cx,ids!(approve)).borrow().unwrap().enabled());
        panel.approval_blocked.insert("one".into());panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(approve)).borrow().unwrap().enabled());
        panel.approval_blocked.clear();panel.approval_checked=false;panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(approve)).borrow().unwrap().enabled());
        panel.approval_checked=true;panel.approvals[0]["proposal"]["expires"]=json!(0);panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(approve)).borrow().unwrap().enabled());
        panel.active=true;panel.page=Page::Agents;
        panel.view.view(&cx,ids!(agent_detail)).set_visible(&mut cx,true);
        panel.view.view(&cx,ids!(runtime_section)).set_visible(&mut cx,true);
        panel.identity="owner-session".into();panel.approval_poll=Some(12);
        let mut late=ApprovalsCompleted {identity:panel.identity.clone(),request:12,agent:agent.clone(),binding:"b1".into(),room:"!one:s".into(),result:Ok(data.clone())};
        assert!(panel.accepts_approval_poll(&mut cx,&late));
        panel.page=Page::Projects;assert!(!panel.accepts_approval_poll(&mut cx,&late));panel.page=Page::Agents;
        late.identity="old-session".into();assert!(!panel.accepts_approval_poll(&mut cx,&late));late.identity=panel.identity.clone();
        panel.select_policy_room(&mut cx,2);assert!(panel.approval_poll.is_none());assert!(panel.approvals.is_empty());assert!(!panel.accepts_approval_poll(&mut cx,&late));
        panel.apply_approvals(&mut cx,&data);assert_eq!(panel.approvals.len(),1);assert_eq!(panel.approvals[0]["proposalId"],"two");
        panel.active=false;
        panel.apply(&mut cx,Target::ApprovalDecision,&json!({"decided":true}));
        assert_eq!(panel.view.label(&cx,ids!(approval_feedback)).text(),crate::i18n::tr("Decision submitted. Execution is not yet confirmed. Check the Room reply and runtime status."));
        panel.apply(&mut cx,Target::ApprovalDecision,&Value::Null);
        assert_eq!(panel.view.label(&cx,ids!(approval_feedback)).text(),crate::i18n::tr("Decision delivery is unconfirmed. Refresh status; do not resubmit this request."));
    }

    use super::*;
    fn local_device_fixture(panel: &mut HagencyPanel, agent: &str) {
        panel.execution_agent_record = json!({"id":agent,"executionDeviceId":"device-here","generation":1});
        panel.current_device_id = "device-here".into();
        panel.execution_devices = vec![json!({"id":"device-here","name":"This computer","revoked":false})];
        panel.devices_loaded = true;
    }
    #[test]
    fn registered_management_lists_fill_available_height_and_badges_share_a_row() {
        use makepad_widgets::makepad_draw::cx_draw::CxDraw;
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());let v=script_eval!(vm,{mod.widgets.HagencyPanel {}});let p=HagencyPanel::script_from_value(vm,v);assert!(vm.take_errors().is_empty());p});
        panel.projects=vec![json!({"id":"p1","name":"First project"}),json!({"id":"p2","name":"Second project"})];
        panel.agents=vec![json!({"id":"a1","displayName":"First agent","generation":1,"state":"active","executionDeviceId":"here","puppetMxid":"@agent1:server","visibility":"private"}),json!({"id":"a2","displayName":"Second agent","generation":1,"state":"active","executionDeviceId":"here","puppetMxid":"@agent2:server","visibility":"public"})];
        panel.devices_loaded=true;panel.current_device_id="here".into();panel.execution_devices=vec![json!({"id":"here","revoked":false})];
        for width in [600.0,980.0] {for page in [Page::Projects,Page::Agents] {for height in [760.0,500.0] {
            panel.set_page(&mut cx,page);panel.list_page(&mut cx);
            {cx.redraw_all();let pass=DrawPass::new(&mut cx);pass.set_size(&mut cx,dvec2(width,height));let mut dl=DrawList2d::new(&mut cx);let overlay=Overlay{draw_list:DrawList::new(&mut cx)};let event=DrawEvent::default();let mut draw=CxDraw::new(&mut cx,&event);let mut cx2d=Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass,None);dl.begin_always(&mut cx2d);overlay.begin(&mut cx2d);cx2d.begin_root_turtle(dvec2(width,height),Layout::flow_overlay());
            panel.draw_walk_all(&mut cx2d,&mut Scope::empty(),Walk::fixed(width,height));
            overlay.end(&mut cx2d);cx2d.end_pass_sized_turtle();dl.end(&mut cx2d);cx2d.end_pass(&pass);}
            let list=panel.view.portal_list(&cx,if page==Page::Projects {ids!(project_list)} else {ids!(agent_list)});
            let rect=list.area().rect(&cx);
            assert!(rect.size.y>height-120.0,"list should fill the body: {rect:?}, height {height}");
            assert!(rect.pos.y+rect.size.y<=height-15.0,"list stays within panel padding: {rect:?}");
            let second=list.item(&mut cx,1,id!(Card)).area().rect(&cx);assert!(second.size.y>0.0 && second.pos.y+second.size.y<height,"second card is fully visible: {second:?}");
            let header=panel.view.view(&cx,ids!(title_row)).area().rect(&cx);
            for id in [ids!(new_item),ids!(refresh)] {
                let button=panel.view.button(&cx,id).area().rect(&cx);
                assert!(button.pos.y>=header.pos.y && button.pos.y+button.size.y<=header.pos.y+header.size.y+0.1,"header must contain the complete button: {button:?}, {header:?}");
            }
            if page==Page::Projects {
                let row=list.item(&mut cx,0,id!(Card));let card=row.area().rect(&cx);
                assert!(card.size.y<=64.0,"Project rows without a topic must remain compact: {card:?}");
                assert!(!row.label(&cx,ids!(summary)).visible());assert!(!row.view(&cx,ids!(badge_row)).visible());
                let open=row.button(&cx,ids!(open)).area().rect(&cx);
                assert!(open.pos.y>=card.pos.y && open.pos.y+open.size.y<=card.pos.y+card.size.y,"project action stays inside its row");
            }
            if page==Page::Agents {
                let row=list.item(&mut cx,0,id!(Card));let device=row.view(&cx,ids!(device_badge)).area().rect(&cx);let runtime=row.button(&cx,ids!(runtime_badge)).area().rect(&cx);
                if width==980.0 {assert!((device.pos.y-runtime.pos.y).abs()<1.0);assert!(runtime.pos.x>=device.pos.x+device.size.x);}
                assert!(device.size.y<=28.0 && runtime.size.y<=28.0);
                let badges=row.view(&cx,ids!(badge_row)).area().rect(&cx);
                for id in [ids!(audience_badge),ids!(device_badge),ids!(runtime_badge)] {
                    let badge=row.widget(&cx,id).area().rect(&cx);
                    assert!(badge.size.x>0.0 && badge.size.y>0.0);
                    assert!(badge.pos.x>=badges.pos.x-1.0 && badge.pos.x+badge.size.x<=badges.pos.x+badges.size.x+1.0,"badge overflows at {width}px: {badge:?} in {badges:?}");
                }
            }
        }}}
        panel.set_page(&mut cx,Page::Projects);panel.view.view(&cx,ids!(project_detail)).set_visible(&mut cx,true);panel.project_section(&mut cx,ids!(members_section));panel.members=vec![json!({"name":"Alice","userId":"@alice:server","membership":"join"})];
        let height=760.0;{cx.redraw_all();let pass=DrawPass::new(&mut cx);pass.set_size(&mut cx,dvec2(980.0,height));let mut dl=DrawList2d::new(&mut cx);let overlay=Overlay{draw_list:DrawList::new(&mut cx)};let event=DrawEvent::default();let mut draw=CxDraw::new(&mut cx,&event);let mut cx2d=Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass,None);dl.begin_always(&mut cx2d);overlay.begin(&mut cx2d);cx2d.begin_root_turtle(dvec2(980.0,height),Layout::flow_overlay());panel.draw_walk_all(&mut cx2d,&mut Scope::empty(),Walk::fixed(980.0,height));overlay.end(&mut cx2d);cx2d.end_pass_sized_turtle();dl.end(&mut cx2d);cx2d.end_pass(&pass);}
        let members=panel.view.portal_list(&cx,ids!(members_list)).area().rect(&cx);assert!(members.size.y>300.0,"members use remaining space: {members:?}");assert!(members.pos.y+members.size.y<=height,"members stay in panel: {members:?}");
        panel.display_blocked=true;panel.sync_navigation(&mut cx);assert!(!panel.view.view(&cx,ids!(details_scroll)).visible());panel.list_page(&mut cx);panel.sync_navigation(&mut cx);assert!(!panel.view.view(&cx,ids!(lists_pane)).visible(),"account authorization failure hides the new common list pane too");
    }
    #[test]
    fn request_notices_fit_content_and_management_scroll_viewports_reach_the_edge() {
        use makepad_widgets::makepad_draw::cx_draw::CxDraw;
        fn draw(cx:&mut Cx,panel:&mut HagencyPanel,width:f64) {
            cx.redraw_all();let pass=DrawPass::new(cx);pass.set_size(cx,dvec2(width,800.0));
            let mut list=DrawList2d::new(cx);let overlay=Overlay{draw_list:DrawList::new(cx)};
            let event=DrawEvent{redraw_all:true,..Default::default()};
            let mut draw=CxDraw::new(cx,&event);let mut cx2d=Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass,None);list.begin_always(&mut cx2d);overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(dvec2(width,800.0),Layout::flow_overlay());
            panel.draw_walk_all(&mut cx2d,&mut Scope::empty(),Walk::fixed(width,800.0));
            overlay.end(&mut cx2d);cx2d.end_pass_sized_turtle();list.end(&mut cx2d);cx2d.end_pass(&pass);
        }
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);
            vm.bx.captured_errors=Some(Vec::new());
            let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});
            let panel=HagencyPanel::script_from_value(vm,value);assert!(vm.take_errors().is_empty());panel
        });
        panel.agents=vec![json!({"id":"agent-a","displayName":"Agent","generation":1,"visibility":"private","state":"active"})];
        for width in [420.0,1080.0,1440.0] {
            panel.set_page(&mut cx,Page::Agents);panel.list_page(&mut cx);
            panel.agent_pending_commands=vec![json!({"id":"request-a","operation":"agent.create","state":"unknown","input":{"displayName":"Pending Agent"}})];
            draw(&mut cx,&mut panel,width);
            let requests=panel.view.flat_list(&cx,ids!(agent_request_list));
            let request_rect=requests.area().rect(&cx);
            let request=requests.item(&mut cx,LiveId::from_str("request-a"),id!(Request)).unwrap();
            let row=request.area().rect(&cx);
            assert!(row.size.y>50.0);
            assert!((request_rect.size.y-row.size.y).abs()<2.0,"request region must fit its row, without a reserved empty block: {request_rect:?}, {row:?}");
            let title=request.label(&cx,ids!(title)).area().rect(&cx);
            let check=request.button(&cx,ids!(check_request)).area().rect(&cx);
            let retry=request.button(&cx,ids!(retry_request)).area().rect(&cx);
            let notice=request.widget(&cx,ids!(request_notice)).area().rect(&cx);
            for button in [check,retry] {
                assert!(button.size.x>0.0 && button.size.y>0.0);
                assert!(button.pos.x>=notice.pos.x && button.pos.x+button.size.x<=notice.pos.x+notice.size.x,"request actions stay inside the notice at {width}px: {button:?}, {notice:?}");
            }
            if width>=1080.0 {
                assert!(request_rect.size.y<110.0,"request actions must not reserve a separate row: {request_rect:?}");
                assert!(check.pos.x>=title.pos.x+title.size.x,"actions follow the title");
                assert!((check.pos.y+check.size.y/2.0-title.pos.y-title.size.y/2.0).abs()<4.0,"actions align with the title: {title:?}, {check:?}");
            }
            let recovery=panel.view.view(&cx,ids!(agent_recovery)).area().rect(&cx);
            assert!((recovery.size.y-request_rect.size.y).abs()<2.0,"no outer panel padding or repeated headings");
            let viewport=panel.view.portal_list(&cx,ids!(agent_list)).area().rect(&cx);
            assert!((viewport.pos.x+viewport.size.x-width).abs()<1.0,"scrolling viewport reaches the panel edge: {viewport:?}");
            let card=panel.view.portal_list(&cx,ids!(agent_list)).item(&mut cx,0,id!(Card)).area().rect(&cx);
            assert!((width-card.pos.x-card.size.x-20.0).abs()<1.0,"content keeps its own right gutter: {card:?}");
            panel.agent_pending_commands.clear();panel.sync_navigation(&mut cx);draw(&mut cx,&mut panel,width);
            assert!(!panel.view.view(&cx,ids!(agent_recovery)).visible());
            assert!(panel.view.portal_list(&cx,ids!(agent_list)).area().rect(&cx).pos.y<viewport.pos.y-50.0);
            panel.set_page(&mut cx,Page::Projects);panel.list_page(&mut cx);draw(&mut cx,&mut panel,width);
            let viewport=panel.view.portal_list(&cx,ids!(project_list)).area().rect(&cx);
            assert!((viewport.pos.x+viewport.size.x-width).abs()<1.0);
        }
    }
    #[test]
    fn lease_wait_is_explicit_in_agent_list() {
        let value=json!({"runtime":{"agentId":"a","phase":"waiting_for_lease"}});
        assert_eq!(runtime_list_label(Some(&Ok(value.clone())),"a"),"Waiting for the previous runtime lease to be released…");
    }
    #[test]
    fn detail_scroll_resets_on_identity_target_change_but_preserves_same_detail() {
        use makepad_widgets::makepad_draw::cx_draw::CxDraw;
        fn draw_small(cx:&mut Cx,panel:&mut HagencyPanel)->f64 {
            {cx.redraw_all();let pass=DrawPass::new(cx);pass.set_size(cx,dvec2(980.0,360.0));let mut dl=DrawList2d::new(cx);let overlay=Overlay{draw_list:DrawList::new(cx)};let e=DrawEvent::default();let mut draw=CxDraw::new(cx,&e);let mut cx2d=Cx2d::new(&mut draw);cx2d.begin_pass(&pass,None);dl.begin_always(&mut cx2d);overlay.begin(&mut cx2d);cx2d.begin_root_turtle(dvec2(980.0,360.0),Layout::flow_overlay());panel.draw_walk_all(&mut cx2d,&mut Scope::empty(),Walk::fixed(980.0,360.0));overlay.end(&mut cx2d);cx2d.end_pass_sized_turtle();dl.end(&mut cx2d);cx2d.end_pass(&pass);}
            panel.view.view(cx,ids!(agent_detail)).area().rect(cx).pos.y
        }
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());let v=script_eval!(vm,{mod.widgets.HagencyPanel {}});let p=HagencyPanel::script_from_value(vm,v);assert!(vm.take_errors().is_empty());p});
        panel.identity="owner".into();panel.agents=vec![json!({"id":"a","displayName":"First"}),json!({"id":"b","displayName":"Second"})];panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");panel.set_page(&mut cx,Page::Agents);panel.agent_detail(&mut cx,0);local_device_fixture(&mut panel,"a");panel.section(&mut cx,ids!(model_section));
        let top=draw_small(&mut cx,&mut panel);panel.view.view(&cx,ids!(details_scroll)).set_scroll_pos(&mut cx,dvec2(0.0,100.0));let scrolled=draw_small(&mut cx,&mut panel);assert!(scrolled<top-50.0,"real detail scrolling must move content: {top}, {scrolled}");
        panel.enter_detail(&mut cx,"agent","a");assert!((draw_small(&mut cx,&mut panel)-scrolled).abs()<1.0,"same detail preserves scrolling");
        panel.agent_detail(&mut cx,1);local_device_fixture(&mut panel,"b");panel.section(&mut cx,ids!(model_section));assert!((draw_small(&mut cx,&mut panel)-top).abs()<1.0,"cross-Agent entry resets to top");
        panel.view.view(&cx,ids!(details_scroll)).set_scroll_pos(&mut cx,dvec2(0.0,100.0));draw_small(&mut cx,&mut panel);panel.enter_detail(&mut cx,"project","same-id");assert!((draw_small(&mut cx,&mut panel)-top).abs()<1.0,"cross-Project target resets too");
        panel.view.view(&cx,ids!(details_scroll)).set_scroll_pos(&mut cx,dvec2(0.0,100.0));draw_small(&mut cx,&mut panel);panel.reset_detail_scroll(&mut cx);assert!((draw_small(&mut cx,&mut panel)-top).abs()<1.0,"explicit Run/new-form entry resets");
    }
    #[test]
    fn local_runtime_badge_does_not_confuse_server_identity_with_processing() {
        assert_eq!(runtime_list_label(None,"a"),"Checking local processing…");
        assert!(runtime_list_label(Some(&Ok(json!({"runtime":null}))),"a").starts_with("Not running"));
        assert!(runtime_list_label(Some(&Ok(json!({"runtime":{"agentId":"foreign","phase":"online"}}))),"a").contains("could not be verified"));
        assert!(runtime_list_label(Some(&Err("failure".into())),"a").contains("could not be verified"));
        assert!(runtime_list_label(Some(&Ok(json!({"runtime":{"agentId":"a","phase":"online"}}))),"a").contains("budgets and policy"));
        assert!(runtime_list_label(Some(&Ok(json!({"runtime":{"agentId":"a","phase":"stopped"}}))),"a").starts_with("Not running"));
        assert!(runtime_list_label(Some(&Ok(json!({"runtime":{"agentId":"a","phase":"reconciliation_only"}}))),"a").contains("Only known original replies"));
        let missing=json!({"service":{"enabled":false,"modelConfigured":false},"runtime":null});
        assert!(runtime_service_description(Some(&Ok(missing)),"a").contains("Model & resources"));
        for code in ["agent_model_required","runtime_profile_unavailable","provider_profile_mismatch","owner_authorization_required"] {
            assert!(!runtime_diagnostic(code).contains("This action could not be completed"));
        }
    }
    #[test]
    fn model_save_progress_and_results_stay_at_the_action_without_dimming_tabs() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.agents=vec![json!({"id":"a","generation":1,"executionDeviceId":"device-here"})];
        panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        local_device_fixture(&mut panel,"a");
        panel.section(&mut cx,ids!(model_section));
        let saved=json!({"agentId":"a","policy":{"revision":1},"modelProfile":{"model":"m","credential_ref":"keychain:test","workspace_root":"/workspace","reasoning_effort":"low"}});
        panel.policy=json!({"agentId":"a","scope":"agent","modelLoaded":true,"modelProfile":saved["modelProfile"]});
        panel.provider_models=vec![json!({"model":"m","credentialRef":"keychain:test"})];
        panel.labels(&mut cx,ids!(model),"Select",&panel.provider_models,"model");panel.view.drop_down(&cx,ids!(model)).set_selected_item(&mut cx,1);
        panel.credential_ref="keychain:test".into();panel.provider_authenticated=true;panel.runtime_efforts=vec!["low".into()];
        panel.model_saving=true;panel.set_busy(&mut cx,true);panel.sync_navigation(&mut cx);
        assert_eq!(panel.view.button(&cx,ids!(save_model)).text(),crate::i18n::tr("Saving…"));
        assert!(panel.view.widget(&cx,ids!(model_save_spinner)).visible());
        assert!(!panel.view.button(&cx,ids!(save_model)).borrow().unwrap().enabled());
        assert!(panel.view.button(&cx,ids!(save_model)).borrow().unwrap().disabled(&cx));
        for id in [ids!(setup_tab),ids!(model_tab),ids!(policy_tab),ids!(runtime_tab)] {
            assert!(panel.view.button(&cx,id).borrow().unwrap().enabled());
            assert!(!panel.view.button(&cx,id).borrow().unwrap().disabled(&cx),"save must not fade the tabs");
        }
        panel.set_busy(&mut cx,false);panel.apply(&mut cx,Target::ModelSaved,&saved);
        assert!(!panel.model_saving);assert!(!panel.busy,"confirmed save must not start a second catalog loading cycle");
        assert!(!panel.view.widget(&cx,ids!(model_save_spinner)).visible());
        assert_eq!(panel.view.inline_notice(&cx,ids!(model_save_feedback)).message(),crate::i18n::tr("Model profile saved."));
        assert!(!panel.model_save_failed);
        assert!(panel.view.button(&cx,ids!(save_model)).borrow().unwrap().enabled());
        panel.model_saving=true;panel.finish_model_save(&mut cx,Err("invalid_model_profile"));
        assert!(panel.model_save_failed);assert!(!panel.view.widget(&cx,ids!(model_save_spinner)).visible());
        assert!(panel.view.inline_notice(&cx,ids!(model_save_feedback)).message().contains("model profile or workspace"));
        panel.clear_model_save_feedback(&mut cx);assert!(!panel.view.inline_notice(&cx,ids!(model_save_feedback)).is_visible());
    }
    #[test]
    fn runtime_badge_click_explains_current_problem_without_starting_or_opening_agent() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.agents=vec![json!({"id":"agent-a","displayName":"Agent A","generation":1,"executionDeviceId":"device-here","state":"active"})];
        local_device_fixture(&mut panel,"agent-a");
        panel.set_page(&mut cx,Page::Agents);panel.list_page(&mut cx);panel.sync_navigation(&mut cx);
        let list=panel.view.portal_list(&cx,ids!(agent_list));let row=list.item(&mut cx,0,id!(Card));
        panel.agent_card_targets.insert(row.widget_uid(),"agent-a".into());
        let button=row.button(&cx,ids!(runtime_badge));
        assert!(button.borrow().is_some(),"runtime status must be an accessible button");
        button.set_text(&mut cx,crate::i18n::tr("Needs attention"));
        let click=cx.capture_actions(|cx|{cx.group_widget_actions(list.widget_uid(),row.widget_uid(),|cx|cx.widget_action(button.widget_uid(),ButtonAction::Clicked(KeyModifiers::default())));});
        for (status,expected) in [
            (Ok(json!({"service":{"enabled":false,"modelConfigured":false},"runtime":null})),"No model is configured"),
            (Err("owner_authorization_required".into()),"Sign in again"),
            (Ok(json!({"service":{"enabled":true,"watcherRunning":true,"problems":[{"error":"room_worker_stopped","retryInSeconds":5}]},"runtime":null})),"Room worker stopped"),
        ] {
            panel.runtime_list.insert("agent-a".into(),status);
            let request=panel.request;
            let emitted=cx.capture_actions(|cx|assert!(panel.open_card(cx,&click)));
            let text=emitted.iter().find_map(|action|match action.as_widget_action().cast() {TooltipAction::HoverIn{text,options,..}=>{assert_eq!(options.position,TooltipPosition::Bottom);Some(text)},_=>None}).expect("click opens a callout");
            assert!(text.contains(expected),"{text}");
            assert!(!text.contains("Agent A"),"the callout only contains the problem and next steps");
            assert_eq!(panel.request,request);assert!(!panel.busy);
            assert!(panel.list_service_pending_agent.is_empty());assert!(panel.wanted_model.is_none());
            assert!(panel.view.view(&cx,ids!(lists_pane)).visible());
        }
        panel.agent_card_targets.insert(row.widget_uid(),"different-agent".into());
        let stale=cx.capture_actions(|cx|assert!(panel.open_card(cx,&click)));
        assert!(!stale.iter().any(|action|matches!(action.as_widget_action().cast(),TooltipAction::HoverIn{..})),"a recycled row must not expose another Agent's diagnostics");
    }
    #[test]
    fn unconfigured_agent_card_opens_model_setup_without_starting_service() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.agents=vec![json!({"id":"agent-a","displayName":"Agent A","generation":1,"executionDeviceId":"device-here","state":"active"})];
        panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");
        local_device_fixture(&mut panel,"agent-a");
        panel.set_page(&mut cx,Page::Agents);panel.list_page(&mut cx);panel.sync_navigation(&mut cx);
        panel.runtime_list.insert("agent-a".into(),Ok(json!({"service":{"enabled":false,"modelConfigured":false},"runtime":null})));
        let list=panel.view.portal_list(&cx,ids!(agent_list));
        let row=list.item(&mut cx,0,id!(Card));
        panel.agent_card_targets.insert(row.widget_uid(),"agent-a".into());
        let button=row.button(&cx,ids!(service_control));
        let actions=cx.capture_actions(|cx|{cx.group_widget_actions(list.widget_uid(),row.widget_uid(),|cx|cx.widget_action(button.widget_uid(),ButtonAction::Clicked(KeyModifiers::default())));});
        assert!(panel.open_card(&mut cx,&actions));
        assert_eq!(panel.agent(&mut cx).as_deref(),Some("agent-a"));
        panel.sync_navigation(&mut cx);
        assert_eq!(panel.wanted_model.as_deref(),Some("agent-a"));
        panel.apply(&mut cx,Target::AgentDetails,&json!({"agent":{"id":"agent-a","generation":1,"executionDeviceId":"device-here","state":"active"}}));
        panel.apply(&mut cx,Target::Bindings,&json!({"bindings":[]}));
        panel.sync_navigation(&mut cx);
        assert!(panel.wanted_model.is_none());
        assert!(panel.view.view(&cx,ids!(model_section)).visible(),"async detail/device loading must not reset the model setup destination");
        assert!(panel.list_service_pending_agent.is_empty());
        assert!(!panel.runtime_room_active);
    }
    #[test]
    fn service_readiness_and_failures_are_consistent_in_list_and_details() {
        let starting=json!({"service":{"enabled":true,"watcherRunning":true,"phase":"starting"},"runtime":null});
        assert!(runtime_service_description(Some(&Ok(starting)),"a").starts_with("Starting"));
        let checking=json!({"service":{"enabled":true,"watcherRunning":true},"runtime":{"agentId":"a","phase":"checking_provider","bindings":[{"phase":"checking_provider"}]}});
        assert!(runtime_service_description(Some(&Ok(checking)),"a").starts_with("Checking this device"));
        let unknown=json!({"service":{"enabled":true,"watcherRunning":true},"runtime":{"agentId":"a","phase":"online","bindings":[{"phase":"online_chat_only","lastError":"profile_or_local_state_unavailable"}]}});
        assert!(runtime_service_description(Some(&Ok(unknown)),"a").starts_with("Running"));
        let stopped=json!({"service":{"enabled":true,"watcherRunning":true},"runtime":{"agentId":"a","phase":"stopped","lastError":"authorization_or_lease_lost","bindings":[]}});
        assert!(runtime_service_description(Some(&Ok(stopped)),"a").contains("stopped with an error"));
        let failed=json!({"service":{"enabled":true,"watcherRunning":false,"lastError":"authorization_or_lease_lost"},"runtime":null});
        let description=runtime_service_description(Some(&Ok(failed)),"a");
        assert!(description.contains("needs attention"));
        assert!(description.contains("authorization_or_lease_lost"));
        let retry=json!({"service":{"enabled":true,"watcherRunning":true,"problems":[{"error":"agent_lease_busy","retryInSeconds":8}]},"runtime":{"agentId":"a","phase":"stopped","lastError":"provider_authorization_lost"}});
        let description=runtime_service_description(Some(&Ok(retry)),"a");
        assert!(description.contains("could not start"));
        assert!(description.contains("agent_lease_busy"));
        assert!(description.contains("provider_authorization_lost"));
        assert!(description.contains("8s"));
        assert!(!runtime_diagnostic("secret /private/provider-stderr token=123").contains("secret"));
    }
    #[test]
    fn remote_device_presence_is_fenced_and_does_not_claim_agent_readiness() {
        let agent=json!({"id":"a","generation":2,"executionDeviceId":"d"});
        let mut status=json!({"agentId":"a","agentGeneration":2,"executionDeviceId":"d","deviceGeneration":1,"online":true,"lastSeenAtMs":123});
        assert_eq!(remote_presence_label(Some(&Ok(status.clone())),&agent),"Device online · Agent readiness is separate");
        status["online"]=json!(false);
        assert_eq!(remote_presence_label(Some(&Ok(status.clone())),&agent),"Device offline · no recent heartbeat");
        status["lastSeenAtMs"]=Value::Null;
        assert_eq!(remote_presence_label(Some(&Ok(status.clone())),&agent),"No device heartbeat observed");
        status["deviceGeneration"]=Value::Null;
        assert_eq!(remote_presence_label(Some(&Ok(status.clone())),&agent),"Device unavailable · no active device session");
        status["agentGeneration"]=json!(1);
        assert_eq!(remote_presence_label(Some(&Ok(status)),&agent),"Device status unknown");
    }
    #[test]
    fn service_logs_present_safe_errors_and_retry_does_not_duplicate_summary() {
        let value=json!({"service":{"logs":[{"atMs":1792443600000i64,"phase":"retrying","error":"codex_web_search_unavailable","bindingId":"b","retryInSeconds":60}]}});
        let text=service_log_text(&value);
        assert!(text.contains("2026-10"));assert!(text.contains("UTC"));assert!(text.contains("codex_web_search_unavailable"));
        assert!(!runtime_diagnostic("codex_provider_configuration_rejected").contains("account changed"));
        let value=json!({"runtime":{"agentId":"a","lastError":"provider_authorization_lost"},"service":{"problems":[{"error":"provider_authorization_lost","retryInSeconds":8}]}});
        assert_eq!(runtime_service_description(Some(&Ok(value)),"a").matches("provider_authorization_lost").count(),1);
    }
    #[test]
    fn service_detail_does_not_claim_readiness_and_preserves_action_failure() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.agents=vec![json!({"id":"a"})];
        panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        local_device_fixture(&mut panel,"a");
        let checking=json!({"service":{"enabled":true,"watcherRunning":true},"runtime":{"agentId":"a","phase":"checking_provider"}});
        panel.apply(&mut cx,Target::Runtime,&checking);
        assert_eq!(panel.view.label(&cx,ids!(runtime_status)).text(),runtime_service_description(Some(&Ok(checking.clone())),"a"));
        panel.runtime_pending_action=Some(true);panel.busy=true;panel.sync_navigation(&mut cx);
        assert_eq!(panel.view.button(&cx,ids!(service_toggle)).text(),"Starting…");
        assert!(!panel.view.button(&cx,ids!(service_toggle)).borrow().unwrap().enabled());
        panel.runtime_pending_action=Some(false);panel.sync_navigation(&mut cx);
        assert_eq!(panel.view.button(&cx,ids!(service_toggle)).text(),"Stopping…");
        panel.runtime_pending_action=None;panel.runtime_action_error=runtime_diagnostic("agent_lease_busy");
        panel.apply(&mut cx,Target::Runtime,&checking);
        assert!(panel.view.label(&cx,ids!(runtime_status)).text().contains("agent_lease_busy"),"polling must retain the failed action diagnostic");
    }
    #[test]
    fn agent_service_can_run_without_a_selected_room_and_stop_drains_legacy_children() {
        let waiting=json!({"runtime":null,"service":{"enabled":true,"running":true,"rooms":[]}});
        assert!(service_enabled(&waiting));
        assert!(runtime_list_label(Some(&Ok(waiting)),"a").contains("Waiting for an available Room"));
        assert!(!service_enabled(&json!({"runtime":{"agentId":"a","phase":"stopped"},"service":{"enabled":false}})));
        assert!(service_enabled(&json!({"runtime":{"agentId":"a","phase":"online_chat_only"},"service":{"enabled":false}})),"legacy workers must remain stoppable before explicit Agent-wide start");
        assert!(!service_enabled(&json!({"runtime":null,"service":{"enabled":false}})));
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.agents=vec![json!({"id":"a"})];
        panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        local_device_fixture(&mut panel,"a");
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(service_toggle)).borrow().unwrap().enabled(),"unknown service state cannot be toggled");
        panel.apply(&mut cx,Target::Runtime,&json!({"runtime":null,"service":{"enabled":false}}));
        panel.sync_navigation(&mut cx);
        assert_eq!(panel.view.button(&cx,ids!(service_toggle)).text(),crate::i18n::tr("Start responding"));
        panel.apply(&mut cx,Target::Runtime,&json!({"runtime":null,"service":{"enabled":true,"rooms":[]}}));
        panel.sync_navigation(&mut cx);
        assert_eq!(panel.view.button(&cx,ids!(service_toggle)).text(),crate::i18n::tr("Stop responding"));
        assert!(panel.view.button(&cx,ids!(service_toggle)).borrow().unwrap().enabled(),"an enabled service remains stoppable without Codex resources or active Rooms");
        assert!(!panel.view.view(&cx,ids!(runtime_start_options)).visible());
        panel.busy=true;
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(service_toggle)).borrow().unwrap().enabled(),"in-flight operation cannot be toggled twice");
    }
    #[test]
    fn token_amount_is_hidden_for_unset_and_unlimited_without_changing_policy() {
        let mut cx=Cx::new(Box::new(|_,_| {}));let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);vm.bx.captured_errors=Some(Vec::new());crate::app::register_widgets(vm);let v=script_eval!(vm,{mod.widgets.HagencyPanel{}});let p=HagencyPanel::script_from_value(vm,v);assert!(vm.take_errors().is_empty());p});
        for limit in [json!("Unset"),json!("Unlimited"),json!({"Tokens":70})] {
            panel.policy=json!({"policies":[{"policy":{"budget":{"limit":limit,"period":"Lifetime"},"requests":"Allow","high_risk":"Deny"}}]});
            panel.render_policy(&mut cx);
            assert_eq!(panel.view.view(&cx,ids!(token_amount_holder)).visible(),limit.is_object());
            assert_eq!(panel.view.view(&cx,ids!(period_holder)).visible(),limit.is_object());
            assert_eq!(panel.view.drop_down(&cx,ids!(limit_mode)).selected_item(),usize::from(limit.is_object()));
            assert_eq!(panel.policy["policies"][0]["policy"]["budget"]["limit"],limit);
        }
    }
    #[test]
    fn agent_run_link_selects_only_its_exact_binding_without_starting() {
        let mut cx=Cx::new(Box::new(|_,_| {}));let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);vm.bx.captured_errors=Some(Vec::new());crate::app::register_widgets(vm);let v=script_eval!(vm,{mod.widgets.HagencyPanel{}});let p=HagencyPanel::script_from_value(vm,v);assert!(vm.take_errors().is_empty());p});
        panel.agents=vec![json!({"id":"a"})];panel.labels(&mut cx,ids!(agents),"Select my Agent",&panel.agents,"id");panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        let bindings=json!({"bindings":[{"id":"other","roomId":"!other:s","state":"active"},{"id":"direct","roomId":"!direct:s","scopeKind":"owner_direct","state":"active"}]});
        // Default selection must not depend on the order returned by the server.
        panel.apply(&mut cx,Target::Bindings,&bindings);
        assert_eq!(panel.binding(&mut cx).as_deref(),Some("direct"));
        // A project Room entry must keep its exact target instead of falling back to the private chat.
        panel.wanted_run=Some(("a".into(),"other".into()));
        panel.apply(&mut cx,Target::Bindings,&bindings);
        assert_eq!(panel.binding(&mut cx).as_deref(),Some("other"));
        assert!(panel.view.view(&cx,ids!(runtime_section)).visible());
        assert!(!panel.view.view(&cx,ids!(room_advanced_content)).visible());
        // Expanding management options is presentation only; it must keep the selected target.
        panel.room_advanced=true; panel.render_room_advanced(&mut cx);
        assert_eq!(panel.binding(&mut cx).as_deref(),Some("other"));

    }
    #[test]
    fn device_metadata_loading_revocation_and_assignment_have_distinct_badges() {
        let local = json!({"id":"a","executionDeviceId":"here","generation":4});
        let remote = json!({"id":"a","executionDeviceId":"there","generation":4});
        let devices = vec![json!({"id":"here","name":"Laptop","revoked":false}),json!({"id":"there","name":"Desktop","revoked":false}),json!({"id":"old","name":"Old laptop","revoked":true})];
        assert_eq!(agent_device_state(&local, "here", &devices, false), AgentDeviceState::Checking);
        assert_eq!(agent_device_state(&local, "here", &devices, true), AgentDeviceState::Here);
        assert_eq!(agent_device_state(&remote, "here", &devices, true), AgentDeviceState::Other);
        assert_eq!(agent_device_state(&json!({"generation":4,"executionDeviceId":null}), "here", &devices, true), AgentDeviceState::Unassigned);
        for id in ["old", "missing"] {assert_eq!(agent_device_state(&json!({"generation":4,"executionDeviceId":id}), "here", &devices, true), AgentDeviceState::Unavailable);}
        let legacy=vec![json!({"id":"dev_12345678","name":"Hagency Desktop"}),json!({"id":"dev_abcdefgh","name":"Hagency Desktop"})];
        assert_eq!(execution_device_name(&legacy,"dev_12345678"),"Hagency Desktop · 12345678");
        assert_ne!(execution_device_name(&legacy,"dev_12345678"),execution_device_name(&legacy,"dev_abcdefgh"));
        assert_eq!(execution_device_name(&devices,"here"),"Laptop");
        assert_eq!(execution_device_name(&devices,"missing"),"missing");
    }
    #[test]
    fn budgets_room_picker_keeps_runtime_target_and_old_revision_separate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            vm.bx.captured_errors = Some(Vec::new());
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, { mod.widgets.HagencyPanel {} });
            let panel = HagencyPanel::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            panel
        });
        panel.agents = vec![json!({"id":"agent-a"})];
        panel.bindings = vec![json!({"id":"direct","roomId":"!direct:server","state":"active"}), json!({"id":"project-room","roomId":"!project:server","state":"active"})];
        panel.labels(&mut cx, ids!(agents), "Select", &panel.agents, "id");
        panel.labels(&mut cx, ids!(bindings), "Select", &panel.bindings, "roomId");
        panel.labels(&mut cx, ids!(policy_binding), "Select", &panel.bindings, "roomId");
        panel.view.drop_down(&cx, ids!(agents)).set_selected_item(&mut cx, 1);
        local_device_fixture(&mut panel, "agent-a");
        panel.section(&mut cx, ids!(policy_section));
        panel.view.drop_down(&cx, ids!(policy_layer)).set_selected_item(&mut cx, 1);
        panel.policy = json!({"agentId":"agent-a","bindingId":"direct","requester":"@alice:server","policies":[],"usage":[{"spent":12,"held":7}]});
        panel.approvals.push(json!({"proposalId":"old-room"}));
        panel.select_policy_room(&mut cx, 2);
        assert!(panel.approvals.is_empty(),"changing Room must discard another Room approval selection");
        panel.sync_navigation(&mut cx);
        assert_eq!(panel.binding(&mut cx).as_deref(), Some("project-room"));
        assert_eq!(panel.view.drop_down(&cx, ids!(policy_binding)).selected_item(), 2);
        assert!(panel.policy.is_null(), "another Room cannot reuse the old policy or expense display");
        assert!(!panel.view.button(&cx, ids!(save_policy)).borrow().unwrap().enabled());
        assert!(panel.view.view(&cx, ids!(policy_room_holder)).visible());
        assert!(!panel.view.view(&cx, ids!(room_controls)).visible(), "budgets must not expose Run's pause/leave card");
        assert!(!panel.view.view(&cx, ids!(requester_holder)).visible());
        assert_eq!(panel.view.label(&cx, ids!(policy_target)).text(), "!project:server");
        panel.view.text_input(&cx, ids!(requester)).set_text(&mut cx, "@alice:server");
        panel.view.drop_down(&cx, ids!(policy_layer)).set_selected_item(&mut cx, 2);
        panel.sync_navigation(&mut cx);
        assert!(panel.view.view(&cx, ids!(requester_holder)).visible());
        assert_eq!(panel.view.label(&cx, ids!(policy_target)).text(), "!project:server · @alice:server");
        panel.view.drop_down(&cx, ids!(policy_layer)).set_selected_item(&mut cx, 0);
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.view(&cx, ids!(policy_room_holder)).visible());
        assert!(!panel.view.view(&cx, ids!(requester_holder)).visible());
        assert!(panel.view.label(&cx, ids!(policy_target)).text().contains("all Rooms"));
        assert_eq!(panel.binding(&mut cx).as_deref(), Some("project-room"), "global layer must not replace Room selection");
    }
    #[test]
    fn registered_resource_editor_keeps_room_targets_and_policy_layers_distinct() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            vm.bx.captured_errors = Some(Vec::new());
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, { mod.widgets.HagencyPanel {} });
            let panel = HagencyPanel::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            panel
        });
        panel.agents = vec![json!({"id":"agent-a"}), json!({"id":"agent-b"})];
        panel.bindings = vec![
            json!({"id":"binding-a", "roomId":"!a:server"}),
            json!({"id":"binding-b", "roomId":"!b:server"}),
        ];
        panel.labels(&mut cx, ids!(agents), "Select", &panel.agents, "id");
        panel.labels(&mut cx, ids!(bindings), "Select", &panel.bindings, "roomId");
        panel
            .view
            .drop_down(&cx, ids!(agents))
            .set_selected_item(&mut cx, 1);
        panel
            .view
            .drop_down(&cx, ids!(bindings))
            .set_selected_item(&mut cx, 1);
        panel
            .view
            .text_input(&cx, ids!(requester))
            .set_text(&mut cx, "@alice:server");
        local_device_fixture(&mut panel, "agent-a");
        panel.policy = json!({"agentId":"agent-a", "bindingId":"binding-a", "requester":"@alice:server",
            "policies":[{"revision":1,"policy":{"budget":{"limit":{"Tokens":900},"period":"Lifetime"},"requests":"Allow","high_risk":"Deny"}},
            {"revision":2,"policy":{"budget":{"limit":{"Tokens":200},"period":"UtcDay"},"requests":"AskOwner","high_risk":"AskOwner"}}],
            "usage":[{"spent":50,"held":20},{"spent":30,"held":10}]});
        panel
            .view
            .drop_down(&cx, ids!(policy_layer))
            .set_selected_item(&mut cx, 0);
        panel.render_policy(&mut cx);
        assert_eq!(panel.input(&mut cx, ids!(token_limit)), "900");
        panel
            .view
            .drop_down(&cx, ids!(policy_layer))
            .set_selected_item(&mut cx, 1);
        panel.render_policy(&mut cx);
        assert_eq!(panel.input(&mut cx, ids!(token_limit)), "200");
        assert_eq!(
            panel
                .view
                .drop_down(&cx, ids!(requests))
                .selected_item(),
            2
        );
        panel.section(&mut cx, ids!(model_section));
        assert!(panel.view.view(&cx, ids!(model_section)).visible());
        assert!(!panel.view.view(&cx, ids!(policy_section)).visible());
        panel
            .view
            .drop_down(&cx, ids!(bindings))
            .set_selected_item(&mut cx, 2);
        panel.sync_navigation(&mut cx);
        assert!(
            !panel
                .view
                .button(&cx, ids!(save_model))
                .borrow()
                .unwrap()
                .enabled()
        );
        assert!(
            !panel
                .view
                .button(&cx, ids!(reset_policy))
                .borrow()
                .unwrap()
                .enabled()
        );
        panel.save_policy(&mut cx);
        assert!(
            !panel.busy,
            "a stale policy must not dispatch a write for the new Room"
        );
        panel.section(&mut cx, ids!(runtime_section));
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.widget(&cx, ids!(leave_confirm)).visible());
        assert!(!panel.view.view(&cx, ids!(room_service_actions)).visible());
        assert!(!panel.view.view(&cx, ids!(retirement_section)).visible());
        assert_eq!(panel.binding(&mut cx).as_deref(), Some("binding-b"));
        panel.section(&mut cx, ids!(setup_section));
        panel.sync_navigation(&mut cx);
        assert!(panel.view.view(&cx, ids!(retirement_section)).visible());
        assert!(panel.view.widget(&cx, ids!(leave_confirm)).visible());
        panel
            .view
            .drop_down(&cx, ids!(agents))
            .set_selected_item(&mut cx, 2);
        panel.save_policy(&mut cx);
        assert!(
            !panel.busy,
            "a stale policy must not dispatch a write for another Agent"
        );
    }

    #[test]
    fn agent_budget_is_editable_without_room_and_admin_controls_require_exact_permission() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, { mod.widgets.HagencyPanel {} });
            HagencyPanel::script_from_value(vm, value)
        });
        panel.agents = vec![json!({"id":"agent-a"})];
        panel.labels(&mut cx, ids!(agents), "Select", &panel.agents, "id");
        panel
            .view
            .drop_down(&cx, ids!(agents))
            .set_selected_item(&mut cx, 1);
        local_device_fixture(&mut panel, "agent-a");
        panel.section(&mut cx, ids!(policy_section));
        panel
            .view
            .drop_down(&cx, ids!(policy_layer))
            .set_selected_item(&mut cx, 0);
        panel.apply(&mut cx, Target::Policy, &json!({"agentId":"agent-a","policy":{"revision":9,"policy":{"budget":{"limit":{"Tokens":700},"period":"Lifetime"},"requests":"Allow","high_risk":"Deny"}},"usage":{"spent":123,"held":20}}));
        panel.sync_navigation(&mut cx);
        assert!(
            panel
                .view
                .button(&cx, ids!(save_policy))
                .borrow()
                .unwrap()
                .enabled()
        );
        assert!(!panel.view.view(&cx, ids!(room_controls)).visible());
        assert!(!panel.view.view(&cx, ids!(requester_holder)).visible());
        assert_eq!(panel.input(&mut cx, ids!(token_limit)), "700");
        assert!(editor_matches(&panel.policy, "agent-a", None, "", 0));
        assert!(!editor_matches(&panel.policy, "agent-b", None, "", 0));
        assert!(!editor_matches(
            &panel.policy,
            "agent-a",
            Some("binding"),
            "@user:server",
            1
        ));
        panel.section(&mut cx, ids!(model_section));
        panel
            .view
            .drop_down(&cx, ids!(policy_layer))
            .set_selected_item(&mut cx, 1);
        local_device_fixture(&mut panel, "agent-a");
        panel.apply(&mut cx,Target::Policy,&json!({"agentId":"agent-a","policy":{"revision":9,"policy":{"budget":{"limit":"Unlimited","period":"Lifetime"},"requests":"Allow","high_risk":"Deny"}},"usage":{"spent":123,"held":20},"modelProfile":{"model":"test-model","workspace_root":"/owned/workspace","credential_ref":"keychain:owned"}}));
        panel.apply(&mut cx,Target::ProviderModels,&json!({"authenticated":true,"credentialRef":"keychain:owned","state":"ready","models":[{"model":"test-model","displayName":"Test model","isDefault":true,"description":"Actual catalog entry","defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"reasoningEffort":"medium"},{"reasoningEffort":"high"}]}]}));
        panel.sync_navigation(&mut cx);
        assert!(
            panel
                .view
                .button(&cx, ids!(save_model))
                .borrow()
                .unwrap()
                .enabled()
        );
        assert!(!panel.view.view(&cx, ids!(room_controls)).visible());
        assert_eq!(panel.policy["modelProfile"]["model"], "test-model");
        panel.policy["agentId"] = "another-agent".into();
        panel.sync_navigation(&mut cx);
        assert!(
            !panel
                .view
                .button(&cx, ids!(save_model))
                .borrow()
                .unwrap()
                .enabled()
        );
        panel.projects = vec![
            json!({"id":"project-a","creationPolicy":"{\"defaultAllow\":true,\"allow\":[],\"deny\":[]}"}),
        ];
        panel.labels(&mut cx, ids!(projects), "Select", &panel.projects, "id");
        panel
            .view
            .drop_down(&cx, ids!(projects))
            .set_selected_item(&mut cx, 1);
        panel.apply(&mut cx,Target::ScopeState,&json!({"projectId":"project-a","roomId":null,"revision":1,"canManagePolicy":false,"servicePaused":false,"creationPolicy":{"defaultAllow":true,"allow":[],"deny":[]}}));
        assert!(!panel.view.view(&cx, ids!(permission_editor)).visible());
        panel.apply(&mut cx,Target::ScopeState,&json!({"projectId":"project-a","roomId":null,"revision":1,"canManagePolicy":true,"servicePaused":true,"creationPolicy":{"defaultAllow":false,"allow":["@alice:server"],"deny":[]}}));
        assert!(panel.view.view(&cx, ids!(permission_editor)).visible());
        assert_eq!(
            panel
                .view
                .drop_down(&cx, ids!(creation_mode))
                .selected_item(),
            1
        );
        assert_eq!(panel.input(&mut cx, ids!(creation_allow)), "@alice:server");
        panel.apply(
            &mut cx,
            Target::ScopeState,
            &json!({"projectId":"project-b","roomId":null,"revision":8,"canManagePolicy":true}),
        );
        assert_eq!(
            panel.scope_state["projectId"], "project-a",
            "late response for another Project must not replace its permission target"
        );
    }
    #[test]
    fn agent_requests_never_block_an_independent_creation() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, { mod.widgets.HagencyPanel {} });
            HagencyPanel::script_from_value(vm, value)
        });
        panel.sync_navigation(&mut cx);
        assert!(
            panel
                .view
                .button(&cx, ids!(create_agent))
                .borrow()
                .unwrap()
                .enabled()
        );
        panel.page=Page::Agents;
        let original = json!({"commands":[{"id":"original-id","operation":"agent.create","state":"unknown","input":{"idempotencyKey":"original-id","projectId":"project-a","roomId":"!a:server","displayName":"Original"}}]});
        let global=json!({"operation":"agent.create","state":"pending","input":{"displayName":"My Agent"}});
        assert!(agent_command_label(&global).contains("My Agent"));
        assert!(!agent_command_details(&global).contains("Room:"));
        assert!(!agent_command_details(&global).contains("Project:"));
        panel.apply(&mut cx, Target::AgentJournal, &original);
        panel.sync_navigation(&mut cx);
        assert!(panel.view.view(&cx, ids!(agent_recovery)).visible());
        assert!(
            panel
                .view
                .button(&cx, ids!(create_agent))
                .borrow()
                .unwrap()
                .enabled(),
            "a pending request in another Room must not block distinct requests"
        );
        assert!(
            panel
                .view
                .button(&cx, ids!(bind_agent))
                .borrow()
                .unwrap()
                .enabled()
        );
        assert_eq!(
            panel.agent_pending_commands[0]["input"],
            original["commands"][0]["input"]
        );
        panel.apply(&mut cx, Target::AgentJournal, &json!({"commands":[]}));
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.view(&cx, ids!(agent_recovery)).visible());
        assert!(
            panel
                .view
                .button(&cx, ids!(create_agent))
                .borrow()
                .unwrap()
                .enabled()
        );
    }
    #[test]
    fn agent_execution_device_blocks_remote_local_settings_and_requires_exact_agent() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            vm.bx.captured_errors = Some(Vec::new());
            crate::app::register_widgets(vm);
            let value = script_eval!(vm,{mod.widgets.HagencyPanel {}});
            let panel = HagencyPanel::script_from_value(vm,value);
            assert!(vm.take_errors().is_empty());
            panel
        });
        panel.agents = vec![json!({"id":"agent-a"}),json!({"id":"agent-b"})];
        panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        panel.apply(&mut cx,Target::AgentDetails,&json!({"agent":{"id":"agent-a","executionDeviceId":"device-other","state":"active","generation":1}}));
        panel.apply(&mut cx,Target::Devices,&json!({"currentDeviceId":"device-here","devices":[{"id":"device-here","name":"Laptop","revoked":false},{"id":"device-other","name":"Desktop","revoked":false},{"id":"revoked","name":"Old","revoked":true}]}));
        panel.sync_navigation(&mut cx);
        assert_eq!(panel.execution_devices.len(),3, "revoked metadata must remain available for device badges");
        assert!(panel.view.view(&cx, ids!(execution_badge)).label(&cx, ids!(device_label)).text().contains("Desktop"));
        assert!(panel.view.label(&cx,ids!(execution_device_details)).text().contains("device-other"));
        assert!(panel.view.label(&cx,ids!(assignment_destination)).text().contains("Laptop"));
        assert!(panel.view.button(&cx, ids!(policy_tab)).borrow().unwrap().enabled(), "remote Agent policy is accessible without moving assignment");
        assert!(panel.view.button(&cx, ids!(model_tab)).borrow().unwrap().enabled());
        assert!(!panel.view.button(&cx, ids!(runtime_tab)).borrow().unwrap().enabled());
        assert!(!panel.view.button(&cx, ids!(assign_current_device)).borrow().unwrap().enabled(), "moving execution requires explicit confirmation");
        panel.view.check_box(&cx, ids!(assignment_confirm)).set_active(&mut cx, true, Animate::No);
        panel.sync_navigation(&mut cx);
        assert!(panel.view.button(&cx, ids!(assign_current_device)).borrow().unwrap().enabled());
        for state in ["retiring", "retired"] {
            panel.execution_agent_record["state"] = state.into();
            panel.sync_navigation(&mut cx);
            assert!(!panel.view.button(&cx, ids!(assign_current_device)).visible(), "{state} Agent cannot move execution");
            assert!(!panel.view.button(&cx, ids!(assign_current_device)).borrow().unwrap().enabled());
            assert!(!panel.view.check_box(&cx, ids!(assignment_confirm)).visible());
        }
        panel.execution_agent_record["state"] = "active".into();
        panel.execution_agent_record["generation"] = 0.into();
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx, ids!(assign_current_device)).visible(), "unconfirmed generation cannot assign");
        panel.execution_agent_record["generation"] = 1.into();
        for state in ["creating", "active", "suspended"] {
            panel.execution_agent_record["state"] = state.into();
            panel.sync_navigation(&mut cx);
            assert!(panel.view.button(&cx, ids!(assign_current_device)).visible());
            assert!(panel.view.button(&cx, ids!(assign_current_device)).borrow().unwrap().enabled());
        }

        panel.policy = json!({"agentId":"agent-a","scope":"agent","modelLoaded":true});
        panel.local(&mut cx, Target::Policy, LocalAction::AgentPolicy, None);
        assert!(!panel.busy, "remote Agent never initiates a local resource request");
        assert!(panel.view.widget(&cx, ids!(execution_devices)).is_empty());
        assert!(panel.view.widget(&cx, ids!(execution_name)).is_empty());
        panel.apply(&mut cx, Target::AgentAssigned, &json!({"agent":{"id":"agent-a","executionDeviceId":"device-here","state":"active","generation":2}}));
        assert_eq!(panel.agents[0]["executionDeviceId"], "device-here");
        panel.sync_navigation(&mut cx);
        assert!(panel.view.view(&cx,ids!(execution_badge)).label(&cx,ids!(device_label)).text().contains("Laptop"));
        assert!(panel.policy.is_null(), "moving execution must not reuse or copy former resource policy");
        assert!(!panel.runtime_resources_checked);
        assert!(!panel.view.check_box(&cx, ids!(assignment_confirm)).active(&cx));

        assert!(!panel.view.button(&cx,ids!(service_toggle)).borrow().unwrap().enabled());
        panel.apply(&mut cx,Target::AgentDetails,&json!({"agent":{"id":"agent-a","executionDeviceId":"device-here","state":"active","generation":2}}));
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(service_toggle)).borrow().unwrap().enabled(),"assignment alone does not prove Codex and Room readiness");
        assert!(panel.view.label(&cx,ids!(start_requirements)).text().contains("Model & resources"));
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,2);
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(service_toggle)).borrow().unwrap().enabled());
        panel.apply(&mut cx,Target::AgentDetails,&json!({"agent":{"id":"agent-a","executionDeviceId":"device-here","state":"active","generation":3}}));
        assert_eq!(panel.execution_agent_record["generation"],2,"late Agent response for another Agent cannot overwrite current target");
        panel.bindings = vec![json!({"id":"direct","roomId":"!private:server","scopeKind":"owner_direct","projectId":null,"state":"suspended"}),json!({"id":"project-room","roomId":"!project:server","scopeKind":"project","projectId":"project-a","state":"active"})];
        panel.labels(&mut cx,ids!(bindings),"Select",&panel.bindings,"roomId");
        panel.view.drop_down(&cx,ids!(bindings)).set_selected_item(&mut cx,1);
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(room_roster_check)).visible());
        assert!(!panel.view.button(&cx,ids!(room_roster_check)).borrow().unwrap().enabled());
        assert!(panel.view.label(&cx,ids!(room_roster)).text().contains("private chat"));
        assert!(panel.view.button(&cx,ids!(resume_binding)).borrow().unwrap().enabled(),"private binding resumes by binding ID, without a synthetic Project");
        panel.view.drop_down(&cx,ids!(bindings)).set_selected_item(&mut cx,2);
        panel.sync_navigation(&mut cx);
        assert!(panel.view.button(&cx,ids!(room_roster_check)).visible());

    }
    #[test]
    fn room_reply_policy_defaults_to_mentions_and_tracks_only_selected_binding() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.bindings = vec![
            json!({"id":"direct","scopeKind":"owner_direct","state":"active"}),
            json!({"id":"room-a","scopeKind":"project","projectId":"project","state":"active","threadAutoReply":false}),
            json!({"id":"room-b","scopeKind":"project","projectId":"project","state":"active","threadAutoReply":true})];
        panel.labels(&mut cx,ids!(bindings),"Select",&panel.bindings,"id");
        panel.view.drop_down(&cx,ids!(bindings)).set_selected_item(&mut cx,1);
        panel.sync_navigation(&mut cx);
        assert!(!panel.view.view(&cx,ids!(reply_policy)).visible());
        panel.view.drop_down(&cx,ids!(bindings)).set_selected_item(&mut cx,2);
        panel.sync_navigation(&mut cx);
        assert!(panel.view.view(&cx,ids!(reply_policy)).visible());
        assert!(panel.view.label(&cx,ids!(reply_policy_status)).text().contains("Every message"));
        panel.apply(&mut cx,Target::ReplyPolicyChanged,&json!({"binding":{"id":"room-a","scopeKind":"project","projectId":"project","state":"active","threadAutoReply":true}}));
        panel.sync_navigation(&mut cx);
        assert_eq!(panel.view.drop_down(&cx,ids!(bindings)).selected_item(),2);
        assert!(panel.view.label(&cx,ids!(reply_policy_status)).text().contains("Everyone"));
        panel.apply(&mut cx,Target::ReplyPolicyChanged,&json!({"binding":{"id":"room-b","scopeKind":"project","projectId":"project","state":"active","threadAutoReply":false}}));
        panel.sync_navigation(&mut cx);
        assert!(panel.view.label(&cx,ids!(reply_policy_status)).text().contains("Everyone"),"another Room response cannot change this Room's behavior");
        panel.busy=true;panel.sync_navigation(&mut cx);
        assert!(!panel.view.button(&cx,ids!(reply_policy_toggle)).borrow().unwrap().enabled());
    }

    #[test]
    fn direct_chat_setup_keeps_created_identity_and_only_opens_confirmed_same_agent_chat() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.agents=vec![json!({"id":"agent-a"}),json!({"id":"agent-b"})];
        panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        panel.agent_command=Some((json!({"displayName":"Created"}),"original-create-key".into()));
        panel.apply(&mut cx,Target::AgentCreated,&json!({"creation":{"agent":{"id":"agent-a"}}}));
        assert!(panel.agent_command.is_none(),"confirmed Agent creation is retained independently of contact setup");
        assert_eq!(panel.wanted_agent.as_deref(),Some("agent-a"));
        assert_eq!(panel.direct_agent,"agent-a");
        panel.apply(&mut cx,Target::OwnerDirectCreated,&json!({"ownerDirect":{"state":"pending","phase":"unknown","roomId":"!private:server"}}));
        assert!(!panel.view.button(&cx,ids!(open_owner_direct)).borrow().unwrap().enabled());
        assert_eq!(panel.wanted_agent.as_deref(),Some("agent-a"));
        panel.apply(&mut cx,Target::OwnerDirect,&json!({"ownerDirect":{"state":"pending","phase":"agent_provisioning","lastError":null}}));
        assert_eq!(panel.direct_retry.as_ref().map(|(_,agent,n)|(agent.as_str(),*n)),Some(("agent-a",1)));
        panel.apply(&mut cx,Target::OwnerDirect,&json!({"ownerDirect":{"state":"pending","phase":"server_joining","lastError":null}}));
        assert_eq!(panel.direct_retry.as_ref().map(|(_,_,n)|*n),Some(2));
        panel.apply(&mut cx,Target::OwnerDirect,&json!({"ownerDirect":{"state":"pending","phase":"creating","lastError":"owner_direct_creation_outcome_unknown"}}));
        assert!(panel.direct_retry.is_none(),"unknown create must not be silently retried");
        panel.direct_retry=Some((panel.identity.clone(),"agent-a".into(),8));
        panel.apply(&mut cx,Target::OwnerDirect,&json!({"ownerDirect":{"state":"pending","phase":"server_joining","lastError":null}}));
        assert!(panel.direct_retry.is_none(),"automatic retries are finite");
        panel.apply(&mut cx,Target::OwnerDirect,&json!({"ownerDirect":{"state":"active","phase":"complete","roomId":"!private:server","bindingId":"direct-binding"}}));
        assert!(panel.view.button(&cx,ids!(open_owner_direct)).borrow().unwrap().enabled());
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,2);
        panel.render_owner_direct(&mut cx);
        assert!(!panel.view.button(&cx,ids!(open_owner_direct)).borrow().unwrap().enabled());
    }
    #[test]
    fn entering_run_reads_saved_resources_without_reauth_and_uses_their_actual_model_choices() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.HagencyPanel {}});
            let panel = HagencyPanel::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); panel
        });
        panel.page = Page::Agents; panel.agents = vec![json!({"id":"exact-agent"})];
        panel.labels(&mut cx, ids!(agents), "Select my Agent", &panel.agents.clone(), "id");
        panel.view.drop_down(&cx, ids!(agents)).set_selected_item(&mut cx, 1);
        panel.section(&mut cx, ids!(runtime_section));
        local_device_fixture(&mut panel, "exact-agent");
        panel.load_runtime_resources(&mut cx);
        panel.sync_navigation(&mut cx);
        assert!(panel.runtime_resources_loading); assert!(!panel.runtime_resources_checked);
        assert!(panel.view.label(&cx, ids!(start_requirements)).text().contains("Checking"));
        assert!(!panel.view.button(&cx, ids!(service_toggle)).borrow().unwrap().enabled());
        let saved = json!({"model":"existing-model","workspace_root":"/existing-owner-workspace","credential_ref":"existing-owner-reference"});
        panel.apply(&mut cx, Target::Policy, &json!({"agentId":"exact-agent","policy":{"revision":1},"usage":{"spent":0,"held":0},"modelProfile":saved}));
        assert!(panel.runtime_resources_loading, "saved profile alone is not proof that Codex is connected");
        panel.apply(&mut cx, Target::ProviderModels, &json!({"authenticated":true,"shared":true,"credentialSource":"local_codex","credentialRef":"existing-owner-reference","models":[{"model":"existing-model","isDefault":true,"defaultReasoningEffort":"low","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"medium"}]}]}));
        assert!(panel.runtime_resources_checked); assert!(!panel.runtime_resources_loading);
        assert!(panel.provider_authenticated); assert_eq!(panel.policy["modelProfile"], saved, "read initialization never overwrites the saved profile");
        assert_eq!(panel.runtime_efforts, vec!["low","medium"]);
        assert_eq!(panel.view.drop_down(&cx, ids!(effort)).selected_item(), 0);
        assert!(!panel.view.button(&cx, ids!(provider_login)).visible());
        assert!(!panel.view.button(&cx, ids!(provider_logout)).visible());
        assert!(!panel.busy, "registration tests contain no SDK account or executable model request");
    }
    #[tokio::test]
    async fn management_wait_expires_at_45_seconds_without_cancelling_original_work() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.HagencyPanel {}});
            HagencyPanel::script_from_value(vm, value)
        });
        panel.active = true; panel.busy = true; panel.identity = "same-account".into(); panel.request = nonce();
        let request = panel.request;
        let (release, waiting) = tokio::sync::oneshot::channel();
        // Simulates a bootstrap already having side effects. UI expiration
        // must neither abort its cleanup nor cause an automatic second call.
        let original = tokio::spawn(async move {waiting.await.unwrap(); Completed {identity:"same-account".into(),request,target:Target::Agents,configuration_fence:None,configuration_digest:None,result:Ok(json!({"agents":[]}))}});
        let before = std::time::Instant::now(); panel.arm_read_wait(&mut cx, &Command::Agents);
        let deadline = panel.read_wait.as_ref().unwrap().deadline;
        assert!(deadline >= before + std::time::Duration::from_secs(45));
        assert!(!panel.expire_read_wait(&mut cx, deadline - std::time::Duration::from_nanos(1)));
        assert!(panel.busy); assert!(panel.expire_read_wait(&mut cx, deadline));
        assert!(!panel.busy); assert!(panel.read_wait.is_none());
        assert!(!original.is_finished(), "UI timeout leaves the original future running");
        release.send(()).unwrap(); let done = original.await.unwrap();
        assert!(!current_completion(panel.active, &panel.identity, panel.request, &done), "late old response cannot repopulate this view");
        assert!(panel.agents.is_empty());
        panel.busy = true; panel.arm_read_wait(&mut cx, &Command::CreateAgent {input:json!({"idempotencyKey":"unchanged"})});
        assert!(panel.read_wait.is_none(), "mutations never receive a read watchdog");
    }
    #[test]
    fn expired_old_account_wait_does_not_unlock_or_replace_a_new_account_view() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.HagencyPanel {}});
            HagencyPanel::script_from_value(vm, value)
        });
        panel.active = true; panel.busy = true; panel.identity = "old-account".into(); panel.request = nonce();
        panel.arm_read_wait(&mut cx, &Command::Agents); let deadline = panel.read_wait.as_ref().unwrap().deadline;
        panel.identity = "new-account".into(); panel.request = nonce(); let new_request = panel.request;
        assert!(!panel.expire_read_wait(&mut cx, deadline));
        assert!(panel.busy); assert_eq!(panel.request, new_request);
    }
    #[test]
    fn agent_direct_navigation_selects_chats_but_project_binding_keeps_project_context() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        for direct in [true, false] {
            let room = matrix_sdk::ruma::OwnedRoomId::try_from("!exact-room:server").unwrap();
            let actions = cx.capture_actions(|cx| navigate_agent_chat(cx, room.clone(), direct, Some(("owner-pin", "project-a"))));
            assert_eq!(actions.iter().filter(|a| matches!(a.downcast_ref::<crate::home::navigation_tab_bar::NavigationBarAction>(), Some(crate::home::navigation_tab_bar::NavigationBarAction::GoToTab(crate::home::navigation_tab_bar::SelectedTab::Contacts)))).count(), usize::from(direct));
            let projects: Vec<_> = actions.iter().filter_map(|a| a.downcast_ref::<ProjectRoomNavigation>()).collect();
            assert_eq!(projects.len(), usize::from(!direct));
            if !direct {assert_eq!(projects[0].project_id, "project-a");assert_eq!(projects[0].identity, "owner-pin");}
            assert!(!actions.iter().any(|a| a.downcast_ref::<HagencyAction>().is_some()), "opening a binding must not replace its chat with management");
            let destinations: Vec<_> = actions.iter().filter_map(|a| match a.downcast_ref::<crate::app::AppStateAction>() {
                Some(crate::app::AppStateAction::NavigateToRoom {destination_room, ..}) => Some(destination_room.room_id().to_owned()),
                _ => None,
            }).collect();
            assert_eq!(destinations, if direct {vec![]} else {vec![room]});
        }
    }
    #[test]
    fn empty_status_does_not_reserve_space_above_agent_controls() {
        use makepad_widgets::makepad_draw::cx_draw::CxDraw;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.HagencyPanel {}});
            let panel = HagencyPanel::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); panel
        });
        panel.page = Page::Agents;
        panel.view.view(&cx, ids!(agents_page)).set_visible(&mut cx, true);
        panel.view.view(&cx, ids!(agent_detail)).set_visible(&mut cx, true);
        panel.view.view(&cx, ids!(agent_list_holder)).set_visible(&mut cx, false);
        panel.section(&mut cx, ids!(model_section));
        let mut positions = Vec::new();
        for message in ["", "Loading…", ""] {
            panel.status(&mut cx, message);
            cx.redraw_all(); let pass = DrawPass::new(&mut cx); pass.set_size(&mut cx, dvec2(980.0, 900.0));
            let mut list = DrawList2d::new(&mut cx); let overlay = Overlay {draw_list: DrawList::new(&mut cx)};
            let event = DrawEvent::default(); let mut draw = CxDraw::new(&mut cx, &event); let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None); list.begin_always(&mut cx2d); overlay.begin(&mut cx2d); cx2d.begin_root_turtle(dvec2(980.0, 900.0), Layout::flow_overlay());
            panel.draw_walk_all(&mut cx2d, &mut Scope::empty(), Walk {width: Size::Fixed(980.0), height: Size::Fixed(860.0), ..Walk::default()});
            overlay.end(&mut cx2d); cx2d.end_pass_sized_turtle(); list.end(&mut cx2d); cx2d.end_pass(&pass);
            drop(cx2d); drop(draw);
            assert!(!panel.view.inline_notice(&cx, ids!(status_holder)).is_visible());
            assert_eq!(panel.view.widget(&cx, ids!(loading_status)).visible(), message == "Loading…");
            positions.push(panel.view.view(&cx, ids!(projects_page)).area().rect(&cx).pos.y);
        }
        assert_eq!(positions[0], positions[2]);
        assert_eq!(positions[1], positions[0], "loading in the title row must not move the tabs");
    }
    #[test]
    fn provider_diagnostics_are_explicit_but_never_render_untrusted_details() {
        for code in ["local_codex_configuration_unsupported", "codex_binary_unavailable", "provider_directory_not_canonical", "provider_keyring_unavailable", "codex_model_catalog_unavailable", "codex_protocol_mismatch"] {
            let message = visible_error(code);
            assert!(message.contains(code), "approved static diagnostic is visible");
            assert!(!message.starts_with(code), "explain the problem before the code");
            assert_ne!(user_error(code), user_error("unknown_error"));
        }
        for unsafe_detail in ["/Users/owner/private/auth.json", "{\"token\":\"secret\"}", "codex_provider_unavailable /Users/owner/secret"] {
            let message = visible_error(unsafe_detail);
            assert!(!message.contains(unsafe_detail));
            assert!(!message.contains("secret"));
            assert!(!message.contains("/Users"));
        }
    }
    #[test]
    fn management_failures_keep_codes_and_explain_creation_failures() {
        for (code, detail) in [
            ("invalid_agent_name", "1–64"),
            ("invalid_arguments", "invalid fields"),
            ("idempotency_key_reused", "different settings"),
            ("agent_command_state_unavailable", "local request state"),
            ("invalid_server_response", "unexpected response"),
            ("appservice_startup_not_ready", "not ready"),
            ("storage_unavailable", "database"),
            ("domain_unavailable", "configuration"),
        ] {
            let message = visible_error(code);
            assert!(message.contains(detail), "{message}");
            assert!(message.ends_with(&format!("({code})")), "{message}");
        }
        // A newer server may return a code this client does not yet explain.
        assert!(visible_error("future_agent_failure").contains("(future_agent_failure)"));
        assert!(visible_error("forbidden").contains("(forbidden)"));
        for detail in ["", "bad\ncode", "code token=secret", "{\"code\":\"secret\"}", "/private/auth.json", "错误", &"x".repeat(81)] {
            assert_eq!(visible_error(detail), crate::i18n::tr(user_error("unknown_error")));
        }
    }
    #[test]
    fn fresh_agent_form_clears_previous_failure_and_keeps_other_requests() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);
            let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});
            HagencyPanel::script_from_value(vm,value)
        });
        panel.page=Page::Agents;
        panel.agent_pending_commands=vec![json!({"id":"old","operation":"agent.create","state":"unknown","input":{"displayName":"Previous"}})];
        panel.agent_command=Some((json!({"displayName":"Previous"}),"old-key".into()));
        panel.show_failure(&mut cx,Target::AgentCreated,"invalid_arguments",&visible_error("invalid_arguments"));
        let notice=panel.view.inline_notice(&cx,ids!(status_holder));
        assert!(notice.is_visible());
        assert!(!notice.message().contains("invalid_arguments"),"diagnostic code belongs in Details");
        assert!(!notice.message().contains("unconfirmed"),"a rejection is not presented as an unknown result");
        panel.begin_agent_creation(&mut cx);panel.sync_navigation(&mut cx);
        assert!(!notice.is_visible());assert!(panel.agent_command.is_none());
        assert_eq!(panel.agent_pending_commands.len(),1);
        assert!(panel.view.button(&cx,ids!(create_agent)).borrow().unwrap().enabled());
        assert!(panel.input(&mut cx,ids!(agent_name)).is_empty());
        panel.agent_creating=true;panel.set_busy(&mut cx,true);panel.sync_navigation(&mut cx);
        assert_eq!(panel.view.button(&cx,ids!(create_agent)).text(),crate::i18n::tr("Creating…"));
        assert!(panel.view.widget(&cx,ids!(agent_create_spinner)).visible());
        assert!(!panel.view.button(&cx,ids!(create_agent)).borrow().unwrap().enabled());
    }
    #[test]
    fn agent_empty_state_requires_a_successful_list_and_creation_placeholder_tracks_kind() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.HagencyPanel {}});
            let panel = HagencyPanel::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); panel
        });
        assert!(!agent_empty_message(panel.agents_loaded, panel.agents_loading).contains("No Agents"));
        panel.agents_loading = true;
        assert_eq!(agent_empty_message(panel.agents_loaded, panel.agents_loading), "Loading your Agents…");
        // A failed load stops the spinner without declaring the account empty.
        panel.agents_loading = false;
        assert!(agent_empty_message(panel.agents_loaded, panel.agents_loading).contains("could not be loaded"));
        panel.apply(&mut cx, Target::Agents, &json!({"agents":[], "projects":[]}));
        assert!(agent_empty_message(panel.agents_loaded, panel.agents_loading).starts_with("No Agents yet"));
        panel.create_room = true; panel.creation_source(&mut cx);
        assert_eq!(panel.view.text_input(&cx, ids!(creation_name)).empty_text(), "Room name");
        panel.create_room = false; panel.creation_source(&mut cx);
        assert_eq!(panel.view.text_input(&cx, ids!(creation_name)).empty_text(), "Project name");
    }
    #[test]
    fn agent_resources_draw_without_recovery_or_setup_and_model_choices_are_real() {
        use makepad_widgets::makepad_draw::cx_draw::CxDraw;
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm| {makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});let panel=HagencyPanel::script_from_value(vm,value);assert!(vm.take_errors().is_empty());panel});
        panel.page=Page::Agents;
        panel.view.view(&cx,ids!(agents_page)).set_visible(&mut cx,true);
        panel.view.view(&cx,ids!(agent_detail)).set_visible(&mut cx,true);
        panel.view.view(&cx,ids!(agent_list_holder)).set_visible(&mut cx,false);
        panel.agent_pending_commands=vec![json!({"id":"original","state":"unknown"})];
        panel.agents = vec![json!({"id":"agent-a"})];
        panel.labels(&mut cx, ids!(agents), "Select", &panel.agents, "id");
        panel.view.drop_down(&cx, ids!(agents)).set_selected_item(&mut cx, 1);
        local_device_fixture(&mut panel, "agent-a");
        panel.section(&mut cx,ids!(model_section));
        panel.policy=json!({"agentId":"a","scope":"agent","modelLoaded":true,"modelProfile":{"model":"catalog-b","reasoning_effort":"medium"}});
        panel.apply(&mut cx,Target::ProviderModels,&json!({"authenticated":true,"credentialRef":"safe-reference","models":[{"model":"catalog-a","displayName":"First","isDefault":true,"defaultReasoningEffort":"low","supportedReasoningEfforts":[{"reasoningEffort":"low"}]},{"model":"catalog-b","displayName":"Second","description":"Real catalog description","defaultReasoningEffort":"high","supportedReasoningEfforts":[{"reasoningEffort":"medium"},{"reasoningEffort":"high"}]}]}));
        assert_eq!(panel.view.drop_down(&cx,ids!(model)).selected_item(),2,"saved exact model takes priority over provider default");
        assert_eq!(panel.runtime_efforts,vec!["medium","high"]);
        assert_eq!(panel.view.drop_down(&cx,ids!(effort)).selected_item(),0,"saved reasoning effort takes priority over the model default");
        assert!(!panel.view.button(&cx,ids!(provider_login)).visible());
        assert!(panel.view.button(&cx,ids!(provider_logout)).visible());
        {
        cx.redraw_all();let pass=DrawPass::new(&mut cx);pass.set_size(&mut cx,dvec2(980.0,900.0));let mut list=DrawList2d::new(&mut cx);let overlay=Overlay {draw_list:DrawList::new(&mut cx)};let event=DrawEvent::default();let mut draw=CxDraw::new(&mut cx,&event);let mut cx2d=Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass,None);list.begin_always(&mut cx2d);overlay.begin(&mut cx2d);cx2d.begin_root_turtle(dvec2(980.0,900.0),Layout::flow_overlay());
        panel.draw_walk_all(&mut cx2d,&mut Scope::empty(),Walk {width:Size::Fixed(980.0),height:Size::Fixed(860.0),..Walk::default()});
        overlay.end(&mut cx2d);cx2d.end_pass_sized_turtle();list.end(&mut cx2d);cx2d.end_pass(&pass);
        }
        assert!(!panel.view.view(&cx,ids!(agent_recovery)).visible());
        assert!(!panel.view.view(&cx,ids!(setup_section)).visible());
        assert!(panel.view.view(&cx,ids!(model_section)).area().rect(&cx).size.y>0.0);
        assert!(!panel.view.view(&cx,ids!(advanced_settings)).visible(),"workspace is hidden by default");
        assert!(panel.view.drop_down(&cx,ids!(effort)).area().rect(&cx).size.y>0.0,"model reasoning settings are visible on the Codex page");
        panel.model_advanced=true;panel.render_model_advanced(&mut cx);
        assert!(panel.view.view(&cx,ids!(advanced_settings)).visible());
        panel.model_advanced=false;panel.render_model_advanced(&mut cx);
        panel.apply(&mut cx,Target::Provider,&json!({"authenticated":true,"state":"ready","shared":true,"credentialSource":"local_codex"}));
        assert!(!panel.view.button(&cx,ids!(provider_logout)).visible());
        assert!(!panel.view.button(&cx,ids!(provider_login)).visible());
        assert!(panel.view.button(&cx,ids!(disconnect_provider)).visible());
        assert!(!panel.view.button(&cx,ids!(use_local_provider)).visible());
        panel.apply(&mut cx,Target::Provider,&json!({"authenticated":false,"state":"signed_out"}));
        assert!(panel.view.button(&cx,ids!(provider_login)).visible());
        assert!(!panel.view.button(&cx,ids!(provider_logout)).visible());
        panel.apply(&mut cx,Target::ProviderModels,&json!({"authenticated":false,"models":[]}));
        assert!(panel.provider_models.is_empty());assert!(panel.runtime_efforts.is_empty());
    }
    #[test]
    fn completed_space_creation_opens_fresh_room_form_but_unknown_keeps_original_intent() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.creation=Some(json!({"phase":"complete","commandId":"completed-space","spaceId":"!space:server"}));panel.pending_input=Some(json!({"kind":"space","name":"Old Project","commandId":"completed-space"}));
        panel.view.text_input(&cx,ids!(creation_name)).set_text(&mut cx,"Old Project");panel.view.label(&cx,ids!(creation_result)).set_text(&mut cx,"complete · !space:server");panel.view.button(&cx,ids!(create_project_room)).set_enabled(&mut cx,false);
        assert!(panel.begin_creation_form(&mut cx,true));assert!(panel.create_room);assert!(panel.creation.is_none());assert!(panel.pending_input.is_none());assert!(panel.input(&mut cx,ids!(creation_name)).is_empty());assert!(panel.view.label(&cx,ids!(creation_result)).text().is_empty());assert!(panel.view.button(&cx,ids!(create_project_room)).borrow().unwrap().enabled());
        let original=json!({"kind":"room","commandId":"original-room","name":"Original","projectId":"project-a","spaceId":"!space:server"});panel.pending_input=Some(original.clone());panel.creation=Some(json!({"phase":"unknown","commandId":"original-room"}));
        assert!(!panel.begin_creation_form(&mut cx,false));assert_eq!(panel.pending_input.as_ref(),Some(&original));assert_eq!(panel.creation.as_ref().unwrap()["commandId"],"original-room");assert!(panel.view.view(&cx,ids!(recovery)).visible());
        panel.creation=Some(json!({"phase":"complete","commandId":"original-room"}));assert!(panel.begin_creation_form(&mut cx,false));assert!(!panel.create_room);assert!(panel.pending_input.is_none());
    }
    #[test]
    fn native_management_widget_registers_and_instantiates_without_script_errors() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            vm.bx.captured_errors = Some(Vec::new());
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, { mod.widgets.HagencyPanel {} });
            let _panel = HagencyPanel::script_from_value(vm, value);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "Makepad registration errors: {errors:?}");
        });
    }
    #[test]
    fn management_responses_require_same_account_epoch_and_open_request() {
        let done = Completed {
            identity: "server/account/epoch1".into(),
            request: 7,
            target: Target::Changed,
            configuration_fence: None,
            configuration_digest: None,
            result: Ok(Value::Null),
        };
        assert!(current_completion(true, "server/account/epoch1", 7, &done));
        assert!(!current_completion(true, "server/account/epoch2", 7, &done));
        assert!(!current_completion(true, "server/account/epoch1", 8, &done));
        assert!(!current_completion(
            false,
            "server/account/epoch1",
            7,
            &done
        ));
    }
    #[test]
    fn policy_revision_cannot_be_reused_for_another_requester_or_room() {
        let p = json!({"agentId":"agent", "bindingId":"room-a", "requester":"@alice:server"});
        assert!(policy_matches(&p, "agent", "room-a", "@alice:server"));
        assert!(!policy_matches(&p, "agent", "room-a", "@bob:server"));
        assert!(!policy_matches(&p, "agent", "room-b", "@alice:server"));
        assert!(!policy_matches(
            &p,
            "other-agent",
            "room-a",
            "@alice:server"
        ));
    }
    #[test]
    fn uncertain_matrix_creation_only_resumes_original_id_and_input() {
        let input = json!({"commandId":"original-id","kind":"room","name":"Original","projectId":"project-a"});
        assert!(
            matches!(recovery_command(input.clone(), "unknown", String::new()), Command::ResumeCreation { id, room_id: None } if id == "original-id")
        );
        assert!(
            matches!(recovery_command(input.clone(), "room_created", "!known:server".into()), Command::ResumeCreation { id, room_id: Some(room) } if id == "original-id" && room == "!known:server")
        );
        assert!(
            matches!(recovery_command(input.clone(), "unrecorded", String::new()), Command::CreateMatrix { input: original } if original == input)
        );
    }
    #[test]
    fn confirmed_create_and_adopt_refresh_reads_without_replaying_mutations() {
        assert!(matches!(
            mutation_refresh(Target::AgentCreated),
            Some(Command::Agents)
        ));
        assert!(matches!(
            mutation_refresh(Target::AgentBound),
            Some(Command::Agents)
        ));
        assert!(matches!(
            mutation_refresh(Target::ProjectAdopted),
            Some(Command::Projects)
        ));
        assert!(matches!(
            mutation_refresh(Target::RoomAdopted),
            Some(Command::Projects)
        ));
        assert!(mutation_refresh(Target::Changed).is_none());
        assert!(mutation_refresh(Target::Policy).is_none());
        assert!(mutation_refresh(Target::Creation).is_none());
    }
    #[test]
    fn native_confirmed_mutations_close_forms_but_policy_saves_keep_selection() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, { mod.widgets.HagencyPanel {} });
            HagencyPanel::script_from_value(vm, value)
        });
        panel.projects = vec![json!({"id":"project-original","spaceId":"!space:server"})];
        panel.labels(&mut cx, ids!(projects), "Select", &panel.projects, "id");
        panel
            .view
            .drop_down(&cx, ids!(projects))
            .set_selected_item(&mut cx, 1);
        panel
            .view
            .view(&cx, ids!(project_form))
            .set_visible(&mut cx, true);
        panel.apply(&mut cx, Target::RoomAdopted, &Value::Null);
        assert!(!panel.view.view(&cx, ids!(project_form)).visible());
        assert_eq!(panel.wanted_project.as_deref(), Some("project-original"));
        panel
            .view
            .view(&cx, ids!(agent_create))
            .set_visible(&mut cx, true);
        panel.agent_command = Some((json!({"displayName":"A"}), "same-id".into()));
        panel.apply(&mut cx, Target::Changed, &Value::Null);
        assert!(panel.view.view(&cx, ids!(agent_create)).visible());
        assert_eq!(
            panel.agent_command.as_ref().map(|v| v.1.as_str()),
            Some("same-id")
        );
        panel.apply(&mut cx, Target::AgentCreated, &Value::Null);
        assert!(!panel.view.view(&cx, ids!(agent_create)).visible());
        assert!(panel.agent_command.is_none());
    }
    #[test]
    fn project_empty_state_requires_a_successful_directory_read() {
        assert_eq!(project_empty_message(false,true),crate::i18n::tr("Loading projects…"));
        assert_eq!(project_empty_message(false,false),crate::i18n::tr("Projects could not be loaded. Refresh to retry."));
        assert!(project_empty_message(true,false).starts_with("No Projects yet"));
    }
    #[test]
    fn portal_empty_state_appears_once_and_real_rows_never_become_placeholders() {
        let templates = (0..8).map(|i| card_template(0, i)).collect::<Vec<_>>();
        assert_eq!(templates.iter().filter(|&&id| id == id!(Empty)).count(), 1);
        assert!(templates[1..].iter().all(|&id| id == id!(Filler)));
        assert_eq!(card_template(2, 0), id!(Card));
        assert_eq!(card_template(2, 1), id!(Card));
        assert_eq!(card_template(2, 2), id!(Filler));
        assert_eq!(card_template(2, 7), id!(Filler));
    }
    #[test]
    fn registered_portal_card_clicks_and_back_return_to_the_same_list() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, { mod.widgets.HagencyPanel {} });
            HagencyPanel::script_from_value(vm, value)
        });
        panel.projects = vec![
            json!({"id":"project-a","name":"A"}),
            json!({"id":"project-b","name":"B"}),
        ];
        panel.agents = vec![
            json!({"id":"agent-a","displayName":"Agent A"}),
            json!({"id":"agent-b","displayName":"Agent B"}),
        ];
        panel.labels(&mut cx, ids!(projects), "Select", &panel.projects, "id");
        panel.labels(&mut cx, ids!(agents), "Select", &panel.agents, "id");
        for page in [Page::Projects, Page::Agents] {
            panel.set_page(&mut cx, page);
            panel.list_page(&mut cx);
            panel.sync_navigation(&mut cx);
            assert!(!panel.view.button(&cx, ids!(back)).visible());
            assert!(panel.view.button(&cx, ids!(close)).borrow().is_none());
            assert!(panel.view.button(&cx, ids!(new_item)).visible());
            assert!(panel.view.button(&cx, ids!(refresh)).visible());
            let list = panel.view.portal_list(
                &cx,
                if page == Page::Projects {
                    ids!(project_list)
                } else {
                    ids!(agent_list)
                },
            );
            let row = list.item(&mut cx, 1, id!(Card));
            assert!(!row.is_empty());
            let button = row.button(&cx, ids!(open));
            assert!(
                button.borrow().is_some(),
                "registered card must contain a real Button"
            );
            let actions = cx.capture_actions(|cx| {
                cx.group_widget_actions(list.widget_uid(), row.widget_uid(), |cx| {
                    cx.widget_action(
                        button.widget_uid(),
                        ButtonAction::Clicked(KeyModifiers::default()),
                    );
                });
            });
            assert!(panel.open_card(&mut cx, &actions));
            panel.sync_navigation(&mut cx);
            assert!(panel.view.button(&cx, ids!(back)).visible());
            assert!(panel.view.button(&cx, ids!(close)).borrow().is_none());
            assert!(!panel.view.button(&cx, ids!(new_item)).visible());
            assert!(!panel.view.button(&cx, ids!(refresh)).visible());
            let (selection, detail, list_id) = if page == Page::Projects {
                (
                    ids!(projects),
                    ids!(project_detail),
                    ids!(project_list_holder),
                )
            } else {
                (ids!(agents), ids!(agent_detail), ids!(agent_list_holder))
            };
            assert_eq!(panel.view.drop_down(&cx, selection).selected_item(), 2);
            assert!(panel.view.view(&cx, detail).visible());
            assert!(!panel.view.view(&cx, list_id).visible());
            let back = panel.view.button(&cx, ids!(back));
            let actions = cx.capture_actions(|cx| {
                cx.widget_action(back.widget_uid(), ButtonAction::Clicked(KeyModifiers::default()));
            });
            assert!(panel.back_to_list(&mut cx, &actions));
            panel.sync_navigation(&mut cx);
            assert!(!panel.view.button(&cx, ids!(back)).visible());
            assert!(panel.view.view(&cx, list_id).visible());
            assert!(!panel.view.view(&cx, detail).visible());
            assert!(panel.view.button(&cx, ids!(close)).borrow().is_none());
        }
    }
    #[test]
    fn project_and_room_members_have_independent_targets_permissions_and_late_result_fences() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.HagencyPanel {}});
            HagencyPanel::script_from_value(vm, value)
        });
        let space = json!({"roomId":"!space:server","space":true,"canInvite":true,"members":[
            {"userId":"@joined:server","membership":"join","role":"Administrator","power":"100"},
            {"userId":"@invited:server","membership":"invite","role":"User","power":"0"}]});
        panel.members_room = Some(("!space:server".into(), true));
        panel.apply(&mut cx, Target::Members, &space);
        assert_eq!(panel.members.len(), 2);
        assert!(
            panel
                .view
                .button(&cx, ids!(invite_member))
                .borrow()
                .unwrap()
                .enabled()
        );
        assert_eq!(member_summary(panel.members.iter().find(|m|m["membership"]=="invite").unwrap()), "Invited · Member");
        panel.clear_members(&mut cx);
        panel.members_room = Some(("!private:server".into(), false));
        panel.apply(&mut cx, Target::Members, &space);
        assert!(
            panel.members.is_empty(),
            "Space members must not populate a private Room"
        );
        assert!(
            !panel
                .view
                .button(&cx, ids!(invite_member))
                .borrow()
                .unwrap()
                .enabled()
        );
        let room = json!({"roomId":"!private:server","space":false,"canInvite":false,"members":[{"userId":"@room-only:server","membership":"join"}]});
        panel.apply(&mut cx, Target::Members, &room);
        assert_eq!(panel.members.len(), 1);
        assert_eq!(string(&panel.members[0], "userId"), "@room-only:server");
        panel.clear_members(&mut cx);
        panel.apply(&mut cx, Target::Members, &room);
        assert!(
            panel.members.is_empty(),
            "clearing an account/target blocks late members"
        );
        assert!(user_error("room_join_required").contains("independent membership"));
    }
    #[test]
    fn project_menu_section_waits_for_its_own_project_rooms() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.HagencyPanel {}});
            HagencyPanel::script_from_value(vm, value)
        });
        panel.projects = vec![json!({"id":"project-a","name":"Design","spaceId":"!space:server"})];
        panel.labels(&mut cx, ids!(projects), "Select", &panel.projects, "id");
        panel
            .view
            .drop_down(&cx, ids!(projects))
            .set_selected_item(&mut cx, 1);
        panel.wanted_project_section = Some(("project-a".into(), ProjectDetailSection::Rooms));
        panel.project_detail(&mut cx, 0);
        assert!(panel.view.view(&cx, ids!(rooms_section)).visible(), "requested tab is visible during loading too");
        assert!(!panel.view.view(&cx, ids!(overview_section)).visible());
        panel.apply(&mut cx, Target::Rooms, &json!({"rooms":[]}));
        assert!(panel.view.view(&cx, ids!(rooms_section)).visible());
        assert!(!panel.view.view(&cx, ids!(overview_section)).visible());
        assert!(panel.wanted_project_section.is_none());
        panel.project_section(&mut cx, ids!(overview_section));
        panel.wanted_project_section = Some(("project-b".into(), ProjectDetailSection::Members));
        panel.apply(&mut cx, Target::Rooms, &json!({"rooms":[]}));
        assert!(panel.view.view(&cx, ids!(overview_section)).visible());
        assert!(!panel.view.view(&cx, ids!(members_section)).visible());
        assert!(
            panel.members_room.is_none(),
            "another project's response cannot start a members request"
        );
    }

    #[test]
    fn project_detail_sections_are_exclusive_and_preserve_project_location() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.HagencyPanel {}});
            HagencyPanel::script_from_value(vm, value)
        });
        panel.projects = vec![json!({"id":"project","name":"Design","spaceId":"!space:server"})];
        panel.labels(&mut cx, ids!(projects), "Select", &panel.projects, "id");
        panel.project_detail(&mut cx, 0);
        assert_eq!(
            panel
                .view
                .drop_down(&cx, ids!(projects))
                .selected_item(),
            1
        );
        for selected in [
            ids!(overview_section),
            ids!(rooms_section),
            ids!(members_section),
        ] {
            panel.project_section(&mut cx, selected);
            for id in [
                ids!(overview_section),
                ids!(rooms_section),
                ids!(members_section),
            ] {
                assert_eq!(panel.view.view(&cx, id).visible(), id == selected);
            }
            assert!(panel.view.view(&cx, ids!(project_detail)).visible());
            assert!(
                !panel
                    .view
                    .view(&cx, ids!(project_list_holder))
                    .visible()
            );
        }
    }
    #[test]
    fn project_encryption_choice_only_appears_for_new_rooms_and_unknown_intents_stay_frozen() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});let panel=HagencyPanel::script_from_value(vm,value);assert!(vm.take_errors().is_empty());panel});
        assert!(panel.begin_creation_form(&mut cx,false));panel.creation_source(&mut cx);
        assert!(!panel.view.view(&cx,ids!(creation_encryption)).visible());
        panel.project_home=json!({"canCreateRoom":true});panel.open_room_creation(&mut cx);
        assert!(panel.view.view(&cx,ids!(creation_encryption)).visible());
        panel.view.drop_down(&cx,ids!(creation_source)).set_selected_item(&mut cx,1);panel.creation_source(&mut cx);
        assert!(!panel.view.view(&cx,ids!(creation_encryption)).visible(),"linking an existing Room must not claim to change its encryption");
        let original=json!({"kind":"room","commandId":"encrypted-room","encrypted":true});
        panel.pending_input=Some(original.clone());panel.creation=Some(json!({"phase":"unknown"}));panel.sync_navigation(&mut cx);
        assert!(panel.view.widget(&cx,ids!(creation_encrypted)).disabled(&cx));
        assert!(panel.view.check_box(&cx,ids!(creation_encrypted)).active(&cx));
        assert!(!panel.begin_creation_form(&mut cx,true));
        assert_eq!(panel.pending_input,Some(original.clone()));
        assert!(matches!(recovery_command(original,"unrecorded",String::new()),Command::CreateMatrix{input} if input["encrypted"]==true));
        panel.creation=Some(json!({"phase":"complete"}));
        assert!(panel.begin_creation_form(&mut cx,true));
        assert!(!panel.view.check_box(&cx,ids!(creation_encrypted)).active(&cx));
    }

}
