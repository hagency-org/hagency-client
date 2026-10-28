//! Storage-independent transitions for both retained files and canonical rows.
//!
//! Every transition leaves its input untouched and uses only the supplied time.
//! `advance` invokes the caller's dispatch callback before marking a node as
//! dispatched. The callback must return a durable message ID, or an ID staged in
//! the same transaction as the eventual graph commit. An error yields no graph
//! transition; a file-backed callback can nevertheless have persisted earlier
//! messages, so adapters must retain dispatch-key deduplication on retry.
use super::{
    Graph, GraphError, Node, evaluate_condition, graph_error, normalize_node_status,
    normalize_result, terminal_node, text,
};
use serde_json::{Value, json};

#[derive(Debug)]
pub(crate) struct GraphTransition {
    pub(crate) graph: Graph,
    /// Whether the legacy state machine requires a graph write. This deliberately
    /// preserves its repeated-terminal-patch behavior, not just value equality.
    pub(crate) changed: bool,
}

/// Cancel unfinished nodes while retaining terminal results and dispatch IDs.
/// Cancelling an already cancelled graph is an exact no-op.
pub(crate) fn cancel(original: &Graph, at: &str) -> GraphTransition {
    let mut graph = original.clone();
    let changed = graph.status != "cancelled";
    if changed {
        graph.status = "cancelled".into();
        graph.completed_at = Some(at.into());
        graph.updated_at = at.into();
        for node in graph.nodes.values_mut() {
            if !terminal_node(&node.status) {
                node.status = "cancelled".into();
                node.completed_at = Some(at.into());
            }
        }
    }
    GraphTransition { graph, changed }
}

/// Apply the retained status/result/error patch contract, including its result
/// size bound and timestamp retention. Graph finalization remains `advance`'s
/// responsibility, as in the original console.
pub(crate) fn patch_node(
    original: &Graph,
    node_id: &str,
    patch: &Value,
    at: &str,
) -> Result<GraphTransition, GraphError> {
    if !original.nodes.contains_key(node_id) {
        return Err(graph_error(
            "node_not_found",
            format!("node not found: {node_id}"),
        ));
    }
    let has_status = patch.get("status").is_some();
    let has_result = patch.get("result").is_some();
    let has_error = patch.get("error").is_some();
    if !has_status && !has_result && !has_error {
        return Err(graph_error(
            "invalid_patch",
            "node patch requires status, result, or error",
        ));
    }
    let mut graph = original.clone();
    let node = graph.nodes.get_mut(node_id).expect("checked above");
    let normalized_result = if has_result {
        Some(normalize_result(patch.get("result"))?)
    } else {
        None
    };
    let mut changed = false;
    if has_status {
        let next_status = normalize_node_status(patch.get("status"), &node.status)?;
        if next_status != node.status {
            node.status = next_status.clone();
            changed = true;
        }
        if next_status == "active" && node.started_at.is_none() {
            node.started_at = Some(at.into());
            changed = true;
        }
        if next_status == "dispatched" && node.dispatched_at.is_none() {
            node.dispatched_at = Some(at.into());
            changed = true;
        }
        if terminal_node(&next_status) {
            if node.completed_at.is_none() {
                node.completed_at = Some(at.into());
            }
            if next_status == "complete" && has_result {
                node.result = normalized_result.clone().unwrap_or(Value::Null);
            }
            if next_status == "failed" {
                node.error = patch
                    .get("error")
                    .and_then(|v| text(v, 4000))
                    .or_else(|| node.error.clone())
                    .or_else(|| Some("node failed".into()));
            }
            changed = true;
        }
    }
    if has_result
        && (!has_status
            || node.status == "complete"
            || node.status == "active"
            || node.status == "dispatched")
    {
        node.result = normalized_result.unwrap_or(Value::Null);
        changed = true;
    }
    if has_error {
        node.error = patch.get("error").and_then(|v| text(v, 4000));
        changed = true;
    }
    if changed {
        graph.updated_at = at.into();
    }
    Ok(GraphTransition { graph, changed })
}

/// Dispatch ready nodes, propagate failed dependencies and finalize the graph.
/// BTreeMap order and repeated passes preserve the retained dispatch order.
/// The adapter must atomically commit its returned graph where supported; this
/// function itself neither persists the graph nor rolls back callback effects.
pub(crate) fn advance(
    original: &Graph,
    at: &str,
    mut dispatch: impl FnMut(&Graph, &Node) -> Result<String, GraphError>,
) -> Result<GraphTransition, GraphError> {
    let mut graph = original.clone();
    if graph.status != "active" {
        return Ok(GraphTransition {
            graph,
            changed: false,
        });
    }
    let mut changed = false;
    loop {
        let mut progress = false;
        let ids: Vec<String> = graph.nodes.keys().cloned().collect();
        for node_id in ids {
            if graph.nodes[&node_id].status != "pending" {
                continue;
            }
            let node = graph.nodes[&node_id].clone();
            let failed: Vec<String> = node
                .depends_on
                .iter()
                .filter(|dep| {
                    matches!(
                        graph.nodes.get(*dep).map(|d| d.status.as_str()),
                        Some("failed" | "cancelled")
                    )
                })
                .cloned()
                .collect();
            if !failed.is_empty() {
                let node = graph.nodes.get_mut(&node_id).expect("checked");
                node.status = "failed".into();
                node.completed_at = Some(at.into());
                node.error = Some(format!("dependency failed: {}", failed.join(", ")));
                changed = true;
                progress = true;
                continue;
            }
            let all_resolved = node.depends_on.iter().all(|dep| {
                matches!(
                    graph.nodes.get(dep).map(|d| d.status.as_str()),
                    Some("complete" | "skipped")
                )
            });
            if !all_resolved {
                continue;
            }
            match evaluate_condition(&graph, &node) {
                None => continue,
                Some(false) => {
                    let node = graph.nodes.get_mut(&node_id).expect("checked");
                    node.status = "skipped".into();
                    node.completed_at = Some(at.into());
                    changed = true;
                    progress = true;
                    continue;
                }
                Some(true) => {}
            }
            let message_id = dispatch(&graph, &node)?;
            let node = graph.nodes.get_mut(&node_id).expect("checked");
            node.status = "dispatched".into();
            node.dispatched_at = Some(at.into());
            node.message_id = Some(message_id);
            changed = true;
            progress = true;
        }
        if !progress {
            break;
        }
    }
    let all_terminal =
        !graph.nodes.is_empty() && graph.nodes.values().all(|node| terminal_node(&node.status));
    if all_terminal && graph.status == "active" {
        graph.status = if graph.nodes.values().any(|node| node.status == "failed") {
            "failed"
        } else {
            "complete"
        }
        .into();
        graph.completed_at = Some(at.into());
        changed = true;
    }
    if changed {
        graph.updated_at = at.into();
    }
    Ok(GraphTransition { graph, changed })
}

pub(crate) fn dispatch_key(graph: &Graph, node: &Node) -> String {
    format!("task_graph_dispatch:{}:{}", graph.id, node.id)
}

/// Build the exact retained dispatch envelope, without allocating IDs/tokens or
/// consulting a clock. Complete dependency results stay intact in the schema;
/// the human-readable body retains the original 500-character excerpt limit.
pub(crate) fn dispatch_message(
    graph: &Graph,
    node: &Node,
    message_id: &str,
    timestamp_ms: u64,
    view_token: &str,
) -> Value {
    let dependency_results: Vec<Value> = node
        .depends_on
        .iter()
        .filter(|dep| graph.nodes.get(*dep).map(|d| d.status.as_str()) == Some("complete"))
        .map(|dep| {
            let dependency = &graph.nodes[dep];
            json!({"nodeId": dep, "assignee": dependency.assignee, "result": dependency.result})
        })
        .collect();
    let deps_text = if dependency_results.is_empty() {
        "No dependencies.".to_string()
    } else {
        format!(
            "Dependency results:\n{}",
            dependency_results
                .iter()
                .map(|dependency| {
                    let serialized = dependency
                        .get("result")
                        .map(Value::to_string)
                        .unwrap_or_default();
                    let truncated: String = serialized.chars().take(500).collect();
                    format!(
                        "- {} ({}): {}",
                        dependency["nodeId"].as_str().unwrap_or_default(),
                        dependency["assignee"].as_str().unwrap_or_default(),
                        truncated
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    json!({
        "id": message_id,
        "ts": timestamp_ms,
        "from": graph.owner,
        "to": node.assignee,
        "group": Value::Null,
        "type": "request",
        "priority": "high",
        "summary": format!("Task assigned: {}", node.description),
        "full": format!("## Task Graph Assignment\n\nGraph: {} ({})\nNode: {}\nDescription: {}\n\n{}", graph.label, graph.id, node.id, node.description, deps_text),
        "mentions": [],
        "reply_to": Value::Null,
        "source": "system",
        "sourceRoom": Value::Null,
        "viewToken": view_token,
        "schema": {
            "kind": "task_graph_dispatch",
            "version": 1,
            "payload": {
                "dispatchKey": dispatch_key(graph, node),
                "graphId": graph.id,
                "nodeId": node.id,
                "description": node.description,
                "dependencyResults": dependency_results,
            }
        }
    })
}

#[cfg(test)]
mod tests;
