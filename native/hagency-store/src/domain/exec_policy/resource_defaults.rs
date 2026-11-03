//! Resource-owner defaults captured once when a new managed Agent is reserved.
use crate::Error;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

pub(crate) fn read(db: &Connection, profile: &str, resource: &str) -> Result<(bool, bool), Error> {
    let raw: Option<String> = db.query_row(
        "SELECT value FROM owner_web_catalog_settings WHERE profile=?1 AND kind='resource_default' AND entity=?2",
        params![profile, resource], |r| r.get(0),
    ).optional()?;
    let saved: Option<Value> = raw.map(|s| serde_json::from_str(&s)).transpose()?;
    let resource = crate::domain::read_resource(db, resource)?;
    let yolo = saved.as_ref().and_then(|v| v["yolo"].as_bool())
        .unwrap_or(resource.execution_policy.is_some_and(|p| p.yolo));
    let web = saved.as_ref().and_then(|v| v["webSearch"].as_bool()).unwrap_or(yolo);
    Ok((yolo, web))
}

pub(crate) fn inherit(db: &Connection, profile: &str, resource: &str, agent: &str, child: &str) -> Result<(), Error> {
    let prior: Option<String> = db.query_row(
        "SELECT id FROM owner_managed_agents WHERE profile=?1 AND agent_id=?2 AND id<>?3 AND json_extract(original_command,'$.resourceId')=?4 ORDER BY allocation_generation DESC LIMIT 1",
        params![profile, agent, child, resource], |r| r.get(0),
    ).optional()?;
    if let Some(prior) = prior {
        // A new allocation generation of the same Agent keeps its own policy.
        db.execute("INSERT INTO owner_shared_tool_policies(child_id,yolo,revision) SELECT ?1,yolo,revision FROM owner_shared_tool_policies WHERE child_id=?2 ON CONFLICT(child_id) DO NOTHING", params![child,prior])?;
        let web = super::shared_web::read(db, &prior, profile, agent)?.enabled;
        insert_web(db, profile, child, web)?;
    } else {
        let (yolo, web) = read(db, profile, resource)?;
        db.execute("INSERT INTO owner_shared_tool_policies(child_id,yolo,revision) VALUES(?1,?2,1) ON CONFLICT(child_id) DO NOTHING", params![child,yolo])?;
        insert_web(db, profile, child, web)?;
    }
    Ok(())
}
fn insert_web(db: &Connection, profile: &str, child: &str, web: bool) -> Result<(), Error> {
    db.execute("INSERT INTO owner_web_catalog_settings(profile,kind,entity,value) VALUES(?1,'agent_live_web',?2,?3) ON CONFLICT(profile,kind,entity) DO NOTHING", params![profile,child,json!({"enabled":web,"revision":1}).to_string()])?;
    Ok(())
}
