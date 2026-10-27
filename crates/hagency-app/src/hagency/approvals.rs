//! Presentation checks supplement, never replace, Host authorization.
use serde_json::Value;
use std::collections::HashSet;

pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(i64::MAX)
}
pub(super) fn matches(row: &Value, agent: &str, binding: &str, room: &str, at: i64) -> bool {
    !agent.is_empty()
        && !binding.is_empty()
        && !room.is_empty()
        && row["proposal"]["scope"]["agent"] == agent
        && row["proposal"]["scope"]["binding"] == binding
        && row["proposal"]["scope"]["room"] == room
        && row["proposal"]["expires"].as_i64().is_some_and(|t| t > at)
        && row["proposalId"].as_str().is_some_and(|s| !s.is_empty())
        && row["argsDigest"]
            .as_str()
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
#[derive(Default)]
pub(super) struct Notices {
    seen: HashSet<String>,
}
impl Notices {
    pub fn new_count(&mut self, rows: &[Value]) -> usize {
        // Only current live IDs are retained, bounding memory with the Host queue.
        let next: HashSet<String> = rows
            .iter()
            .filter_map(|r| r["proposalId"].as_str().map(str::to_owned))
            .collect();
        let count = next.difference(&self.seen).count();
        self.seen = next;
        count
    }
}
pub(super) fn details(row: &Value, at: i64) -> String {
    let p = &row["proposal"];
    let text = |key: &str| p[key].as_str().unwrap_or("");
    let line = |label: &str, value: &str| format!("{}: {}", crate::i18n::tr(label), value);
    let args = &p["arguments"];
    let mut lines = vec![
        line("Tool", text("tool")),
        line("Room", p["scope"]["room"].as_str().unwrap_or("")),
        line("Requester", p["scope"]["requester"].as_str().unwrap_or("")),
        line("Working directory", text("canonical_directory")),
        line(
            "Risk",
            match text("risk") {
                "high" => crate::i18n::tr("High"),
                "model_request" => crate::i18n::tr("Model request"),
                v => v,
            },
        ),
        crate::i18n::format(
            "Expires in {seconds} seconds",
            &[(
                "seconds",
                p["expires"]
                    .as_i64()
                    .unwrap_or(at)
                    .saturating_sub(at)
                    .max(0)
                    .to_string(),
            )],
        ),
    ];
    if let Some(preview) = args["inputPreview"].as_str() {
        lines.push(line("Request preview", preview));
    }
    if let Some(command) = args["request"]["command"].as_str() {
        lines.push(line("Command", command));
    }
    lines.push(line(
        "Exact arguments",
        &serde_json::to_string_pretty(args).unwrap_or_default(),
    ));
    lines.push(crate::i18n::tr("Approval applies only to this exact request. It does not confirm successful execution.").into());
    lines.join("\n")
}
pub(super) fn remote_details(value: &Value) -> String {
    match value["phase"].as_str() {
        Some("publishing") => crate::i18n::tr("Publishing private approval...").into(),
        Some("sent") => {
            crate::i18n::tr("Private approval published. Waiting for a verified decision.").into()
        }
        Some("unknown") => crate::i18n::tr(
            "Publication unconfirmed. Refresh original status; do not publish again.",
        )
        .into(),
        Some("approved") => {
            crate::i18n::tr("Remote approval accepted. Execution is not yet confirmed.").into()
        }
        Some("denied") => crate::i18n::tr("Remote approval denied.").into(),
        _ => crate::i18n::tr("No private approval publication is recorded.").into(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn row(id: &str) -> Value {
        json!({"proposalId":id,"argsDigest":"a".repeat(64),"proposal":{"scope":{"agent":"a","binding":"b","room":"!room:s","requester":"@owner:s"},"tool":"hagency_file_create","risk":"high","canonical_directory":"/owned","arguments":{"file":{"Create":{"path":"f","content":"exact bytes"}}},"expires":100}})
    }
    #[test]
    fn cross_room_agent_binding_malformed_and_expired_requests_are_not_actionable() {
        let r = row("one");
        assert!(matches(&r, "a", "b", "!room:s", 99));
        for (a, b, room, at) in [
            ("other", "b", "!room:s", 99),
            ("a", "other", "!room:s", 99),
            ("a", "b", "!other:s", 99),
            ("a", "b", "!room:s", 100),
            ("a", "b", "", 99),
        ] {
            assert!(!matches(&r, a, b, room, at));
        }
        let mut bad = r.clone();
        bad["argsDigest"] = json!("invalid");
        assert!(!matches(&bad, "a", "b", "!room:s", 99));
        bad = r;
        bad["proposal"]["expires"] = Value::Null;
        assert!(!matches(&bad, "a", "b", "!room:s", 99));
    }
    #[test]
    fn repeated_poll_does_not_repeat_notification_and_new_requests_do() {
        let mut notices = Notices::default();
        assert_eq!(notices.new_count(&[row("one")]), 1);
        assert_eq!(notices.new_count(&[row("one")]), 0);
        assert_eq!(notices.new_count(&[row("one"), row("two")]), 1);
        assert_eq!(notices.new_count(&[]), 0);
    }
    #[test]
    fn readable_details_retain_exact_write_arguments() {
        let d = details(&row("one"), 99);
        assert!(d.contains("exact bytes"));
        assert!(d.contains("hagency_file_create"));
        assert!(d.contains("!room:s"));
        assert!(d.contains("@owner:s"));
        assert!(d.contains("/owned"));
    }
}
