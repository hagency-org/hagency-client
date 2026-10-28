//! Native generated graph IDs and durable review/recovery. Preparing an edit
//! does not change graphs; applying an existing command only queues held work.
use super::{NativeError, NativeManagement, failure, management::store_error};
use crate::operator_graphs::{
    canonical::{
        self, OperatorGraphChange, OperatorGraphJournal, OperatorGraphNodeInput,
        OperatorGraphNodePatch,
    },
};
use hagency_store::{
    Error,
    management::{
        Intent, IntentOperation, IntentReply, Permission, TaskField, OPERATOR_GRAPH_SCOPE,
    },
    owner_resources::graph_journal::GraphJournalReceipt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphNodeDraft {
    pub assignee: String,
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "TaskField::is_unchanged")]
    pub condition: TaskField<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_json: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphCreate {
    pub label: String,
    pub nodes: BTreeMap<String, OperatorGraphNodeDraft>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphPatchDraft {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "TaskField::is_unchanged")]
    pub result: TaskField<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_json: Option<String>,
    #[serde(default, skip_serializing_if = "TaskField::is_unchanged")]
    pub error: TaskField<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphUpdate {
    pub id: String,
    pub expected_revision: u64,
    pub patch: OperatorGraphPatchDraft,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphCancel {
    pub id: String,
    pub expected_revision: u64,
}
fn invalid() -> Error {
    hagency_core::InvalidInput("invalid graph arguments").into()
}
fn text_json(raw: &str, max: usize) -> Result<Value, Error> {
    if raw.len() > max {
        return Err(invalid());
    }
    // Reject duplicate object keys instead of silently changing the reviewed JSON.
    canonical::parse_draft_json(raw).map_err(|_| invalid())
}
fn request_bound(value: &impl Serialize, max: usize) -> Result<(), Error> {
    if serde_json::to_vec(value)?.len() > max {
        Err(invalid())
    } else {
        Ok(())
    }
}
fn revision(id: &str, revision: u64) -> Result<(), Error> {
    canonical::exact_id(id)?;
    if revision == 0 || revision >= i64::MAX as u64 {
        return Err(invalid());
    }
    Ok(())
}
impl OperatorGraphCreate {
    fn change(self) -> Result<OperatorGraphChange, Error> {
        request_bound(&self, 256 * 1024)?;
        let nodes = self
            .nodes
            .into_iter()
            .map(|(id, node)| {
                let condition = match (node.condition, node.condition_json) {
                    (TaskField::Unchanged, Some(raw)) => Some(text_json(&raw, 64 * 1024)?),
                    (TaskField::Unchanged, None) => None,
                    (TaskField::Set(value), None) => value,
                    (TaskField::Set(_), Some(_)) => return Err(invalid()),
                };
                Ok((
                    id,
                    OperatorGraphNodeInput {
                        assignee: node.assignee,
                        description: node.description,
                        depends_on: node.depends_on,
                        condition: condition.filter(|v| !v.is_null()),
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        let change = OperatorGraphChange::Create {
            label: self.label,
            nodes,
        };
        change.validate()?;
        Ok(change)
    }
    pub fn validate(&self) -> Result<(), String> {
        self.clone()
            .change()
            .map(|_| ())
            .map_err(|_| "invalid graph creation".into())
    }
}
impl OperatorGraphUpdate {
    fn change(self) -> Result<(String, u64, OperatorGraphChange), Error> {
        request_bound(&self, 256 * 1024)?;
        revision(&self.id, self.expected_revision)?;
        let result = match (self.patch.result, self.patch.result_json) {
            (TaskField::Unchanged, Some(raw)) => TaskField::Set(Some(text_json(&raw, 128 * 1024)?)),
            (value, None) => value,
            (TaskField::Set(_), Some(_)) => return Err(invalid()),
        };
        let change = OperatorGraphChange::UpdateNode {
            patch: OperatorGraphNodePatch {
                node_id: self.patch.node_id,
                status: self.patch.status,
                result,
                error: self.patch.error,
            },
        };
        change.validate()?;
        Ok((self.id, self.expected_revision, change))
    }
    pub fn validate(&self) -> Result<(), String> {
        self.clone()
            .change()
            .map(|_| ())
            .map_err(|_| "invalid graph update".into())
    }
}
impl OperatorGraphCancel {
    pub fn validate(&self) -> Result<(), String> {
        revision(&self.id, self.expected_revision).map_err(|_| "invalid graph cancellation".into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OperatorGraphCommand {
    command_id: String,
    graph_id: String,
    expected_revision: u64,
    change: OperatorGraphChange,
}
impl OperatorGraphCommand {
    fn validate(&self) -> Result<(), Error> {
        hagency_core::project::identifier(&self.command_id, 128)?;
        canonical::exact_id(&self.graph_id)?;
        if !self.command_id.starts_with("graphcmd_")
            || self.expected_revision >= i64::MAX as u64
            || matches!(self.change, OperatorGraphChange::Create { .. })
                != (self.expected_revision == 0)
        {
            return Err(invalid());
        }
        self.change.validate()
    }
    fn operation(&self) -> IntentOperation {
        operation(&self.change)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphResult {
    pub command_id: String,
    pub scope_id: String,
    pub graph_id: String,
    pub revision: u64,
    pub document_digest: String,
    pub queued_messages: u64,
    pub suppressed_messages: u64,
    pub delivery_state: String,
    pub runtime_delivery_authorized: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedOperatorGraph {
    pub command_id: String,
    pub graph_id: String,
    pub expected_revision: u64,
    pub operation: IntentOperation,
    pub change: OperatorGraphChange,
    pub change_json: String,
    pub result: Option<OperatorGraphResult>,
    pub error: Option<String>,
    pub delivery_state: &'static str,
    pub runtime_delivery_authorized: bool,
}
fn delivery(change: &OperatorGraphChange) -> &'static str {
    match change {
        OperatorGraphChange::Cancel => "suppressed",
        OperatorGraphChange::BindRecipient { .. } => "binding_saved",
        _ => "pending_binding",
    }
}
fn operation(change: &OperatorGraphChange) -> IntentOperation {
    match change {
        OperatorGraphChange::Create { .. } => IntentOperation::GraphCreate,
        OperatorGraphChange::UpdateNode { .. } => IntentOperation::GraphUpdateNode,
        OperatorGraphChange::Cancel => IntentOperation::GraphCancel,
        OperatorGraphChange::BindRecipient { .. } => IntentOperation::GraphBindRecipient,
    }
}
fn permission(change: &OperatorGraphChange) -> Permission {
    match change {
        OperatorGraphChange::Create { .. } => Permission::CreateOperatorGraphs,
        OperatorGraphChange::UpdateNode { .. } => Permission::UpdateOperatorGraphNodes,
        OperatorGraphChange::Cancel => Permission::CancelOperatorGraphs,
        OperatorGraphChange::BindRecipient { .. } => Permission::BindOperatorGraphRecipients,
    }
}
pub(super) fn is_operation(operation: IntentOperation) -> bool {
    matches!(
        operation,
        IntentOperation::GraphCreate
            | IntentOperation::GraphUpdateNode
            | IntentOperation::GraphCancel
            | IntentOperation::GraphBindRecipient
    )
}
pub(super) fn known_failure(error: &NativeError) -> bool {
    matches!(
        (error.status, error.code.as_str()),
        (400, "invalid_management_arguments")
            | (404, "management_not_found")
            | (409, "management_revision_conflict")
            | (409, "management_recipient_changed")
            | (409, "management_recipient_unavailable")
    )
}
fn native_id(prefix: &str) -> Result<String, NativeError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| failure(503, "native_random_unavailable"))?;
    Ok(format!(
        "{prefix}{}",
        bytes.iter().map(|v| format!("{v:02x}")).collect::<String>()
    ))
}
fn prepared(
    command: OperatorGraphCommand,
    result: Option<OperatorGraphResult>,
    error: Option<String>,
) -> Result<PreparedOperatorGraph, NativeError> {
    let change_json = serde_json::to_string(&command.change)
        .map_err(|_| failure(503, "management_saved_command_invalid"))?;
    Ok(PreparedOperatorGraph {
        change_json,
        command_id: command.command_id,
        graph_id: command.graph_id,
        expected_revision: command.expected_revision,
        operation: operation(&command.change),
        delivery_state: delivery(&command.change),
        change: command.change,
        result,
        error,
        runtime_delivery_authorized: false,
    })
}
fn result(receipt: GraphJournalReceipt, change: &OperatorGraphChange) -> OperatorGraphResult {
    OperatorGraphResult {
        command_id: receipt.command_id,
        scope_id: receipt.scope_id,
        graph_id: receipt.graph_id,
        revision: receipt.revision,
        document_digest: receipt.document_digest,
        queued_messages: receipt.queued_messages,
        suppressed_messages: receipt.suppressed_messages,
        delivery_state: delivery(change).into(),
        runtime_delivery_authorized: false,
    }
}
fn validate_result(
    command: &OperatorGraphCommand,
    result: &OperatorGraphResult,
) -> Result<(), NativeError> {
    if result.command_id != command.command_id
        || result.graph_id != command.graph_id
        || result.scope_id != OPERATOR_GRAPH_SCOPE
        || result.revision != command.expected_revision + 1
        || result.queued_messages > 256
        || result.suppressed_messages > 256
        || result.runtime_delivery_authorized
        || result.delivery_state != delivery(&command.change)
        || result.document_digest.len() != 64
        || !result
            .document_digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(failure(503, "management_saved_result_invalid"));
    }
    Ok(())
}
impl NativeManagement {
    pub async fn prepare_operator_graph_create(
        &self,
        input: OperatorGraphCreate,
    ) -> Result<PreparedOperatorGraph, NativeError> {
        let change = input.change().map_err(store_error)?;
        self.prepare_operator_graph(None, 0, change).await
    }
    pub async fn prepare_operator_graph_update(
        &self,
        input: OperatorGraphUpdate,
    ) -> Result<PreparedOperatorGraph, NativeError> {
        let (id, revision, change) = input.change().map_err(store_error)?;
        self.prepare_operator_graph(Some(id), revision, change)
            .await
    }
    pub async fn prepare_operator_graph_cancel(
        &self,
        input: OperatorGraphCancel,
    ) -> Result<PreparedOperatorGraph, NativeError> {
        input
            .validate()
            .map_err(|_| failure(400, "invalid_management_arguments"))?;
        self.prepare_operator_graph(
            Some(input.id),
            input.expected_revision,
            OperatorGraphChange::Cancel,
        )
        .await
    }
    pub(super) async fn prepare_operator_graph(
        &self,
        graph_id: Option<String>,
        expected_revision: u64,
        change: OperatorGraphChange,
    ) -> Result<PreparedOperatorGraph, NativeError> {
        change.validate().map_err(store_error)?;
        let _mutation = self.owner.0.management.mutations.lock().await;
        let (context, session) = self.authorized().await?;
        let (registry, profile) = session
            .operator_graph_access(&context, permission(&change))
            .map_err(store_error)?;
        if let Some(id) = &graph_id {
            let journal = OperatorGraphJournal::new(registry, profile, OPERATOR_GRAPH_SCOPE.into())
                .map_err(store_error)?;
            let record = self
                .finish(&context, journal.read(id.clone()).await, false)
                .await?
                .ok_or_else(|| failure(404, "management_not_found"))?;
            if record.revision != expected_revision {
                return Err(failure(409, "management_revision_conflict"));
            }
            if let OperatorGraphChange::UpdateNode { patch } = &change
                && record.graph["nodes"].get(&patch.node_id).is_none()
            {
                return Err(failure(404, "management_not_found"));
            }
            if let OperatorGraphChange::BindRecipient { node_id, snapshot } = &change {
                let node = record.graph["nodes"].get(node_id).ok_or_else(||failure(404,"management_not_found"))?;
                if snapshot.graph_id != *id || snapshot.node_id != *node_id || snapshot.revision != expected_revision || record.graph["status"] != "active" || !matches!(node["status"].as_str(),Some("pending"|"dispatched")) {
                    return Err(failure(409,"management_recipient_unavailable"));
                }
            }
        }
        let command = OperatorGraphCommand {
            command_id: native_id("graphcmd_")?,
            graph_id: match graph_id {
                Some(id) => id,
                None => native_id("graph_")?,
            },
            expected_revision,
            change,
        };
        command.validate().map_err(store_error)?;
        let intent = Intent::new(command.operation(), command.command_id.clone(), &command)
            .map_err(store_error)?;
        let saved = session.prepare_intent(&context, intent).await;
        self.finish(&context, saved, true).await?;
        prepared(command, None, None)
    }
    async fn saved_operator_graph(
        &self,
        command_id: String,
    ) -> Result<(OperatorGraphCommand, Option<IntentReply>), NativeError> {
        super::key(&command_id)?;
        let saved = self
            .saved_commands()
            .await?
            .into_iter()
            .find(|v| v.command_id == command_id)
            .ok_or_else(|| failure(404, "management_saved_command_not_found"))?;
        if !is_operation(saved.operation) {
            return Err(failure(400, "invalid_management_arguments"));
        }
        let command: OperatorGraphCommand = serde_json::from_value(saved.input)
            .map_err(|_| failure(503, "management_saved_command_invalid"))?;
        command
            .validate()
            .map_err(|_| failure(503, "management_saved_command_invalid"))?;
        if command.command_id != command_id || command.operation() != saved.operation {
            return Err(failure(503, "management_saved_command_invalid"));
        }
        let (context, session) = self.authorized().await?;
        let access = session.operator_graph_access(&context, permission(&command.change));
        self.finish(&context, access, false).await?;
        Ok((command, saved.reply))
    }
    pub async fn review_saved_operator_graph(
        &self,
        command_id: String,
    ) -> Result<PreparedOperatorGraph, NativeError> {
        let (command, reply) = self.saved_operator_graph(command_id).await?;
        let (result, error) = match reply {
            Some(IntentReply::Success(value)) => {
                let result: OperatorGraphResult = serde_json::from_value(value)
                    .map_err(|_| failure(503, "management_saved_result_invalid"))?;
                validate_result(&command, &result)?;
                (Some(result), None)
            }
            Some(IntentReply::Failure { code, .. }) => (None, Some(code)),
            None => (None, None),
        };
        prepared(command, result, error)
    }
    pub async fn apply_saved_operator_graph(
        &self,
        command_id: String,
    ) -> Result<OperatorGraphResult, NativeError> {
        let (command, _) = self.saved_operator_graph(command_id).await?;
        self.execute_operator_graph(command).await
    }
    pub(super) async fn execute_operator_graph(
        &self,
        command: OperatorGraphCommand,
    ) -> Result<OperatorGraphResult, NativeError> {
        command.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        let access = session.operator_graph_access(&context, permission(&command.change));
        self.finish(&context, access, false).await?;
        let intent = Intent::new(command.operation(), command.command_id.clone(), &command)
            .map_err(store_error)?;
        let original = command.clone();
        let applied = self
            .with_intent(intent, async {
                let (context, session) = self.authorized().await?;
                let (registry, profile) = session
                    .operator_graph_access(&context, permission(&command.change))
                    .map_err(store_error)?;
                let journal =
                    OperatorGraphJournal::new(registry, profile, OPERATOR_GRAPH_SCOPE.into())
                        .map_err(store_error)?;
                if let OperatorGraphChange::BindRecipient { snapshot, .. } = &command.change {
                    // A lost reply must recover the original committed receipt,
                    // even after the remote recipient has subsequently changed.
                    let receipt = journal.original_receipt(command.command_id.clone(),command.graph_id.clone(),command.expected_revision,&command.change).await;
                    if let Some(receipt) = self.finish(&context,receipt,false).await? {
                        return Ok(result(receipt,&command.change));
                    }
                    // Keep optional remote recipient resolution out of every
                    // create/update/cancel future's stack allocation.
                    Box::pin(self.reobserve_graph_recipient(snapshot)).await?;
                }
                let applied = journal
                    .apply(
                        command.command_id,
                        command.graph_id,
                        command.expected_revision,
                        command.change.clone(),
                    )
                    .await;
                let receipt = self.finish(&context, applied, true).await?;
                Ok(result(receipt, &command.change))
            })
            .await?;
        validate_result(&original, &applied)?;
        Ok(applied)
    }
}

#[cfg(test)]
#[path = "operator_graph_command_tests.rs"]
mod tests;
