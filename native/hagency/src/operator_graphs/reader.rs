//! Explicit historical projections. Recorded names are not current Matrix
//! principals, and no view here can dispatch work or expose message tokens.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphHistoryQuery {
    pub archive_id: String,
    pub after: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphHistoryDetailQuery {
    pub archive_id: String,
    pub id: String,
    pub after: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphHistoryNodeQuery {
    pub archive_id: String,
    pub id: String,
    pub node_id: String,
}
fn archive(id: &str) -> Result<(), GraphError> {
    hagency_core::project::identifier(id, 128).map_err(|_| invalid())
}
fn invalid() -> GraphError {
    graph_error(
        "graph_history_review_required",
        "Task graph history needs review",
    )
}
impl GraphHistoryQuery {
    pub fn validate(&self) -> Result<(), String> {
        archive(&self.archive_id)
            .and_then(|_| {
                self.after
                    .as_deref()
                    .map(history::exact_id)
                    .unwrap_or(Ok(()))
            })
            .map_err(|e| e.code.into())
    }
}
impl GraphHistoryDetailQuery {
    pub fn validate(&self) -> Result<(), String> {
        GraphHistoryQuery {
            archive_id: self.archive_id.clone(),
            after: self.after.clone(),
        }
        .validate()?;
        history::exact_id(&self.id).map_err(|e| e.code.into())
    }
}
impl GraphHistoryNodeQuery {
    pub fn validate(&self) -> Result<(), String> {
        GraphHistoryQuery {
            archive_id: self.archive_id.clone(),
            after: None,
        }
        .validate()?;
        history::exact_id(&self.id)
            .and_then(|_| history::exact_id(&self.node_id))
            .map_err(|e| e.code.into())
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphHistorySummary {
    id: String,
    label: String,
    owner: String,
    status: String,
    node_count: usize,
    complete_count: usize,
}
impl From<&Graph> for GraphHistorySummary {
    fn from(g: &Graph) -> Self {
        Self {
            id: g.id.clone(),
            label: g.label.clone(),
            owner: g.owner.clone(),
            status: g.status.clone(),
            node_count: g.nodes.len(),
            complete_count: g.nodes.values().filter(|n| n.status == "complete").count(),
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphHistoryPage {
    archive_id: String,
    graphs: Vec<GraphHistorySummary>,
    next_after: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphHistoryNodeSummary {
    id: String,
    assignee: String,
    status: String,
    dependency_count: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphHistoryDetail {
    archive_id: String,
    graph: GraphHistorySummary,
    nodes: Vec<GraphHistoryNodeSummary>,
    next_after: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphHistoryNode {
    archive_id: String,
    graph_id: String,
    id: String,
    assignee: String,
    description: String,
    status: String,
    depends_on: Vec<String>,
    dispatched_at: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    result_recorded: bool,
    condition_recorded: bool,
}
fn graphs(bytes: Option<Vec<u8>>) -> Result<BTreeMap<String, Graph>, GraphError> {
    if bytes.as_ref().is_some_and(|v| v.len() > MAX_GRAPH_BYTES) {
        return Err(invalid());
    }
    let value = history::parse(bytes.as_deref(), json!({}))?;
    let raw = value.as_object().ok_or_else(invalid)?;
    if raw.len() > 10_000 {
        return Err(invalid());
    }
    let mut out = BTreeMap::new();
    let mut total = 0;
    for (id, v) in raw {
        history::exact_id(id)?;
        let nodes = v
            .get("nodes")
            .and_then(Value::as_object)
            .ok_or_else(invalid)?;
        total += nodes.len();
        if nodes.len() > 256 || total > 10_000 {
            return Err(invalid());
        }
        for (key, node) in nodes {
            history::exact_id(key)?;
            if node.get("id").is_some_and(|v| v.as_str() != Some(key)) {
                return Err(invalid());
            }
            if let Some(deps) = node.get("depends_on") {
                let deps = deps.as_array().ok_or_else(invalid)?;
                if deps.len() > 256 {
                    return Err(invalid());
                }
                for dep in deps {
                    history::exact_id(dep.as_str().ok_or_else(invalid)?)?;
                }
            }
        }
        let graph = normalize_graph(v, Some(id.clone()))?;
        if graph.id != *id {
            return Err(invalid());
        }
        out.insert(id.clone(), graph);
    }
    Ok(out)
}
pub(crate) fn page(
    bytes: Option<Vec<u8>>,
    q: GraphHistoryQuery,
) -> Result<GraphHistoryPage, GraphError> {
    let rows = graphs(bytes)?;
    let mut rows = rows
        .values()
        .filter(|g| q.after.as_ref().is_none_or(|a| g.id > *a));
    let page: Vec<_> = rows
        .by_ref()
        .take(16)
        .map(GraphHistorySummary::from)
        .collect();
    let next_after = rows.next().and_then(|_| page.last().map(|g| g.id.clone()));
    Ok(GraphHistoryPage {
        archive_id: q.archive_id,
        graphs: page,
        next_after,
    })
}
pub(crate) fn detail(
    bytes: Option<Vec<u8>>,
    q: GraphHistoryDetailQuery,
) -> Result<GraphHistoryDetail, GraphError> {
    let rows = graphs(bytes)?;
    let g = rows
        .get(&q.id)
        .ok_or_else(|| graph_error("graph_not_found", "Historical graph not found"))?;
    let mut nodes = g
        .nodes
        .values()
        .filter(|n| q.after.as_ref().is_none_or(|a| n.id > *a));
    let page: Vec<_> = nodes
        .by_ref()
        .take(16)
        .map(|n| GraphHistoryNodeSummary {
            id: n.id.clone(),
            assignee: n.assignee.clone(),
            status: n.status.clone(),
            dependency_count: n.depends_on.len(),
        })
        .collect();
    let next_after = nodes.next().and_then(|_| page.last().map(|n| n.id.clone()));
    Ok(GraphHistoryDetail {
        archive_id: q.archive_id,
        graph: g.into(),
        nodes: page,
        next_after,
    })
}
pub(crate) fn node(
    bytes: Option<Vec<u8>>,
    q: GraphHistoryNodeQuery,
) -> Result<GraphHistoryNode, GraphError> {
    let rows = graphs(bytes)?;
    let n = rows
        .get(&q.id)
        .and_then(|g| g.nodes.get(&q.node_id))
        .ok_or_else(|| graph_error("graph_node_not_found", "Historical node not found"))?;
    Ok(GraphHistoryNode {
        archive_id: q.archive_id,
        graph_id: q.id,
        id: n.id.clone(),
        assignee: n.assignee.clone(),
        description: n.description.clone(),
        status: n.status.clone(),
        depends_on: n.depends_on.clone(),
        dispatched_at: n.dispatched_at.clone(),
        started_at: n.started_at.clone(),
        completed_at: n.completed_at.clone(),
        result_recorded: !n.result.is_null(),
        condition_recorded: n.condition.is_some(),
    })
}

#[cfg(test)]
mod tests;
