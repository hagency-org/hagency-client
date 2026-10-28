use super::*;
use crate::operator_graphs::{MAX_RESULT_BYTES, normalize_graph};
use std::collections::BTreeMap;

const CREATED: &str = "2026-10-01T01:02:03.004Z";
const NOW: &str = "2026-10-19T05:06:07.008Z";
const LATER: &str = "2026-10-19T06:07:08.009Z";

fn graph(nodes: Value) -> Graph {
    normalize_graph(
        &json!({
            "id":"original-graph", "owner":"operator", "label":"Original work",
            "createdAt":CREATED, "updatedAt":CREATED, "nodes":nodes
        }),
        None,
    )
    .unwrap()
}

fn snapshot(graph: &Graph) -> Value {
    serde_json::to_value(graph).unwrap()
}

#[test]
fn ready_branches_follow_results_and_keep_the_exact_dispatch_envelope() {
    let original = graph(json!({
        "a":{"assignee":"alpha", "description":"Inspect", "status":"complete", "result":{"approved":true}},
        "b":{"assignee":"beta", "description":"Apply", "depends_on":["a"],
            "condition":{"dep":"a", "path":"approved", "eq":true}},
        "c":{"assignee":"gamma", "description":"Alternative", "depends_on":["a"],
            "condition":{"dep":"a", "path":"approved", "eq":false}}
    }));
    let before = snapshot(&original);
    let mut messages = Vec::new();
    let next = advance(&original, NOW, |graph, node| {
        messages.push(dispatch_message(
            graph,
            node,
            "msg_0042",
            42,
            "private-token",
        ));
        Ok("msg_0042".into())
    })
    .unwrap();
    assert_eq!(snapshot(&original), before);
    assert!(next.changed);
    assert_eq!(next.graph.nodes["b"].status, "dispatched");
    assert_eq!(
        next.graph.nodes["b"].message_id.as_deref(),
        Some("msg_0042")
    );
    assert_eq!(next.graph.nodes["b"].dispatched_at.as_deref(), Some(NOW));
    assert_eq!(next.graph.nodes["c"].status, "skipped");
    assert_eq!(next.graph.nodes["c"].completed_at.as_deref(), Some(NOW));
    assert_eq!(next.graph.status, "active");
    assert_eq!(
        messages,
        vec![json!({
            "id":"msg_0042", "ts":42, "from":"operator", "to":"beta", "group":null,
            "type":"request", "priority":"high", "summary":"Task assigned: Apply",
            "full":"## Task Graph Assignment\n\nGraph: Original work (original-graph)\nNode: b\nDescription: Apply\n\nDependency results:\n- a (alpha): {\"approved\":true}",
            "mentions":[], "reply_to":null, "source":"system", "sourceRoom":null,
            "viewToken":"private-token", "schema":{
                "kind":"task_graph_dispatch", "version":1, "payload":{
                    "dispatchKey":"task_graph_dispatch:original-graph:b",
                    "graphId":"original-graph", "nodeId":"b", "description":"Apply",
                    "dependencyResults":[{"nodeId":"a", "assignee":"alpha", "result":{"approved":true}}]
                }
            }
        })]
    );
}

#[test]
fn failed_and_cancelled_dependencies_cascade_in_reverse_key_order() {
    for failed_status in ["failed", "cancelled"] {
        let original = graph(json!({
            "a":{"assignee":"alpha", "description":"Last", "depends_on":["b"]},
            "b":{"assignee":"beta", "description":"Middle", "depends_on":["z"]},
            "z":{"assignee":"zeta", "description":"First", "status":failed_status}
        }));
        let next = advance(&original, NOW, |_, _| {
            panic!("failed dependencies cannot dispatch")
        })
        .unwrap();
        assert!(next.changed);
        assert_eq!(next.graph.status, "failed");
        assert_eq!(next.graph.completed_at.as_deref(), Some(NOW));
        assert_eq!(next.graph.updated_at, NOW);
        for (node, dependency) in [("a", "b"), ("b", "z")] {
            assert_eq!(next.graph.nodes[node].status, "failed");
            assert_eq!(next.graph.nodes[node].completed_at.as_deref(), Some(NOW));
            assert_eq!(
                next.graph.nodes[node].error.as_deref(),
                Some(format!("dependency failed: {dependency}").as_str())
            );
            assert!(next.graph.nodes[node].message_id.is_none());
        }
        let unchanged = advance(&next.graph, LATER, |_, _| panic!("terminal graph")).unwrap();
        assert!(!unchanged.changed);
        assert_eq!(snapshot(&unchanged.graph), snapshot(&next.graph));
    }
}

#[test]
fn cancellation_retains_terminal_results_and_inflight_message_identity() {
    let original = graph(json!({
        "a":{"assignee":"alpha", "description":"Done", "status":"complete", "result":{"report":"retained"}, "completedAt":CREATED},
        "b":{"assignee":"beta", "description":"Running", "status":"active", "message_id":"msg_0082", "startedAt":CREATED},
        "c":{"assignee":"gamma", "description":"Skipped", "status":"skipped", "completedAt":CREATED},
        "d":{"assignee":"delta", "description":"Pending"}
    }));
    let next = cancel(&original, NOW);
    assert!(next.changed);
    assert_eq!(next.graph.status, "cancelled");
    assert_eq!(next.graph.completed_at.as_deref(), Some(NOW));
    assert_eq!(next.graph.nodes["a"].status, "complete");
    assert_eq!(next.graph.nodes["a"].result, json!({"report":"retained"}));
    assert_eq!(next.graph.nodes["a"].completed_at.as_deref(), Some(CREATED));
    assert_eq!(next.graph.nodes["c"].status, "skipped");
    assert_eq!(next.graph.nodes["b"].status, "cancelled");
    assert_eq!(
        next.graph.nodes["b"].message_id.as_deref(),
        Some("msg_0082")
    );
    assert_eq!(next.graph.nodes["b"].started_at.as_deref(), Some(CREATED));
    assert_eq!(next.graph.nodes["d"].status, "cancelled");
    assert_eq!(next.graph.nodes["d"].completed_at.as_deref(), Some(NOW));
    let again = cancel(&next.graph, LATER);
    assert!(!again.changed);
    assert_eq!(snapshot(&again.graph), snapshot(&next.graph));
    assert_eq!(original.status, "active");
}

#[test]
fn patches_enforce_serialized_result_byte_bounds_without_partial_state() {
    let original = graph(json!({"a":{"assignee":"alpha", "description":"Inspect"}}));
    let before = snapshot(&original);
    let boundary = "x".repeat(MAX_RESULT_BYTES - 2);
    let accepted = patch_node(
        &original,
        "a",
        &json!({"status":"complete", "result":boundary}),
        NOW,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&accepted.graph.nodes["a"].result)
            .unwrap()
            .len(),
        MAX_RESULT_BYTES
    );
    for result in [
        json!("x".repeat(MAX_RESULT_BYTES - 1)),
        json!("中".repeat(MAX_RESULT_BYTES / 3)),
    ] {
        let error = patch_node(
            &original,
            "a",
            &json!({"status":"complete", "result":result}),
            NOW,
        )
        .unwrap_err();
        assert_eq!(error.code, "result_too_large");
        assert_eq!(snapshot(&original), before);
    }
    assert_eq!(
        patch_node(
            &original,
            "a",
            &json!({"status":"unknown", "result":"allowed"}),
            NOW
        )
        .unwrap_err()
        .code,
        "invalid_node_status"
    );
    assert_eq!(
        patch_node(&original, "a", &json!({"description":"ignored"}), NOW)
            .unwrap_err()
            .code,
        "invalid_patch"
    );
    assert_eq!(
        patch_node(&original, "missing", &json!({}), NOW)
            .unwrap_err()
            .code,
        "node_not_found"
    );
    assert_eq!(snapshot(&original), before);
}

#[test]
fn supplied_time_and_durable_message_ids_make_repeated_evaluation_deterministic() {
    let original = graph(json!({
        "z":{"assignee":"zeta", "description":"Second"},
        "a":{"assignee":"alpha", "description":"First"}
    }));
    let run = || {
        let mut order = Vec::new();
        let next = advance(&original, NOW, |_, node| {
            order.push(node.id.clone());
            Ok(format!("durable-{}", node.id))
        })
        .unwrap();
        (snapshot(&next.graph), order)
    };
    let first = run();
    assert_eq!(first, run());
    assert_eq!(first.1, vec!["a", "z"]);
    assert_eq!(first.0["updatedAt"], NOW);
    assert_eq!(first.0["nodes"]["a"]["dispatchedAt"], NOW);
    let active = patch_node(&original, "a", &json!({"status":"active"}), NOW).unwrap();
    let repeated = patch_node(&active.graph, "a", &json!({"status":"active"}), LATER).unwrap();
    assert!(!repeated.changed);
    assert_eq!(repeated.graph.nodes["a"].started_at.as_deref(), Some(NOW));
    assert_eq!(snapshot(&repeated.graph), snapshot(&active.graph));
    let completed = patch_node(&active.graph, "a", &json!({"status":"complete"}), NOW).unwrap();
    let repeated = patch_node(&completed.graph, "a", &json!({"status":"complete"}), LATER).unwrap();
    assert!(
        repeated.changed,
        "retained terminal patches still request persistence"
    );
    assert_eq!(repeated.graph.updated_at, LATER);
    assert_eq!(repeated.graph.nodes["a"].completed_at.as_deref(), Some(NOW));
}

#[test]
fn dispatch_error_returns_no_graph_success_and_retry_reuses_prior_durable_messages() {
    let original = graph(json!({
        "a":{"assignee":"alpha", "description":"First"},
        "b":{"assignee":"beta", "description":"Second"}
    }));
    let before = snapshot(&original);
    let mut durable = BTreeMap::new();
    let mut fail = true;
    let mut dispatch = |graph: &Graph, node: &Node| {
        if node.id == "b" && fail {
            fail = false;
            return Err(graph_error(
                "graph_dispatch_failed",
                "durable outbox refused write",
            ));
        }
        let key = dispatch_key(graph, node);
        let id = format!("message-{}", durable.len() + 1);
        Ok(durable.entry(key).or_insert(id).clone())
    };
    let error = advance(&original, NOW, &mut dispatch).unwrap_err();
    assert_eq!(error.code, "graph_dispatch_failed");
    assert_eq!(
        snapshot(&original),
        before,
        "caller must not commit a partial graph"
    );
    let retried = advance(&original, NOW, &mut dispatch).unwrap();
    assert_eq!(
        retried.graph.nodes["a"].message_id.as_deref(),
        Some("message-1")
    );
    assert_eq!(
        retried.graph.nodes["b"].message_id.as_deref(),
        Some("message-2")
    );
    assert_eq!(
        durable.len(),
        2,
        "retry must not duplicate the first durable message"
    );
    let unchanged = advance(&retried.graph, LATER, |_, _| panic!("already dispatched")).unwrap();
    assert!(!unchanged.changed);
    assert_eq!(snapshot(&unchanged.graph), snapshot(&retried.graph));
}

#[test]
fn skipped_dependencies_allow_unconditional_work_but_fail_branch_conditions() {
    let original = graph(json!({
        "a":{"assignee":"alpha", "description":"Skipped", "status":"skipped"},
        "b":{"assignee":"beta", "description":"Unconditional", "depends_on":["a"]},
        "c":{"assignee":"gamma", "description":"Conditional", "depends_on":["a"], "condition":{"dep":"a", "eq":true}}
    }));
    let mut messages = Vec::new();
    let next = advance(&original, NOW, |graph, node| {
        messages.push(dispatch_message(graph, node, "durable", 1, "token"));
        Ok("durable".into())
    })
    .unwrap();
    assert_eq!(next.graph.nodes["b"].status, "dispatched");
    assert_eq!(next.graph.nodes["c"].status, "skipped");
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0]["schema"]["payload"]["dependencyResults"],
        json!([])
    );
    assert!(
        messages[0]["full"]
            .as_str()
            .unwrap()
            .ends_with("No dependencies.")
    );
}

#[test]
fn independent_condition_waits_until_its_referenced_node_finishes() {
    let original = graph(json!({
        "a":{"assignee":"alpha", "description":"Conditional", "condition":{"dep":"z", "eq":true}},
        "z":{"assignee":"zeta", "description":"Decision"}
    }));
    let next = advance(&original, NOW, |_, node| {
        assert_eq!(node.id, "z");
        Ok("decision-message".into())
    })
    .unwrap();
    assert_eq!(next.graph.nodes["a"].status, "pending");
    let complete = patch_node(
        &next.graph,
        "z",
        &json!({"status":"complete", "result":false}),
        LATER,
    )
    .unwrap();
    let finalized = advance(&complete.graph, LATER, |_, _| panic!("false condition")).unwrap();
    assert_eq!(finalized.graph.nodes["a"].status, "skipped");
    assert_eq!(finalized.graph.status, "complete");
    assert_eq!(finalized.graph.completed_at.as_deref(), Some(LATER));
}

#[test]
fn dispatch_body_excerpt_does_not_truncate_structured_dependency_results() {
    let long = "中".repeat(600);
    let original = graph(json!({
        "a":{"assignee":"alpha", "description":"Inspect", "status":"complete", "result":long},
        "b":{"assignee":"beta", "description":"Apply", "depends_on":["a"]}
    }));
    let first = dispatch_message(&original, &original.nodes["b"], "msg_1", 1, "token");
    assert_eq!(
        first,
        dispatch_message(&original, &original.nodes["b"], "msg_1", 1, "token")
    );
    assert_eq!(
        first["schema"]["payload"]["dependencyResults"][0]["result"],
        json!(long)
    );
    let expected_excerpt: String = serde_json::to_string(&long)
        .unwrap()
        .chars()
        .take(500)
        .collect();
    assert!(first["full"].as_str().unwrap().ends_with(&expected_excerpt));
    assert!(!first["full"].as_str().unwrap().contains(&long));
}

#[test]
fn patch_preserves_retained_failed_result_and_explicit_error_clear_rules() {
    let original =
        graph(json!({"a":{"assignee":"alpha", "description":"Inspect", "result":"original"}}));
    let failed = patch_node(
        &original,
        "a",
        &json!({"status":"failed", "result":"ignored"}),
        NOW,
    )
    .unwrap();
    assert_eq!(failed.graph.nodes["a"].result, json!("original"));
    assert_eq!(
        failed.graph.nodes["a"].error.as_deref(),
        Some("node failed")
    );
    let cleared = patch_node(&failed.graph, "a", &json!({"error":null}), LATER).unwrap();
    assert!(cleared.graph.nodes["a"].error.is_none());
    let result_only = patch_node(
        &cleared.graph,
        "a",
        &json!({"result":"retained correction"}),
        LATER,
    )
    .unwrap();
    assert_eq!(
        result_only.graph.nodes["a"].result,
        json!("retained correction")
    );
    assert_eq!(result_only.graph.nodes["a"].status, "failed");
    assert_eq!(
        result_only.graph.status, "active",
        "patching alone does not advance the graph"
    );
}
