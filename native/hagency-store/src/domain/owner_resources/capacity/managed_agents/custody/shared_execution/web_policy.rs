//! Private owner settings, captured only when a new owner start is journaled.
//! No model, Matrix body, server response or script can write these settings.
use super::{Connection, SharedRuntimeCommand};
#[cfg(test)]
use crate::domain::exec_policy::shared_web::legacy_allows as allows;
pub(super) fn owner_live_web(db: &Connection, original: &SharedRuntimeCommand) -> bool {
    use rusqlite::OptionalExtension;
    // The UI's explicit on/off setting wins over the legacy file, including an
    // explicit off. Resolve the immutable reservation, not just a display name.
    let saved = db.query_row(
        "SELECT id,profile FROM owner_managed_agents WHERE agent_id=?1 AND original_command_id=?2 AND state='reserved' AND permanent_fence=0",
        rusqlite::params![original.reserve_command.proposed_agent_id, original.reserve_command.command_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    ).optional();
    match saved {
        Ok(Some((child, profile))) => return crate::domain::exec_policy::shared_web::read(db, &child, &profile, &original.reserve_command.proposed_agent_id).is_ok_and(|s| s.enabled),
        Ok(None) => {},
        Err(_) => return false,
    }
    crate::domain::exec_policy::shared_web::legacy_enabled(db, &original.reserve_command.proposed_agent_id)
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
        db.execute_batch("CREATE TABLE owner_managed_agents(id TEXT,profile TEXT,agent_id TEXT,original_command_id TEXT,state TEXT,permanent_fence INTEGER)").unwrap();
        let path = root.path().join("owner-shared-tools.json");
        let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"liveWebAgentIds":[original.reserve_command.proposed_agent_id]})).unwrap();
        crate::private::write_new(&path, &bytes).unwrap();
        assert!(owner_live_web(&db, &original));
        db.execute_batch("CREATE TABLE owner_web_catalog_settings(profile TEXT,kind TEXT,entity TEXT,value TEXT,PRIMARY KEY(profile,kind,entity))").unwrap();
        db.execute("INSERT INTO owner_managed_agents VALUES('child','profile',?1,?2,'reserved',0)", rusqlite::params![original.reserve_command.proposed_agent_id, original.reserve_command.command_id]).unwrap();
        crate::domain::exec_policy::shared_web::write(&db, "child", "profile", &original.reserve_command.proposed_agent_id, false).unwrap();
        assert!(!owner_live_web(&db, &original));
        crate::domain::exec_policy::shared_web::write(&db, "child", "profile", &original.reserve_command.proposed_agent_id, true).unwrap();
        assert!(owner_live_web(&db, &original));
        db.execute("DELETE FROM owner_web_catalog_settings", []).unwrap();
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
