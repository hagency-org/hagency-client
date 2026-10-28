//! Ceiling overrun alarms (ADR-124, slice a): one row per resource dedupe
//! key, raised and resolved from the same drawn figure admission enforces on.
//!
//! An alert is DIAGNOSTIC, never enforcement: raising one must not revoke or
//! end engagements, block or permit admission, release leases, or authorize
//! retries; auto-resolve flips the row's display state and nothing else.

use super::{DomainRepository, Error, usage::ceiling_report};
use hagency_core::JSON_SAFE_MAX;
use hagency_core::project::Resource;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

/// Retained bound (`lib/alert-store.js:25`): resolved alerts live 7 days.
const RESOLVED_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// Retained bound (`lib/alert-store.js:28`, `MAX_PAYLOAD_SIZE`): the detail
/// JSON string is capped at 4096 bytes.
const MAX_DETAIL_BYTES: usize = 4096;
/// Publication bound: one operator read returns at most this many open rows.
/// DELIBERATE DIVERGENCE from the retained `listAlerts` cap of 500
/// (`lib/alert-store.js:410`, `Math.min(parseInt(limit) || 100, 500)`): the
/// retained route CLAMPS an over-large limit, while native refuses it with
/// `Error::Invalid` the way every other bounded read in this store behaves —
/// a silently-clamped limit hides a client bug; a refusal surfaces it. The
/// native cap is 200, tighter than the retained 500 for the same reason
/// (bounded-cost reads on a table with at most one row per resource).
pub const MAX_OPEN_CEILING_ALERTS: usize = 200;
/// Counters one sweep produced. `raised` counts newly-open alerts (fresh
/// insert or reopen after resolution); `updated` counts repeats against an
/// already-open row; `resolved` and `pruned` count display-state and
/// retention transitions.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct SweepOutcome {
    pub raised: u64,
    pub updated: u64,
    pub resolved: u64,
    pub pruned: u64,
}

fn dedupe_key(resource_id: &str) -> String {
    format!("agent_ceiling_overrun:{resource_id}")
}

fn bounded(value: u64) -> Result<u64, Error> {
    if value > JSON_SAFE_MAX {
        return Err(Error::Capacity);
    }
    Ok(value)
}

/// Read the stored `detail` (E1 of the console alerts review): the retained
/// `truncatePayload` slices the JSON STRING (`alert-store.js:61-64`), so an
/// over-long row legitimately holds text that is not valid JSON — and the
/// retained consumer renders it as text (`mapAlert` passes `detail` through
/// unchanged, `mockup/lib/api.js:203`). The read therefore PARSES when it
/// can and FALLS BACK to the raw string when it cannot: never `Error::Schema`
/// — one truncated row must not blind the operator to every good one. The
/// client validator's union (object or ≤4096 string) is built for exactly
/// this payload.
fn read_detail(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()))
}

/// The retained ingest payload (`backend-v2.js:9422-9447`) with raw numbers:
/// `detail` is a JSON string, never an object, capped at 4096 bytes the way
/// the retained `truncatePayload` caps it (`lib/alert-store.js:61-64`):
/// string-encode, then slice to the first MAX_PAYLOAD_SIZE characters — a
/// truncated value stays a valid row (Node logs and continues; an aborting
/// sweep would let one long resource id suppress every other alert). The
/// column CHECK remains the last line of defence, never the truncation site.
fn detail_json(
    resource_id: &str,
    preset_name: &str,
    ceiling: u64,
    committed: u64,
    measured: Option<u64>,
    drawn: u64,
    over: u64,
) -> String {
    let detail = serde_json::json!({
        "agent": resource_id,
        "presetId": preset_name,
        "ceilingTokens": ceiling,
        "committedTokens": committed,
        "measuredTokens": measured,
        "drawnTokens": drawn,
        "overByTokens": over,
    });
    let text = serde_json::to_string(&detail).unwrap_or_default();
    if text.len() > MAX_DETAIL_BYTES {
        text.chars().take(MAX_DETAIL_BYTES).collect()
    } else {
        text
    }
}

/// Shared diagnostic reconciliation. This never changes admission or capacity.
fn reconcile_resource(db: &rusqlite::Connection, resource: &Resource, now: u64) -> Result<SweepOutcome, Error> {
    let mut outcome = SweepOutcome::default();
    // No declared ceiling is unknown, not zero: such a resource cannot
    // be past a limit that does not exist and is skipped entirely
    // (`backend-v2.js:9397-9400`).
    let Some(ceiling) = resource
        .ceiling
        .as_ref()
        .and_then(|c| c.tokens)
        .map(u64::from)
    else {
        return Ok(outcome);
    };
    let ceiling = bounded(ceiling)?;
    let report = ceiling_report(db, &resource.id(), now)?;
    let key = dedupe_key(&resource.id());
    if report.drawn > ceiling {
        let over = report.drawn - ceiling;
        let detail = detail_json(
            &resource.id(),
            &report.preset_name,
            ceiling,
            report.reserved,
            report.spent,
            report.drawn,
            over,
        );
        let summary = format!(
            "{} has drawn {} against a ceiling of {} — {} past it",
            resource.id(),
            report.drawn,
            ceiling,
            over
        );
        let runbook = format!(
            "raise the ceiling on preset {} to cover what is already committed, or revoke engagements on {} until the drawn figure is back under it",
            report.preset_name,
            resource.id()
        );
        let existing: Option<(bool, Option<String>, Option<u64>)> = db
            .query_row(
                "SELECT resolved_at_ms IS NOT NULL, status, suppress_until_ms FROM ceiling_alerts WHERE dedupe_key=?1",
                [&key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        // The preserved contract is one OPEN alert per resource,
        // occurrences riding on it. On dedupe-repeat and reopen the
        // retained store refreshes summary/lastPayload and the
        // counter but never rewrites runbook/impact/recoveryCondition
        // (`lib/alert-store.js:231-249,254-271`); only a fresh insert
        // writes the four text fields.
        match existing {
            None => {
                const IMPACT: &str = "no new engagement can be approved against this agent; the work already approved keeps running, because admission control cannot retract a commitment it already granted";
                const RECOVERY: &str = "the drawn figure falls back under the ceiling, by raising the ceiling or ending engagements — this alert auto-resolves when that happens";
                // The retained ingest files the full identity
                // (`backend-v2.js:9422-9447`): kind
                // `agent_ceiling_overrun`, severity `warning`,
                // source `backend`, sourceAgent the resource id.
                db.execute(
                    "INSERT INTO ceiling_alerts(dedupe_key,resource_id,summary,detail,runbook,impact,recovery_condition,occurrences,first_seen_ms,last_seen_ms,alert_type,severity,source,source_agent,tags) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?8,'agent_ceiling_overrun','warning','backend',?2,'[\"ceiling\",\"budget\"]')",
                    params![key, resource.id(), summary, detail, runbook, IMPACT, RECOVERY, now],
                )?;
                outcome.raised += 1;
            }
            Some((was_resolved, status, suppress_until)) => {
                let changed = db.execute(
                    if was_resolved {
                        // A resolved row re-raised reopens as a FRESH
                        // episode: display state reset beside the
                        // resolved columns the sweep always cleared.
                        "UPDATE ceiling_alerts SET resource_id=?2,summary=?3,detail=?4,occurrences=occurrences+1,last_seen_ms=?5,resolved_at_ms=NULL,resolved_by=NULL,status='open',note=NULL,transitioned_at_ms=NULL,transitioned_by=NULL,suppress_until_ms=NULL WHERE dedupe_key=?1"
                    } else if status.as_deref() == Some("suppressed")
                        && suppress_until.is_some_and(|until| now > until)
                    {
                        // The retained Bug-1 fix
                        // (`lib/alert-store.js:245-248`): a
                        // suppressed row reopens on a new occurrence
                        // once its `suppressUntil` has passed, and
                        // stays suppressed inside the window.
                        "UPDATE ceiling_alerts SET resource_id=?2,summary=?3,detail=?4,occurrences=occurrences+1,last_seen_ms=?5,status='open',suppress_until_ms=NULL WHERE dedupe_key=?1"
                    } else {
                        // An unresolved row rides occurrences and
                        // KEEPS its operator status; inside the
                        // suppression window the row stays
                        // suppressed, exactly like the retained
                        // store.
                        "UPDATE ceiling_alerts SET resource_id=?2,summary=?3,detail=?4,occurrences=occurrences+1,last_seen_ms=?5 WHERE dedupe_key=?1"
                    },
                    params![key, resource.id(), summary, detail, now],
                )?;
                if was_resolved {
                    outcome.raised += 1;
                } else {
                    outcome.updated += 1;
                }
                debug_assert_eq!(changed, 1);
            }
        }
    } else {
        // Back under (or exactly on) the ceiling resolves by the same
        // rule the ingest path would use, not a hand-rolled
        // transition; a no-op when no alert is open
        // (`lib/alert-store.js:337-355`). Exactly-on is not over,
        // which is also why the raise side is strictly greater.
        let changed = db.execute(
            "UPDATE ceiling_alerts SET status='resolved',resolved_at_ms=?2,resolved_by='system' WHERE dedupe_key=?1 AND resolved_at_ms IS NULL",
            params![key, now],
        )?;
        outcome.resolved += u64::try_from(changed).unwrap_or_default();
    }
    Ok(outcome)
}

impl DomainRepository {
    /// Sweep every resource with a declared finite ceiling and reconcile its
    /// overrun alert (`backend-v2.js:9393-9452`, slice a of the alarm plan):
    /// strictly `drawn > ceiling` raises, `drawn <= ceiling` auto-resolves,
    /// repeats increment `occurrences`, a re-over after resolution reopens
    /// the same row, and resolved rows older than 7 days are pruned. One
    /// `Immediate` transaction. The draw comes from the read-side projection
    /// `usage::ceiling_report` (`resource_ceiling`, ADR-121) — the SAME drawn
    /// rule admission enforces on (`max(reserved, spent)`, unknown falls back
    /// to reserved, `backend-v2.js:14052-14053` cited by both), though a
    /// distinct code path from `budget()`/`resource_budget`, exactly the
    /// retained split: Node's sweep reads `ceilingSpendFor` while admission
    /// reads `remainingFor` (`backend-v2.js:9402` vs `:14823`). The two
    /// agree by shared rule and oracle, not by construction.
    pub fn sweep_ceiling_overruns(&mut self, now: u64) -> Result<SweepOutcome, Error> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut outcome = SweepOutcome::default();
        let mut statement = tx.prepare("SELECT config FROM resources")?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for row in rows {
            let resource: Resource = serde_json::from_str(&row)?;
            let observed = reconcile_resource(&tx, &resource, now)?;
            outcome.raised += observed.raised;
            outcome.updated += observed.updated;
            outcome.resolved += observed.resolved;
        }
        // Retention (`ALERT_RESOLVED_TTL_MS` parity): resolved rows older
        // than 7 days are pruned whole.
        let cutoff = now.saturating_sub(RESOLVED_RETENTION_MS);
        let pruned = tx.execute(
            "DELETE FROM ceiling_alerts WHERE resolved_at_ms IS NOT NULL AND resolved_at_ms<?1",
            params![cutoff],
        )?;
        outcome.pruned = u64::try_from(pruned).unwrap_or_default();
        tx.commit()?;
        Ok(outcome)
    }

    /// One authorized resource per writer job. Shared canonical owners must not
    /// sweep or prune another profile's resource. The authority closure is checked
    /// inside the same transaction both before and after the observation.
    pub(crate) fn observe_resource_alert(
        &mut self,
        resource_id: &str,
        now: u64,
        check: impl Fn(&rusqlite::Connection) -> Result<(), Error>,
    ) -> Result<SweepOutcome, Error> {
        bounded(now)?;
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        check(&tx)?;
        let config: String = tx.query_row("SELECT config FROM resources WHERE id=?1", [resource_id], |row| row.get(0)).optional()?.ok_or(Error::NotFound)?;
        let resource: Resource = serde_json::from_str(&config)?;
        if resource.id() != resource_id { return Err(Error::Schema); }
        let mut outcome = reconcile_resource(&tx, &resource, now)?;
        outcome.pruned = tx.execute("DELETE FROM ceiling_alerts WHERE resource_id=?1 AND resolved_at_ms IS NOT NULL AND resolved_at_ms<?2", params![resource_id, now.saturating_sub(RESOLVED_RETENTION_MS)])? as u64;
        check(&tx)?;
        tx.commit().map_err(|_| Error::OutcomeUnknown)?;
        Ok(outcome)
    }

    /// Open alerts for the operator read (ADR-124 slice b), newest activity
    /// first, at most `MAX_OPEN_CEILING_ALERTS` rows. A limit of 0 or above
    /// the bound is refused with `Error::Invalid` — a deliberate divergence
    /// from the retained route's clamping (`alert-store.js:410`), chosen for
    /// consistency with every other bounded read in this store. The stored
    /// `detail` string is PARSED
    /// here: a corrupt row is `Error::Schema`, never a silently-empty object.
    pub fn open_ceiling_alerts(&self, limit: u32) -> Result<Vec<CeilingAlert>, Error> {
        let limit = usize::try_from(limit)
            .map_err(|_| Error::Invalid(hagency_core::InvalidInput("invalid alert limit")))?;
        if limit == 0 || limit > MAX_OPEN_CEILING_ALERTS {
            return Err(Error::Invalid(hagency_core::InvalidInput(
                "invalid alert limit",
            )));
        }
        let mut statement = self.db.prepare(&format!(
            "SELECT {ALERT_COLUMNS} FROM ceiling_alerts WHERE resolved_at_ms IS NULL ORDER BY last_seen_ms DESC LIMIT ?1"
        ))?;
        let rows = statement
            .query_map(params![limit], alert_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The retained list route's filter set (`alert-store.js:388-414`):
    /// comma-separated `status`/`severity`, exact `sourceAgent`/`alertType`/
    /// `assignee`, sorted by `lastSeenAt` desc, then paginated. SQL applies
    /// the exact-match filters; the comma lists and pagination are applied
    /// after the read so the retained clamp (`min(parseInt(limit)||100,
    /// 500)`) decides the page size, not SQL.
    pub fn list_alerts(&self, filter: &AlertListFilter) -> Result<Vec<CeilingAlert>, Error> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {ALERT_COLUMNS} FROM ceiling_alerts
             WHERE (?1 IS NULL OR source_agent = ?1)
               AND (?2 IS NULL OR alert_type = ?2)
               AND (?3 IS NULL OR assignee = ?3)
             ORDER BY last_seen_ms DESC"
        ))?;
        let mut rows = statement
            .query_map(
                params![filter.source_agent, filter.alert_type, filter.assignee,],
                alert_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(statuses) = &filter.statuses {
            rows.retain(|a| statuses.iter().any(|s| s == &a.status));
        }
        if let Some(severities) = &filter.severities {
            rows.retain(|a| severities.iter().any(|s| s == &a.severity));
        }
        let limit = filter.limit.unwrap_or(100).min(500) as usize;
        let offset = filter.offset.unwrap_or(0) as usize;
        Ok(rows.into_iter().skip(offset).take(limit).collect())
    }

    /// One alert by dedupe key (`GET /api/alerts/:id`,
    /// `alert-store.js:383-385`): the row plus its notes history.
    pub fn get_alert(&self, key: &str) -> Result<(CeilingAlert, Vec<AlertNote>), Error> {
        let alert = read_alert(&self.db, key)?;
        let notes = self.alert_notes(key)?;
        Ok((alert, notes))
    }

    /// The retained notes history (`alert-store.js:456-459`), oldest first.
    pub fn alert_notes(&self, key: &str) -> Result<Vec<AlertNote>, Error> {
        let mut statement = self.db.prepare(
            "SELECT author,text,ts_ms FROM ceiling_alert_notes WHERE dedupe_key=?1 ORDER BY seq",
        )?;
        let rows = statement
            .query_map([key], |row| {
                Ok(AlertNote {
                    author: row.get(0)?,
                    text: row.get(1)?,
                    ts_ms: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// `addNote` parity (`alert-store.js:447-462`): 404 when the alert is
    /// missing, 400 (`bad_request`) on empty text after the retained
    /// trim-and-bound, `author` defaulting to `anonymous` when blank —
    /// note the retained ROUTE defaults `author` to 'operator'
    /// (`backend-v2.js:16133`), so the route passes its own default.
    pub fn add_alert_note(
        &mut self,
        key: &str,
        author: &str,
        text: &str,
        now: u64,
    ) -> Result<(CeilingAlert, Vec<AlertNote>), Error> {
        let text = text.trim().chars().take(2048).collect::<String>();
        if text.is_empty() {
            return Err(hagency_core::InvalidInput("note text required").into());
        }
        let author = author.trim().chars().take(128).collect::<String>();
        let author = if author.is_empty() {
            "anonymous".to_owned()
        } else {
            author
        };
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: Option<u8> = tx
            .query_row(
                "SELECT 1 FROM ceiling_alerts WHERE dedupe_key=?1",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(Error::NotFound);
        }
        let seq: u64 = tx.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM ceiling_alert_notes WHERE dedupe_key=?1",
            [key],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO ceiling_alert_notes(dedupe_key,seq,author,text,ts_ms) VALUES(?1,?2,?3,?4,?5)",
            params![key, seq, author, text, now],
        )?;
        // Keep the 025 latest-note column in step: the console transition
        // renders it, and the retained `notes` array's tail IS the latest.
        tx.execute(
            "UPDATE ceiling_alerts SET note=?2 WHERE dedupe_key=?1",
            params![key, text],
        )?;
        let alert = read_alert(&tx, key)?;
        let notes = alert_notes_tx(&tx, key)?;
        tx.commit()?;
        Ok((alert, notes))
    }

    /// `updateAlert` parity (`alert-store.js:464-525`): tags bounded (64
    /// chars, ≤20), linkedTaskId (128), owner/assignee/runbook/impact/
    /// recoveryCondition, and — when any actionable field moves — the
    /// retained actionability rebuild: a warning/critical missing any of
    /// owner+assignee, runbook, impact, recoveryCondition is filed as
    /// `info` with `originalSeverity` and `missingActionableFields`
    /// recorded (`alert-store.js:102-138`); supplying them restores the
    /// original severity.
    pub fn update_alert(&mut self, key: &str, patch: &AlertPatch) -> Result<CeilingAlert, Error> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(String, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT severity, original_severity, status FROM ceiling_alerts WHERE dedupe_key=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((severity, original_severity, _status)) = current else {
            return Err(Error::NotFound);
        };
        if let Some(tags) = &patch.tags {
            let tags: Vec<String> = tags
                .iter()
                .map(|t| t.trim().chars().take(64).collect::<String>())
                .filter(|t| !t.is_empty())
                .take(20)
                .collect();
            tx.execute(
                "UPDATE ceiling_alerts SET tags=?2 WHERE dedupe_key=?1",
                params![key, serde_json::to_string(&tags).unwrap_or_default()],
            )?;
        }
        if let Some(linked) = &patch.linked_task_id {
            let linked = linked.trim().chars().take(128).collect::<String>();
            let linked = if linked.is_empty() {
                None
            } else {
                Some(linked)
            };
            tx.execute(
                "UPDATE ceiling_alerts SET linked_task_id=?2 WHERE dedupe_key=?1",
                params![key, linked],
            )?;
        }
        for (column, value, bound) in [
            ("source_agent", &patch.source_agent, 128usize),
            ("assignee", &patch.assignee, 128),
            ("runbook", &patch.runbook, 512),
            ("impact", &patch.impact, 1024),
            ("recovery_condition", &patch.recovery_condition, 1024),
        ] {
            if let Some(value) = value {
                let trimmed = value.trim().chars().take(bound).collect::<String>();
                let value = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                };
                tx.execute(
                    &format!("UPDATE ceiling_alerts SET {column}=?2 WHERE dedupe_key=?1"),
                    params![key, value],
                )?;
            }
        }
        // The actionability rebuild (`alert-store.js:486-514`): it runs
        // whenever an actionable field was touched, against the requested
        // severity (the original when a downgrade is recorded).
        let action_touched = patch.assignee.is_some()
            || patch.runbook.is_some()
            || patch.impact.is_some()
            || patch.recovery_condition.is_some()
            || patch.owner.is_some();
        if action_touched {
            let requested = original_severity.unwrap_or(severity);
            let owner = patch
                .owner
                .clone()
                .map(|o| o.trim().chars().take(128).collect::<String>())
                .filter(|o| !o.is_empty());
            #[allow(clippy::type_complexity)]
            let row: (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>) = tx
                .query_row(
                    "SELECT assignee, runbook, impact, recovery_condition, resource_id FROM ceiling_alerts WHERE dedupe_key=?1",
                    [key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )?;
            let (assignee, runbook, impact, recovery, _resource) = row;
            let mut missing: Vec<String> = Vec::new();
            let paging = requested == "warning" || requested == "critical";
            if paging {
                if assignee.is_none() && owner.is_none() {
                    missing.push("owner".to_owned());
                }
                if runbook.is_none() {
                    missing.push("runbook".to_owned());
                }
                if impact.is_none() {
                    missing.push("impact".to_owned());
                }
                if recovery.is_none() {
                    missing.push("recoveryCondition".to_owned());
                }
            }
            let (severity, original): (&str, Option<&str>) = if paging && !missing.is_empty() {
                ("info", Some(requested.as_str()))
            } else {
                (requested.as_str(), None)
            };
            tx.execute(
                "UPDATE ceiling_alerts SET severity=?2, original_severity=?3, missing_actionable_fields=?4 WHERE dedupe_key=?1",
                params![
                    key,
                    severity,
                    original,
                    serde_json::to_string(&missing).unwrap_or_default()
                ],
            )?;
        }
        let alert = read_alert(&tx, key)?;
        tx.commit()?;
        Ok(alert)
    }

    /// `deleteAlert` parity (`alert-store.js:527-538`): remove the row and
    /// return it; NOT_FOUND when the id is unknown (the retained route
    /// answers 404 `alert not found`, `backend-v2.js:16152-16154`).
    pub fn delete_alert(&mut self, key: &str) -> Result<CeilingAlert, Error> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let alert = read_alert(&tx, key)?;
        tx.execute("DELETE FROM ceiling_alerts WHERE dedupe_key=?1", [key])?;
        tx.commit()?;
        Ok(alert)
    }

    /// `getStats` parity (`alert-store.js:540-552`): every status bucket
    /// zero-seeded over the retained five states; severity buckets count
    /// non-resolved rows only.
    pub fn alert_stats(&self) -> Result<AlertStats, Error> {
        let mut by_status: std::collections::BTreeMap<String, u64> = ALERT_STATUSES
            .iter()
            .map(|s| ((*s).to_owned(), 0))
            .collect();
        let mut by_severity: std::collections::BTreeMap<String, u64> = ALERT_SEVERITIES
            .iter()
            .map(|s| ((*s).to_owned(), 0))
            .collect();
        let mut statement = self
            .db
            .prepare("SELECT status, severity, resolved_at_ms FROM ceiling_alerts")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<u64>>(2)?,
            ))
        })?;
        let mut total = 0u64;
        for row in rows {
            let (status, severity, resolved_at) = row?;
            total += 1;
            *by_status.entry(status).or_insert(0) += 1;
            if resolved_at.is_none() {
                *by_severity.entry(severity).or_insert(0) += 1;
            }
        }
        Ok(AlertStats {
            total,
            by_status,
            by_severity,
        })
    }
}

/// One overrun alert with every retained field (`backend-v2.js:9422-9447`,
/// `lib/alert-store.js:300-328`): `detail` is the parsed object, not the
/// stored string; the resolved state is carried so the projection can state
/// it without a second column. The display-state columns (migration 025)
/// and the retained identity/severity/assignee/suppression columns
/// (migration 045, task #24) ride along.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct CeilingAlert {
    pub dedupe_key: String,
    pub resource_id: String,
    pub summary: String,
    pub detail: serde_json::Value,
    pub runbook: String,
    pub impact: String,
    pub recovery_condition: String,
    pub occurrences: u64,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
    pub resolved: bool,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub transitioned_at_ms: Option<u64>,
    #[serde(default)]
    pub transitioned_by: Option<String>,
    /// The retained `alertType` (`lib/alert-store.js:301`): ceiling rows
    /// always carry `agent_ceiling_overrun`.
    #[serde(default = "default_alert_type")]
    pub alert_type: String,
    /// The retained `severity` (`alert-store.js:5`): the ceiling sweep
    /// always files `warning` (`backend-v2.js:9427`).
    #[serde(default = "default_severity")]
    pub severity: String,
    /// The retained `source` (`alert-store.js:6`): `backend` for the sweep.
    #[serde(default = "default_source")]
    pub source: String,
    /// The retained `sourceAgent` (`alert-store.js:306`): the resource id.
    #[serde(default)]
    pub source_agent: Option<String>,
    /// The retained `assignee` (`alert-store.js:317,430`): set by a
    /// transition into `assigned` or by the update route.
    #[serde(default)]
    pub assignee: Option<String>,
    /// The retained `suppressUntil` (`alert-store.js:433-434`): an absolute
    /// millisecond deadline, or NULL for operator-released suppression.
    #[serde(default)]
    pub suppress_until_ms: Option<u64>,
    /// The retained `linkedTaskId` (`alert-store.js:477-478`).
    #[serde(default)]
    pub linked_task_id: Option<String>,
    /// The retained `originalSeverity` (`alert-store.js:132-133`): the
    /// severity the caller asked for when the store downgraded a
    /// warning/critical for missing actionable fields; NULL when filed at
    /// the requested severity.
    #[serde(default)]
    pub original_severity: Option<String>,
    /// The retained `missingActionableFields` (`alert-store.js:116-136`).
    #[serde(default)]
    pub missing_actionable_fields: Vec<String>,
    /// The retained `owner` (`alert-store.js:317,470`).
    #[serde(default)]
    pub owner: Option<String>,
    /// The retained `tags` (`alert-store.js:317`).
    #[serde(default)]
    pub tags: Vec<String>,
}
fn default_status() -> String {
    "open".into()
}
fn default_alert_type() -> String {
    "agent_ceiling_overrun".into()
}
fn default_severity() -> String {
    "warning".into()
}
fn default_source() -> String {
    "backend".into()
}

/// One retained alert note (`lib/alert-store.js:456-459`): author, text,
/// and the statement time.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct AlertNote {
    pub author: String,
    pub text: String,
    pub ts_ms: u64,
}

/// The ONE legal-transition map (task #24, `lib/alert-store.js:4-14`
/// parity): the retained FIVE states, `assigned` restored. Server-owned,
/// served to every consumer — the store, both routes and the page's buttons
/// all derive from this single definition. The retained console's
/// `NEXT_STATUS` (mockup/app/alerts/page.jsx:31-37) DIVERGES from the
/// retained store's `TRANSITIONS` (it offers `acknowledged→suppressed` and
/// `assigned→suppressed`, which the store refuses, and hides
/// `suppressed→assigned`, which it allows); that drift is NOT ported — this
/// map matches the retained STORE exactly, `resolved` terminal.
pub const ALERT_STATUSES: [&str; 5] =
    ["open", "acknowledged", "assigned", "resolved", "suppressed"];
/// `lib/alert-store.js:5` parity.
pub const ALERT_SEVERITIES: [&str; 3] = ["info", "warning", "critical"];
/// `TRANSITIONS`, `lib/alert-store.js:8-14`: open→{acknowledged, assigned,
/// resolved, suppressed}; acknowledged→{assigned, resolved}; assigned→
/// {resolved}; suppressed→{open, assigned}; resolved terminal.
pub fn allowed_transitions(from: &str) -> &'static [&'static str] {
    match from {
        "open" => &["acknowledged", "assigned", "resolved", "suppressed"],
        "acknowledged" => &["assigned", "resolved"],
        "assigned" => &["resolved"],
        "suppressed" => &["open", "assigned"],
        _ => &[],
    }
}
/// The retained suppression default (`ALERT_SUPPRESS_DEFAULT_MS`,
/// `lib/alert-store.js:26`): 24 hours, applied when the operator
/// transition carries no explicit `suppressUntil`.
pub const ALERT_SUPPRESS_DEFAULT_MS: u64 = 24 * 60 * 60 * 1000;
/// A transition is DISPLAY STATE ONLY: it mutates how the alert renders and
/// nothing else — no admission, lease, engagement or retry consults it.
/// `assignee` and `suppress_until_ms` ride the transition exactly like the
/// retained `meta.assignee` / `meta.suppressUntil`
/// (`lib/alert-store.js:427-437`).
#[derive(serde::Serialize)]
pub struct AlertTransition {
    pub key: String,
    pub to: &'static str,
    pub actor: String,
    pub note: Option<String>,
    pub assignee: Option<String>,
    pub suppress_until_ms: Option<u64>,
    pub now: u64,
}
impl DomainRepository {
    /// Apply one operator display-state transition (`lib/alert-store.js:415-439`
    /// parity): legal pairs only (`bad_transition` otherwise), `resolved` sets
    /// `resolved_at_ms`/`resolved_by` to the actor like the retained
    /// `meta.actor || 'operator'`; `assigned` adopts `meta.assignee`
    /// (normalized to 128) when one is supplied; `suppressed` sets
    /// `suppressUntil = meta.suppressUntil || now + ALERT_SUPPRESS_DEFAULT_MS`;
    /// reopening to `open` clears the suppression window exactly like the
    /// retained store. One `Immediate` transaction, same as the sweep — a
    /// refused transition writes nothing.
    pub fn transition_ceiling_alert(
        &mut self,
        command: AlertTransition,
    ) -> Result<CeilingAlert, Error> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let alert = transition_in_transaction(&tx, command)?;
        tx.commit()?;
        Ok(alert)
    }
}

fn transition_in_transaction(tx: &rusqlite::Connection, command: AlertTransition) -> Result<CeilingAlert, Error> {
        let AlertTransition {
            key,
            to,
            actor,
            note,
            assignee,
            suppress_until_ms,
            now,
        } = command;
        let actor = if actor.is_empty() {
            "operator".to_owned()
        } else {
            actor
        };
        let assignee = assignee.map(|a| a.trim().chars().take(128).collect::<String>());
        if actor.len() > 128 || note.as_deref().is_some_and(|n| n.len() > 2048) {
            return Err(hagency_core::InvalidInput("invalid alert transition").into());
        }
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM ceiling_alerts WHERE dedupe_key=?1",
                [&key],
                |r| r.get(0),
            )
            .optional()?;
        let Some(status) = status else {
            return Err(Error::NotFound);
        };
        if !allowed_transitions(&status).contains(&to) {
            return Err(hagency_core::InvalidInput("bad_transition").into());
        }
        let resolved_at = (to == "resolved").then_some(now);
        // `suppressed` carries the retained window (`alert-store.js:433-434`);
        // `open` clears it (`alert-store.js:435-437`); every other target
        // leaves the stored window alone.
        let suppress_until = match to {
            "suppressed" => {
                Some(Some(suppress_until_ms.unwrap_or_else(|| {
                    now.saturating_add(ALERT_SUPPRESS_DEFAULT_MS)
                })))
            }
            "open" => Some(None),
            _ => None,
        };
        let changed = tx.execute(
            "UPDATE ceiling_alerts SET status=?2,note=?3,transitioned_at_ms=?4,transitioned_by=?5,resolved_at_ms=COALESCE(?6,resolved_at_ms),resolved_by=CASE WHEN ?6 IS NOT NULL THEN ?7 ELSE resolved_by END,assignee=CASE WHEN ?8 THEN COALESCE(?9,assignee) ELSE assignee END,suppress_until_ms=CASE WHEN ?10 THEN ?11 ELSE suppress_until_ms END WHERE dedupe_key=?1",
            rusqlite::params![
                key,
                to,
                note,
                now,
                actor,
                resolved_at,
                actor,
                to == "assigned",
                assignee,
                suppress_until.is_some(),
                suppress_until.flatten(),
            ],
        )?;
        debug_assert_eq!(changed, 1);
        // A note carried by the transition is filed in the notes history
        // exactly like the retained `addNote` (`alert-store.js:447-462`).
        if let Some(text) = note.as_deref().filter(|t| !t.trim().is_empty()) {
            let seq: u64 = tx.query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM ceiling_alert_notes WHERE dedupe_key=?1",
                [&key],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO ceiling_alert_notes(dedupe_key,seq,author,text,ts_ms) VALUES(?1,?2,?3,?4,?5)",
                params![key, seq, actor, text.chars().take(2048).collect::<String>(), now],
            )?;
        }
        let alert = read_alert(tx, &key)?;
        Ok(alert)
}

/// The one column list every alert read selects (migration 045 shape), in
/// declaration order — the row parser below consumes exactly this order.
const ALERT_COLUMNS: &str = "dedupe_key,resource_id,summary,detail,runbook,impact,recovery_condition,occurrences,first_seen_ms,last_seen_ms,resolved_at_ms,resolved_by,status,note,transitioned_at_ms,transitioned_by,alert_type,severity,source,source_agent,assignee,suppress_until_ms,linked_task_id,original_severity,missing_actionable_fields,owner,tags";

/// Parse one row selected with `ALERT_COLUMNS` into the wire struct.
/// `missing_actionable_fields`/`tags` are stored as JSON arrays the way the
/// retained record carries them (`alert-store.js:317`); a corrupt cell is
/// `Error::Schema`, never a silently-empty list.
fn alert_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CeilingAlert> {
    let detail: String = row.get(3)?;
    let missing: String = row.get(24)?;
    let tags: String = row.get(26)?;
    let resolved_at_ms: Option<u64> = row.get(10)?;
    Ok(CeilingAlert {
        dedupe_key: row.get(0)?,
        resource_id: row.get(1)?,
        summary: row.get(2)?,
        detail: read_detail(&detail),
        runbook: row.get(4)?,
        impact: row.get(5)?,
        recovery_condition: row.get(6)?,
        occurrences: row.get::<_, i64>(7)?.try_into().unwrap_or_default(),
        first_seen_ms: row.get(8)?,
        last_seen_ms: row.get(9)?,
        resolved: resolved_at_ms.is_some(),
        status: row.get(12)?,
        note: row.get(13)?,
        transitioned_at_ms: row.get(14)?,
        transitioned_by: row.get(15)?,
        alert_type: row.get(16)?,
        severity: row.get(17)?,
        source: row.get(18)?,
        source_agent: row.get(19)?,
        assignee: row.get(20)?,
        suppress_until_ms: row.get(21)?,
        linked_task_id: row.get(22)?,
        original_severity: row.get(23)?,
        missing_actionable_fields: serde_json::from_str(&missing).unwrap_or_default(),
        owner: row.get(25)?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
    })
}

/// The retained list filters (`alert-store.js:388-414`): comma-separated
/// status/severity lists, exact sourceAgent/alertType/assignee matches,
/// and pagination with the retained default 100 / cap 500.
#[derive(Debug, Default)]
pub struct AlertListFilter {
    pub statuses: Option<Vec<String>>,
    pub severities: Option<Vec<String>>,
    pub source_agent: Option<String>,
    pub alert_type: Option<String>,
    pub assignee: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// The retained PATCH body (`alert-store.js:464-525`): every field
/// optional; an absent field is untouched, `null` clears it.
#[derive(Debug, Default)]
pub struct AlertPatch {
    pub tags: Option<Vec<String>>,
    pub linked_task_id: Option<String>,
    pub owner: Option<String>,
    pub assignee: Option<String>,
    pub source_agent: Option<String>,
    pub runbook: Option<String>,
    pub impact: Option<String>,
    pub recovery_condition: Option<String>,
}

/// The retained stats payload (`alert-store.js:540-552`).
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct AlertStats {
    pub total: u64,
    pub by_status: std::collections::BTreeMap<String, u64>,
    pub by_severity: std::collections::BTreeMap<String, u64>,
}

/// The notes history read inside a transaction (`add_alert_note` reads the
/// history it just wrote before committing), the same SELECT the public
/// `alert_notes` runs. A `&Transaction` derefs to `&Connection`, so this
/// one helper serves both.
fn alert_notes_tx(db: &rusqlite::Connection, key: &str) -> Result<Vec<AlertNote>, Error> {
    let mut statement = db.prepare(
        "SELECT author,text,ts_ms FROM ceiling_alert_notes WHERE dedupe_key=?1 ORDER BY seq",
    )?;
    let rows = statement
        .query_map([key], |row| {
            Ok(AlertNote {
                author: row.get(0)?,
                text: row.get(1)?,
                ts_ms: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// One alert row by dedupe key, through the read's own row type.
fn read_alert(db: &rusqlite::Connection, key: &str) -> Result<CeilingAlert, Error> {
    db.query_row(
        &format!("SELECT {ALERT_COLUMNS} FROM ceiling_alerts WHERE dedupe_key=?1"),
        [key],
        alert_from_row,
    )
    .map_err(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => Error::NotFound,
        other => other.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B4: the `detail` JSON string truncates the way the retained
    /// `truncatePayload` does (`lib/alert-store.js:61-64`: encode, then slice
    /// to the first MAX_PAYLOAD_SIZE characters) instead of aborting the
    /// sweep. Valid sweep inputs cannot reach the bound — preset ids are
    /// ≤128 chars and resource ids are fixed-length hashes — so the bound is
    /// defence-in-depth, and this pins it at the function that owns it. The
    /// retained sweep files its alert with the truncated detail and
    /// continues; the CHECK constraint is the last line of defence, never
    /// the truncation site.
    #[test]
    fn native_ceiling_alert_detail_truncates_like_retained_store() {
        let short = detail_json("resource_a", "pool", 1_000, 100, None, 1_500, 500);
        assert!(short.len() <= MAX_DETAIL_BYTES);
        assert!(short.starts_with('{'));
        // The over-long id path: an absurd agent name pushes the encoded
        // detail past the cap; the result is still exactly-capped and the
        // caller proceeds (no Error::Capacity, no sweep abort).
        let absurd = "a".repeat(8_192);
        let long = detail_json(&absurd, "pool", 1_000, 100, None, 1_500, 500);
        assert!(
            long.len() <= MAX_DETAIL_BYTES,
            "capped at {MAX_DETAIL_BYTES}"
        );
        assert_eq!(long.chars().count(), MAX_DETAIL_BYTES);
        assert!(long.starts_with('{'), "a prefix of the encoded JSON");
        // The retained rule slices characters, not bytes: an ASCII slice is
        // both. (The composed detail is all-ASCII digits/keys in practice.)
        assert!(
            long.is_ascii(),
            "an ASCII slice is characters and bytes alike"
        );
    }
}

pub(crate) mod management;
