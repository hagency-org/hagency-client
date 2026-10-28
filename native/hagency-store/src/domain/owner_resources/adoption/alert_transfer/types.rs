//! Exact original rows, not the public projection: raw detail and nullable
//! provenance remain byte-preserved. No path or serialized proof grants custody.
use super::*;
pub(super) const MAX_ALERTS: usize = 1_000;
pub(super) const MAX_NOTES: usize = 10_000;
pub(super) const MAX_BYTES: usize = 128 * 1024;
pub(super) const ROW_BYTES: usize = 64 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in super::super) struct Summary {
    pub version: u16,
    pub alerts: u32,
    pub notes: u32,
    pub bytes: u64,
    pub digest: String,
}
impl Summary {
    pub(in super::super) fn validate(&self) -> Result<(), Error> {
        if self.version != 3
            || self.alerts as usize > MAX_ALERTS
            || self.notes as usize > MAX_NOTES
            || self.bytes > MAX_BYTES as u64
        {
            return Err(Error::Quarantined);
        }
        hash(&self.digest)
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Alert {
    pub dedupe_key: String,
    pub resource_id: String,
    pub summary: String,
    pub detail: String,
    pub runbook: String,
    pub impact: String,
    pub recovery_condition: String,
    pub occurrences: u64,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
    pub resolved_at_ms: Option<u64>,
    pub resolved_by: Option<String>,
    pub status: String,
    pub note: Option<String>,
    pub transitioned_at_ms: Option<u64>,
    pub transitioned_by: Option<String>,
    pub alert_type: String,
    pub severity: String,
    pub source: String,
    pub source_agent: Option<String>,
    pub assignee: Option<String>,
    pub suppress_until_ms: Option<u64>,
    pub linked_task_id: Option<String>,
    pub original_severity: Option<String>,
    pub missing_actionable_fields: String,
    pub tags: String,
    pub owner: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Note {
    pub dedupe_key: String,
    pub seq: u64,
    pub author: String,
    pub text: String,
    pub ts_ms: u64,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in super::super) struct History {
    pub(super) alerts: Vec<Alert>,
    pub(super) notes: Vec<Note>,
}
fn text(s: &str, max: usize) -> Result<(), Error> {
    if s.len() > max * 4 || s.chars().count() > max {
        return Err(Error::Capacity);
    }
    Ok(())
}
fn array(raw: &str, count: usize, length: usize) -> Result<(), Error> {
    let rows: Vec<String> = serde_json::from_str(raw)?;
    if rows.len() > count {
        return Err(Error::Capacity);
    }
    for row in rows {
        text(&row, length)?;
    }
    Ok(())
}
impl History {
    pub(in super::super) fn summary(&self, resources: &[Resource]) -> Result<Summary, Error> {
        if self.alerts.len() > MAX_ALERTS || self.notes.len() > MAX_NOTES {
            return Err(Error::Capacity);
        }
        let ids: std::collections::BTreeSet<String> = resources.iter().map(Resource::id).collect();
        let mut keys = std::collections::BTreeSet::new();
        let mut previous: Option<&str> = None;
        for a in &self.alerts {
            if a.dedupe_key.is_empty()
                || previous.is_some_and(|v| v >= a.dedupe_key.as_str())
                || !ids.contains(&a.resource_id)
            {
                return Err(Error::Quarantined);
            }
            previous = Some(&a.dedupe_key);
            keys.insert(a.dedupe_key.as_str());
            text(&a.dedupe_key, 256)?;
            text(&a.resource_id, 128)?;
            text(&a.summary, 4096)?;
            text(&a.detail, 4096)?;
            text(&a.runbook, 2048)?;
            text(&a.impact, 2048)?;
            text(&a.recovery_condition, 2048)?;
            if let Some(v) = &a.resolved_by {
                text(v, 128)?;
            }
            text(&a.status, 16)?;
            if let Some(v) = &a.note {
                text(v, 2048)?;
            }
            if let Some(v) = &a.transitioned_by {
                text(v, 128)?;
            }
            text(&a.alert_type, 128)?;
            text(&a.severity, 16)?;
            text(&a.source, 16)?;
            if let Some(v) = &a.source_agent {
                text(v, 128)?;
            }
            if let Some(v) = &a.assignee {
                text(v, 128)?;
            }
            if let Some(v) = &a.linked_task_id {
                text(v, 128)?;
            }
            if let Some(v) = &a.original_severity {
                text(v, 16)?;
            }
            text(&a.missing_actionable_fields, 4096)?;
            text(&a.tags, 4096)?;
            if let Some(v) = &a.owner {
                text(v, 128)?;
            }
            if !crate::ALERT_STATUSES.contains(&a.status.as_str())
                || !crate::domain::ceiling_alerts::ALERT_SEVERITIES.contains(&a.severity.as_str())
                || !["backend", "bridge", "supervisor", "system"].contains(&a.source.as_str())
                || a.original_severity
                    .as_deref()
                    .is_some_and(|v| !crate::domain::ceiling_alerts::ALERT_SEVERITIES.contains(&v))
                || a.occurrences == 0
            {
                return Err(Error::Quarantined);
            }
            for n in [
                Some(a.occurrences),
                Some(a.first_seen_ms),
                Some(a.last_seen_ms),
                a.resolved_at_ms,
                a.transitioned_at_ms,
                a.suppress_until_ms,
            ]
            .into_iter()
            .flatten()
            {
                if n > JSON_SAFE_MAX {
                    return Err(Error::Capacity);
                }
            }
            array(&a.tags, 20, 64)?;
            array(&a.missing_actionable_fields, 20, 128)?;
        }
        let mut previous: Option<(&str, u64)> = None;
        for n in &self.notes {
            let key = (n.dedupe_key.as_str(), n.seq);
            if !keys.contains(n.dedupe_key.as_str())
                || previous.is_some_and(|p| p >= key)
                || n.seq == 0
                || n.seq > JSON_SAFE_MAX
                || n.ts_ms > JSON_SAFE_MAX
            {
                return Err(Error::Quarantined);
            }
            previous = Some(key);
            text(&n.dedupe_key, 256)?;
            text(&n.author, 128)?;
            text(&n.text, 2048)?;
        }
        let raw = serde_json::to_vec(self)?;
        if raw.len() > MAX_BYTES {
            return Err(Error::Capacity);
        }
        Ok(Summary {
            version: 3,
            alerts: self.alerts.len() as u32,
            notes: self.notes.len() as u32,
            bytes: raw.len() as u64,
            digest: canonical::digest(&serde_json::to_value(self)?)?,
        })
    }
}
