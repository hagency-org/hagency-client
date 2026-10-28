//! Current recipient observation. Every transport below is a closed read route;
//! binding never sends a message, creates a room, or starts a provider.
use super::{NativeError, NativeManagement, failure, management::store_error, ManagementCommand};
use crate::console::{
    device_execution::DeviceIdentity,
    server_login::{
        OwnerOperation,
        matrix_creations::{MatrixOperation, service::Scope as MatrixScope},
    },
};
use crate::operator_graphs::{
    canonical::{self, OperatorGraphChange},
    current::OperatorGraphNodeQuery,
    recipients::{self, OperatorGraphRecipientSnapshot, OperatorGraphRecipientTarget},
};
use hagency_store::management::{HostContext, Permission, OPERATOR_GRAPH_SCOPE};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipientKind {
    SharedAgent,
    PersonalAgent,
    Human,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphRecipientOptionsQuery {
    pub id: String,
    pub node_id: String,
    pub kind: RecipientKind,
    pub project_id: Option<String>,
    pub agent_id: Option<String>,
    pub room_id: Option<String>,
    pub after: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphRecipientObserve {
    pub id: String,
    pub node_id: String,
    pub target: OperatorGraphRecipientTarget,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphBindRecipient {
    pub id: String,
    pub node_id: String,
    pub expected_revision: u64,
    pub target: OperatorGraphRecipientTarget,
    pub observation_digest: String,
}
fn argument_error() -> NativeError {
    failure(400, "invalid_management_arguments")
}
fn invalid() -> NativeError {
    failure(502, "management_recipient_response_invalid")
}
fn validate_node(id: &str, node: &str) -> Result<(), String> {
    canonical::exact_id(id)
        .and_then(|_| canonical::exact_id(node))
        .map_err(|_| "invalid graph node".into())
}
impl OperatorGraphRecipientObserve {
    pub fn validate(&self) -> Result<(), String> {
        validate_node(&self.id, &self.node_id)?;
        self.target
            .validate()
            .map_err(|_| "invalid recipient target".into())
    }
}
impl OperatorGraphBindRecipient {
    pub fn validate(&self) -> Result<(), String> {
        validate_node(&self.id, &self.node_id)?;
        self.target
            .validate()
            .map_err(|_| "invalid recipient target")?;
        if self.expected_revision == 0
            || self.expected_revision >= i64::MAX as u64
            || self.observation_digest.len() != 64
            || !self
                .observation_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid recipient observation".into());
        }
        Ok(())
    }
}
impl OperatorGraphRecipientOptionsQuery {
    pub fn validate(&self) -> Result<(), String> {
        validate_node(&self.id, &self.node_id)?;
        for id in [&self.project_id, &self.agent_id].into_iter().flatten() {
            recipients::key(id).map_err(|_| "invalid recipient identifier")?;
        }
        if let Some(room) = &self.room_id {
            recipients::matrix_id(room, '!').map_err(|_| "invalid recipient room")?;
        }
        if let Some(after) = &self.after {
            match self.kind {
                RecipientKind::Human if self.room_id.is_some() => recipients::matrix_id(after, '@'),
                RecipientKind::Human if self.project_id.is_some() => {
                    recipients::matrix_id(after, '!')
                }
                _ => recipients::key(after),
            }
            .map_err(|_| "invalid cursor")?;
        }
        let valid = match self.kind {
            RecipientKind::SharedAgent => self.agent_id.is_none() && self.room_id.is_none(),
            RecipientKind::PersonalAgent => self.project_id.is_none() && self.room_id.is_none(),
            RecipientKind::Human => {
                self.agent_id.is_none() && (self.room_id.is_none() || self.project_id.is_some())
            }
        };
        if !valid {
            return Err("invalid recipient picker stage".into());
        }
        Ok(())
    }
}
fn text(value: &Value, key: &str) -> Result<String, NativeError> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 2048 && !s.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(invalid)
}
fn rows(value: &Value, key: &str) -> Result<Vec<Value>, NativeError> {
    value[key]
        .as_array()
        .filter(|v| v.len() <= 4096)
        .cloned()
        .ok_or_else(invalid)
}
fn select(value: &Value, field: &str, id_field: &str, id: &str) -> Result<Value, NativeError> {
    let matches = rows(value, field)?
        .into_iter()
        .filter(|v| v[id_field] == id)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(failure(409, "management_recipient_unavailable"));
    }
    Ok(matches.into_iter().next().unwrap())
}
fn fields(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .map(|k| ((*k).into(), value[*k].clone()))
            .collect(),
    )
}
fn version(value: &Value, key: &str) -> Result<(), NativeError> {
    if value[key]
        .as_u64()
        .is_some_and(|n| n > 0 && n <= hagency_core::JSON_SAFE_MAX)
    {
        Ok(())
    } else {
        Err(invalid())
    }
}
struct Observed {
    snapshot: OperatorGraphRecipientSnapshot,
    blockers: Vec<String>,
}
impl Observed {
    fn projection(self) -> Result<Value, NativeError> {
        let mut value = serde_json::to_value(self.snapshot).map_err(|_| invalid())?;
        value["canBind"] = json!(self.blockers.is_empty());
        value["blockingReasons"] = json!(self.blockers);
        Ok(value)
    }
}
impl NativeManagement {
    async fn recipient_context(&self) -> Result<HostContext, NativeError> {
        let (context, session) = self.authorized().await?;
        session
            .operator_graph_access(&context, Permission::ReadOperatorGraphRecipients)
            .map_err(store_error)?;
        Ok(context)
    }
    async fn recipient_finish<T>(&self, context: &HostContext, value: T) -> Result<T, NativeError> {
        let current = self.recipient_context().await?;
        if current != *context {
            return Err(failure(401, "management_scope_changed"));
        }
        Ok(value)
    }
    async fn recipient_owner_read(&self, operation: OwnerOperation) -> Result<Value, NativeError> {
        let context = self.recipient_context().await?;
        let console = &self.owner.0.host.console;
        let device = console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        let reply = console
            .owner_api_pinned(&DeviceIdentity::from(&device), operation)
            .await;
        self.recipient_finish(&context, ()).await?;
        let reply = reply.map_err(|e| failure(e.status, &e.code))?;
        if reply.owner != device.owner_mxid()
            || reply.origin != device.origin()
            || reply.subject != device.subject()
            || reply.issuer != device.issuer()
        {
            return Err(failure(401, "management_scope_changed"));
        }
        Ok(reply.value)
    }
    async fn recipient_room_state(&self, room: &str) -> Result<Value, NativeError> {
        recipients::matrix_id(room, '!').map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .operator_graph_access(&context, Permission::ReadOperatorGraphRecipients)
            .map_err(store_error)?;
        let console = &self.owner.0.host.console;
        let device = console
            .authorized_device()
            .await
            .map_err(|_| failure(401, "owner_authorization_required"))?;
        let cookie = self.owner.0.cookie.lock().await.clone();
        let cookie = cookie
            .strip_prefix("hagency_console=")
            .ok_or_else(|| failure(401, "owner_authorization_required"))?;
        let gate_context = context.clone();
        let gate_device = device.clone();
        let scope = MatrixScope::management(
            console,
            cookie,
            &device,
            Box::new(move || {
                gate_device.bearer().is_ok()
                    && session
                        .authorize(&gate_context, Permission::ReadOperatorGraphRecipients)
                        .is_ok()
            }),
        )
        .await
        .map_err(|e| failure(e.status, &e.code))?;
        let reply = scope
            .matrix(MatrixOperation::Managed(
                super::project_setup::matrix::Operation::State(room.into()),
            ))
            .await;
        scope
            .check()
            .await
            .map_err(|e| failure(e.status, &e.code))?;
        self.recipient_finish(&context, ()).await?;
        Ok(reply.map_err(|e| failure(e.status, &e.code))?.value)
    }
    async fn observe_recipient(
        &self,
        input: OperatorGraphRecipientObserve,
    ) -> Result<Observed, NativeError> {
        input.validate().map_err(|_| argument_error())?;
        let context = self.recipient_context().await?;
        let node = self
            .operator_graph_node(OperatorGraphNodeQuery {
                id: input.id.clone(),
                node_id: input.node_id.clone(),
            })
            .await?;
        let mut blockers = Vec::new();
        if !matches!(node.status.as_str(), "pending" | "dispatched") {
            blockers.push("node_not_held".into());
        }
        let account = self.owner.0.expected_owner.clone();
        let server = self.owner.0.expected_origin.clone();
        let (identity, recipient, room) = match &input.target {
            OperatorGraphRecipientTarget::SharedAgent {
                project_id,
                agent_request_id,
            } => {
                let (ctx, session) = self.authorized().await?;
                session
                    .authorize(&ctx, Permission::ReadProjectSetup)
                    .map_err(store_error)?;
                let readiness = super::project_setup::recipient_readiness(
                    self.recipient_owner_read(OwnerOperation::ProjectSetup(
                        super::project_setup::ServerOperation::recipient_read(project_id.clone()),
                    ))
                    .await?,
                    project_id,
                )?;
                // The shared status adapter contains the general management
                // dispatcher. Keep that future on the heap, including when a
                // caller selects a personal agent or Matrix member instead.
                let status = Box::pin(self.agent_status(agent_request_id.clone())).await?;
                shared_identity(
                    &account,
                    project_id,
                    agent_request_id,
                    &readiness,
                    &status,
                    &mut blockers,
                )?
            }
            OperatorGraphRecipientTarget::PersonalAgent {
                agent_id,
                binding_id,
            } => {
                let agent = self
                    .recipient_owner_read(OwnerOperation::Agent {
                        agent: agent_id.clone(),
                    })
                    .await?["agent"]
                    .clone();
                let binding = select(
                    &self
                        .recipient_owner_read(OwnerOperation::Bindings {
                            agent: agent_id.clone(),
                        })
                        .await?,
                    "bindings",
                    "id",
                    binding_id,
                )?;
                let device = self
                    .owner
                    .0
                    .host
                    .console
                    .authorized_device()
                    .await
                    .map_err(|_| failure(401, "owner_authorization_required"))?;
                personal_identity(
                    device.user_id(),
                    agent_id,
                    binding_id,
                    &agent,
                    &binding,
                    &mut blockers,
                )?
            }
            OperatorGraphRecipientTarget::Human {
                project_id,
                room_id,
                user_id,
            } => {
                let project = select(
                    &self.recipient_owner_read(OwnerOperation::Projects).await?,
                    "projects",
                    "id",
                    project_id,
                )?;
                let room = select(
                    &self
                        .recipient_owner_read(OwnerOperation::ProjectRooms {
                            project: project_id.clone(),
                        })
                        .await?,
                    "rooms",
                    "roomId",
                    room_id,
                )?;
                version(&project, "revision")?;
                version(&room, "revision")?;
                if project["active"] != true
                    || room["active"] != true
                    || room["projectId"] != *project_id
                {
                    blockers.push("project_room_inactive".into());
                }
                (
                    json!({"project":fields(&project,&["id","spaceId","active","revision","creationPolicy"]),"room":fields(&room,&["roomId","projectId","active","revision","creationPolicy"])}),
                    user_id.clone(),
                    room_id.clone(),
                )
            }
        };
        let mut identity = identity;
        if !recipient.is_empty() && !room.is_empty() {
            recipients::matrix_id(&recipient, '@').map_err(|_| invalid())?;
            recipients::matrix_id(&room, '!').map_err(|_| invalid())?;
            let state = self.recipient_room_state(&room).await?;
            let (proof, room_blockers) =
                recipients::matrix_proof(&state, &account, &recipient).map_err(|_| invalid())?;
            identity["matrix"] = proof;
            blockers.extend(room_blockers);
        }
        let mut snapshot = OperatorGraphRecipientSnapshot {
            graph_id: input.id,
            node_id: input.node_id,
            revision: node.revision,
            target: input.target,
            account,
            server,
            recipient_mxid: recipient,
            room_id: room,
            identity_json: serde_json::to_string(&identity).map_err(|_| invalid())?,
            observation_digest: String::new(),
            runtime_delivery_authorized: false,
        };
        snapshot.observation_digest = snapshot.digest().map_err(store_error)?;
        if serde_json::to_vec(&snapshot).map_err(|_| invalid())?.len() > 32 * 1024 {
            return Err(failure(409, "management_recipient_unavailable"));
        }
        if blockers.is_empty() {
            snapshot.validate().map_err(|_| invalid())?;
        }
        self.recipient_finish(&context, Observed { snapshot, blockers })
            .await
    }
    pub async fn operator_graph_recipient_observe(
        &self,
        input: OperatorGraphRecipientObserve,
    ) -> Result<Value, NativeError> {
        self.observe_recipient(input).await?.projection()
    }
    pub async fn prepare_operator_graph_bind_recipient(
        &self,
        input: OperatorGraphBindRecipient,
    ) -> Result<super::PreparedOperatorGraph, NativeError> {
        input.validate().map_err(|_| argument_error())?;
        let (context, session) = self.authorized().await?;
        session
            .operator_graph_access(&context, Permission::BindOperatorGraphRecipients)
            .map_err(store_error)?;
        let observed = self
            .observe_recipient(OperatorGraphRecipientObserve {
                id: input.id.clone(),
                node_id: input.node_id.clone(),
                target: input.target,
            })
            .await?;
        if observed.snapshot.revision != input.expected_revision {
            return Err(failure(409, "management_revision_conflict"));
        }
        if !observed.blockers.is_empty() {
            return Err(failure(409, "management_recipient_unavailable"));
        }
        if observed.snapshot.observation_digest != input.observation_digest {
            return Err(failure(409, "management_recipient_changed"));
        }
        self.recipient_finish(&context, ()).await?;
        self.prepare_operator_graph(
            Some(input.id),
            input.expected_revision,
            OperatorGraphChange::BindRecipient {
                node_id: input.node_id,
                snapshot: observed.snapshot,
            },
        )
        .await
    }
    pub(super) async fn reobserve_graph_recipient(
        &self,
        snapshot: &OperatorGraphRecipientSnapshot,
    ) -> Result<(), NativeError> {
        let observed = self
            .observe_recipient(OperatorGraphRecipientObserve {
                id: snapshot.graph_id.clone(),
                node_id: snapshot.node_id.clone(),
                target: snapshot.target.clone(),
            })
            .await?;
        if !observed.blockers.is_empty() || observed.snapshot != *snapshot {
            return Err(failure(409, "management_recipient_changed"));
        }
        Ok(())
    }
    pub async fn operator_graph_recipient_get(
        &self,
        query: OperatorGraphNodeQuery,
    ) -> Result<Value, NativeError> {
        query.validate().map_err(store_error)?;
        let context = self.recipient_context().await?;
        let node = self.operator_graph_node(query.clone()).await?;
        let (ctx, session) = self.authorized().await?;
        let (registry, profile) = session
            .operator_graph_access(&ctx, Permission::ReadOperatorGraphRecipients)
            .map_err(store_error)?;
        let binding = registry
            .operator_graph_recipient(&profile, OPERATOR_GRAPH_SCOPE, &query.id, &query.node_id)
            .await
            .map_err(store_error)?;
        let (state, binding) = if let Some(binding) = binding {
            let saved: OperatorGraphRecipientSnapshot =
                serde_json::from_slice(&binding.snapshot)
                    .map_err(|_| failure(503, "management_saved_result_invalid"))?;
            saved
                .validate()
                .map_err(|_| failure(503, "management_saved_result_invalid"))?;
            if saved.graph_id != query.id
                || saved.node_id != query.node_id
                || saved.account != self.owner.0.expected_owner
                || saved.server != self.owner.0.expected_origin
                || binding.revision != saved.revision + 1
                || binding.revision > node.revision
            {
                return Err(failure(503, "management_saved_result_invalid"));
            }
            let observed = self
                .observe_recipient(OperatorGraphRecipientObserve {
                    id: query.id.clone(),
                    node_id: query.node_id.clone(),
                    target: saved.target.clone(),
                })
                .await;
            // A bind itself increments graph revision. Compare remote identity;
            // revision changes still remain visible to the caller separately.
            let same = match observed {
                Ok(observed) => {
                    let current = &observed.snapshot;
                    if current.revision != node.revision {
                        return Err(failure(409, "management_revision_conflict"));
                    }
                    observed.blockers.is_empty()
                        && saved.target == current.target
                        && saved.identity_json == current.identity_json
                        && saved.account == current.account
                        && saved.server == current.server
                        && saved.recipient_mxid == current.recipient_mxid
                        && saved.room_id == current.room_id
                }
                Err(error)
                    if error.status == 404
                        || error.status == 409
                            && error.code == "management_recipient_unavailable" =>
                {
                    false
                }
                Err(error) => return Err(error),
            };
            (
                if same {
                    "binding_saved"
                } else {
                    "binding_stale"
                },
                serde_json::to_value(saved).map_err(|_| invalid())?,
            )
        } else {
            ("unbound", Value::Null)
        };
        self.recipient_finish(&context,json!({"graphId":query.id,"nodeId":query.node_id,"revision":node.revision,"state":state,"binding":binding,"runtimeDeliveryAuthorized":false})).await
    }
}

fn personal_identity(
    owner_user: &str,
    agent_id: &str,
    binding_id: &str,
    agent: &Value,
    binding: &Value,
    blockers: &mut Vec<String>,
) -> Result<(Value, String, String), NativeError> {
    version(agent, "generation")?;
    version(binding, "generation")?;
    if agent["id"] != agent_id
        || binding["id"] != binding_id
        || binding["agentId"] != agent_id
        || agent["ownerUserId"] != owner_user
    {
        return Err(failure(409, "management_recipient_unavailable"));
    }
    if agent["state"] != "active"
        || binding["state"] != "active"
        || binding["ownerServicePaused"] == true
    {
        blockers.push("personal_binding_inactive".into());
    }
    if !matches!(
        binding["scopeKind"].as_str(),
        Some("project" | "owner_direct")
    ) {
        blockers.push("personal_binding_scope_unavailable".into());
    }
    if binding["scopeKind"] == "owner_direct" && agent["ownerDirectRoomId"] != binding["roomId"] {
        blockers.push("personal_binding_scope_unavailable".into());
    }
    Ok((
        json!({"agent":fields(agent,&["id","ownerUserId","puppetMxid","state","generation","executionDeviceId","ownerDirectRoomId"]),"binding":fields(binding,&["id","agentId","projectId","scopeKind","roomId","state","generation","ownerServicePaused","threadAutoReply"])}),
        text(agent, "puppetMxid")?,
        text(binding, "roomId")?,
    ))
}
fn shared_identity(
    account: &str,
    project: &str,
    request: &str,
    readiness: &super::ProjectReadiness,
    status: &super::AgentStatus,
    blockers: &mut Vec<String>,
) -> Result<(Value, String, String), NativeError> {
    let r = &status.agent_request;
    if r.revision <= 0
        || r.authority_revision <= 0
        || r.revision as u64 > hagency_core::JSON_SAFE_MAX
        || r.authority_revision as u64 > hagency_core::JSON_SAFE_MAX
    {
        return Err(invalid());
    }
    if r.id != request || r.project_id != project || readiness.project_id != project {
        return Err(failure(409, "management_recipient_unavailable"));
    }
    if r.manager_mxid != account {
        blockers.push("shared_project_manager_required".into());
    }
    if r.state != "approved" {
        blockers.push("shared_agent_not_approved".into());
    }
    let mut identity = json!({"request":fields(&serde_json::to_value(r).map_err(|_|invalid())?,&["id","projectId","proposedAgentId","managerMxid","administratorMxid","authorityRevision","state","revision","definitionDigest"])});
    if !readiness.matrix_ready || !readiness.capacity_ready {
        blockers.push("shared_project_not_ready".into());
    }
    if let Some(setup) = &readiness.setup {
        if setup.manager_mxid != account
            || setup.state != "ready"
            || setup.current_binding.is_none()
        {
            blockers.push("shared_project_not_ready".into());
        }
        let mut setup = serde_json::to_value(setup).map_err(|_| invalid())?;
        setup
            .as_object_mut()
            .ok_or_else(invalid)?
            .remove("verifiedAtMs");
        identity["setup"] = setup;
        identity["eligibleAllocation"] =
            serde_json::to_value(&readiness.eligible_allocation).map_err(|_| invalid())?;
    } else {
        blockers.push("shared_project_not_ready".into());
    }
    let (Some(assignment), Some(member)) = (&status.assignment.record, &status.membership.record)
    else {
        blockers.push("shared_membership_unavailable".into());
        return Ok((identity, String::new(), String::new()));
    };
    if assignment.generation <= 0 || assignment.generation as u64 > hagency_core::JSON_SAFE_MAX {
        return Err(invalid());
    }
    if assignment.state == "fenced" {
        blockers.push("shared_membership_unavailable".into());
    }
    if assignment.request_id != request
        || member.request_id != request
        || assignment.agent_id != r.proposed_agent_id
        || member.agent_id != assignment.agent_id
        || member.assignment_id != assignment.id
    {
        return Err(failure(409, "management_recipient_unavailable"));
    }
    if member.state != "matrix_verified"
        || readiness.setup.as_ref().and_then(|s| s.room_id.as_ref()) != Some(&member.room_id)
    {
        blockers.push("shared_membership_unavailable".into());
    }
    identity["assignment"] = json!({"id":assignment.id,"requestId":assignment.request_id,"agentId":assignment.agent_id,"state":assignment.state,"generation":assignment.generation});
    identity["membership"] = json!({"assignmentId":member.assignment_id,"requestId":member.request_id,"agentId":member.agent_id,"puppetMxid":member.puppet_mxid,"roomId":member.room_id,"state":member.state});
    Ok((identity, member.puppet_mxid.clone(), member.room_id.clone()))
}

include!("operator_graph_recipients/options.rs");
#[cfg(test)]
#[path = "operator_graph_recipients/tests.rs"]
mod tests;
