//! Typed creation service shared by HTTP and the native owner. This is the legacy
//! owner-managed project workflow, not the separately gated managed setup protocol.
use super::*;
use crate::console::{Console, authority::Session};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Creating,
    Created,
    Unknown,
    Partial,
    Complete,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Creation {
    pub command_id: String,
    pub kind: Kind,
    pub name: String,
    pub project_id: Option<String>,
    pub space_id: Option<String>,
    pub room_id: Option<String>,
    pub phase: Phase,
    /// Bounded owner-server projection; never accepted as a command or authority.
    pub project: Option<Value>,
    pub last_error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub input: Input,
    pub creation: Creation,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreationReply {
    pub creation: Creation,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListReply {
    /// At most 256 entries from the private, at most 1 MiB journal.
    pub creations: Vec<Entry>,
}
#[derive(Serialize)]
#[serde(untagged)]
pub(in crate::console) enum Reply {
    One(CreationReply),
    List(ListReply),
}
pub(in crate::console) enum Action {
    Start(Input),
    Status(String),
    List,
    Resume { id: String, input: Resume },
}
fn public(command: &Command) -> Result<Creation, OwnerError> {
    Ok(Creation {
        command_id: command.input.command_id.clone(),
        kind: command.input.kind.clone(),
        name: command.input.name.clone(),
        project_id: command.input.project_id.clone(),
        space_id: command.input.space_id.clone(),
        room_id: command.room_id.clone(),
        phase: match command.phase.as_str() {
            "creating" => Phase::Creating,
            "created" => Phase::Created,
            "unknown" => Phase::Unknown,
            "partial" => Phase::Partial,
            "complete" => Phase::Complete,
            _ => return Err(error(503, "local_creation_journal_unavailable")),
        },
        project: command.project.clone(),
        last_error: command.last_error.clone(),
    })
}
fn one(command: &Command) -> Result<Reply, OwnerError> {
    Ok(Reply::One(CreationReply {
        creation: public(command)?,
    }))
}

/// One invocation's private authority. The session token is never serialized,
/// logged, returned to callers, or stored in the creation journal.
pub(in crate::console) struct Scope<'a> {
    console: &'a Console,
    cookie: &'a str,
    local: Session,
    binding: super::super::Binding,
    token: String,
    path: PathBuf,
    native_gate: Option<Box<dyn Fn() -> bool + Send + Sync + 'a>>,
}
impl<'a> Scope<'a> {
    async fn new(console: &'a Console, cookie: &'a str) -> Result<Self, OwnerError> {
        let local = console
            .0
            .authority
            .authenticate(cookie)
            .map_err(|_| error(401, "sign_in_required"))?;
        let login = &console.0.server_login;
        // Preserve the legacy owner-project read authorization before accessing
        // its journal. It is not permission to invoke managed project setup.
        let identity = login.owner_api(cookie, OwnerOperation::Projects).await?;
        let session = login
            .sessions
            .lock()
            .await
            .get(&session_key(cookie))
            .cloned()
            .ok_or_else(|| error(401, "sign_in_required"))?;
        if session.binding.origin != identity.origin
            || session.binding.owner.as_deref() != Some(&identity.owner)
            || session.binding.subject.as_deref() != Some(&identity.subject)
        {
            return Err(error(401, "matrix_account_changed"));
        }
        let root = login
            .state_directory()
            .ok_or_else(|| error(503, "local_state_unavailable"))?;
        let directory = crate::console::owned_agents::journal_profile_directory(
            &root,
            &identity.origin,
            &identity.issuer,
            &identity.subject,
            &identity.owner,
        )
        .map_err(|_| error(503, "local_state_unavailable"))?;
        let scope = Self {
            console,
            cookie,
            local,
            binding: session.binding,
            token: session.token,
            path: directory.join("matrix-creations.json"),
            native_gate: None,
        };
        scope.check().await?;
        Ok(scope)
    }
    pub(in crate::console::server_login) fn matches(
        &self,
        session: &super::super::RemoteSession,
    ) -> Result<(), OwnerError> {
        let now = Instant::now();
        if self.native_gate.as_ref().is_some_and(|gate| !gate()) {
            return Err(error(401, "management_scope_changed"));
        }
        if session.invalidated
            || session.expires <= now
            || session.oauth_expires <= now
            || session.authorized_until <= now
            || session.token != self.token
            || session.binding.origin != self.binding.origin
            || session.binding.owner != self.binding.owner
            || session.binding.subject != self.binding.subject
            || session.binding.client_id != self.binding.client_id
            || session.binding.installation_id != self.binding.installation_id
        {
            return Err(error(401, "matrix_account_changed"));
        }
        Ok(())
    }
    pub(in crate::console) async fn check(&self) -> Result<(), OwnerError> {
        self.console
            .0
            .authority
            .check(&self.local)
            .map_err(|_| error(401, "sign_in_required"))?;
        let login = &self.console.0.server_login;
        if login.stopped.load(Ordering::Acquire) {
            return Err(error(401, "sign_in_required"));
        }
        let session = login
            .sessions
            .lock()
            .await
            .get(&session_key(self.cookie))
            .cloned()
            .ok_or_else(|| error(401, "sign_in_required"))?;
        self.matches(&session)?;
        if let Some(source) = &session.matrix_source {
            let token = source.access_token().await.map_err(|e| {
                let code = crate::console::native::sdk_source_failure(e);
                error(
                    if code.ends_with("unavailable") {
                        503
                    } else {
                        401
                    },
                    code,
                )
            })?;
            if token.client_id != self.binding.client_id {
                return Err(error(401, "matrix_account_changed"));
            }
        }
        // A queued SDK read may finish after logout, account replacement, or
        // retirement. Never use that stale success for the next remote effect.
        self.console
            .0
            .authority
            .check(&self.local)
            .map_err(|_| error(401, "sign_in_required"))?;
        let sessions = login.sessions.lock().await;
        self.matches(
            sessions
                .get(&session_key(self.cookie))
                .ok_or_else(|| error(401, "sign_in_required"))?,
        )?;
        if login.stopped.load(Ordering::Acquire) {
            return Err(error(401, "sign_in_required"));
        }
        Ok(())
    }
    /// Separate managed authority constructor: no legacy project/adoption permission.
    pub(in crate::console) async fn management(
        console: &'a Console,
        cookie: &'a str,
        device: &crate::console::AuthorizedDevice,
        native_gate: Box<dyn Fn() -> bool + Send + Sync + 'a>,
    ) -> Result<Self, OwnerError> {
        let local = console
            .0
            .authority
            .authenticate(cookie)
            .map_err(|_| error(401, "sign_in_required"))?;
        let login = &console.0.server_login;
        login
            .check_matrix_source(cookie)
            .await
            .map_err(|code| error(401, code))?;
        let session = login
            .sessions
            .lock()
            .await
            .get(&session_key(cookie))
            .cloned()
            .ok_or_else(|| error(401, "sign_in_required"))?;
        if session.binding.origin != device.origin()
            || session.binding.owner.as_deref() != Some(device.owner_mxid())
            || session.binding.subject.as_deref() != Some(device.subject())
            || format!("{}auth/", session.binding.origin) != device.issuer()
        {
            return Err(error(401, "matrix_account_changed"));
        }
        let root = login
            .state_directory()
            .ok_or_else(|| error(503, "local_state_unavailable"))?;
        let directory = crate::console::owned_agents::journal_profile_directory(
            &root,
            device.origin(),
            device.issuer(),
            device.subject(),
            device.owner_mxid(),
        )
        .map_err(|_| error(503, "local_state_unavailable"))?;
        let scope = Self {
            console,
            cookie,
            local,
            binding: session.binding,
            token: session.token,
            path: directory.join("matrix-creations.json"),
            native_gate: Some(native_gate),
        };
        scope.check().await?;
        Ok(scope)
    }
    pub(in crate::console) fn management_path(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap()
            .join("managed-project-setups-v1.json")
    }
    pub(in crate::console) fn management_identity(
        &self,
        device: &str,
    ) -> crate::console::native::project_setup::Identity {
        crate::console::native::project_setup::Identity {
            origin: self.binding.origin.clone(),
            owner: self.binding.owner.clone().unwrap(),
            issuer: format!("{}auth/", self.binding.origin),
            subject: self.binding.subject.clone().unwrap(),
            client: self.binding.client_id.clone(),
            installation: self.binding.installation_id.clone(),
            device: device.into(),
        }
    }
    fn owns(&self, command: &Command) -> bool {
        command.origin == self.binding.origin
            && Some(&command.owner) == self.binding.owner.as_ref()
            && Some(&command.subject) == self.binding.subject.as_ref()
    }
    pub(in crate::console) async fn matrix(
        &self,
        op: MatrixOperation,
    ) -> Result<OwnerReply, OwnerError> {
        self.console
            .0
            .server_login
            .matrix_api_scoped(self.cookie, op, Some(self))
            .await
    }
}

pub(in crate::console) async fn invoke(
    console: &Console,
    cookie: &str,
    action: Action,
    expected: Option<(&str, &str)>,
) -> Result<Reply, OwnerError> {
    match &action {
        Action::Start(input) => validate_input(input)?,
        Action::Status(id) | Action::Resume { id, .. } => super::super::operation_id(id)?,
        Action::List => {}
    }
    if let Action::Resume {
        input: Resume {
            room_id: Some(room),
        },
        ..
    } = &action
    {
        room_id(room)?;
    }
    let scope = Scope::new(console, cookie).await?;
    if expected.is_some_and(|(origin, owner)| {
        scope.binding.origin != origin || scope.binding.owner.as_deref() != Some(owner)
    }) {
        return Err(error(401, "matrix_account_mismatch"));
    }
    let _guard = COMMANDS.lock().await;
    scope.check().await?;
    let mut records = load(&scope.path)?;
    let reply = match action {
        Action::Start(input) => start(&scope, &mut records, input).await?,
        Action::Status(id) => {
            let command = records
                .iter()
                .find(|c| c.input.command_id == id)
                .ok_or_else(|| error(404, "creation_command_not_found"))?;
            if !scope.owns(command) {
                return Err(error(403, "creation_identity_mismatch"));
            }
            one(command)?
        }
        Action::List => Reply::List(ListReply {
            creations: records
                .iter()
                .filter(|c| scope.owns(c))
                .map(|c| {
                    Ok(Entry {
                        input: c.input.clone(),
                        creation: public(c)?,
                    })
                })
                .collect::<Result<_, OwnerError>>()?,
        }),
        Action::Resume { id, input } => resume(&scope, &mut records, &id, input).await?,
    };
    // Includes cached complete/replay/list/status paths, which previously had
    // fewer checks than mutations. Do not return another account's journal.
    scope.check().await?;
    Ok(reply)
}
/// The Agent request permission can create only a private, projectless room.
/// Repeated calls recover the same journal entry; uncertain POSTs are not sent again.
pub(in crate::console) async fn invoke_pool<'a>(
    console:&'a Console,cookie:&'a str,device:&crate::console::AuthorizedDevice,
    gate:Box<dyn Fn()->bool+Send+Sync+'a>,input:Input,
) -> Result<CreationReply,OwnerError> {
    if input.kind!=Kind::AgentRoom {return Err(error(400,"invalid_arguments"));}
    validate_input(&input)?;
    let scope=Scope::management(console,cookie,device,gate).await?;
    let _guard=COMMANDS.lock().await;
    scope.check().await?;
    let mut records=load(&scope.path)?;
    let id=input.command_id.clone();
    let result=start(&scope,&mut records,input).await?;
    let result=match &result {
        Reply::One(r) if r.creation.phase!=Phase::Complete=>resume(&scope,&mut records,&id,Resume::default()).await?,
        _=>result,
    };
    scope.check().await?;
    match result {Reply::One(r)=>Ok(r),_=>Err(error(503,"invalid_native_response"))}
}
async fn start(
    scope: &Scope<'_>,
    records: &mut Vec<Command>,
    input: Input,
) -> Result<Reply, OwnerError> {
    if let Some(old) = records
        .iter()
        .find(|c| c.input.command_id == input.command_id)
    {
        if old.input != input || !scope.owns(old) {
            return Err(error(409, "creation_command_conflict"));
        }
        return one(old);
    }
    if records.len() >= 256 {
        return Err(error(503, "local_creation_journal_full"));
    }
    let login = &scope.console.0.server_login;
    checked_project(login, scope.cookie, &input, Some(scope)).await?;
    let discovery = scope.matrix(MatrixOperation::Discovery).await?.value;
    let owner = scope
        .binding
        .owner
        .clone()
        .ok_or_else(|| error(401, "sign_in_required"))?;
    let service = service_identity(&discovery, &owner)?;
    let mut command = Command {
        input: input.clone(),
        origin: scope.binding.origin.clone(),
        owner,
        subject: scope
            .binding
            .subject
            .clone()
            .ok_or_else(|| error(401, "sign_in_required"))?,
        room_id: None,
        phase: "creating".into(),
        project: None,
        last_error: None,
    };
    scope.check().await?;
    records.push(command.clone());
    save(&scope.path, records)?;
    // The durable intent always precedes this sole POST. A failure after it
    // remains uncertain; replay and resume cannot submit another createRoom.
    match scope
        .matrix(MatrixOperation::Create(
            input,
            command.owner.clone(),
            service,
        ))
        .await
    {
        Ok(reply) => {
            if let Some(room) = reply.value["room_id"]
                .as_str()
                .filter(|r| room_id(r).is_ok())
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
    save(&scope.path, records)?;
    if command.room_id.is_some() {
        if let Err(failure) = continue_known(login, scope.cookie, &mut command, Some(scope)).await {
            command.phase = "partial".into();
            command.last_error = Some(failure.code);
        }
        *records.last_mut().unwrap() = command.clone();
        save(&scope.path, records)?;
    }
    one(&command)
}
async fn resume(
    scope: &Scope<'_>,
    records: &mut [Command],
    id: &str,
    input: Resume,
) -> Result<Reply, OwnerError> {
    let index = records
        .iter()
        .position(|c| c.input.command_id == id)
        .ok_or_else(|| error(404, "creation_command_not_found"))?;
    let mut command = records[index].clone();
    if !scope.owns(&command) {
        return Err(error(403, "creation_identity_mismatch"));
    }
    if command.phase == "complete" {
        return one(&command);
    }
    if command.room_id.is_none() {
        let candidates = if let Some(room) = input.room_id {
            vec![room]
        } else {
            let joined = scope.matrix(MatrixOperation::JoinedRooms).await?.value;
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
            if let Ok(state) = scope.matrix(MatrixOperation::State(room.clone())).await
                && created_by(&state.value, &command)
            {
                matches.push(room);
            }
            scope.check().await?;
        }
        if matches.len() != 1 {
            return Err(error(409, "matrix_creation_outcome_unknown"));
        }
        command.room_id = matches.pop();
        command.phase = "created".into();
        records[index] = command.clone();
        save(&scope.path, records)?;
    }
    scope.check().await?;
    if let Err(failure) = continue_known(
        &scope.console.0.server_login,
        scope.cookie,
        &mut command,
        Some(scope),
    )
    .await
    {
        command.phase = "partial".into();
        command.last_error = Some(failure.code);
    }
    records[index] = command.clone();
    save(&scope.path, records)?;
    one(&command)
}

#[cfg(test)]
mod tests;
