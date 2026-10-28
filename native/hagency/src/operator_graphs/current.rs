//! Bounded current-graph projections. A retained node status is not proof of
//! provider execution; every projection keeps the unresolved delivery boundary.
use super::{Graph, canonical::exact_id, graph_statuses, node_statuses};
use hagency_store::{Error, owner_resources::graph_journal::GraphJournalRecord};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphQuery {
    pub after: Option<String>,
    pub status: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphDetailQuery {
    pub id: String,
    pub node_after: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphNodeQuery {
    pub id: String,
    pub node_id: String,
}
impl OperatorGraphQuery {
    pub fn validate(&self) -> Result<(), Error> {
        if let Some(after) = &self.after {
            exact_id(after)?;
        }
        if self
            .status
            .as_ref()
            .is_some_and(|s| !graph_statuses().contains(&s.as_str()))
        {
            return Err(hagency_core::InvalidInput("invalid graph status").into());
        }
        Ok(())
    }
}
impl OperatorGraphDetailQuery {
    pub fn validate(&self) -> Result<(), Error> {
        exact_id(&self.id)?;
        if let Some(after) = &self.node_after {
            exact_id(after)?;
        }
        Ok(())
    }
}
impl OperatorGraphNodeQuery {
    pub fn validate(&self) -> Result<(), Error> {
        exact_id(&self.id)?;
        exact_id(&self.node_id)
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorGraphSummary {
    pub id: String,
    pub revision: u64,
    pub label: String,
    pub owner: String,
    pub status: String,
    pub node_count: usize,
    pub complete_count: usize,
    pub delivery_state: &'static str,
    pub runtime_delivery_authorized: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorGraphPage {
    pub supported: bool,
    pub graphs: Vec<OperatorGraphSummary>,
    pub next_after: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorGraphNodeSummary {
    pub id: String,
    pub assignee: String,
    pub status: String,
    pub dependency_count: usize,
    pub delivery_state: &'static str,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorGraphDetail {
    pub graph: OperatorGraphSummary,
    pub nodes: Vec<OperatorGraphNodeSummary>,
    pub next_after: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorGraphNode {
    pub graph_id: String,
    pub revision: u64,
    pub id: String,
    pub assignee: String,
    pub description: String,
    pub status: String,
    pub depends_on: Vec<String>,
    pub result: Value,
    pub result_json: String,
    pub error: Option<String>,
    pub condition: Option<Value>,
    pub condition_json: Option<String>,
    pub dispatched_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub delivery_state: &'static str,
    pub runtime_delivery_authorized: bool,
}
fn decode(record: &GraphJournalRecord, id: &str, owner: &str) -> Result<Graph, Error> {
    if record.document.len() > super::MAX_GRAPH_BYTES || record.revision == 0 {
        return Err(Error::Quarantined);
    }
    let graph: Graph = serde_json::from_slice(&record.document)?;
    if graph.id != id
        || graph.owner != owner
        || graph.nodes.is_empty()
        || graph.nodes.len() > 256
        || graph.label.chars().count() > 4000
        || !graph_statuses().contains(&graph.status.as_str())
    {
        return Err(Error::Quarantined);
    }
    for (id, node) in &graph.nodes {
        exact_id(id)?;
        if node.id != *id
            || node.depends_on.len() > 256
            || node.assignee.chars().count() > 255
            || node.description.chars().count() > 4000
            || node
                .error
                .as_ref()
                .is_some_and(|e| e.chars().count() > 4000)
            || !node_statuses().contains(&node.status.as_str())
            || serde_json::to_vec(&node.result)?.len() > super::MAX_RESULT_BYTES
            || node
                .condition
                .as_ref()
                .map(serde_json::to_vec)
                .transpose()?
                .is_some_and(|v| v.len() > 64 * 1024)
        {
            return Err(Error::Quarantined);
        }
        for dep in &node.depends_on {
            exact_id(dep)?;
            if !graph.nodes.contains_key(dep) {
                return Err(Error::Quarantined);
            }
        }
    }
    Ok(graph)
}
fn state(graph: &Graph) -> &'static str {
    if graph.status == "cancelled" {
        "suppressed"
    } else {
        "pending_binding"
    }
}
fn project_summary(graph: &Graph, revision: u64) -> OperatorGraphSummary {
    OperatorGraphSummary {
        id: graph.id.clone(),
        revision,
        label: graph.label.clone(),
        owner: graph.owner.clone(),
        status: graph.status.clone(),
        node_count: graph.nodes.len(),
        complete_count: graph
            .nodes
            .values()
            .filter(|n| n.status == "complete")
            .count(),
        delivery_state: state(graph),
        runtime_delivery_authorized: false,
    }
}
pub(crate) fn summary(
    record: GraphJournalRecord,
    id: String,
    owner: String,
) -> Result<OperatorGraphSummary, Error> {
    let graph = decode(&record, &id, &owner)?;
    Ok(project_summary(&graph, record.revision))
}
pub(crate) fn detail(
    record: GraphJournalRecord,
    query: OperatorGraphDetailQuery,
    owner: String,
) -> Result<OperatorGraphDetail, Error> {
    let graph = decode(&record, &query.id, &owner)?;
    let mut rows = graph
        .nodes
        .values()
        .filter(|n| query.node_after.as_ref().is_none_or(|after| n.id > *after));
    let nodes: Vec<_> = rows
        .by_ref()
        .take(16)
        .map(|n| OperatorGraphNodeSummary {
            id: n.id.clone(),
            assignee: n.assignee.clone(),
            status: n.status.clone(),
            dependency_count: n.depends_on.len(),
            delivery_state: state(&graph),
        })
        .collect();
    let next_after = rows.next().and_then(|_| nodes.last().map(|n| n.id.clone()));
    Ok(OperatorGraphDetail {
        graph: project_summary(&graph, record.revision),
        nodes,
        next_after,
    })
}
pub(crate) fn node(
    record: GraphJournalRecord,
    query: OperatorGraphNodeQuery,
    owner: String,
) -> Result<OperatorGraphNode, Error> {
    let graph = decode(&record, &query.id, &owner)?;
    let node = graph.nodes.get(&query.node_id).ok_or(Error::NotFound)?;
    Ok(OperatorGraphNode {
        graph_id: query.id,
        revision: record.revision,
        id: node.id.clone(),
        assignee: node.assignee.clone(),
        description: node.description.clone(),
        status: node.status.clone(),
        depends_on: node.depends_on.clone(),
        result: node.result.clone(),
        result_json: serde_json::to_string(&node.result)?,
        error: node.error.clone(),
        condition: node.condition.clone(),
        condition_json: node
            .condition
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
        dispatched_at: node.dispatched_at.clone(),
        started_at: node.started_at.clone(),
        completed_at: node.completed_at.clone(),
        delivery_state: state(&graph),
        runtime_delivery_authorized: false,
    })
}
