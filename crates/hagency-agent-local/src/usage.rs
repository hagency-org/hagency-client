//! Owner-local usage history and budget alerts. Cached input is contained in
//! input; reasoning output is contained in output. Neither is charged twice.
use crate::{
    Error, Ledger, Limit, Period, Policy, Result, Scope, Snapshot, UNREPORTED_ZERO, Usage, hash,
    key,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub(crate) const SCHEMA: &str = include_str!("usage/schema.sql");

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Day,
    Month,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub agent: String,
    pub binding: Option<String>,
    pub requester: Option<String>,
    pub from: i64,
    pub until: i64,
    pub group: Group,
    #[serde(default)]
    pub alerts_before: Option<String>,
}
impl Query {
    pub fn validate(&self) -> Result<()> {
        key(&self.agent)?;
        if let Some(cursor) = &self.alerts_before {
            key(cursor)?;
        }
        if let Some(binding) = &self.binding {
            key(binding)?;
        }
        if let Some(requester) = &self.requester {
            key(requester)?;
            if !requester.starts_with('@') || !requester.contains(':') {
                return Err(Error::Invalid("requester"));
            }
        }
        if self.from < 0
            || self.until <= self.from
            || self.until - self.from > 366 * 86400 * 10
            || (self.group == Group::Day && self.until - self.from > 366 * 86400)
        {
            return Err(Error::Invalid("usage time range"));
        }
        time::OffsetDateTime::from_unix_timestamp(self.from).map_err(|_| Error::Invalid("time"))?;
        time::OffsetDateTime::from_unix_timestamp(self.until)
            .map_err(|_| Error::Invalid("time"))?;
        Ok(())
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub calls: u64,
    pub input: u64,
    pub output: u64,
    pub cached_input: u64,
    pub reasoning_output: u64,
    pub pending_calls: u64,
    pub unreported_calls: u64,
    pub reserved: u64,
}
impl Totals {
    fn add(&mut self, other: &Self) -> Result<()> {
        for (to, value) in [
            (&mut self.calls, other.calls),
            (&mut self.input, other.input),
            (&mut self.output, other.output),
            (&mut self.cached_input, other.cached_input),
            (&mut self.reasoning_output, other.reasoning_output),
            (&mut self.pending_calls, other.pending_calls),
            (&mut self.unreported_calls, other.unreported_calls),
            (&mut self.reserved, other.reserved),
        ] {
            *to = to
                .checked_add(value)
                .ok_or(Error::Invalid("usage overflow"))?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub window: String,
    pub totals: Totals,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    pub id: String,
    pub scope: Vec<String>,
    pub window: String,
    pub revision: i64,
    pub threshold: u64,
    pub token_limit: u64,
    pub spent: u64,
    pub held: u64,
    pub observed_at: i64,
    pub acknowledged_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub query: Query,
    pub totals: Totals,
    pub buckets: Vec<Bucket>,
    /// Pre-migration records have no known date; never guess from file mtime.
    pub undated: Totals,
    /// Most recent 100 matching alerts; acknowledged records remain auditable.
    pub alerts: Vec<Alert>,
    pub alerts_truncated: bool,
    pub alerts_next: Option<String>,
}
impl Ledger {
    /// A consistent owner-only report. Refreshes alerts from current committed
    /// account balances, including a newly lowered budget, in this transaction.
    pub fn usage_report(&mut self, owner: &str, query: &Query, now: i64) -> Result<Report> {
        self.owner(owner)?;
        query.validate()?;
        if now < 0 {
            return Err(Error::Invalid("time"));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        refresh_agent(&tx, query, now)?;
        let mut buckets: BTreeMap<String, Totals> = BTreeMap::new();
        let mut totals = Totals::default();
        let mut undated = Totals::default();
        let sql = "SELECT o.started_at,c.state,c.reserved,c.usage FROM calls c JOIN bindings b ON b.id=c.binding LEFT JOIN call_observations o ON o.call_id=c.id WHERE b.agent=?1 AND (?2 IS NULL OR c.binding=?2) AND (?3 IS NULL OR json_extract(c.scope,'$.requester')=?3) AND (o.started_at IS NULL OR (o.started_at>=?4 AND o.started_at<?5)) ORDER BY o.started_at,c.id";
        let mut stmt = tx.prepare(sql)?;
        let mut rows = stmt.query((
            &query.agent,
            &query.binding,
            &query.requester,
            query.from,
            query.until,
        ))?;
        while let Some(row) = rows.next()? {
            let stamp: Option<i64> = row.get(0)?;
            let state: String = row.get(1)?;
            let reserved: u64 = row.get(2)?;
            let usage: Option<String> = row.get(3)?;
            let mut part = Totals {
                calls: 1,
                ..Default::default()
            };
            if state == "settled" {
                let usage: Usage = serde_json::from_str(usage.as_deref().ok_or(Error::Conflict)?)?;
                usage.total()?;
                // These rows prove a refused execution for replay protection;
                // they are not provider invocations or missing-usage receipts.
                if usage.accounting_version == "no-provider-call-v1" {
                    continue;
                }
                part.input = usage.input;
                part.output = usage.output;
                part.cached_input = usage.cached_input;
                part.reasoning_output = usage.reasoning_output;
                part.unreported_calls = u64::from(usage.accounting_version == UNREPORTED_ZERO);
            } else {
                part.pending_calls = 1;
                part.reserved = reserved;
            }
            if let Some(stamp) = stamp {
                let date = time::OffsetDateTime::from_unix_timestamp(stamp)
                    .map_err(|_| Error::Invalid("time"))?;
                let window = match query.group {
                    Group::Day => format!(
                        "{:04}-{:02}-{:02}",
                        date.year(),
                        u8::from(date.month()),
                        date.day()
                    ),
                    Group::Month => format!("{:04}-{:02}", date.year(), u8::from(date.month())),
                };
                buckets.entry(window).or_default().add(&part)?;
                totals.add(&part)?;
            } else {
                undated.add(&part)?;
            }
        }
        drop(rows);
        drop(stmt);
        let mut alerts = list_alerts(&tx, query)?;
        let alerts_truncated = alerts.len() > 100;
        alerts.truncate(100);
        let alerts_next = alerts_truncated.then(|| alerts.last().unwrap().id.clone());
        tx.commit()?;
        Ok(Report {
            query: query.clone(),
            totals,
            buckets: buckets
                .into_iter()
                .map(|(window, totals)| Bucket { window, totals })
                .collect(),
            undated,
            alerts,
            alerts_truncated,
            alerts_next,
        })
    }
    pub fn acknowledge_usage_alert(
        &mut self,
        owner: &str,
        agent: &str,
        id: &str,
        now: i64,
    ) -> Result<()> {
        self.owner(owner)?;
        key(agent)?;
        key(id)?;
        if now < 0 {
            return Err(Error::Invalid("time"));
        }
        let changed=self.db.execute("UPDATE usage_alerts SET acknowledged_at=coalesce(acknowledged_at,?) WHERE id=? AND json_extract(scope,'$[1]')=?",(now,id,agent))?;
        if changed != 1 {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
}
pub(crate) fn now() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::Invalid("time"))?
        .as_secs()
        .try_into()
        .map_err(|_| Error::Invalid("time"))
}
pub(crate) fn observe_scope(db: &Connection, scope: &Scope, now: i64) -> Result<()> {
    for layer in scope.layers()? {
        let policy: Option<String> = db
            .query_row("SELECT config FROM policies WHERE scope=?", [&layer], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(policy) = policy {
            let policy: Policy = serde_json::from_str(&policy)?;
            observe(db, &layer, &policy.budget.period.window(now)?, now)?;
        }
    }
    Ok(())
}
pub(crate) fn observe_snapshots(db: &Connection, snapshots: &[Snapshot], now: i64) -> Result<()> {
    for snapshot in snapshots {
        observe(db, &snapshot.scope, &snapshot.window, now)?;
    }
    Ok(())
}
fn observe(db: &Connection, scope: &str, window: &str, now: i64) -> Result<()> {
    let policy: Option<(i64, String)> = db
        .query_row(
            "SELECT revision,config FROM policies WHERE scope=?",
            [scope],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((revision, encoded)) = policy else {
        return Ok(());
    };
    let policy: Policy = serde_json::from_str(&encoded)?;
    let Limit::Tokens(limit) = policy.budget.limit else {
        return Ok(());
    };
    let same = match policy.budget.period {
        Period::Lifetime => window == "lifetime",
        Period::UtcDay => window.starts_with("day:"),
        Period::UtcMonth => window.starts_with("month:"),
    };
    if !same {
        return Ok(());
    }
    let (spent, held): (u64, u64) = db
        .query_row(
            "SELECT spent,held FROM accounts WHERE scope=? AND window=?",
            (scope, window),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, 0));
    let used = u128::from(spent) + u128::from(held);
    for threshold in [80, 100] {
        if (limit == 0 && threshold == 100)
            || (limit > 0 && used * 100 >= u128::from(limit) * threshold)
        {
            let id = hash(&("usage-budget-alert-v1", scope, window, revision, threshold))?;
            db.execute("INSERT INTO usage_alerts VALUES (?,?,?,?,?,?,?,?,?,NULL) ON CONFLICT(id) DO NOTHING",(id,scope,window,revision,threshold as u64,limit,spent,held,now))?;
        }
    }
    Ok(())
}
fn refresh_agent(db: &Connection, query: &Query, now: i64) -> Result<()> {
    let policies = db
        .prepare("SELECT scope,config FROM policies WHERE json_extract(scope,'$[1]')=?")?
        .query_map([&query.agent], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (scope, encoded) in policies {
        let policy: Policy = serde_json::from_str(&encoded)?;
        observe(db, &scope, &policy.budget.period.window(now)?, now)?;
    }
    Ok(())
}
fn list_alerts(db: &Connection, query: &Query) -> Result<Vec<Alert>> {
    let before: Option<i64> = if let Some(id) = &query.alerts_before {
        Some(db.query_row("SELECT observed_at FROM usage_alerts WHERE id=?1 AND json_extract(scope,'$[1]')=?2 AND (?3 IS NULL OR json_extract(scope,'$[2]')=?3) AND (?4 IS NULL OR json_extract(scope,'$[3]')=?4)",(id,&query.agent,&query.binding,&query.requester),|r|r.get(0)).optional()?.ok_or(Error::Unauthorized)?)
    } else {
        None
    };
    let mut stmt=db.prepare("SELECT id,scope,window,revision,threshold,token_limit,spent,held,observed_at,acknowledged_at FROM usage_alerts WHERE json_extract(scope,'$[1]')=?1 AND (?2 IS NULL OR json_extract(scope,'$[2]')=?2) AND (?3 IS NULL OR json_extract(scope,'$[3]')=?3) AND (?4 IS NULL OR observed_at<?4 OR (observed_at=?4 AND id<?5)) ORDER BY observed_at DESC,id DESC LIMIT 101")?;
    let mut rows = stmt.query((
        &query.agent,
        &query.binding,
        &query.requester,
        before,
        &query.alerts_before,
    ))?;
    let mut alerts = Vec::new();
    while let Some(row) = rows.next()? {
        alerts.push(Alert {
            id: row.get(0)?,
            scope: serde_json::from_str(&row.get::<_, String>(1)?)?,
            window: row.get(2)?,
            revision: row.get(3)?,
            threshold: row.get(4)?,
            token_limit: row.get(5)?,
            spent: row.get(6)?,
            held: row.get(7)?,
            observed_at: row.get(8)?,
            acknowledged_at: row.get(9)?,
        });
    }
    Ok(alerts)
}

#[cfg(test)]
mod tests;
