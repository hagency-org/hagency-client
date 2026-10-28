//! Owner policy captured in each immutable successor command.
use super::*;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct SharedAgentToolPolicy {
    pub mode: String,
    pub revision: u64,
}
impl SharedAgentToolPolicy {
    pub fn validate(&self) -> Result<(), Error> {
        if !matches!(self.mode.as_str(), "standard" | "yolo") { return Err(Error::LocalAuthority); }
        exact(self.revision)
    }
}
pub(super) fn owner_tool_policy(db: &Connection, original: &SharedRuntimeCommand) -> Result<Option<SharedAgentToolPolicy>, Error> {
    let saved = db.query_row("SELECT p.yolo,p.revision FROM owner_shared_tool_policies p JOIN owner_managed_agents a ON a.id=p.child_id WHERE a.agent_id=?1 AND a.original_command_id=?2 AND a.state='reserved' AND NOT a.permanent_fence", params![original.reserve_command.proposed_agent_id,original.reserve_command.command_id], |r| Ok((r.get::<_,bool>(0)?,r.get::<_,u64>(1)?))).optional()?;
    Ok(saved.map(|(yolo,revision)| SharedAgentToolPolicy {mode:if yolo {"yolo"} else {"standard"}.into(),revision}))
}
