//! HTTP adapter for the shared operator task-graph domain.
use super::{Error, body, console, failed, recheck};
use crate::refusal;
use crate::operator_graphs::{GraphError, graph_error, graph_statuses, text};
pub(super) use crate::operator_graphs::GraphStore;
use salvo::prelude::*;
use serde_json::{Value, json};

pub(super) fn router() -> Router {
    Router::with_path("task-graphs")
        .post(create)
        .get(list)
        .push(Router::with_path("{id}").get(read).delete(remove))
        .push(Router::with_path("{id}/nodes/{node_id}").patch(update_node))
}

fn error_response(res: &mut Response, error: GraphError, fallback: &str) {
    // TS `taskGraphErrorStatus` (`backend-v2.js:3961`).
    let status = match error.code {
        "graph_not_found" | "node_not_found" => StatusCode::NOT_FOUND,
        "graph_exists" => StatusCode::CONFLICT,
        "graph_persistence_failed" | "graph_dispatch_failed" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    res.status_code(status);
    res.render(Json(
        json!({ "error": if error.message.is_empty() { fallback.into() } else { error.message } }),
    ));
}

fn store_for(depot: &Depot) -> Option<&GraphStore> {
    console(depot).ok()?.graphs()
}

fn graph_id(req: &Request) -> Result<String, Error> {
    let id = req.param::<String>("id").ok_or(Error::Invalid)?;
    if text(&Value::String(id.clone()), 255).is_none() {
        return Err(Error::Invalid);
    }
    Ok(id)
}

#[handler]
async fn create(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if let Err(error) = recheck(depot) {
        failed(res, error);
        return;
    }
    let Some(store) = store_for(depot) else {
        refusal(res, StatusCode::SERVICE_UNAVAILABLE, "console_unavailable");
        return;
    };
    let input: Value = match serde_json::from_slice(&match body(req, 64 * 1024).await {
        Ok(bytes) => bytes,
        Err(error) => {
            failed(res, error);
            return;
        }
    }) {
        Ok(value) => value,
        Err(_) => {
            failed(res, Error::Invalid);
            return;
        }
    };
    match store.create(&input).and_then(|created| {
        store
            .advance(&created.id)
            .map(|advanced| advanced.unwrap_or(created))
    }) {
        Ok(graph) => res.render(Json(json!({ "ok": true, "graph": graph }))),
        Err(error) => error_response(res, error, "failed to create task graph"),
    }
}

#[handler]
async fn list(req: &mut Request, depot: &Depot, res: &mut Response) {
    let Some(store) = store_for(depot) else {
        refusal(res, StatusCode::SERVICE_UNAVAILABLE, "console_unavailable");
        return;
    };
    let status = req
        .query::<String>("status")
        .and_then(|s| text(&Value::String(s), 32));
    let status = match status {
        Some(status) if !graph_statuses().contains(&status.as_str()) => {
            error_response(
                res,
                graph_error(
                    "invalid_graph_status",
                    format!("invalid graph status: {status}"),
                ),
                "failed to list task graphs",
            );
            return;
        }
        other => other,
    };
    res.render(Json(json!(store.list(status.as_deref()))));
}

#[handler]
async fn read(req: &mut Request, depot: &Depot, res: &mut Response) {
    let Some(store) = store_for(depot) else {
        refusal(res, StatusCode::SERVICE_UNAVAILABLE, "console_unavailable");
        return;
    };
    let id = match graph_id(req) {
        Ok(id) => id,
        Err(error) => {
            failed(res, error);
            return;
        }
    };
    match store.get(&id) {
        Some(graph) => res.render(Json(json!(graph))),
        None => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(json!({ "error": "task graph not found" })));
        }
    }
}

#[handler]
async fn remove(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if let Err(error) = recheck(depot) {
        failed(res, error);
        return;
    }
    let Some(store) = store_for(depot) else {
        refusal(res, StatusCode::SERVICE_UNAVAILABLE, "console_unavailable");
        return;
    };
    let id = match graph_id(req) {
        Ok(id) => id,
        Err(error) => {
            failed(res, error);
            return;
        }
    };
    match store.delete(&id) {
        Ok(Some(graph)) => res.render(Json(json!({ "ok": true, "graph": graph }))),
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(json!({ "error": "task graph not found" })));
        }
        Err(error) => error_response(res, error, "failed to delete task graph"),
    }
}

#[handler]
async fn update_node(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    // TS gates the node PATCH on the node ASSIGNEE's agent token
    // (`backend-v2.js:16009` `requireAgentToken(_tokenFromNodeAssignee)`).
    // The native console session is the operator's authority over the same
    // surface, so the console login stands in for it here; the wire shape and
    // behaviour are unchanged.
    if let Err(error) = recheck(depot) {
        failed(res, error);
        return;
    }
    let Some(store) = store_for(depot) else {
        refusal(res, StatusCode::SERVICE_UNAVAILABLE, "console_unavailable");
        return;
    };
    let id = match graph_id(req) {
        Ok(id) => id,
        Err(error) => {
            failed(res, error);
            return;
        }
    };
    let node_id = req.param::<String>("node_id").ok_or(Error::Invalid);
    let node_id = match node_id {
        Ok(node_id) if text(&Value::String(node_id.clone()), 255).is_some() => node_id,
        _ => {
            failed(res, Error::Invalid);
            return;
        }
    };
    let patch: Value = match serde_json::from_slice(&match body(req, 128 * 1024).await {
        Ok(bytes) => bytes,
        Err(error) => {
            failed(res, error);
            return;
        }
    }) {
        Ok(value) => value,
        Err(_) => {
            failed(res, Error::Invalid);
            return;
        }
    };
    let updated = store.update_node(&id, &node_id, &patch);
    match updated {
        Ok((graph, node)) => {
            let advanced = store.advance(&id).ok().flatten().unwrap_or(graph);
            res.render(Json(json!({ "ok": true, "graph": advanced, "node": node })))
        }
        Err(error) => error_response(res, error, "failed to update task graph node"),
    }
}
