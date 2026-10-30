//! Closed user Matrix creation workflow. No model caller, AS token, room deletion
//! or automatic retry of an uncertain POST createRoom. The private command journal
//! is scoped to the native installation's pinned origin/MXID/Pasion subject.
use super::{OwnerError, OwnerOperation, OwnerReply, ServerLogin, http, origin, session_key};
use crate::console::{body, console, cookie, current, recheck, same_origin};
use reqwest::{Method, Url};
use salvo::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
const MARKER: &str = "im.hagency.client.creation";
static COMMANDS: Mutex<()> = Mutex::const_new(());
fn error(status: u16, code: &str) -> OwnerError {
    OwnerError::new(status, code)
}
pub(super) fn room_id(room: &str) -> Result<(), OwnerError> {
    if room.starts_with('!')
        && room.len() <= 255
        && !room
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || ['/', '?', '#', '\\'].contains(&c))
    {
        Ok(())
    } else {
        Err(error(400, "invalid_room_id"))
    }
}
pub(super) fn segment(room: &str) -> Result<String, OwnerError> {
    room_id(room)?;
    let mut url = Url::parse("https://matrix.invalid/").unwrap();
    url.path_segments_mut().unwrap().push(room);
    Ok(url.path()[1..].into())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Space,
    Room,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub command_id: String,
    pub kind: Kind,
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub encrypted: bool,
    pub project_id: Option<String>,
    pub space_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Command {
    input: Input,
    origin: String,
    owner: String,
    subject: String,
    room_id: Option<String>,
    phase: String,
    project: Option<Value>,
    last_error: Option<String>,
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Resume {
    room_id: Option<String>,
}
// This enum is deliberately private: browser input never becomes arbitrary paths,
// state types, request bodies, authorization headers, or a remote proxy.
pub(in crate::console) enum MatrixOperation {
    DirectCreate {
        agent: String,
        owner: String,
        puppet: String,
        name: String,
    },
    DirectAlias {
        agent: String,
        owner: String,
    },
    DirectIndexGet(String),
    DirectIndexPut(String, Value),
    Create(Input, String, String),
    Discovery,
    JoinedRooms,
    State(String),
    Child(String, String, Value),
    Parent(String, String, Value),
}
impl MatrixOperation {
    fn request(self) -> Result<(Method, String, Option<Value>), OwnerError> {
        Ok(match self {
            Self::DirectCreate {
                agent,
                owner,
                puppet,
                name,
            } => {
                super::operation_id(&agent)?;
                let server = owner
                    .split_once(':')
                    .filter(|(local, server)| local.starts_with('@') && !server.is_empty())
                    .ok_or_else(|| error(400, "invalid_arguments"))?
                    .1;
                if !puppet.starts_with('@')
                    || puppet.split_once(':').map(|(_, s)| s) != Some(server)
                {
                    return Err(error(400, "invalid_arguments"));
                }
                (
                    Method::POST,
                    "/_matrix/client/v3/createRoom".into(),
                    Some(
                        json!({"name":name,"visibility":"private","preset":"private_chat","is_direct":true,"room_alias_name":format!("hagency-agent-{agent}"),"invite":[puppet],"initial_state":[{"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}},{"type":"m.room.history_visibility","state_key":"","content":{"history_visibility":"joined"}},{"type":"m.room.guest_access","state_key":"","content":{"guest_access":"forbidden"}},{"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}},{"type":"im.hagency.agent.owner_direct","state_key":"","content":{"version":1,"agentId":agent,"ownerMxid":owner}}]}),
                    ),
                )
            }
            Self::DirectAlias { agent, owner } => {
                super::operation_id(&agent)?;
                let server = owner
                    .split_once(':')
                    .filter(|(local, server)| local.starts_with('@') && !server.is_empty())
                    .ok_or_else(|| error(400, "invalid_arguments"))?
                    .1;
                let alias = format!("#hagency-agent-{agent}:{server}");
                let mut url = Url::parse("https://matrix.invalid/").unwrap();
                url.path_segments_mut().unwrap().push(&alias);
                (
                    Method::GET,
                    format!("/_matrix/client/v3/directory/room/{}", &url.path()[1..]),
                    None,
                )
            }
            Self::DirectIndexGet(owner) => {
                let mut url = Url::parse("https://matrix.invalid/").unwrap();
                url.path_segments_mut().unwrap().push(&owner);
                (
                    Method::GET,
                    format!(
                        "/_matrix/client/v3/user/{}/account_data/m.direct",
                        &url.path()[1..]
                    ),
                    None,
                )
            }
            Self::DirectIndexPut(owner, index) => {
                if !index.is_object() {
                    return Err(error(400, "invalid_arguments"));
                }
                let mut url = Url::parse("https://matrix.invalid/").unwrap();
                url.path_segments_mut().unwrap().push(&owner);
                (
                    Method::PUT,
                    format!(
                        "/_matrix/client/v3/user/{}/account_data/m.direct",
                        &url.path()[1..]
                    ),
                    Some(index),
                )
            }
            Self::Create(input, owner, service) => {
                let mut marker = json!({"commandId":input.command_id,"owner":owner,"kind":input.kind,"projectId":input.project_id,"spaceId":input.space_id});
                if input.encrypted {
                    marker["encrypted"] = json!(true);
                }
                let mut body = json!({"name":input.name,"visibility":"private","preset":"private_chat","invite":[service],"initial_state":[{"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}},{"type":MARKER,"state_key":"","content":marker}]});
                if input.encrypted {
                    if input.kind != Kind::Room {
                        return Err(error(400, "invalid_arguments"));
                    }
                    body["initial_state"].as_array_mut().unwrap().push(json!({"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}}));
                }
                if input.kind == Kind::Space {
                    body["creation_content"] = json!({"type":"m.space"});
                }
                (
                    Method::POST,
                    "/_matrix/client/v3/createRoom".into(),
                    Some(body),
                )
            }
            Self::Discovery => (Method::GET, "/hagency/api/v1/discovery".into(), None),
            Self::JoinedRooms => (Method::GET, "/_matrix/client/v3/joined_rooms".into(), None),
            Self::State(room) => (
                Method::GET,
                format!("/_matrix/client/v3/rooms/{}/state", segment(&room)?),
                None,
            ),
            Self::Child(space, room, content) => (
                Method::PUT,
                format!(
                    "/_matrix/client/v3/rooms/{}/state/m.space.child/{}",
                    segment(&space)?,
                    segment(&room)?
                ),
                Some(content),
            ),
            Self::Parent(room, space, content) => (
                Method::PUT,
                format!(
                    "/_matrix/client/v3/rooms/{}/state/m.space.parent/{}",
                    segment(&room)?,
                    segment(&space)?
                ),
                Some(content),
            ),
        })
    }
}
impl ServerLogin {
    /// Discover only Rooms returned by this user's OAuth joined_rooms endpoint.
    /// The cursor is the last scanned ID, including failures/non-Space Rooms.
    pub(crate) async fn space_candidates(
        &self,
        cookie: &str,
        cursor: Option<String>,
    ) -> Result<Value, OwnerError> {
        let projects = self.owner_api(cookie, OwnerOperation::Projects).await?;
        let joined = self
            .matrix_api(cookie, MatrixOperation::JoinedRooms)
            .await?;
        if !same_reply_identity(&projects, &joined) {
            return Err(error(401, "sign_in_required"));
        }
        let ids = candidate_ids(&joined.value, cursor.as_deref())?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut spaces = Vec::new();
        let mut errors = Vec::new();
        let mut scanned = 0;
        let mut last = None;
        for id in &ids {
            if scanned >= 128 || spaces.len() >= 32 || Instant::now() >= deadline {
                break;
            }
            scanned += 1;
            last = Some(id.clone());
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                self.matrix_api(cookie, MatrixOperation::State(id.clone())),
            )
            .await;
            match result {
                Ok(Ok(reply)) => {
                    if !same_reply_identity(&projects, &reply) {
                        return Err(error(401, "sign_in_required"));
                    }
                    match candidate_space(id, &reply.value, &projects.owner, &projects.value) {
                        Ok(Some(space)) => spaces.push(space),
                        Ok(None) => {}
                        Err(e) => errors.push(json!({"roomId":id,"code":e.code})),
                    }
                }
                Ok(Err(e)) if e.status == 401 => return Err(e),
                Ok(Err(e)) => errors.push(json!({"roomId":id,"code":e.code})),
                Err(_) => errors.push(json!({"roomId":id,"code":"matrix_state_timeout"})),
            }
        }
        let check = self.owner_api(cookie, OwnerOperation::Projects).await?;
        if !same_reply_identity(&projects, &check) {
            return Err(error(401, "sign_in_required"));
        }
        let next = (scanned < ids.len()).then_some(last).flatten();
        Ok(
            json!({"spaces":spaces,"nextCursor":next,"incomplete":!errors.is_empty(),"errors":errors}),
        )
    }
    async fn matrix_identity(&self, cookie: &str) -> Result<(String, String, String), OwnerError> {
        let _ = self.owner_api(cookie, OwnerOperation::Projects).await?;
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(&session_key(cookie))
            .ok_or_else(|| error(401, "sign_in_required"))?;
        Ok((
            session.binding.origin.clone(),
            session
                .binding
                .owner
                .clone()
                .ok_or_else(|| error(401, "sign_in_required"))?,
            session
                .binding
                .subject
                .clone()
                .ok_or_else(|| error(401, "sign_in_required"))?,
        ))
    }
    pub(in crate::console) async fn matrix_api(
        &self,
        cookie: &str,
        op: MatrixOperation,
    ) -> Result<OwnerReply, OwnerError> {
        let (method, path, body) = op.request()?;
        if self.stopped.load(Ordering::Acquire) {
            return Err(error(401, "sign_in_required"));
        }
        let renewal = self.renewal.lock().await;
        let key = session_key(cookie);
        self.renew_session_inner(&key).await.map_err(|failure| {
            let unavailable = matches!(failure, super::Error::Unavailable);
            error(
                if unavailable { 503 } else { 401 },
                if unavailable {
                    "matrix_authorization_unavailable"
                } else {
                    "sign_in_required"
                },
            )
        })?;
        let session = self
            .sessions
            .lock()
            .await
            .get(&key)
            .cloned()
            .filter(|s| !s.invalidated && s.expires > Instant::now())
            .ok_or_else(|| error(401, "sign_in_required"))?;
        drop(renewal);
        let shared_token = if let Some(source) = &session.matrix_source {
            let snapshot = source.access_token().await.map_err(|failure| {
                let code = super::super::native::sdk_source_failure(failure);
                error(
                    if code.ends_with("unavailable") {
                        503
                    } else {
                        401
                    },
                    code,
                )
            })?;
            if snapshot.client_id != session.binding.client_id {
                return Err(error(401, "matrix_account_changed"));
            }
            Some(snapshot.access_token)
        } else {
            None
        };
        let server = origin(&session.binding.origin).map_err(|_| error(401, "sign_in_required"))?;
        let mut request = http()
            .map_err(|_| error(503, "matrix_unavailable"))?
            .request(method, server.join(&path).unwrap())
            .bearer_auth(shared_token.as_ref().unwrap_or(&session.oauth_token));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| error(503, "matrix_request_outcome_unknown"))?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| error(503, "matrix_request_outcome_unknown"))?
        {
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(error(502, "invalid_matrix_response"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| error(502, "invalid_matrix_response"))?;
        if !status.is_success() {
            return Err(error(
                status.as_u16(),
                if status.as_u16() == 403 {
                    "matrix_permission_denied"
                } else {
                    "matrix_request_failed"
                },
            ));
        }
        if let Some(source) = &session.matrix_source {
            let after = source.access_token().await.map_err(|failure| {
                let code = super::super::native::sdk_source_failure(failure);
                error(
                    if code.ends_with("unavailable") {
                        503
                    } else {
                        401
                    },
                    code,
                )
            })?;
            if after.client_id != session.binding.client_id {
                return Err(error(401, "matrix_account_changed"));
            }
        }
        let sessions = self.sessions.lock().await;
        let current = sessions.values().find(|s| s.token == session.token);
        if current.is_none_or(|s| {
            s.invalidated || s.expires <= Instant::now() || s.oauth_expires <= Instant::now()
        }) || self.stopped.load(Ordering::Acquire)
        {
            return Err(error(401, "sign_in_required"));
        }
        Ok(OwnerReply {
            owner: session
                .binding
                .owner
                .clone()
                .ok_or_else(|| error(401, "sign_in_required"))?,
            origin: session.binding.origin.clone(),
            issuer: format!("{}auth/", session.binding.origin),
            subject: session
                .binding
                .subject
                .clone()
                .ok_or_else(|| error(401, "sign_in_required"))?,
            value,
        })
    }
}
fn load(path: &Path) -> Result<Vec<Command>, OwnerError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = super::read_private_json(path, 1024 * 1024)
        .map_err(|_| error(503, "local_creation_journal_unavailable"))?;
    let records: Vec<Command> = serde_json::from_slice(&bytes)
        .map_err(|_| error(503, "local_creation_journal_unavailable"))?;
    if records.len() > 256 {
        return Err(error(503, "local_creation_journal_full"));
    }
    Ok(records)
}
fn save(path: &Path, records: &[Command]) -> Result<(), OwnerError> {
    let bytes = serde_json::to_vec(records)
        .map_err(|_| error(503, "local_creation_journal_unavailable"))?;
    if bytes.len() > 1024 * 1024 {
        return Err(error(503, "local_creation_journal_full"));
    }
    hagency_store::private::replace(path, &bytes)
        .map_err(|_| error(503, "local_creation_journal_unavailable"))
}
fn service_identity(discovery: &Value, owner: &str) -> Result<String, OwnerError> {
    let server = owner
        .split_once(':')
        .filter(|(local, server)| local.starts_with('@') && !server.is_empty())
        .ok_or_else(|| error(502, "invalid_owner_identity"))?
        .1;
    let service = format!("@_hagency_service:{server}");
    if discovery["serverName"] != server || discovery["serviceMxid"] != service {
        return Err(error(502, "invalid_service_identity"));
    }
    Ok(service)
}
fn validate_input(input: &Input) -> Result<(), OwnerError> {
    super::operation_id(&input.command_id)?;
    if input.name.trim().is_empty()
        || input.name.chars().count() > 128
        || input.name.chars().any(char::is_control)
    {
        return Err(error(400, "invalid_arguments"));
    }
    match input.kind {
        Kind::Space
            if input.project_id.is_none() && input.space_id.is_none() && !input.encrypted =>
        {
            Ok(())
        }
        Kind::Room => {
            super::operation_id(
                input
                    .project_id
                    .as_deref()
                    .ok_or_else(|| error(400, "invalid_arguments"))?,
            )?;
            room_id(
                input
                    .space_id
                    .as_deref()
                    .ok_or_else(|| error(400, "invalid_arguments"))?,
            )
        }
        _ => Err(error(400, "invalid_arguments")),
    }
}
fn content<'a>(state: &'a Value, kind: &str, key: &str) -> Option<&'a Value> {
    state
        .as_array()?
        .iter()
        .find(|e| e["type"] == kind && e["state_key"] == key)
        .map(|e| &e["content"])
}
fn created_by(state: &Value, command: &Command) -> bool {
    let Some(events) = state.as_array() else {
        return false;
    };
    let mut creates = events
        .iter()
        .filter(|e| e["type"] == "m.room.create" && e["state_key"] == "");
    let mut markers = events
        .iter()
        .filter(|e| e["type"] == MARKER && e["state_key"] == "");
    let (Some(create), Some(marker)) = (creates.next(), markers.next()) else {
        return false;
    };
    if creates.next().is_some() || markers.next().is_some() {
        return false;
    }
    let encryption_matches = if command.input.encrypted {
        let mut encryption = events
            .iter()
            .filter(|e| e["type"] == "m.room.encryption" && e["state_key"] == "");
        encryption
            .next()
            .is_some_and(|e| e["content"]["algorithm"] == "m.megolm.v1.aes-sha2")
            && encryption.next().is_none()
    } else {
        true
    };
    let creator = create["content"]
        .get("creator")
        .is_none_or(|v| v == &command.owner);
    let mut expected_marker = json!({"commandId":command.input.command_id,"owner":command.owner,"kind":command.input.kind,"projectId":command.input.project_id,"spaceId":command.input.space_id});
    if command.input.encrypted {
        expected_marker["encrypted"] = json!(true);
    }
    create["sender"] == command.owner
        && creator
        && marker["sender"] == command.owner
        && marker["content"] == expected_marker
        && (create["content"]["type"] == "m.space") == (command.input.kind == Kind::Space)
        && encryption_matches
        && content(state, "m.room.join_rules", "").is_some_and(|c| c["join_rule"] == "invite")
}
fn public(command: &Command) -> Value {
    json!({"commandId":command.input.command_id,"kind":command.input.kind,"name":command.input.name,"encrypted":command.input.encrypted,"projectId":command.input.project_id,"spaceId":command.input.space_id,"roomId":command.room_id,"phase":command.phase,"project":command.project,"lastError":command.last_error})
}
async fn checked_project(
    login: &ServerLogin,
    cookie: &str,
    input: &Input,
) -> Result<(), OwnerError> {
    if input.kind == Kind::Room {
        let projects = login.owner_api(cookie, OwnerOperation::Projects).await?;
        if !projects.value["projects"].as_array().is_some_and(|p| {
            p.iter().any(|p| {
                Some(p["id"].as_str().unwrap_or("")) == input.project_id.as_deref()
                    && Some(p["spaceId"].as_str().unwrap_or("")) == input.space_id.as_deref()
            })
        }) {
            return Err(error(403, "project_space_mismatch"));
        }
    }
    Ok(())
}
async fn continue_known(
    login: &ServerLogin,
    cookie: &str,
    command: &mut Command,
) -> Result<(), OwnerError> {
    checked_project(login, cookie, &command.input).await?;
    let room = command
        .room_id
        .clone()
        .ok_or_else(|| error(409, "matrix_creation_outcome_unknown"))?;
    let state = login
        .matrix_api(cookie, MatrixOperation::State(room.clone()))
        .await?
        .value;
    if !created_by(&state, command) {
        return Err(error(409, "matrix_creation_proof_mismatch"));
    }
    if command.input.kind == Kind::Space {
        command.project = Some(
            login
                .owner_api(cookie, OwnerOperation::AdoptProject { space: room })
                .await?
                .value["project"]
                .clone(),
        );
    } else {
        let space = command.input.space_id.clone().unwrap();
        let space_state = login
            .matrix_api(cookie, MatrixOperation::State(space.clone()))
            .await?
            .value;
        let host = command
            .owner
            .split_once(':')
            .ok_or_else(|| error(502, "invalid_owner_identity"))?
            .1;
        let child = json!({"via":[host],"suggested":false});
        if let Some(existing) = content(&space_state, "m.space.child", &room) {
            if existing != &child {
                return Err(error(409, "matrix_space_child_conflict"));
            }
        } else {
            login
                .matrix_api(
                    cookie,
                    MatrixOperation::Child(space.clone(), room.clone(), child),
                )
                .await?;
        }
        let parent = json!({"via":[host],"canonical":true});
        if let Some(existing) = content(&state, "m.space.parent", &space) {
            if existing != &parent {
                return Err(error(409, "matrix_space_parent_conflict"));
            }
        } else {
            login
                .matrix_api(cookie, MatrixOperation::Parent(room.clone(), space, parent))
                .await?;
        }
        login
            .owner_api(
                cookie,
                OwnerOperation::AdoptRoom {
                    project: command.input.project_id.clone().unwrap(),
                    room,
                },
            )
            .await?;
    }
    command.phase = "complete".into();
    command.last_error = None;
    Ok(())
}
pub(crate) fn router() -> Router {
    Router::with_path("matrix-creations")
        .get(creations)
        .post(start)
        .push(
            Router::with_path("{commandId}")
                .get(creation_status)
                .push(Router::with_path("resume").post(resume)),
        )
}
async fn context<'a>(
    req: &Request,
    depot: &'a Depot,
    mutating: bool,
) -> Result<(&'a ServerLogin, String, PathBuf, (String, String, String)), OwnerError> {
    if req.uri().query().is_some() || !same_origin(req, depot, mutating) {
        return Err(error(403, "invalid_origin"));
    }
    current(req, depot).map_err(|_| error(401, "sign_in_required"))?;
    let console = console(depot).map_err(|_| error(503, "local_state_unavailable"))?;
    let cookie = cookie(req)
        .map_err(|_| error(401, "sign_in_required"))?
        .to_owned();
    let login = &console.0.server_login;
    let identity = login.matrix_identity(&cookie).await?;
    let root = login
        .state_directory()
        .ok_or_else(|| error(503, "local_state_unavailable"))?;
    let ledger = crate::console::owned_agents::ledger_path(
        &root,
        &identity.0,
        &format!("{}auth/", identity.0),
        &identity.2,
        &identity.1,
    )
    .map_err(|_| error(503, "local_state_unavailable"))?;
    let path = ledger.parent().unwrap().join("matrix-creations.json");
    Ok((login, cookie, path, identity))
}
fn finish(res: &mut Response, value: Result<Value, OwnerError>) {
    match value {
        Ok(value) => res.render(Json(value)),
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(json!({"code":error.code})));
        }
    }
}
#[handler]
async fn start(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    finish(res, do_start(req, depot).await);
}
async fn do_start(req: &mut Request, depot: &Depot) -> Result<Value, OwnerError> {
    let bytes = body(req, 16384)
        .await
        .map_err(|_| error(400, "invalid_arguments"))?;
    let input: Input =
        serde_json::from_slice(&bytes).map_err(|_| error(400, "invalid_arguments"))?;
    validate_input(&input)?;
    let (login, cookie, path, identity) = context(req, depot, true).await?;
    let _guard = COMMANDS.lock().await;
    let mut records = load(&path)?;
    if let Some(old) = records
        .iter()
        .find(|c| c.input.command_id == input.command_id)
    {
        if old.input != input
            || (&old.origin, &old.owner, &old.subject) != (&identity.0, &identity.1, &identity.2)
        {
            return Err(error(409, "creation_command_conflict"));
        }
        return Ok(json!({"creation":public(old)}));
    }
    if records.len() >= 256 {
        return Err(error(503, "local_creation_journal_full"));
    }
    checked_project(login, &cookie, &input).await?;
    recheck(depot).map_err(|_| error(401, "sign_in_required"))?;
    let discovery = login
        .matrix_api(&cookie, MatrixOperation::Discovery)
        .await?
        .value;
    let service = service_identity(&discovery, &identity.1)?;
    let mut command = Command {
        input: input.clone(),
        origin: identity.0,
        owner: identity.1,
        subject: identity.2,
        room_id: None,
        phase: "creating".into(),
        project: None,
        last_error: None,
    };
    records.push(command.clone());
    save(&path, &records)?;
    let created = login
        .matrix_api(
            &cookie,
            MatrixOperation::Create(input, command.owner.clone(), service),
        )
        .await;
    match created {
        Ok(reply) => {
            if let Some(room) = reply.value["room_id"]
                .as_str()
                .filter(|room| room_id(room).is_ok())
            {
                command.room_id = Some(room.into());
                command.phase = "created".into();
            } else {
                command.phase = "unknown".into();
                command.last_error = Some("invalid_matrix_response".into());
            }
        }
        Err(failure) => {
            command.phase = "unknown".into();
            command.last_error = Some(failure.code);
        }
    }
    *records.last_mut().unwrap() = command.clone();
    save(&path, &records)?;
    if command.room_id.is_some() {
        if let Err(failure) = continue_known(login, &cookie, &mut command).await {
            command.phase = "partial".into();
            command.last_error = Some(failure.code);
        }
        *records.last_mut().unwrap() = command.clone();
        save(&path, &records)?;
    }
    recheck(depot).map_err(|_| error(401, "sign_in_required"))?;
    Ok(json!({"creation":public(&command)}))
}
#[handler]
async fn creation_status(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    finish(res, do_status(req, depot).await);
}
async fn do_status(req: &Request, depot: &Depot) -> Result<Value, OwnerError> {
    let (_, _, path, identity) = context(req, depot, false).await?;
    let command_id = req
        .param::<String>("commandId")
        .ok_or_else(|| error(400, "invalid_arguments"))?;
    super::operation_id(&command_id)?;
    let _guard = COMMANDS.lock().await;
    let records = load(&path)?;
    let command = records
        .iter()
        .find(|c| c.input.command_id == command_id)
        .ok_or_else(|| error(404, "creation_command_not_found"))?;
    if (&command.origin, &command.owner, &command.subject)
        != (&identity.0, &identity.1, &identity.2)
    {
        return Err(error(403, "creation_identity_mismatch"));
    }
    Ok(json!({"creation":public(command)}))
}
#[handler]
async fn resume(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    finish(res, do_resume(req, depot).await);
}
async fn do_resume(req: &mut Request, depot: &Depot) -> Result<Value, OwnerError> {
    let raw = body(req, 4096)
        .await
        .map_err(|_| error(400, "invalid_arguments"))?;
    let input: Resume = if raw.is_empty() {
        Resume::default()
    } else {
        serde_json::from_slice(&raw).map_err(|_| error(400, "invalid_arguments"))?
    };
    let (login, cookie, path, identity) = context(req, depot, true).await?;
    let id = req
        .param::<String>("commandId")
        .ok_or_else(|| error(400, "invalid_arguments"))?;
    super::operation_id(&id)?;
    let _guard = COMMANDS.lock().await;
    let mut records = load(&path)?;
    let index = records
        .iter()
        .position(|c| c.input.command_id == id)
        .ok_or_else(|| error(404, "creation_command_not_found"))?;
    let mut command = records[index].clone();
    if (&command.origin, &command.owner, &command.subject)
        != (&identity.0, &identity.1, &identity.2)
    {
        return Err(error(403, "creation_identity_mismatch"));
    }
    if command.phase == "complete" {
        return Ok(json!({"creation":public(&command)}));
    }
    if command.room_id.is_none() {
        let candidates = if let Some(room) = input.room_id {
            room_id(&room)?;
            vec![room]
        } else {
            let joined = login
                .matrix_api(&cookie, MatrixOperation::JoinedRooms)
                .await?
                .value;
            let rooms = joined["joined_rooms"]
                .as_array()
                .filter(|r| r.len() <= 64)
                .ok_or_else(|| error(409, "creation_recovery_room_id_required"))?;
            rooms
                .iter()
                .map(|r| {
                    r.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| error(502, "invalid_matrix_response"))
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut matches = Vec::new();
        for room in candidates {
            if let Ok(state) = login
                .matrix_api(&cookie, MatrixOperation::State(room.clone()))
                .await
                && created_by(&state.value, &command)
            {
                matches.push(room);
            }
        }
        if matches.len() != 1 {
            return Err(error(409, "matrix_creation_outcome_unknown"));
        }
        command.room_id = matches.pop();
        command.phase = "created".into();
        records[index] = command.clone();
        save(&path, &records)?;
    }
    recheck(depot).map_err(|_| error(401, "sign_in_required"))?;
    if let Err(failure) = continue_known(login, &cookie, &mut command).await {
        command.phase = "partial".into();
        command.last_error = Some(failure.code);
    }
    records[index] = command.clone();
    save(&path, &records)?;
    recheck(depot).map_err(|_| error(401, "sign_in_required"))?;
    Ok(json!({"creation":public(&command)}))
}

#[cfg(test)]
mod tests;

fn same_reply_identity(a: &OwnerReply, b: &OwnerReply) -> bool {
    a.origin == b.origin && a.owner == b.owner && a.issuer == b.issuer && a.subject == b.subject
}
fn candidate_ids(value: &Value, cursor: Option<&str>) -> Result<Vec<String>, OwnerError> {
    let rooms = value["joined_rooms"]
        .as_array()
        .ok_or_else(|| error(502, "invalid_matrix_response"))?;
    if rooms.len() > 4096 {
        return Err(error(409, "space_discovery_capacity_exceeded"));
    }
    if let Some(cursor) = cursor {
        room_id(cursor)?;
    }
    let mut ids = std::collections::BTreeSet::new();
    for room in rooms {
        let id = room
            .as_str()
            .ok_or_else(|| error(502, "invalid_matrix_response"))?;
        room_id(id)?;
        ids.insert(id.to_owned());
    }
    Ok(ids
        .into_iter()
        .filter(|id| cursor.is_none_or(|c| id.as_str() > c))
        .collect())
}
fn candidate_space(
    id: &str,
    state: &Value,
    owner: &str,
    projects: &Value,
) -> Result<Option<Value>, OwnerError> {
    let state = state
        .as_array()
        .ok_or_else(|| error(502, "invalid_matrix_response"))?;
    let content = |kind: &str, key: &str| {
        state
            .iter()
            .find(|e| e["type"] == kind && e["state_key"] == key)
            .and_then(|e| e.get("content"))
    };
    if !content("m.room.create", "").is_some_and(|c| c["type"] == "m.space")
        || !content("m.room.member", owner).is_some_and(|c| c["membership"] == "join")
    {
        return Ok(None);
    }
    let name = content("m.room.name", "").and_then(|c| c["name"].as_str());
    let topic = content("m.room.topic", "").and_then(|c| c["topic"].as_str());
    if name.is_some_and(|s| s.len() > 1024) || topic.is_some_and(|s| s.len() > 4096) {
        return Err(error(502, "invalid_matrix_response"));
    }
    let project = projects["projects"]
        .as_array()
        .ok_or_else(|| error(502, "invalid_server_response"))?
        .iter()
        .find(|p| p["spaceId"] == id)
        .and_then(|p| p["id"].as_str());
    Ok(Some(
        json!({"spaceId":id,"name":name,"topic":topic,"projectId":project}),
    ))
}

#[handler]
async fn creations(req: &Request, depot: &Depot, res: &mut Response) {
    let result = async {
        let (_, _, path, identity) = context(req, depot, false).await?;
        let _guard = COMMANDS.lock().await;
        let records = load(&path)?;
        let values = records
            .iter()
            .filter(|c| {
                (&c.origin, &c.owner, &c.subject) == (&identity.0, &identity.1, &identity.2)
            })
            .map(|c| json!({"input":c.input,"creation":public(c)}))
            .collect::<Vec<_>>();
        recheck(depot).map_err(|_| error(401, "sign_in_required"))?;
        Ok(json!({"creations":values}))
    }
    .await;
    finish(res, result);
}
