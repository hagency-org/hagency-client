//! Owner telemetry: real runtime usage and provider quota observations for
//! the shared owner dashboard (ADR 0012).
//!
//! Attribution comes only from trusted execution records: the shared runtime
//! command (resource, allocation, Agent reservation, assignment), the admitted
//! work item (room, requester) and the store-issued call ID. Usage numbers are
//! accepted only after the existing `SharedCallSink` verified the event's
//! process source and contiguous sequence. Nothing here scans directories or
//! transcripts, reads credentials, logs message text, or changes budget
//! accounting: budget authority stays with the shared-execution ledger.
//!
//! Persistence is optional and explicit: the host attaches a private
//! directory (the owner daemon's metadata, never a new file in an adopted
//! owner root). Without it, telemetry is in-memory and its coverage starts at
//! process start, so earlier periods report a coverage gap.

mod hook;
mod store;
mod personal;
mod provider;
#[cfg(test)]
mod tests;

pub(in crate::console::native) use hook::TurnTelemetry;
pub use store::{STORE_FILE, StoreError};

use hagency_metering::dashboard::collector::{
    CollectorError, DashboardCollector, ResourceRegistration, SnapshotOptions, UsageReport, resource_state,
};
use hagency_metering::dashboard::ledger::{
    ExecutionBinding, IngestOutcome, Runtime, UsageEvent, UsageLedger, UsageQuery,
};
use hagency_metering::dashboard::quota::{
    AccountIdentity, AccountRegistration, CodexRateLimitsRead, CodexRateLimitsUpdated, QuotaCapability,
    RefreshPolicy,
};
use hagency_metering::dashboard::{ProviderAccount, ResourceState, ResourceUsage, UsageSummary};
use hagency_metering::observation::UsageObservation;
use hagency_metering::TokenCounts;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TelemetryError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Collector(#[from] CollectorError),
    #[error("telemetry state is unavailable")]
    Unavailable,
}

/// Which execution path an observation came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPath {
    /// Rented/pooled Agents executed by this owner's shared-execution engine.
    ManagedRental,
    /// The owner's own personal Agents (hosted by a personal-agent host).
    Personal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    /// Wired in this build and fed by actual runtime events.
    Collected,
    /// A typed ingestion API exists; the hosting process must call it.
    HostIngest,
    /// Not implemented for this runtime/path; never shown as zero.
    Unsupported,
}

/// Static, honest capability statement per runtime and path. Presence of a
/// DTO field never implies support; this table does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCapability {
    pub runtime: Runtime,
    pub path: ExecutionPath,
    pub usage: Support,
    pub quota: Support,
    pub detail: &'static str,
}

pub fn capabilities() -> Vec<RuntimeCapability> {
    use ExecutionPath::*;
    use Support::*;
    vec![
        RuntimeCapability {
            runtime: Runtime::Codex,
            path: ManagedRental,
            usage: Collected,
            quota: Collected,
            detail: "thread/tokenUsage/updated after sink source+sequence verification; account/rateLimits/read and /updated from the same authorized process",
        },
        RuntimeCapability {
            runtime: Runtime::Claude,
            path: ManagedRental,
            usage: Unsupported,
            quota: Unsupported,
            detail: "the managed pool launcher executes Codex only",
        },
        RuntimeCapability {
            runtime: Runtime::Octos,
            path: ManagedRental,
            usage: Unsupported,
            quota: Unsupported,
            detail: "the managed pool launcher does not execute Octos",
        },
        RuntimeCapability {
            runtime: Runtime::Codex,
            path: Personal,
            usage: Collected,
            quota: Unsupported,
            detail: "verified personal Codex terminal usage, including failed turns; budget accounting unchanged",
        },
        RuntimeCapability {
            runtime: Runtime::Claude,
            path: Personal,
            usage: Collected,
            quota: HostIngest,
            detail: "verified personal Claude terminal usage including cache writes; provider plan observations require an upstream quota adapter",
        },
        RuntimeCapability {
            runtime: Runtime::Octos,
            path: Personal,
            usage: Collected,
            quota: Unsupported,
            detail: "verified personal Octos terminal usage with persisted cumulative baselines; historical missing cache-write categories remain unknown",
        },
    ]
}

/// Per-resource measured usage for the daemon's resource rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetry {
    pub resource_id: String,
    /// Verified (or resource-scoped unverified) provider account.
    pub account_id: Option<String>,
    pub counts: TokenCounts,
    pub state: ResourceState,
    pub executions: u64,
    pub observed_at_ms: Option<u64>,
}

/// What the daemon merges into its snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryParts {
    pub accounts: Vec<ProviderAccount>,
    pub usage: UsageSummary,
    pub resources: Vec<ResourceTelemetry>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryStatus {
    pub persistent: bool,
    pub store_path: Option<PathBuf>,
    pub executions: u64,
    pub accounts: u64,
    /// Telemetry writes that failed; execution was never affected.
    pub failures: u64,
    pub last_failure: Option<String>,
    pub coverage_start_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachReport {
    pub bindings: usize,
    pub events: usize,
    pub rejected_rows: usize,
    /// In-memory observations recorded before attachment and now persisted.
    pub carried_over: usize,
}

struct Inner {
    // Live current-account metadata is deliberately separate from immutable
    // historical execution attribution. Re-observe after daemon restart.
    current_accounts: std::collections::BTreeMap<String,String>,
    collector: DashboardCollector,
    store: Option<store::Store>,
    /// Pending in-memory records to persist on attach.
    unsaved: Vec<Unsaved>,
    failures: u64,
    last_failure: Option<String>,
}

#[derive(Clone)]
enum Unsaved {
    Binding(ExecutionBinding),
    Event(UsageEvent, String),
    Account(String, AccountRegistration),
    Quota(String, String, serde_json::Value, u64),
}
const MAX_UNSAVED: usize = 16_384;

/// Cloneable handle. Every clone shares state.
#[derive(Clone)]
pub struct OwnerTelemetry(Arc<Mutex<Inner>>);

impl Default for OwnerTelemetry {
    fn default() -> Self {
        Self::new(now_ms())
    }
}

impl std::fmt::Debug for OwnerTelemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerTelemetry").finish_non_exhaustive()
    }
}

impl OwnerTelemetry {
    /// In-memory telemetry whose coverage starts at `coverage_start_ms`.
    pub fn new(coverage_start_ms: u64) -> Self {
        Self(Arc::new(Mutex::new(Inner {
            current_accounts: Default::default(),
            collector: DashboardCollector::new(UsageLedger::new(coverage_start_ms), RefreshPolicy::default()),
            store: None,
            unsaved: Vec::new(),
            failures: 0,
            last_failure: None,
        })))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, TelemetryError> {
        self.0.lock().map_err(|_| TelemetryError::Unavailable)
    }

    /// Attach private persistence and restore it. Observations recorded
    /// in memory before attachment are written and merged. Re-attaching the
    /// same store is idempotent.
    pub fn attach_store(&self, dir: &Path) -> Result<AttachReport, TelemetryError> {
        let store = store::Store::open(dir, now_ms())?;
        let mut inner = self.lock()?;
        let unsaved = inner.unsaved.clone();
        let mut carried_over = 0;
        for record in &unsaved {
            let written = match record {
                Unsaved::Binding(b) => store.put_binding(b, b.started_at_ms).map(|_| ()),
                Unsaved::Event(e, epoch) => store.put_event(e, epoch).map(|_| ()),
                Unsaved::Account(id, r) => store.put_account(id, r),
                Unsaved::Quota(id, source, value, at) => store.put_quota(id, source, value, *at),
            };
            match written {
                Ok(()) => carried_over += 1,
                Err(error) => { inner.fail(&error); return Err(error.into()); },
            }
        }
        let loaded = store.load()?;
        inner.failures = inner.failures.saturating_add(loaded.rejected_rows as u64);
        if loaded.rejected_rows != 0 { inner.last_failure = Some("telemetry_history_incomplete".into()); }
        let mut collector = DashboardCollector::new(loaded.ledger, inner.collector.quotas().policy());
        for account in &loaded.accounts {
            if collector.quotas_mut().register(account.clone()).is_err() {
                inner.failures += 1;
            }
        }
        for (account, source, value, at) in &loaded.quotas {
            apply_quota(&mut collector, account, source, value, *at);
        }
        inner.collector = collector;
        inner.unsaved.clear();
        inner.store = Some(store);
        Ok(AttachReport {
            bindings: loaded.bindings,
            events: loaded.events,
            rejected_rows: loaded.rejected_rows,
            carried_over,
        })
    }

    pub fn status(&self) -> TelemetryStatus {
        let Ok(inner) = self.lock() else {
            return TelemetryStatus {
                last_failure: Some("telemetry_unavailable".into()),
                ..TelemetryStatus::default()
            };
        };
        TelemetryStatus {
            persistent: inner.store.is_some(),
            store_path: inner.store.as_ref().map(|s| s.path().to_owned()),
            executions: inner.collector.ledger().execution_count() as u64,
            accounts: inner.collector.quotas().project(now_ms()).len() as u64,
            failures: inner.failures,
            last_failure: inner.last_failure.clone(),
            coverage_start_ms: inner.collector.ledger().coverage_start_ms(),
        }
    }

    /// Accounts, period usage and per-resource usage for the daemon snapshot.
    pub fn parts(&self, now_ms: u64, options: SnapshotOptions) -> Result<TelemetryParts, TelemetryError> {
        let inner = self.lock()?;
        let range = options
            .period
            .resolve(now_ms, options.utc_offset_minutes)
            .map_err(CollectorError::from)?;
        let ledger = inner.collector.ledger();
        let total = ledger.aggregate(&UsageQuery::range(range));
        let mut resources: Vec<String> = ledger.bindings().map(|b| b.resource_id.clone()).collect();
        resources.extend(inner.current_accounts.keys().cloned());
        resources.sort();
        resources.dedup();
        let resources = resources
            .into_iter()
            .map(|resource_id| {
                let mut query = UsageQuery::range(range);
                query.resource_id = Some(resource_id.clone());
                let usage = ledger.aggregate(&query);
                let accounts: std::collections::BTreeSet<Option<String>> = ledger.bindings()
                    .filter(|b| b.resource_id == resource_id).map(|b| b.account_id.clone()).collect();
                // Historical consumption can span accounts after a resource
                // reconnect. Never attribute the entire sum to its latest one.
                let idle=accounts.is_empty();
                let account_id = if idle { inner.current_accounts.get(&resource_id).cloned() }
                    else if accounts.len() == 1 { accounts.into_iter().next().flatten() } else { None };
                ResourceTelemetry {
                    resource_id,
                    account_id,
                    counts: if idle {hagency_metering::dashboard::UNKNOWN_COUNTS}else{usage.counts},
                    state: if idle {ResourceState::Unmeasured}
                        else if inner.failures > 0 { if usage.measured() {ResourceState::Partial} else {ResourceState::Unmeasured} }
                        else {resource_state(&usage)},
                    executions: usage.executions,
                    observed_at_ms: usage.observed_at_ms,
                }
            })
            .collect();
        Ok(TelemetryParts {
            accounts: inner.collector.quotas().project(now_ms).into_iter().map(|mut account| {
                let mut query = UsageQuery::range(range);
                query.account_id = Some(account.id.clone());
                let usage = ledger.aggregate(&query);
                account.usage = Some(UsageSummary { counts: if usage.executions==0 {hagency_metering::dashboard::UNKNOWN_COUNTS}else{usage.counts}, complete: usage.executions>0 && usage.complete && inner.failures == 0,
                    observed_at_ms: usage.observed_at_ms, estimated_cost_usd: None });
                account
            }).collect(),
            usage: UsageSummary {
                counts: total.counts,
                complete: total.complete && inner.failures == 0,
                observed_at_ms: total.observed_at_ms,
                estimated_cost_usd: None,
            },
            resources,
        })
    }

    /// Fill counts/state/account for daemon resource rows whose `id` equals
    /// a trusted binding's `resource_id`. Rows with no binding keep their
    /// own (unknown/unmeasured) values; nothing is matched by name.
    pub fn fill_resources(
        &self,
        rows: &mut [ResourceUsage],
        now_ms: u64,
        options: SnapshotOptions,
    ) -> Result<usize, TelemetryError> {
        let parts = self.parts(now_ms, options)?;
        let mut filled = 0;
        for row in rows.iter_mut() {
            if let Some(found) = parts.resources.iter().find(|r| r.resource_id == row.id) {
                row.counts = found.counts;
                row.state = found.state.as_str().to_owned();
                if row.account_id.is_none() {
                    row.account_id = found.account_id.clone();
                }
                filled += 1;
            }
        }
        Ok(filled)
    }

    pub fn usage_report(
        &self,
        now_ms: u64,
        options: SnapshotOptions,
        filter: Option<UsageQuery>,
    ) -> Result<UsageReport, TelemetryError> {
        let inner = self.lock()?;
        let mut report = inner.collector.usage_report(now_ms, options, filter)?;
        if inner.failures > 0 {
            report.total.complete = false;
            for (_, usage) in report.by_account.iter_mut().chain(report.by_resource.iter_mut()).chain(report.by_model.iter_mut()).chain(report.by_requester.iter_mut()) {
                usage.complete = false;
            }
            for day in &mut report.daily { day.usage.complete = false; }
        }
        Ok(report)
    }

    /// Read-only access to the underlying collector (ledger, quota book).
    pub fn read<R>(&self, f: impl FnOnce(&DashboardCollector) -> R) -> Result<R, TelemetryError> {
        Ok(f(&self.lock()?.collector))
    }

    /// Register resource metadata (name, budget, agents) from trusted
    /// management data, for `DashboardCollector::snapshot` users.
    pub fn upsert_resource(&self, resource: ResourceRegistration) -> Result<(), TelemetryError> {
        Ok(self.lock()?.collector.upsert_resource(resource)?)
    }

    /// Record a trusted execution binding from a host that launched it.
    pub fn bind_execution(&self, binding: ExecutionBinding) -> Result<bool, TelemetryError> {
        let mut inner = self.lock()?;
        inner.bind(binding)
    }

    /// Ingest normalized runtime evidence for a bound execution, e.g. from a
    /// personal-agent host's `hagency-execution` capture (Codex/Claude) or an
    /// Octos runtime adapter. The host must have verified the source and
    /// sequence; `epoch` identifies the runtime process incarnation.
    pub fn ingest_observation(
        &self,
        execution_id: &str,
        stream: &str,
        epoch: &str,
        sequence: Option<u64>,
        observation: &UsageObservation,
    ) -> Result<IngestOutcome, TelemetryError> {
        let mut event = UsageEvent::from_observation(observation, execution_id, stream, epoch, now_ms())
            .map_err(|e| TelemetryError::Collector(e.into()))?;
        event.sequence = sequence;
        let mut inner = self.lock()?;
        inner.event(event, epoch)
    }

    /// Register a provider account. Verified identity must be a digest of the
    /// runtime-reported account subject, never an email or path.
    pub fn register_account(&self, registration: AccountRegistration) -> Result<String, TelemetryError> {
        let mut inner = self.lock()?;
        inner.account(registration)
    }
}

impl Inner {
    fn fail(&mut self, error: &impl std::fmt::Display) {
        self.failures = self.failures.saturating_add(1);
        self.last_failure = Some(error.to_string());
        tracing::warn!(error = %error, "owner telemetry write failed; execution unaffected");
    }

    fn remember(&mut self, record: Unsaved) {
        if self.unsaved.len() < MAX_UNSAVED {
            self.unsaved.push(record);
        } else {
            self.failures = self.failures.saturating_add(1);
            self.last_failure = Some("unsaved_capacity".into());
        }
    }

    fn persist(&mut self, record: Unsaved) {
        let Some(store) = self.store.as_ref() else { self.remember(record); return; };
        // Preserve order after a transient SQLite failure. A successful later
        // observation flushes earlier bindings/events before its own record.
        let result = self.unsaved.iter().try_for_each(|pending| persist_record(store, pending));
        if let Err(error) = result {
            self.fail(&error);
            self.remember(record);
            return;
        }
        self.unsaved.clear();
        if let Err(error) = persist_record(self.store.as_ref().unwrap(), &record) {
            self.fail(&error);
            self.remember(record);
        }
    }

    fn bind(&mut self, binding: ExecutionBinding) -> Result<bool, TelemetryError> {
        let added = self.collector.bind_execution(binding.clone())?;
        if added { self.persist(Unsaved::Binding(binding)); }
        Ok(added)
    }

    fn event(&mut self, event: UsageEvent, epoch: &str) -> Result<IngestOutcome, TelemetryError> {
        let outcome = self.collector.ingest_usage(&event)?;
        if matches!(outcome, IngestOutcome::Applied | IngestOutcome::Conflict) {
            self.persist(Unsaved::Event(event, epoch.to_owned()));
        }
        Ok(outcome)
    }

    fn account(&mut self, registration: AccountRegistration) -> Result<String, TelemetryError> {
        let id = self.collector.quotas_mut().register(registration.clone())
            .map_err(|e| TelemetryError::Collector(e.into()))?;
        self.persist(Unsaved::Account(id.clone(), registration));
        Ok(id)
    }

    fn quota(&mut self, account: &str, source: &str, value: serde_json::Value, at_ms: u64) {
        if !apply_quota(&mut self.collector, account, source, &value, at_ms) {
            self.failures = self.failures.saturating_add(1);
            return;
        }
        self.persist(Unsaved::Quota(account.into(), source.into(), value, at_ms));
    }

}

fn persist_record(store: &store::Store, record: &Unsaved) -> Result<(), StoreError> {
    match record {
        Unsaved::Binding(binding) => store.put_binding(binding, binding.started_at_ms).map(|_| ()),
        Unsaved::Event(event, epoch) => store.put_event(event, epoch).map(|_| ()),
        Unsaved::Account(id, registration) => store.put_account(id, registration),
        Unsaved::Quota(id, source, value, at) => store.put_quota(id, source, value, *at),
    }
}

pub(crate) const CODEX_READ: &str = "codex.rate_limits.read";
pub(crate) const CODEX_UPDATED: &str = "codex.rate_limits.updated";

fn apply_quota(collector: &mut DashboardCollector, account: &str, source: &str, value: &serde_json::Value, at: u64) -> bool {
    let quotas = collector.quotas_mut();
    match source {
        CODEX_READ => serde_json::from_value::<CodexRateLimitsRead>(value.clone())
            .ok()
            .is_some_and(|read| quotas.apply_codex_read(account, at, &read).is_ok()),
        CODEX_UPDATED => serde_json::from_value::<CodexRateLimitsUpdated>(value.clone())
            .ok()
            .is_some_and(|update| quotas.apply_codex_updated(account, at, &update).is_ok()),
        _ => false,
    }
}

/// Account registration for a Codex ChatGPT account seen on an actual
/// managed process. `subject_digest` is the runtime's own account digest.
pub(crate) fn codex_account(subject_digest: Option<&str>, resource_id: &str, plan: Option<&str>) -> AccountRegistration {
    let identity = match subject_digest {
        Some(subject) => AccountIdentity::Verified {
            provider: "openai".into(),
            subject: subject.into(),
        },
        None => AccountIdentity::Unverified {
            provider: "openai".into(),
            resource_id: resource_id.into(),
        },
    };
    AccountRegistration {
        identity,
        label: "ChatGPT (Codex)".into(),
        plan: plan.map(str::to_owned),
        capability: QuotaCapability::Supported,
        runtime_version: None,
    }
}
