//! Wire contract shared conceptually with the native Client; no runner capabilities.
use crate::InvalidInput;
type Result<T> = std::result::Result<T, InvalidInput>;
fn key(value: &str) -> Result<()> {
    crate::project::identifier(value, 128)
}
use serde::{Deserialize, Serialize};

pub const STATES: [&str; 5] = ["todo", "in_progress", "blocked", "in_review", "done"];
pub const PRIORITIES: [&str; 4] = ["low", "normal", "high", "urgent"];
pub const CAPABILITY: &str = "project-board-v5";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fields {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "normal")]
    pub priority: String,
    #[serde(default)]
    pub assignee_mxid: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checklist: Vec<ChecklistItem>,
}
impl Default for Fields {
    fn default() -> Self {
        Self {
            title: String::new(),
            description: String::new(),
            priority: normal(),
            assignee_mxid: None,
            labels: Vec::new(),
            due_date: None,
            parent_id: None,
            checklist: Vec::new(),
        }
    }
}
fn normal() -> String {
    "normal".into()
}
impl Fields {
    pub fn validate(&mut self) -> Result<()> {
        if let Some(parent) = &self.parent_id {
            key(parent)?;
        }
        if self.checklist.len() > 100 {
            return Err(InvalidInput("invalid_board_checklist"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for item in &mut self.checklist {
            key(&item.id)?;
            item.title = item.title.trim().into();
            if item.title.is_empty()
                || item.title.chars().count() > 160
                || item.title.chars().any(char::is_control)
                || !ids.insert(item.id.clone())
            {
                return Err(InvalidInput("invalid_board_checklist"));
            }
        }
        self.title = self.title.trim().into();
        if self.title.is_empty()
            || self.title.chars().count() > 240
            || self.title.chars().any(char::is_control)
            || self.description.len() > 32768
            || !PRIORITIES.contains(&self.priority.as_str())
            || self.labels.len() > 12
            || self.assignee_mxid.as_ref().is_some_and(|s| s.len() > 255)
        {
            return Err(InvalidInput("invalid_board_fields"));
        }
        let mut labels = std::collections::BTreeSet::new();
        for label in &self.labels {
            let label = label.trim();
            if label.is_empty() || label.chars().count() > 32 || label.chars().any(char::is_control)
            {
                return Err(InvalidInput("invalid_board_labels"));
            }
            labels.insert(label.to_owned());
        }
        self.labels = labels.into_iter().collect();
        if let Some(date) = &self.due_date {
            validate_date(date)?;
        }
        Ok(())
    }
}
pub fn validate_date(date: &str) -> Result<()> {
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return Err(InvalidInput("invalid_board_date"));
    }
    let year: u32 = date[..4]
        .parse()
        .map_err(|_| InvalidInput("invalid_board_date"))?;
    let month: u32 = date[5..7]
        .parse()
        .map_err(|_| InvalidInput("invalid_board_date"))?;
    let day: u32 = date[8..]
        .parse()
        .map_err(|_| InvalidInput("invalid_board_date"))?;
    let max = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    if year == 0 || day == 0 || day > max {
        return Err(InvalidInput("invalid_board_date"));
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Create {
    pub idempotency_key: String,
    pub fields: Fields,
    pub state: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Update {
    /// Save field edits and an optional status change in one versioned transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub idempotency_key: String,
    pub expected_version: i64,
    pub fields: Fields,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Move {
    pub idempotency_key: String,
    pub expected_version: i64,
    pub state: String,
    /// Exact neighbours are allowed only when the lane revision is still current.
    pub expected_sequence: Option<i64>,
    pub before: Option<String>,
    pub after: Option<String>,
    #[serde(default)]
    pub position: Position,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    #[default]
    Append,
    Prepend,
    Between,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Versioned {
    pub idempotency_key: String,
    pub expected_version: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Comment {
    pub idempotency_key: String,
    pub body: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub state: Option<String>,
    pub assignee: Option<String>,
    pub label: Option<String>,
    pub query: Option<String>,
    #[serde(default)]
    pub overdue: bool,
    #[serde(default)]
    pub archived: bool,
    pub cursor: Option<String>,
    pub sequence: Option<i64>,
    pub limit: Option<i64>,
}
impl Query {
    pub fn validate(&self) -> Result<()> {
        if self.state.as_ref().is_some_and(|s| !valid_state(s))
            || self.query.as_ref().is_some_and(|s| s.chars().count() > 120)
            || self.assignee.as_ref().is_some_and(|s| s.len() > 255)
            || self.label.as_ref().is_some_and(|s| s.chars().count() > 32)
            || self.limit.is_some_and(|n| !(1..=100).contains(&n))
            || self.sequence.is_some_and(|n| n < 0)
        {
            return Err(InvalidInput("invalid_board_query"));
        }
        if let Some(cursor) = &self.cursor {
            key(cursor)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChecklistItem {
    pub id: String,
    pub title: String,
    pub checked: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ColumnDefinition {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    pub id: String,
    pub title: String,
    pub wip_limit: Option<i64>,
    pub is_done: bool,
}
pub fn default_columns() -> Vec<ColumnDefinition> {
    STATES
        .into_iter()
        .zip(["To do", "In progress", "Blocked", "In review", "Done"])
        .map(|(id, title)| ColumnDefinition {
            archived: false,
            id: id.into(),
            title: title.into(),
            wip_limit: None,
            is_done: id == "done",
        })
        .collect()
}
pub fn valid_state(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigureColumns {
    pub idempotency_key: String,
    pub expected_sequence: i64,
    pub columns: Vec<ColumnDefinition>,
}
impl ConfigureColumns {
    pub fn validate(&mut self) -> Result<()> {
        key(&self.idempotency_key)?;
        if self.expected_sequence < 0
            || !(2..=12).contains(&self.columns.len())
            || self.columns.iter().filter(|c| !c.archived).count() < 2
            || !self.columns.iter().any(|c| c.is_done && !c.archived)
        {
            return Err(InvalidInput("invalid_board_columns"));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut titles = std::collections::BTreeSet::new();
        for column in &mut self.columns {
            column.title = column.title.trim().into();
            if !valid_state(&column.id)
                || !ids.insert(column.id.clone())
                || column.title.is_empty()
                || column.title.chars().count() > 48
                || column.title.chars().any(char::is_control)
                || !titles.insert(column.title.to_lowercase())
                || column.wip_limit.is_some_and(|n| !(1..=10000).contains(&n))
            {
                return Err(InvalidInput("invalid_board_columns"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BoardAction {
    Overview,
    SetNavigationDefault {
        kind: String,
        target_id: String,
        expected_revision: i64,
    },
    SetProjectArchived {
        archived: bool,
        expected_revision: i64,
    },
    Boards,
    CreateBoard {
        idempotency_key: String,
        name: String,
    },
    Within {
        board: String,
        command: Box<BoardAction>,
    },
    Snapshot {
        query: Query,
    },
    Page {
        query: Query,
    },
    Detail {
        task: String,
    },
    Changes {
        after: i64,
    },
    Comments {
        task: String,
        after: i64,
    },
    Activity {
        task: String,
        after: i64,
    },
    Notifications {
        before: i64,
    },
    ReadNotification {
        input: NotificationRead,
    },
    Attachment {
        task: String,
        attachment: String,
    },
    UploadAttachment {
        task: String,
        input: AttachmentUpload,
    },
    RemoveAttachment {
        task: String,
        input: AttachmentRemove,
    },
    ConfigureColumns {
        input: ConfigureColumns,
    },
    Create {
        input: Create,
    },
    Update {
        task: String,
        input: Update,
    },
    Move {
        task: String,
        input: Move,
    },
    Archive {
        task: String,
        input: Versioned,
    },
    Restore {
        task: String,
        input: Versioned,
    },
    Comment {
        task: String,
        input: Comment,
    },
}
impl BoardAction {
    pub fn is_upload(&self) -> bool {
        match self {
            Self::UploadAttachment { .. } => true,
            Self::Within { command, .. } => command.is_upload(),
            _ => false,
        }
    }

    pub fn is_read(&self) -> bool {
        if let Self::Within { command, .. } = self {
            return command.is_read();
        }
        matches!(
            self,
            Self::Overview
                | Self::Boards
                | Self::Snapshot { .. }
                | Self::Notifications { .. }
                | Self::Attachment { .. }
                | Self::Page { .. }
                | Self::Detail { .. }
                | Self::Changes { .. }
                | Self::Comments { .. }
                | Self::Activity { .. }
        )
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardTask {
    pub id: String,
    pub project_id: String,
    pub number: i64,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub state: String,
    pub rank: i64,
    pub priority: String,
    pub assignee_mxid: Option<String>,
    pub labels: Vec<String>,
    pub due_date: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub parent_number: Option<i64>,
    #[serde(default)]
    pub checklist: Vec<ChecklistItem>,
    #[serde(default)]
    pub checklist_total: i64,
    #[serde(default)]
    pub checklist_done: i64,
    #[serde(default)]
    pub subtask_total: i64,
    #[serde(default)]
    pub subtask_done: i64,
    #[serde(default)]
    pub attachment_count: i64,
    #[serde(default)]
    pub comment_count: i64,
    pub version: i64,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived_at: Option<i64>,
}
impl BoardTask {
    pub fn fields(&self) -> Fields {
        Fields {
            title: self.title.clone(),
            description: self.description.clone(),
            priority: self.priority.clone(),
            assignee_mxid: self.assignee_mxid.clone(),
            labels: self.labels.clone(),
            due_date: self.due_date.clone(),
            parent_id: self.parent_id.clone(),
            checklist: self.checklist.clone(),
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lane {
    #[serde(default)]
    pub archived: bool,
    pub state: String,
    pub title: String,
    pub wip_limit: Option<i64>,
    pub is_done: bool,
    pub task_count: i64,
    #[serde(default)]
    pub all_task_count: Option<i64>,
    pub total: i64,
    pub tasks: Vec<BoardTask>,
    pub next_cursor: Option<String>,
    pub sequence: i64,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub project_id: String,
    pub sequence: i64,
    pub lanes: Vec<Lane>,
    pub members: Vec<String>,
    #[serde(default)]
    pub label_suggestions: Vec<String>,
    #[serde(default)]
    pub has_more_labels: bool,
    #[serde(default)]
    pub member_names: std::collections::HashMap<String, String>,
    pub capability: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentUpload {
    pub idempotency_key: String,
    pub filename: String,
    pub content_type: String,
    pub data: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentRemove {
    pub idempotency_key: String,
    pub attachment: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationRead {
    pub sequence: i64,
    #[serde(default)]
    pub all_before: bool,
}
