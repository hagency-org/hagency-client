//! Owner-reviewed search settings, committed with the management receipt.
use crate::Error;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

const KIND: &str = "agent_live_web";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Setting {
    pub enabled: bool,
    pub revision: u64,
}

pub(crate) fn read(db: &Connection, child: &str, profile: &str, agent: &str) -> Result<Setting, Error> {
    let raw: Option<String> = db.query_row(
        "SELECT value FROM owner_web_catalog_settings WHERE profile=?1 AND kind=?2 AND entity=?3",
        params![profile, KIND, child], |row| row.get(0),
    ).optional()?;
    if let Some(raw) = raw {
        let saved: Setting = serde_json::from_str(&raw)?;
        if saved.revision == 0 || saved.revision > 9_007_199_254_740_991 { return Err(Error::Schema); }
        return Ok(saved);
    }
    Ok(Setting { enabled: legacy_enabled(db, agent), revision: 0 })
}

pub(crate) fn write(db: &Connection, child: &str, profile: &str, agent: &str, enabled: bool) -> Result<(), Error> {
    let current = read(db, child, profile, agent)?;
    if current.revision >= 9_007_199_254_740_991 { return Err(Error::Capacity); }
    let saved = Setting { enabled, revision: current.revision + 1 };
    db.execute(
        "INSERT INTO owner_web_catalog_settings(profile,kind,entity,value) VALUES(?1,?2,?3,?4) ON CONFLICT(profile,kind,entity) DO UPDATE SET value=excluded.value",
        params![profile, KIND, child, serde_json::to_string(&saved)?],
    )?;
    Ok(())
}

// Preserve existing owner opt-ins until that Agent is explicitly edited in the UI.
pub(crate) fn legacy_enabled(db: &Connection, agent: &str) -> bool {
    let Some(root) = db.path().and_then(|p| Path::new(p).parent()) else { return false; };
    let Ok(file) = crate::private::open(&root.join("owner-shared-tools.json"), false) else { return false; };
    let mut bytes = Vec::new();
    if file.take(16 * 1024 + 1).read_to_end(&mut bytes).is_err() || bytes.len() > 16 * 1024 { return false; }
    legacy_allows(&bytes, agent)
}

pub(crate) fn legacy_allows(bytes: &[u8], agent: &str) -> bool {
    #[derive(Deserialize)]
    #[serde(rename_all="camelCase", deny_unknown_fields)]
    struct Legacy { version: u16, live_web_agent_ids: Vec<String> }
    if bytes.len() > 16 * 1024 { return false; }
    serde_json::from_slice::<Legacy>(bytes).is_ok_and(|s| s.version == 1
        && s.live_web_agent_ids.len() <= 128
        && s.live_web_agent_ids.iter().all(|id| !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        && s.live_web_agent_ids.iter().any(|id| id == agent))
}
