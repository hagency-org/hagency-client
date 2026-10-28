//! Lossless, descriptive inventory of the retained operator documents.
//!
//! This is not an import, an execution queue or a name-to-Matrix identity map.
//! Preserve the original bytes (including unknown fields and message tokens)
//! privately; only bounded counts/digests may leave the native source review.
use super::*;
use serde::{Deserialize, de};
use sha2::{Digest, Sha256};

pub(crate) const DOCUMENTS: [(&str, usize); 3] = [
    (GRAPHS_FILE, MAX_GRAPH_BYTES),
    (MESSAGES_FILE, MAX_MESSAGES_BYTES),
    (COUNTER_FILE, 512),
];
const MAX_ROWS: usize = 10_000;
const MAX_NODES_PER_GRAPH: usize = 256;

pub use hagency_store::owner_resources::graph_archives::OperatorGraphHistorySummary;

/// No Deserialize, Serialize or Debug: retained bytes are never a UI payload
/// or reconstructed from a mini-app's assertion of source contents.
pub(crate) struct RetainedGraphHistory {
    pub(super) documents: [Option<Vec<u8>>; 3],
    summary: OperatorGraphHistorySummary,
}
impl RetainedGraphHistory {
    pub(crate) fn inspect(documents: [Option<Vec<u8>>; 3]) -> Result<Self, GraphError> {
        let mut digest = Sha256::new();
        digest.update(b"hagency.operator-graph-documents.v1\0");
        for ((name, limit), bytes) in DOCUMENTS.iter().zip(&documents) {
            digest.update(name.as_bytes());
            digest.update([0, u8::from(bytes.is_some())]);
            if let Some(bytes) = bytes {
                if bytes.len() > *limit {
                    return Err(invalid());
                }
                digest.update((bytes.len() as u64).to_be_bytes());
                digest.update(bytes);
            }
        }
        let graphs = parse(documents[0].as_deref(), json!({}))?;
        let messages = parse(documents[1].as_deref(), json!([]))?;
        let counter = parse(documents[2].as_deref(), json!(0))?;
        let original_counter = counter.as_u64().ok_or_else(invalid)?;
        let mut effective_counter = original_counter;
        let graphs = graphs.as_object().ok_or_else(invalid)?;
        let messages = messages.as_array().ok_or_else(invalid)?;
        if graphs.len() > MAX_ROWS || messages.len() > MAX_ROWS {
            return Err(invalid());
        }

        let mut normalized = BTreeMap::new();
        let mut names = BTreeSet::new();
        let mut nodes = 0;
        for (id, raw) in graphs {
            exact_id(id)?;
            let raw_nodes = raw
                .get("nodes")
                .and_then(Value::as_object)
                .ok_or_else(invalid)?;
            // Bound the retained recursive DAG validator before invoking it.
            if raw_nodes.len() > MAX_NODES_PER_GRAPH {
                return Err(invalid());
            }
            nodes += raw_nodes.len();
            if nodes > MAX_ROWS {
                return Err(invalid());
            }
            for (key, node) in raw_nodes {
                exact_id(key)?;
                if node.get("id").is_some_and(|v| v.as_str() != Some(key)) {
                    return Err(invalid());
                }
                if let Some(deps) = node.get("depends_on") {
                    let deps = deps.as_array().ok_or_else(invalid)?;
                    if deps.len() > MAX_NODES_PER_GRAPH {
                        return Err(invalid());
                    }
                    for dep in deps {
                        exact_id(dep.as_str().ok_or_else(invalid)?)?;
                    }
                }
            }
            let graph = normalize_graph(raw, Some(id.clone())).map_err(|_| invalid())?;
            if graph.id != *id || raw.get("owner").and_then(Value::as_str) != Some(&graph.owner) {
                return Err(invalid());
            }
            names.insert(graph.owner.clone());
            for (id, node) in &graph.nodes {
                if raw_nodes[id].get("assignee").and_then(Value::as_str) != Some(&node.assignee) {
                    return Err(invalid());
                }
                if let Some(message) = raw_nodes[id].get("message_id")
                    && !message.is_null()
                {
                    exact_id(message.as_str().ok_or_else(invalid)?)?;
                }
                names.insert(node.assignee.clone());
            }
            normalized.insert(id.clone(), graph);
        }
        let mut ids = BTreeSet::new();
        let mut dispatches = BTreeMap::new();
        let mut dispatch_keys = BTreeSet::new();
        let mut dispatch_messages = 0;
        let mut orphans = 0;
        for message in messages {
            let id = message
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            exact_id(id)?;
            if !ids.insert(id) {
                return Err(invalid());
            }
            if let Some(decimal) = id.strip_prefix("msg_")
                && !decimal.is_empty()
                && decimal.bytes().all(|v| v.is_ascii_digit())
            {
                effective_counter =
                    effective_counter.max(decimal.parse::<u64>().map_err(|_| invalid())?);
            }
            let Some(schema) = message.get("schema") else {
                continue;
            };
            if schema.get("kind").and_then(Value::as_str) != Some("task_graph_dispatch") {
                continue;
            }
            if schema.get("version").and_then(Value::as_u64) != Some(1) {
                return Err(invalid());
            }
            let payload = schema.get("payload").ok_or_else(invalid)?;
            let graph = payload
                .get("graphId")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            let node = payload
                .get("nodeId")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            exact_id(graph)?;
            exact_id(node)?;
            let key = payload
                .get("dispatchKey")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            if key != format!("task_graph_dispatch:{graph}:{node}")
                || dispatches.insert((graph, node), id).is_some()
                || !dispatch_keys.insert(key)
            {
                return Err(invalid());
            }
            dispatch_messages += 1;
            if let Some(g) = normalized.get(graph)
                && let Some(n) = g.nodes.get(node)
            {
                if message.get("from").and_then(Value::as_str) != Some(&g.owner)
                    || message.get("to").and_then(Value::as_str) != Some(&n.assignee)
                {
                    return Err(invalid());
                }
            } else {
                orphans += 1;
            }
        }
        for graph in normalized.values() {
            for node in graph.nodes.values() {
                if let Some(id) = &node.message_id
                    && dispatches
                        .get(&(graph.id.as_str(), node.id.as_str()))
                        .copied()
                        != Some(id.as_str())
                {
                    return Err(invalid());
                }
            }
        }
        let summary = OperatorGraphHistorySummary {
            source_digest: format!("{:x}", digest.finalize()),
            present_documents: documents.iter().filter(|d| d.is_some()).count(),
            graphs: graphs.len(),
            nodes,
            messages: messages.len(),
            dispatch_messages,
            orphan_dispatch_messages: orphans,
            unresolved_principal_names: names.len(),
            original_counter: documents[2].as_ref().map(|_| original_counter.to_string()),
            effective_counter: effective_counter.to_string(),
        };
        Ok(Self { documents, summary })
    }
    pub(crate) fn summary(&self) -> OperatorGraphHistorySummary {
        self.summary.clone()
    }
    pub(crate) fn bytes(&self, index: usize) -> Option<&[u8]> {
        self.documents[index].as_deref()
    }
}

fn invalid() -> GraphError {
    graph_error(
        "graph_history_review_required",
        "retained task graph documents need review",
    )
}
pub(super) fn exact_id(id: &str) -> Result<(), GraphError> {
    if id.is_empty() || id.len() > 255 || id.trim() != id || id.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}
pub(super) fn parse(bytes: Option<&[u8]>, absent: Value) -> Result<Value, GraphError> {
    bytes
        .map(|bytes| {
            serde_json::from_slice::<UniqueValue>(bytes)
                .map(|v| v.0)
                .map_err(|_| invalid())
        })
        .unwrap_or(Ok(absent))
}

/// serde_json::Value silently takes the last duplicate object key. A migration
/// inventory must refuse ambiguous JSON instead of losing an original record.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("unambiguous JSON")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(json!(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(json!(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(json!(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(json!(v)))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some(UniqueValue(v)) = seq.next_element()? {
                    if out.len() >= MAX_ROWS {
                        return Err(de::Error::custom("document row limit"));
                    }
                    out.push(v);
                }
                Ok(UniqueValue(Value::Array(out)))
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = Map::new();
                while let Some((key, UniqueValue(value))) =
                    map.next_entry::<String, UniqueValue>()?
                {
                    if out.len() >= MAX_ROWS {
                        return Err(de::Error::custom("document row limit"));
                    }
                    if out.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                }
                Ok(UniqueValue(Value::Object(out)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests;
