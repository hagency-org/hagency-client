use super::*;

fn graph() -> Value {
    json!({"retained":{"id":"retained","owner":"old operator","label":"Private label",
        "createdAt":"2025-01-01T00:00:00.000Z", "futureField":{"preserve":true},
        "nodes":{"a":{"id":"a","assignee":"old agent","description":"Private task",
            "status":"complete","result":{"report":"Original result"},"message_id":"msg_0042"}}}})
}
fn message() -> Value {
    json!({"id":"msg_0042","from":"old operator","to":"old agent","viewToken":"never serialize this",
        "unknownOriginalField":{"keep":"exactly"},
        "schema":{"kind":"task_graph_dispatch","version":1,"payload":{
            "graphId":"retained","nodeId":"a","dispatchKey":"task_graph_dispatch:retained:a"}}})
}
fn captured(graphs: Value, messages: Value) -> [Option<Vec<u8>>; 3] {
    [
        Some(serde_json::to_vec_pretty(&graphs).unwrap()),
        Some(serde_json::to_vec_pretty(&messages).unwrap()),
        Some(b" 3\n".to_vec()),
    ]
}

#[test]
fn graph_history_preserves_exact_original_documents_without_dispatch_or_normalization() {
    let original = captured(
        graph(),
        json!([{"id":"msg_0001","unknown":"unrelated"}, message()]),
    );
    let saved = RetainedGraphHistory::inspect(original.clone()).unwrap();
    assert_eq!(saved.documents, original);
    let summary = saved.summary();
    assert_eq!(
        (
            summary.graphs,
            summary.nodes,
            summary.messages,
            summary.dispatch_messages
        ),
        (1, 1, 2, 1)
    );
    assert_eq!(summary.original_counter.as_deref(), Some("3"));
    assert_eq!(summary.effective_counter, "42");
    assert_eq!(summary.unresolved_principal_names, 2);
    let wire = serde_json::to_string(&summary).unwrap();
    for secret in [
        "old operator",
        "old agent",
        "Private label",
        "Private task",
        "Original result",
        "never serialize this",
    ] {
        assert!(!wire.contains(secret));
    }
    assert_eq!(
        saved.summary(),
        RetainedGraphHistory::inspect(original).unwrap().summary()
    );
}

#[test]
fn graph_history_digest_distinguishes_absent_empty_and_exact_bytes() {
    let absent = RetainedGraphHistory::inspect([None, None, None]).unwrap();
    let empty = RetainedGraphHistory::inspect([
        Some(b"{}".to_vec()),
        Some(b"[]".to_vec()),
        Some(b"0".to_vec()),
    ])
    .unwrap();
    let whitespace = RetainedGraphHistory::inspect([
        Some(b"{}\n".to_vec()),
        Some(b"[]".to_vec()),
        Some(b"0".to_vec()),
    ])
    .unwrap();
    assert_eq!(absent.summary().present_documents, 0);
    assert_eq!(empty.summary().present_documents, 3);
    assert_ne!(
        absent.summary().source_digest,
        empty.summary().source_digest
    );
    assert_ne!(
        empty.summary().source_digest,
        whitespace.summary().source_digest
    );
    assert_eq!(absent.summary().original_counter, None);
}

#[test]
fn graph_history_refuses_ambiguous_keys_ids_links_and_counter_overflow() {
    for bytes in [
        b"{\"same\":{},\"same\":{}}".as_slice(),
        b"{\"x\":{\"nodes\":{\"a\":{},\"a\":{}}}}".as_slice(),
        b"[".as_slice(),
    ] {
        assert!(RetainedGraphHistory::inspect([Some(bytes.to_vec()), None, None]).is_err());
    }
    let mut wrong_node = graph();
    wrong_node["retained"]["nodes"]["a"]["id"] = json!("different");
    let mut wrong_graph = graph();
    wrong_graph["retained"]["id"] = json!("different");
    let mut wrong_address = message();
    wrong_address["to"] = json!("someone else");
    let mut wrong_key = message();
    wrong_key["schema"]["payload"]["dispatchKey"] = json!("different");
    let mut other_id = message();
    other_id["id"] = json!("msg_0043");
    for documents in [
        captured(wrong_node, json!([message()])),
        captured(wrong_graph, json!([message()])),
        captured(graph(), json!([wrong_address])),
        captured(graph(), json!([wrong_key])),
        captured(graph(), json!([])),
        captured(graph(), json!([message(), message()])),
        captured(graph(), json!([message(), other_id])),
        captured(json!({}), json!([{"id":"msg_18446744073709551616"}])),
    ] {
        assert!(RetainedGraphHistory::inspect(documents).is_err());
    }
}

#[test]
fn graph_history_retains_orphan_dispatch_and_large_exact_counter_without_claiming_ownership() {
    let mut documents = captured(json!({}), json!([message()]));
    documents[2] = Some(b"18446744073709551615".to_vec());
    let history = RetainedGraphHistory::inspect(documents.clone()).unwrap();
    assert_eq!(history.summary().orphan_dispatch_messages, 1);
    assert_eq!(history.summary().effective_counter, u64::MAX.to_string());
    assert_eq!(history.documents, documents);
}

#[test]
fn graph_history_bounds_documents_and_dependency_walk_before_normalizing() {
    let mut value = graph();
    value["retained"]["nodes"] = json!({});
    for index in 0..=MAX_NODES_PER_GRAPH {
        value["retained"]["nodes"][format!("n{index}")] =
            json!({"assignee":"agent","description":"Task"});
    }
    assert!(RetainedGraphHistory::inspect(captured(value, json!([]))).is_err());
    for (index, (_, limit)) in DOCUMENTS.iter().enumerate() {
        let mut documents = [None, None, None];
        documents[index] = Some(vec![b' '; limit + 1]);
        assert!(RetainedGraphHistory::inspect(documents).is_err());
    }
}
