use super::*;
/// Finite first transfer contract. Excess history blocks before source fencing;
/// it is never truncated into an apparently complete import.
pub(super) const MAX_ROWS: usize = 10_000;
pub(super) const MAX_BYTES: usize = 4 * 1024 * 1024;
pub(super) const ROW_BYTES: usize = 64 * 1024;
const SAFE: i64 = 9_007_199_254_740_991;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in super::super) struct Summary {
    pub version: u16,
    pub contract: String,
    pub tasks: u32,
    pub comments: u32,
    pub bytes: u64,
    pub digest: String,
}
impl Summary {
    pub(in super::super) fn validate(&self) -> Result<(), Error> {
        if self.version != 2
            || self.contract != "operator_tasks_v2"
            || self.tasks as usize > MAX_ROWS
            || self.comments as usize > MAX_ROWS
            || self.bytes > MAX_BYTES as u64
        {
            return Err(Error::Quarantined);
        }
        hash(&self.digest)
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: String,
    pub granularity: String,
    pub assignee: Option<String>,
    pub created_by: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub heartbeat_at: Option<i64>,
    pub waiting_reason: Option<String>,
    pub waiting_until: Option<String>,
    pub parent_id: Option<String>,
    pub labels: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Comment {
    pub sequence: i64,
    pub task_id: String,
    pub ordinal: u32,
    pub author: String,
    pub text: String,
    pub at_ms: i64,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in super::super) struct History {
    pub(super) tasks: Vec<Task>,
    pub(super) comments: Vec<Comment>,
}
fn text(value: &str, max: usize) -> Result<(), Error> {
    if value.len() > max * 4 || value.chars().count() > max {
        Err(Error::Quarantined)
    } else {
        Ok(())
    }
}
fn optional(value: &Option<String>, max: usize) -> Result<(), Error> {
    value.as_deref().map_or(Ok(()), |s| text(s, max))
}
impl History {
    pub(super) fn validate(&self) -> Result<(), Error> {
        if self.tasks.len() > MAX_ROWS || self.comments.len() > MAX_ROWS {
            return Err(Error::Capacity);
        }
        let mut ids = std::collections::BTreeMap::<&str, u32>::new();
        let mut previous: Option<&str> = None;
        for t in &self.tasks {
            identifier(&t.id, 64)?;
            text(&t.title, 255)?;
            text(&t.description, 4096)?;
            if t.id.is_empty()
                || t.title.is_empty()
                || previous.is_some_and(|p| p >= t.id.as_str())
                || !["created", "accepted", "in_progress", "blocked", "done"]
                    .contains(&t.status.as_str())
                || !["p0", "p1", "p2", "p3"].contains(&t.priority.as_str())
                || !["epic", "task", "subtask"].contains(&t.granularity.as_str())
            {
                return Err(Error::Quarantined);
            }
            previous = Some(&t.id);
            ids.insert(&t.id, 0);
            optional(&t.assignee, 128)?;
            optional(&t.created_by, 128)?;
            optional(&t.waiting_reason, 1024)?;
            optional(&t.waiting_until, 64)?;
            optional(&t.parent_id, 64)?;
            for at in [
                Some(t.created_at),
                Some(t.updated_at),
                t.started_at,
                t.completed_at,
                t.heartbeat_at,
            ]
            .into_iter()
            .flatten()
            {
                if !(0..=SAFE).contains(&at) {
                    return Err(Error::Quarantined);
                }
            }
            if t.labels.len() > 20 {
                return Err(Error::Quarantined);
            }
            for label in &t.labels {
                text(label, 64)?;
            }
            // Original deletion deliberately permits dangling parent IDs. A
            // historical creator/assignee is data, never current authority.
        }
        let mut previous = 0;
        for c in &self.comments {
            text(&c.author, 128)?;
            text(&c.text, 4096)?;
            let ordinal = ids.get_mut(c.task_id.as_str()).ok_or(Error::Quarantined)?;
            if c.sequence <= previous
                || c.sequence > SAFE
                || c.ordinal != *ordinal
                || c.ordinal >= 100
                || !(0..=SAFE).contains(&c.at_ms)
            {
                return Err(Error::Quarantined);
            }
            *ordinal += 1;
            previous = c.sequence;
        }
        Ok(())
    }
    pub(in super::super) fn summary(&self) -> Result<Summary, Error> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_BYTES {
            return Err(Error::Capacity);
        }
        Ok(Summary {
            version: 2,
            contract: "operator_tasks_v2".into(),
            tasks: self.tasks.len() as u32,
            comments: self.comments.len() as u32,
            bytes: bytes.len() as u64,
            digest: canonical::digest(&serde_json::to_value(self)?)?,
        })
    }
}
