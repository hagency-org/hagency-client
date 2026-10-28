//! Private owner settings, captured only when a new owner start is journaled.
//! No model, Matrix body, server response or script can write these settings.
use super::{Connection, SharedRuntimeCommand};
use std::{io::Read, path::Path};
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    version: u16,
    live_web_agent_ids: Vec<String>,
}
pub(super) fn owner_live_web(db: &Connection, original: &SharedRuntimeCommand) -> bool {
    let Some(root) = db.path().and_then(|p| Path::new(p).parent()) else { return false; };
    let Ok(file) = crate::private::open(&root.join("owner-shared-tools.json"), false) else { return false; };
    let mut bytes = Vec::new();
    if file.take(16 * 1024 + 1).read_to_end(&mut bytes).is_err() { return false; }
    allows(&bytes, &original.reserve_command.proposed_agent_id)
}
fn allows(bytes: &[u8], agent: &str) -> bool {
    if bytes.len() > 16 * 1024 { return false; }
    serde_json::from_slice::<Settings>(bytes).is_ok_and(|s| s.version == 1
        && s.live_web_agent_ids.len() <= 128
        && s.live_web_agent_ids.iter().all(|id| !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        && s.live_web_agent_ids.iter().any(|id| id == agent))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_web_opt_in_is_exact_and_default_off() {
        assert!(!allows(b"", "agt_one"));
        for ids in [vec!["*"], vec!["agt_one_more"], vec![]] {
            assert!(!allows(&serde_json::to_vec(&serde_json::json!({"version":1,"liveWebAgentIds":ids})).unwrap(), "agt_one"));
        }
        assert!(allows(br#"{"version":1,"liveWebAgentIds":["agt_two","agt_one"]}"#, "agt_one"));
        assert!(!allows(br#"{"version":2,"liveWebAgentIds":["agt_one"]}"#, "agt_one"));
        assert!(!allows(&vec![b'x';16385], "x"));
    }
    #[test]
    fn live_web_permission_belongs_to_the_private_owner_ledger() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("management-shared-attempt-v2.json")).unwrap();
        let original: SharedRuntimeCommand = serde_json::from_value(fixture["command"]["originalRuntimeCommand"].clone()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let db = Connection::open(root.path().join("domain.sqlite3")).unwrap();
        assert!(!owner_live_web(&db, &original));
        let path = root.path().join("owner-shared-tools.json");
        let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"liveWebAgentIds":[original.reserve_command.proposed_agent_id]})).unwrap();
        crate::private::write_new(&path, &bytes).unwrap();
        assert!(owner_live_web(&db, &original));
        let other = tempfile::tempdir().unwrap();
        let other_db = Connection::open(other.path().join("domain.sqlite3")).unwrap();
        assert!(!owner_live_web(&other_db, &original));
        #[cfg(unix)] {
            std::fs::remove_file(&path).unwrap();
            let foreign = other.path().join("foreign.json");
            crate::private::write_new(&foreign, &bytes).unwrap();
            std::os::unix::fs::symlink(&foreign, &path).unwrap();
            assert!(!owner_live_web(&db, &original));
        }
    }
}
