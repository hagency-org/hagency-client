use super::*;
fn doc(count: usize) -> Option<Vec<u8>> {
    let mut out = Map::new();
    for i in 0..count {
        let id = format!("graph_{i:03}");
        out.insert(id.clone(),json!({"id":id,"owner":"Recorded owner","label":"Original graph","status":"active","createdAt":"2026-10-01T00:00:00Z","updatedAt":"2026-10-01T00:00:00Z","unknown":"private-extra","nodes":{
            "first":{"id":"first","assignee":"Recorded agent","description":"First step","status":"complete","result":{"secret":"private-result"}},
            "second":{"id":"second","assignee":"Original agent","description":"Second step","depends_on":["first"],"status":"pending","message_id":"msg_original"}
        }}));
    }
    Some(serde_json::to_vec(&out).unwrap())
}
#[test]
fn graph_reader_pages_original_ids_without_gaps() {
    let first = page(
        doc(33),
        GraphHistoryQuery {
            archive_id: "original".into(),
            after: None,
        },
    )
    .unwrap();
    assert_eq!(first.graphs.len(), 16);
    assert_eq!(first.next_after.as_deref(), Some("graph_015"));
    let second = page(
        doc(33),
        GraphHistoryQuery {
            archive_id: "original".into(),
            after: first.next_after,
        },
    )
    .unwrap();
    assert_eq!(second.graphs[0].id, "graph_016");
    let last = page(
        doc(33),
        GraphHistoryQuery {
            archive_id: "original".into(),
            after: second.next_after,
        },
    )
    .unwrap();
    assert_eq!(last.graphs.len(), 1);
    assert_eq!(last.graphs[0].id, "graph_032");
    assert!(last.next_after.is_none());
}
#[test]
fn graph_reader_keeps_dependencies_and_recorded_names_but_no_private_payloads() {
    let detail = detail(
        doc(1),
        GraphHistoryDetailQuery {
            archive_id: "archive".into(),
            id: "graph_000".into(),
            after: None,
        },
    )
    .unwrap();
    assert_eq!(detail.graph.complete_count, 1);
    assert_eq!(detail.nodes[1].dependency_count, 1);
    let n = node(
        doc(1),
        GraphHistoryNodeQuery {
            archive_id: "archive".into(),
            id: "graph_000".into(),
            node_id: "second".into(),
        },
    )
    .unwrap();
    assert_eq!(n.depends_on, vec!["first"]);
    assert_eq!(n.assignee, "Original agent");
    let first = node(
        doc(1),
        GraphHistoryNodeQuery {
            archive_id: "archive".into(),
            id: "graph_000".into(),
            node_id: "first".into(),
        },
    )
    .unwrap();
    assert!(first.result_recorded);
    let projected = serde_json::to_string(&(detail, n, first)).unwrap();
    for secret in ["private-extra", "private-result", "msg_original"] {
        assert!(!projected.contains(secret));
    }
}
#[test]
fn graph_reader_refuses_duplicate_keys_mismatched_identity_and_unbounded_nodes() {
    assert!(
        page(
            Some(br#"{"x":{},"x":{}}"#.to_vec()),
            GraphHistoryQuery {
                archive_id: "archive".into(),
                after: None
            }
        )
        .is_err()
    );
    for change in [0, 1, 2] {
        let mut value: Value = serde_json::from_slice(&doc(1).unwrap()).unwrap();
        match change {
            0 => value["graph_000"]["id"] = json!("different"),
            1 => value["graph_000"]["nodes"]["first"]["id"] = json!("different"),
            _ => value["graph_000"]["nodes"]["second"]["depends_on"] = json!(vec!["first"; 257]),
        }
        assert!(
            page(
                Some(serde_json::to_vec(&value).unwrap()),
                GraphHistoryQuery {
                    archive_id: "archive".into(),
                    after: None
                }
            )
            .is_err()
        );
    }
}
#[test]
fn graph_reader_absence_is_empty_but_invalid_or_missing_identity_is_not() {
    assert!(
        page(
            None,
            GraphHistoryQuery {
                archive_id: "archive".into(),
                after: None
            }
        )
        .unwrap()
        .graphs
        .is_empty()
    );
    assert!(
        page(
            Some(vec![]),
            GraphHistoryQuery {
                archive_id: "archive".into(),
                after: None
            }
        )
        .is_err()
    );
    assert!(
        detail(
            doc(1),
            GraphHistoryDetailQuery {
                archive_id: "archive".into(),
                id: "wrong".into(),
                after: None
            }
        )
        .is_err()
    );
    assert!(
        node(
            doc(1),
            GraphHistoryNodeQuery {
                archive_id: "archive".into(),
                id: "graph_000".into(),
                node_id: "wrong".into()
            }
        )
        .is_err()
    );
}
#[test]
fn graph_reader_closed_queries_retain_unicode_original_identity() {
    let q: GraphHistoryNodeQuery =
        serde_json::from_value(json!({"archiveId":"archive","id":"原始图","nodeId":"节点"}))
            .unwrap();
    assert!(q.validate().is_ok());
    for extra in ["profile", "owner", "path", "limit", "commandId"] {
        let mut value = json!({"archiveId":"archive"});
        value[extra] = json!("forged");
        assert!(serde_json::from_value::<GraphHistoryQuery>(value).is_err());
    }
    for bad in ["", " leading", "a\nb"] {
        assert!(
            GraphHistoryQuery {
                archive_id: "archive".into(),
                after: Some(bad.into())
            }
            .validate()
            .is_err()
        );
    }
}

#[test]
fn graph_reader_does_not_invent_history_times_from_missing_original_fields() {
    let mut value: Value = serde_json::from_slice(&doc(1).unwrap()).unwrap();
    let graph = value["graph_000"].as_object_mut().unwrap();
    graph.remove("createdAt");
    graph.remove("updatedAt");
    let bytes = Some(serde_json::to_vec(&value).unwrap());
    let query = GraphHistoryQuery {
        archive_id: "archive".into(),
        after: None,
    };
    let first = serde_json::to_value(page(bytes.clone(), query.clone()).unwrap()).unwrap();
    let second = serde_json::to_value(page(bytes, query).unwrap()).unwrap();
    assert_eq!(first, second);
    assert!(
        !first["graphs"][0]
            .as_object()
            .unwrap()
            .contains_key("updatedAt")
    );
}
