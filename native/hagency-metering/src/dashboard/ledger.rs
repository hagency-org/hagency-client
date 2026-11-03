//! Execution-bound usage ledger.
//!
//! Attribution comes only from [`ExecutionBinding`]s that the trusted host
//! (owner daemon) records when it launches an execution; a usage event names
//! an execution and never carries its own resource/account claims. Events for
//! unbound executions are refused rather than guessed from working
//! directories, display names or credential paths.
//!
//! Normalization rules:
//!
//! - Categories are fresh input, output, cache write and cache read
//!   ([`TokenCounts`]). Reasoning is a breakdown of output and is never added.
//! - Cumulative streams are tracked per `(stream, epoch)` with a per-category
//!   high-water mark; only the increase is recorded, so duplicate, repeated
//!   and out-of-order snapshots never double count. A new epoch (runtime
//!   restart) starts from zero. An unexplained decrease within an epoch is
//!   recorded as a regression and makes the measurement partial.
//! - Delta streams are de-duplicated by event ID or sequence. A delta without
//!   either is counted but flagged undeduplicable (possible over-count).
//! - A category never reported by a stream is unknown (`None`), not zero.
//! - When an ancestor execution reports usage that includes its subagents,
//!   descendants' usage is excluded from totals so the same work is not
//!   counted twice.
//! - Budget-charge arithmetic is unchanged: use
//!   [`TokenCounts::ceiling_volume`] on the reported counts.

use super::period::{
    BUCKET_MS, MAX_SERIES_DAYS, MS_PER_DAY, PeriodError, TimeRange, local_day_start,
};
use crate::observation::UsageObservation;
use crate::{Framework, MAX_TOKEN_COUNT, TokenCounts};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const MAX_ID_BYTES: usize = 256;
pub const MAX_EXECUTIONS: usize = 65_536;
pub const MAX_STREAMS_PER_EXECUTION: usize = 64;
pub const MAX_SEEN_PER_STREAM: usize = 4096;
pub const MAX_BUCKETS_PER_STREAM: usize = 35_040;
pub const MAX_SUBAGENT_DEPTH: usize = 16;
/// Bound on a serialized export accepted by [`UsageLedger::import_json`].
pub const MAX_EXPORT_BYTES: usize = 64 * 1024 * 1024;
pub const LEDGER_EXPORT_VERSION: u32 = 1;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LedgerError {
    #[error("usage refers to an execution with no trusted binding")]
    UnknownExecution,
    #[error("execution is already bound with different attribution")]
    BindingConflict,
    #[error("an identifier is empty or exceeds the bounded length")]
    InvalidIdentifier,
    #[error("a counter exceeds the exact supported range")]
    InvalidCounter,
    #[error("stream changed between cumulative and delta counting")]
    ModeMismatch,
    #[error("ledger capacity exceeded")]
    Capacity,
    #[error("normalized token arithmetic overflowed")]
    Overflow,
    #[error("observation has no runtime evidence or digest to ingest")]
    UnusableObservation,
    #[error("ledger export is malformed, unsupported or exceeds its bounds")]
    InvalidExport,
    #[error(transparent)]
    Period(#[from] PeriodError),
}

/// Agent runtime that produced a measurement. Distinct from provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    Codex,
    Claude,
    Octos,
}

impl From<Framework> for Runtime {
    fn from(value: Framework) -> Self {
        match value {
            Framework::Codex => Self::Codex,
            Framework::Claude => Self::Claude,
            Framework::Octos => Self::Octos,
        }
    }
}

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Octos => "octos",
        }
    }
}

/// Trusted launch/execution record. `account_id` must be a verified account
/// key (see `quota::AccountIdentity::account_id`) or `None`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionBinding {
    pub execution_id: String,
    pub resource_id: String,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub allocation_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub requester_id: Option<String>,
    #[serde(default)]
    pub room_id: Option<String>,
    pub runtime: Runtime,
    #[serde(default)]
    pub runtime_version: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Parent execution when this execution is a subagent of another.
    #[serde(default)]
    pub parent_execution_id: Option<String>,
    pub started_at_ms: u64,
}

impl ExecutionBinding {
    fn validate(&self) -> Result<(), LedgerError> {
        check_id(&self.execution_id)?;
        check_id(&self.resource_id)?;
        for value in [
            &self.account_id,
            &self.allocation_id,
            &self.agent_id,
            &self.requester_id,
            &self.room_id,
            &self.runtime_version,
            &self.model,
            &self.parent_execution_id,
        ]
        .into_iter()
        .flatten()
        {
            check_id(value)?;
        }
        if self.parent_execution_id.as_deref() == Some(self.execution_id.as_str()) {
            return Err(LedgerError::InvalidIdentifier);
        }
        Ok(())
    }
}

fn check_id(value: &str) -> Result<(), LedgerError> {
    if value.is_empty() || value.len() > MAX_ID_BYTES || value.chars().any(char::is_control) {
        return Err(LedgerError::InvalidIdentifier);
    }
    Ok(())
}

/// How a usage event's counts relate to earlier events of the same stream.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CountMode {
    /// Running total since the start of `epoch` (a runtime process/session
    /// incarnation chosen by the host). A new epoch restarts from zero.
    Cumulative { epoch: String },
    /// Usage of this event only.
    Delta,
}

/// Whether the reported counts include subagents' usage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CountScope {
    #[default]
    Own,
    IncludesSubagents,
}

/// Where the measurement came from (provenance, not authenticity).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// Codex App Server `thread/tokenUsage/updated` (via runtime evidence).
    CodexRuntime,
    /// Claude Agent SDK result/step usage (via runtime evidence).
    ClaudeRuntime,
    /// Octos runtime normalized usage (via runtime evidence).
    OctosRuntime,
    /// Bounded transcript normalization (`UsageObservation::parse`).
    Transcript,
    /// Claude Code OpenTelemetry token metrics forwarded by the host.
    ClaudeTelemetry,
    /// Octos persistent usage analytics import.
    OctosAnalytics,
    /// Historical import; coverage gaps are retained.
    HistoricalImport,
}

/// One sanitized usage measurement for a bound execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub execution_id: String,
    /// Runtime stream within the execution (thread/session ID); host-chosen.
    pub stream: String,
    /// Provider/runtime event identity for retry de-duplication.
    #[serde(default)]
    pub event_id: Option<String>,
    /// Monotonic sequence within the stream (and epoch, for cumulative).
    #[serde(default)]
    pub sequence: Option<u64>,
    pub observed_at_ms: u64,
    /// Provider measurement time, when reported.
    #[serde(default)]
    pub measured_at_ms: Option<u64>,
    pub mode: CountMode,
    #[serde(default)]
    pub scope: CountScope,
    pub counts: TokenCounts,
    /// Breakdown of `counts.output`; never added to totals.
    #[serde(default)]
    pub reasoning_output: Option<u64>,
    /// False when the source could not prove complete capture.
    pub complete: bool,
    pub source: UsageSource,
}

impl UsageEvent {
    /// Convert an existing normalized observation. Runtime evidence produces
    /// a cumulative snapshot for `stream`/`epoch`; its content digest becomes
    /// the event ID, so re-delivering the same observation is a duplicate.
    /// Runtime evidence is conservatively incomplete, and so is the event.
    /// A failed transcript parse becomes an incomplete, all-unknown event so
    /// the execution reads as partial rather than unmeasured or zero.
    pub fn from_observation(
        observation: &UsageObservation,
        execution_id: impl Into<String>,
        stream: impl Into<String>,
        epoch: impl Into<String>,
        observed_at_ms: u64,
    ) -> Result<Self, LedgerError> {
        let encoded =
            serde_json::to_value(observation).map_err(|_| LedgerError::UnusableObservation)?;
        let digest = encoded
            .get("snapshot_digest")
            .and_then(serde_json::Value::as_str)
            .filter(|digest| !digest.is_empty())
            .ok_or(LedgerError::UnusableObservation)?;
        let mut scope = CountScope::Own;
        let mut reasoning_output = None;
        let source = if let Some(evidence) = observation.runtime_evidence() {
            reasoning_output = evidence.usage().total.reasoning_output_tokens;
            UsageSource::CodexRuntime
        } else if observation.claude_runtime_evidence().is_some() {
            UsageSource::ClaudeRuntime
        } else if let Some(evidence) = observation.octos_runtime_evidence() {
            let usage = evidence.usage();
            reasoning_output = usage.reasoning;
            if usage.coverage == crate::octos_usage::Coverage::Session {
                scope = CountScope::IncludesSubagents;
            }
            UsageSource::OctosRuntime
        } else {
            UsageSource::Transcript
        };
        Ok(Self {
            execution_id: execution_id.into(),
            stream: stream.into(),
            event_id: Some(format!("sha256:{digest}")),
            sequence: None,
            observed_at_ms,
            measured_at_ms: None,
            mode: CountMode::Cumulative {
                epoch: epoch.into(),
            },
            scope,
            counts: observation.counts().unwrap_or(super::UNKNOWN_COUNTS),
            reasoning_output,
            complete: !observation.incomplete(),
            source,
        })
    }

    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence = Some(sequence);
        self
    }

    pub fn with_measured_at(mut self, measured_at_ms: u64) -> Self {
        self.measured_at_ms = Some(measured_at_ms);
        self
    }

    fn validate(&self) -> Result<(), LedgerError> {
        check_id(&self.execution_id)?;
        check_id(&self.stream)?;
        if let Some(id) = &self.event_id {
            check_id(id)?;
        }
        if let CountMode::Cumulative { epoch } = &self.mode {
            check_id(epoch)?;
        }
        for value in categories(&self.counts)
            .into_iter()
            .chain([self.reasoning_output])
            .flatten()
        {
            if value > MAX_TOKEN_COUNT {
                return Err(LedgerError::InvalidCounter);
            }
        }
        Ok(())
    }

    fn at_ms(&self) -> u64 {
        self.measured_at_ms.unwrap_or(self.observed_at_ms)
    }
}

fn categories(counts: &TokenCounts) -> [Option<u64>; 4] {
    [
        counts.input,
        counts.output,
        counts.cache_write,
        counts.cache_read,
    ]
}

/// Result of a successful ingest. Only `Applied` changed totals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestOutcome {
    Applied,
    /// Identical re-delivery; ignored.
    Duplicate,
    /// Older cumulative snapshot already covered by the high-water mark.
    Stale,
    /// Same identity with different counts; first value kept, flagged partial.
    Conflict,
}

/// Per-stream quality counters. Any non-zero field except `duplicates` and
/// `stale` makes a measurement partial.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDiagnostics {
    pub duplicates: u64,
    pub stale: u64,
    pub conflicts: u64,
    pub regressions: u64,
    pub undeduplicable: u64,
    pub incomplete_captures: u64,
    /// A category reported earlier was omitted by a later cumulative snapshot.
    pub missing_after_known: u64,
}

impl UsageDiagnostics {
    fn add(&mut self, other: &Self) {
        self.duplicates += other.duplicates;
        self.stale += other.stale;
        self.conflicts += other.conflicts;
        self.regressions += other.regressions;
        self.undeduplicable += other.undeduplicable;
        self.incomplete_captures += other.incomplete_captures;
        self.missing_after_known += other.missing_after_known;
    }
    fn degrades(&self) -> bool {
        self.conflicts
            + self.regressions
            + self.undeduplicable
            + self.incomplete_captures
            + self.missing_after_known
            > 0
    }
}

type Fingerprint = ([Option<u64>; 4], Option<u64>);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Bucket {
    counts: [u64; 4],
    reasoning: u64,
    /// Delta streams: categories some event in this bucket left unknown.
    unknown_mask: u8,
    reasoning_unknown: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamState {
    cumulative: bool,
    includes_subagents: bool,
    source: Option<UsageSource>,
    /// Cumulative: per-category high-water (None = never reported).
    high: [Option<u64>; 4],
    reasoning_high: Option<u64>,
    last_sequence: Option<u64>,
    last_fingerprint: Option<Fingerprint>,
    latest_complete: bool,
    seen_ids: BTreeMap<String, Fingerprint>,
    seen_sequences: BTreeMap<u64, Fingerprint>,
    buckets: BTreeMap<u64, Bucket>,
    first_at_ms: Option<u64>,
    last_at_ms: Option<u64>,
    last_observed_ms: Option<u64>,
    diagnostics: UsageDiagnostics,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionRecord {
    binding: ExecutionBinding,
    /// Keyed by `stream` + `\u{1f}` + epoch (empty for delta streams).
    streams: BTreeMap<String, StreamState>,
}

/// Dimension for [`UsageLedger::aggregate_by`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    Account,
    Resource,
    Model,
    Requester,
    Runtime,
    Room,
    Agent,
}

/// Filter over trusted binding fields within a time range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageQuery {
    pub range: TimeRange,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub resource_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub requester_id: Option<String>,
    #[serde(default)]
    pub runtime: Option<Runtime>,
}

impl UsageQuery {
    pub fn range(range: TimeRange) -> Self {
        Self {
            range,
            account_id: None,
            resource_id: None,
            model: None,
            requester_id: None,
            runtime: None,
        }
    }
    fn matches(&self, binding: &ExecutionBinding) -> bool {
        fn eq(filter: &Option<String>, value: &Option<String>) -> bool {
            filter
                .as_ref()
                .is_none_or(|filter| value.as_ref() == Some(filter))
        }
        eq(&self.account_id, &binding.account_id)
            && self
                .resource_id
                .as_ref()
                .is_none_or(|r| *r == binding.resource_id)
            && eq(&self.model, &binding.model)
            && eq(&self.requester_id, &binding.requester_id)
            && self.runtime.is_none_or(|r| r == binding.runtime)
    }
}

/// Aggregated measured consumption over one scope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageAggregate {
    /// Unknown categories are `None`; counts are aggregated before any ratio.
    pub counts: TokenCounts,
    /// Output tokens that the provider classified as reasoning (subset).
    pub reasoning_output: Option<u64>,
    /// True only with full ledger coverage of the range, every execution
    /// measured and no degrading diagnostics or unknown categories.
    pub complete: bool,
    /// Latest observation time of a contributing measurement.
    pub observed_at_ms: Option<u64>,
    /// Bound executions overlapping the range (excluding contained subagents).
    pub executions: u64,
    /// Executions in the range with no usage measurement at all.
    pub unmeasured_executions: u64,
    /// Subagent executions excluded because an ancestor's usage includes them.
    pub contained_executions: u64,
    /// The ledger's coverage began after the range start.
    pub coverage_gap: bool,
    pub sources: Vec<UsageSource>,
    pub diagnostics: UsageDiagnostics,
}

impl UsageAggregate {
    /// Combine disjoint execution shards. Families must not span shards.
    pub fn merge(&mut self, other: &Self) {
        let a_empty = !self.measured();
        let b_empty = !other.measured();
        let sum = |a: Option<u64>, b: Option<u64>| {
            a.or(a_empty.then_some(0)).zip(b.or(b_empty.then_some(0)))
                .and_then(|(a,b)| add(a,b).ok())
        };
        self.counts = TokenCounts { input:sum(self.counts.input,other.counts.input), output:sum(self.counts.output,other.counts.output), cache_write:sum(self.counts.cache_write,other.counts.cache_write), cache_read:sum(self.counts.cache_read,other.counts.cache_read) };
        self.reasoning_output = if a_empty && b_empty { None } else { sum(self.reasoning_output,other.reasoning_output) };
        self.coverage_gap |= other.coverage_gap;
        if a_empty && b_empty && self.coverage_gap { self.counts = super::UNKNOWN_COUNTS; }
        self.complete &= other.complete;
        self.complete &= categories(&self.counts).iter().all(Option::is_some);
        self.observed_at_ms = self.observed_at_ms.max(other.observed_at_ms);
        self.executions = self.executions.saturating_add(other.executions);
        self.unmeasured_executions = self.unmeasured_executions.saturating_add(other.unmeasured_executions);
        self.contained_executions = self.contained_executions.saturating_add(other.contained_executions);
        self.diagnostics.add(&other.diagnostics);
        self.sources.extend(other.sources.iter().copied()); self.sources.sort(); self.sources.dedup();
    }

    pub fn cache_hit_ratio(&self) -> Option<f64> {
        super::cache_hit_ratio(&self.counts)
    }
    /// Whether any measurement contributed.
    pub fn measured(&self) -> bool {
        self.observed_at_ms.is_some()
    }
}

/// One local calendar day of a series.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    pub day_start_ms: u64,
    pub usage: UsageAggregate,
}

/// Explicit, versioned, bounded persistence format. The host chooses where
/// to store it; it contains attribution IDs and counts but no credentials.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerExport {
    version: u32,
    coverage_start_ms: u64,
    executions: Vec<ExecutionRecord>,
}

impl LedgerExport {
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn executions(&self) -> usize {
        self.executions.len()
    }
}

/// Append-only journal record; replaying a journal is idempotent because
/// ingestion de-duplicates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum LedgerRecord {
    Bind(ExecutionBinding),
    Usage(UsageEvent),
}

#[derive(Clone, Debug)]
pub struct UsageLedger {
    coverage_start_ms: u64,
    executions: BTreeMap<String, ExecutionRecord>,
    buckets: usize,
}

impl UsageLedger {
    /// `coverage_start_ms` is when this ledger began binding every hosted
    /// execution. Ranges starting earlier are reported with a coverage gap.
    pub fn new(coverage_start_ms: u64) -> Self {
        Self {
            coverage_start_ms,
            executions: BTreeMap::new(),
            buckets: 0,
        }
    }

    pub fn coverage_start_ms(&self) -> u64 {
        self.coverage_start_ms
    }

    pub fn execution_count(&self) -> usize {
        self.executions.len()
    }

    pub fn binding(&self, execution_id: &str) -> Option<&ExecutionBinding> {
        self.executions
            .get(execution_id)
            .map(|record| &record.binding)
    }

    pub fn bindings(&self) -> impl Iterator<Item = &ExecutionBinding> {
        self.executions.values().map(|record| &record.binding)
    }

    /// Record a trusted execution binding. Re-binding identically is a no-op;
    /// different attribution for the same execution is refused.
    pub fn bind_execution(&mut self, binding: ExecutionBinding) -> Result<bool, LedgerError> {
        binding.validate()?;
        if let Some(existing) = self.executions.get(&binding.execution_id) {
            return if existing.binding == binding {
                Ok(false)
            } else {
                Err(LedgerError::BindingConflict)
            };
        }
        if self.executions.len() >= MAX_EXECUTIONS {
            return Err(LedgerError::Capacity);
        }
        self.executions.insert(
            binding.execution_id.clone(),
            ExecutionRecord {
                binding,
                streams: BTreeMap::new(),
            },
        );
        Ok(true)
    }

    /// Ingest one usage event for a bound execution.
    pub fn ingest(&mut self, event: &UsageEvent) -> Result<IngestOutcome, LedgerError> {
        event.validate()?;
        let total_buckets = self.buckets;
        let record = self
            .executions
            .get_mut(&event.execution_id)
            .ok_or(LedgerError::UnknownExecution)?;
        let (cumulative, epoch) = match &event.mode {
            CountMode::Cumulative { epoch } => (true, epoch.as_str()),
            CountMode::Delta => (false, ""),
        };
        let key = format!("{}\u{1f}{}", event.stream, epoch);
        if !record.streams.contains_key(&key) {
            if record.streams.len() >= MAX_STREAMS_PER_EXECUTION {
                return Err(LedgerError::Capacity);
            }
            let mode_conflict = record.streams.iter().any(|(existing, state)| {
                existing.split('\u{1f}').next() == Some(event.stream.as_str())
                    && state.cumulative != cumulative
            });
            if mode_conflict {
                return Err(LedgerError::ModeMismatch);
            }
            record.streams.insert(
                key.clone(),
                StreamState {
                    cumulative,
                    ..StreamState::default()
                },
            );
        }
        let stream = record.streams.get_mut(&key).expect("inserted above");
        let before = stream.buckets.len();
        let outcome = if cumulative {
            stream.ingest_cumulative(event, total_buckets)?
        } else {
            stream.ingest_delta(event, total_buckets)?
        };
        if outcome == IngestOutcome::Applied {
            stream.includes_subagents |= event.scope == CountScope::IncludesSubagents;
            stream.source.get_or_insert(event.source);
            stream.last_observed_ms = stream.last_observed_ms.max(Some(event.observed_at_ms));
        }
        self.buckets = self.buckets - before + stream.buckets.len();
        Ok(outcome)
    }

    /// Apply a journal record (bind or usage).
    pub fn apply(&mut self, record: &LedgerRecord) -> Result<Option<IngestOutcome>, LedgerError> {
        match record {
            LedgerRecord::Bind(binding) => self.bind_execution(binding.clone()).map(|_| None),
            LedgerRecord::Usage(event) => self.ingest(event).map(Some),
        }
    }

    /// Replay an append-only journal; duplicates are harmless.
    pub fn replay<'a>(
        &mut self,
        records: impl IntoIterator<Item = &'a LedgerRecord>,
    ) -> Result<(), LedgerError> {
        for record in records {
            self.apply(record)?;
        }
        Ok(())
    }

    /// Drop executions whose binding and every measurement precede
    /// `cutoff_ms`, and move the coverage start forward accordingly.
    pub fn prune_before(&mut self, cutoff_ms: u64) -> usize {
        let before = self.executions.len();
        self.executions.retain(|_, record| {
            let last = record
                .streams
                .values()
                .filter_map(|stream| stream.last_at_ms.max(stream.last_observed_ms))
                .max()
                .unwrap_or(0)
                .max(record.binding.started_at_ms);
            last >= cutoff_ms
        });
        self.coverage_start_ms = self.coverage_start_ms.max(cutoff_ms);
        self.buckets = self.count_buckets();
        before - self.executions.len()
    }

    /// Inclusive evidence bounds for skipping cold shards on range queries.
    pub fn time_bounds(&self) -> Option<(u64,u64)> {
        let mut start = u64::MAX; let mut end = 0;
        for record in self.executions.values() {
            start = start.min(record.binding.started_at_ms); end = end.max(record.binding.started_at_ms);
            for stream in record.streams.values() {
                if let Some(first) = stream.first_at_ms { start = start.min(first); }
                if let Some(last) = stream.last_at_ms.max(stream.last_observed_ms) { end = end.max(last); }
            }
        }
        (!self.executions.is_empty()).then_some((start,end))
    }

    /// Move entire inactive execution families into a separate bounded shard.
    /// Parent/child containment and every immutable binding, high-water mark,
    /// dedup identity and quality flag remain together in exactly one shard.
    pub fn split_inactive(&mut self, keep: usize, protected: &BTreeSet<String>) -> Self {
        self.split_inactive_budget(keep, usize::MAX, protected)
    }
    pub fn encoded_size(&self) -> usize {
        128 + self.executions.values().map(|r| serde_json::to_vec(r).map_or(MAX_EXPORT_BYTES, |v|v.len()+1)).sum::<usize>()
    }
    fn families(&self) -> Vec<Vec<String>> {
        // Connected components also preserve malformed/deep parent chains:
        // no cycle or missing ancestor can split containment across shards.
        let mut links: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, record) in &self.executions {
            links.entry(id.clone()).or_default();
            if let Some(parent)=&record.binding.parent_execution_id {
                links.entry(id.clone()).or_default().push(parent.clone());
                links.entry(parent.clone()).or_default().push(id.clone());
            }
        }
        let mut seen=BTreeSet::new();let mut families=BTreeMap::new();
        for id in self.executions.keys() {
            if seen.contains(id) { continue; }
            let mut stack=vec![id.clone()];let mut family=Vec::new();
            while let Some(id)=stack.pop() {
                if !seen.insert(id.clone()) { continue; }
                if self.executions.contains_key(&id) { family.push(id.clone()); }
                if let Some(neighbors)=links.get(&id) { stack.extend(neighbors.iter().cloned()); }
            }
            families.insert(id.clone(),family);
        }
        families.into_values().collect()
    }
    pub fn split_inactive_budget(&mut self, keep: usize, max_bytes:usize, protected:&BTreeSet<String>) -> Self {
        let mut bytes=self.encoded_size();
        let mut candidates: Vec<_> = self.families().into_iter()
            .filter(|ids| !ids.iter().any(|id| protected.contains(id)))
            // An unresolved parent may arrive later. Keep that entire family
            // hot until its immutable containment context is known.
            .filter(|ids| ids.iter().all(|id| self.executions[id].binding.parent_execution_id.as_ref().is_none_or(|parent| self.executions.contains_key(parent))))
            .map(|ids| (ids.iter().map(|id| self.executions[id].binding.started_at_ms).max().unwrap_or(0), ids))
            .collect();
        candidates.sort_by(|a,b| a.0.cmp(&b.0).then_with(||a.1.cmp(&b.1)));
        let mut archived = Self::new(self.coverage_start_ms);
        for (_, ids) in candidates {
            if self.executions.len() <= keep && bytes <= max_bytes { break; }
            for id in ids {
                if let Some(record) = self.executions.remove(&id) {
                    bytes=bytes.saturating_sub(serde_json::to_vec(&record).map_or(MAX_EXPORT_BYTES,|v|v.len()+1));
                    archived.executions.insert(id, record);
                }
            }
        }
        self.buckets = self.count_buckets();
        archived.buckets = archived.count_buckets();
        archived
    }
    /// Cold shards are byte-bounded, keeping complete families together.
    pub fn into_shards(mut self, max_bytes:usize) -> Vec<Self> {
        let families=self.families();let mut shards=Vec::new();let mut current=Self::new(self.coverage_start_ms);let mut size=128;
        for family in families {
            let bytes=family.iter().map(|id|serde_json::to_vec(&self.executions[id]).map_or(MAX_EXPORT_BYTES,|v|v.len()+1)).sum::<usize>();
            if size+bytes>max_bytes && current.execution_count()>0 {
                current.buckets=current.count_buckets();shards.push(current);current=Self::new(self.coverage_start_ms);size=128;
            }
            for id in family { if let Some(record)=self.executions.remove(&id){current.executions.insert(id,record);} }
            size+=bytes;
        }
        if current.execution_count()>0 {current.buckets=current.count_buckets();shards.push(current);}
        shards
    }

    fn count_buckets(&self) -> usize {
        self.executions
            .values()
            .flat_map(|record| record.streams.values())
            .map(|stream| stream.buckets.len())
            .sum()
    }

    pub fn export(&self) -> LedgerExport {
        LedgerExport {
            version: LEDGER_EXPORT_VERSION,
            coverage_start_ms: self.coverage_start_ms,
            executions: self.executions.values().cloned().collect(),
        }
    }

    pub fn export_json(&self) -> Result<Vec<u8>, LedgerError> {
        let bytes = serde_json::to_vec(&self.export()).map_err(|_| LedgerError::InvalidExport)?;
        if bytes.len() > MAX_EXPORT_BYTES {
            return Err(LedgerError::Capacity);
        }
        Ok(bytes)
    }

    /// Validate and restore an export. Bounds and invariants are re-checked;
    /// a corrupted export is refused as a whole.
    pub fn import(export: LedgerExport) -> Result<Self, LedgerError> {
        if export.version != LEDGER_EXPORT_VERSION || export.executions.len() > MAX_EXECUTIONS {
            return Err(LedgerError::InvalidExport);
        }
        let mut ledger = Self::new(export.coverage_start_ms);
        for record in export.executions {
            record
                .binding
                .validate()
                .map_err(|_| LedgerError::InvalidExport)?;
            if record.streams.len() > MAX_STREAMS_PER_EXECUTION {
                return Err(LedgerError::InvalidExport);
            }
            for (key, stream) in &record.streams {
                let (name, epoch) = key.split_once('\u{1f}').ok_or(LedgerError::InvalidExport)?;
                check_id(name).map_err(|_| LedgerError::InvalidExport)?;
                if stream.cumulative == epoch.is_empty() {
                    return Err(LedgerError::InvalidExport);
                }
                stream.validate().map_err(|_| LedgerError::InvalidExport)?;
            }
            let id = record.binding.execution_id.clone();
            if ledger.executions.insert(id, record).is_some() {
                return Err(LedgerError::InvalidExport);
            }
        }
        ledger.buckets = ledger.count_buckets();
        if ledger.buckets > MAX_EXECUTIONS * 16 {
            return Err(LedgerError::InvalidExport);
        }
        Ok(ledger)
    }

    pub fn import_json(bytes: &[u8]) -> Result<Self, LedgerError> {
        if bytes.len() > MAX_EXPORT_BYTES {
            return Err(LedgerError::InvalidExport);
        }
        let export: LedgerExport =
            serde_json::from_slice(bytes).map_err(|_| LedgerError::InvalidExport)?;
        Self::import(export)
    }

    /// Merge another ledger (e.g. a historical import) into this one. Shared
    /// executions must have identical bindings; overlapping streams keep the
    /// existing state and are reported as conflicts. The coverage start is
    /// unchanged: imported history contributes its measurements but cannot
    /// prove that nothing else ran before this ledger's coverage began, so
    /// earlier ranges keep reporting a coverage gap.
    pub fn merge(&mut self, other: UsageLedger) -> Result<(), LedgerError> {
        for (id, incoming) in other.executions {
            match self.executions.get_mut(&id) {
                None => {
                    if self.executions.len() >= MAX_EXECUTIONS {
                        return Err(LedgerError::Capacity);
                    }
                    self.executions.insert(id, incoming);
                }
                Some(existing) if existing.binding == incoming.binding => {
                    for (key, stream) in incoming.streams {
                        if let Some(current) = existing.streams.get_mut(&key) {
                            if current.buckets != stream.buckets {
                                current.diagnostics.conflicts += 1;
                            }
                        } else if existing.streams.len() < MAX_STREAMS_PER_EXECUTION {
                            existing.streams.insert(key, stream);
                        } else {
                            return Err(LedgerError::Capacity);
                        }
                    }
                }
                Some(_) => return Err(LedgerError::BindingConflict),
            }
        }
        self.buckets = self.count_buckets();
        Ok(())
    }

    /// Executions excluded from totals because an ancestor reports usage that
    /// includes its subagents.
    fn contained(&self, record: &ExecutionRecord) -> bool {
        let mut parent = record.binding.parent_execution_id.as_deref();
        for _ in 0..MAX_SUBAGENT_DEPTH {
            let Some(id) = parent else { return false };
            let Some(ancestor) = self.executions.get(id) else {
                return false;
            };
            if ancestor
                .streams
                .values()
                .any(|stream| stream.includes_subagents)
            {
                return true;
            }
            parent = ancestor.binding.parent_execution_id.as_deref();
        }
        false
    }

    pub fn aggregate(&self, query: &UsageQuery) -> UsageAggregate {
        let mut builder = AggregateBuilder::new(query.range, self.coverage_start_ms);
        for record in self.executions.values() {
            if query.matches(&record.binding) {
                builder.add(record, self.contained(record));
            }
        }
        builder.finish()
    }

    /// Aggregate per dimension value. Executions without that dimension are
    /// grouped under `None`; nothing is merged by display name or path.
    pub fn aggregate_by(
        &self,
        query: &UsageQuery,
        group: GroupBy,
    ) -> BTreeMap<Option<String>, UsageAggregate> {
        let mut builders: BTreeMap<Option<String>, AggregateBuilder> = BTreeMap::new();
        for record in self.executions.values() {
            if !query.matches(&record.binding) {
                continue;
            }
            let binding = &record.binding;
            let key = match group {
                GroupBy::Account => binding.account_id.clone(),
                GroupBy::Resource => Some(binding.resource_id.clone()),
                GroupBy::Model => binding.model.clone(),
                GroupBy::Requester => binding.requester_id.clone(),
                GroupBy::Runtime => Some(binding.runtime.as_str().to_owned()),
                GroupBy::Room => binding.room_id.clone(),
                GroupBy::Agent => binding.agent_id.clone(),
            };
            let contained = self.contained(record);
            builders
                .entry(key)
                .or_insert_with(|| AggregateBuilder::new(query.range, self.coverage_start_ms))
                .add(record, contained);
        }
        builders
            .into_iter()
            .filter(|(_, builder)| builder.touched)
            .map(|(key, builder)| (key, builder.finish()))
            .collect()
    }

    /// Per local-day aggregates covering `query.range` (bounded length).
    pub fn daily_series(
        &self,
        query: &UsageQuery,
        utc_offset_minutes: i32,
    ) -> Result<Vec<DailyUsage>, LedgerError> {
        let first = local_day_start(query.range.start_ms, utc_offset_minutes)?;
        let days = query
            .range
            .end_ms
            .saturating_sub(first)
            .div_ceil(MS_PER_DAY);
        if days > MAX_SERIES_DAYS {
            return Err(PeriodError::TooLong.into());
        }
        let mut series = Vec::with_capacity(days as usize);
        for index in 0..days {
            let start = first + index * MS_PER_DAY;
            let range = TimeRange::new(
                start.max(query.range.start_ms),
                (start + MS_PER_DAY).min(query.range.end_ms),
            )?;
            let usage = self.aggregate(&UsageQuery {
                range,
                ..query.clone()
            });
            series.push(DailyUsage {
                day_start_ms: start,
                usage,
            });
        }
        Ok(series)
    }

    /// Bound executions overlapping `range`, newest first, with their
    /// per-execution aggregate; for activity and drill-down views.
    pub fn executions_in(
        &self,
        range: TimeRange,
        limit: usize,
    ) -> Vec<(ExecutionBinding, UsageAggregate)> {
        let mut found: Vec<_> = self
            .executions
            .values()
            .filter_map(|record| {
                let mut builder = AggregateBuilder::new(range, self.coverage_start_ms);
                builder.add(record, self.contained(record));
                builder
                    .touched
                    .then(|| (record.binding.clone(), builder.finish()))
            })
            .collect();
        found.sort_by(|a, b| {
            b.0.started_at_ms
                .cmp(&a.0.started_at_ms)
                .then_with(|| a.0.execution_id.cmp(&b.0.execution_id))
        });
        found.truncate(limit);
        found
    }
}

fn fingerprint(event: &UsageEvent) -> Fingerprint {
    (categories(&event.counts), event.reasoning_output)
}

impl StreamState {
    fn validate(&self) -> Result<(), LedgerError> {
        let counters = self.high.iter().chain([&self.reasoning_high]).flatten();
        if counters.into_iter().any(|value| *value > MAX_TOKEN_COUNT) {
            return Err(LedgerError::InvalidCounter);
        }
        if self.seen_ids.len() > MAX_SEEN_PER_STREAM
            || self.seen_sequences.len() > MAX_SEEN_PER_STREAM
            || self.buckets.len() > MAX_BUCKETS_PER_STREAM
            || self.seen_ids.keys().any(|id| check_id(id).is_err())
            || self.buckets.keys().any(|start| start % BUCKET_MS != 0)
            || self.buckets.values().any(|bucket| {
                bucket.counts.iter().any(|count| *count > MAX_TOKEN_COUNT)
                    || bucket.reasoning > MAX_TOKEN_COUNT
            })
        {
            return Err(LedgerError::Capacity);
        }
        Ok(())
    }

    /// Returns `Some(outcome)` when the identity was already seen.
    fn check_identity(&mut self, event: &UsageEvent) -> Result<Option<IngestOutcome>, LedgerError> {
        let print = fingerprint(event);
        let verdict = event
            .event_id
            .as_ref()
            .and_then(|id| self.seen_ids.get(id))
            .or_else(|| {
                event
                    .sequence
                    .and_then(|sequence| self.seen_sequences.get(&sequence))
            })
            .map(|previous| *previous == print);
        match verdict {
            Some(true) => {
                self.diagnostics.duplicates += 1;
                return Ok(Some(IngestOutcome::Duplicate));
            }
            Some(false) => {
                self.diagnostics.conflicts += 1;
                return Ok(Some(IngestOutcome::Conflict));
            }
            None => {}
        }
        if let Some(id) = &event.event_id {
            if self.seen_ids.len() >= MAX_SEEN_PER_STREAM {
                // A cumulative re-delivery without its remembered ID is still
                // caught by the high-water mark (stale), so evicting is safe.
                if self.cumulative {
                    self.seen_ids.pop_first();
                } else {
                    return Err(LedgerError::Capacity);
                }
            }
            self.seen_ids.insert(id.clone(), print);
        }
        if let Some(sequence) = event.sequence {
            if self.seen_sequences.len() >= MAX_SEEN_PER_STREAM {
                // Cumulative streams only need the latest sequence; keep the
                // most recent window instead of failing.
                if self.cumulative {
                    self.seen_sequences.pop_first();
                } else {
                    return Err(LedgerError::Capacity);
                }
            }
            self.seen_sequences.insert(sequence, print);
        }
        Ok(None)
    }

    fn bucket(&mut self, at_ms: u64, total_buckets: usize) -> Result<&mut Bucket, LedgerError> {
        let start = at_ms - at_ms % BUCKET_MS;
        if !self.buckets.contains_key(&start)
            && (self.buckets.len() >= MAX_BUCKETS_PER_STREAM
                || total_buckets >= MAX_EXECUTIONS * 16)
        {
            return Err(LedgerError::Capacity);
        }
        self.first_at_ms = Some(self.first_at_ms.map_or(at_ms, |first| first.min(at_ms)));
        self.last_at_ms = Some(self.last_at_ms.map_or(at_ms, |last| last.max(at_ms)));
        Ok(self.buckets.entry(start).or_default())
    }

    fn ingest_cumulative(
        &mut self,
        event: &UsageEvent,
        total_buckets: usize,
    ) -> Result<IngestOutcome, LedgerError> {
        if let Some(outcome) = self.check_identity(event)? {
            return Ok(outcome);
        }
        let print = fingerprint(event);
        let values = categories(&event.counts);
        let older = match (event.sequence, self.last_sequence) {
            (Some(sequence), Some(last)) => sequence < last,
            _ => false,
        };
        if event.sequence.is_none() && self.last_fingerprint == Some(print) {
            // Same running total re-delivered without identity: no new usage.
            self.diagnostics.duplicates += 1;
            return Ok(IngestOutcome::Duplicate);
        }
        let decreased = values
            .iter()
            .zip(self.high)
            .any(|(now, high)| now.zip(high).is_some_and(|(now, high)| now < high))
            || event
                .reasoning_output
                .zip(self.reasoning_high)
                .is_some_and(|(now, high)| now < high);
        let increments: [Option<u64>; 4] = std::array::from_fn(|i| {
            values[i].map(|now| now.saturating_sub(self.high[i].unwrap_or(0)))
        });
        let grows = increments.iter().flatten().any(|delta| *delta > 0)
            || values
                .iter()
                .zip(self.high)
                .any(|(now, high)| now.is_some() && high.is_none());
        if older {
            // An older snapshot cannot add usage beyond a later high-water.
            // If it exceeds it, counters were not monotonic: keep the
            // lower-bound high-water and flag the stream.
            if !grows {
                self.diagnostics.stale += 1;
                return Ok(IngestOutcome::Stale);
            }
            self.diagnostics.regressions += 1;
        } else {
            if decreased {
                self.diagnostics.regressions += 1;
            }
            let lost = values
                .iter()
                .zip(self.high)
                .any(|(now, high)| now.is_none() && high.is_some());
            if lost {
                self.diagnostics.missing_after_known += 1;
            }
            self.last_sequence = event.sequence.or(self.last_sequence);
            self.last_fingerprint = Some(print);
            self.latest_complete = event.complete;
        }
        let reasoning_increment = event
            .reasoning_output
            .map(|now| now.saturating_sub(self.reasoning_high.unwrap_or(0)));
        let at = event.at_ms();
        let bucket = self.bucket(at, total_buckets)?;
        for (slot, increment) in bucket.counts.iter_mut().zip(increments) {
            *slot = add(*slot, increment.unwrap_or(0))?;
        }
        bucket.reasoning = add(bucket.reasoning, reasoning_increment.unwrap_or(0))?;
        for (high, now) in self.high.iter_mut().zip(values) {
            if let Some(now) = now {
                *high = Some(high.map_or(now, |high| high.max(now)));
            }
        }
        if let Some(now) = event.reasoning_output {
            self.reasoning_high = Some(self.reasoning_high.map_or(now, |high| high.max(now)));
        }
        Ok(IngestOutcome::Applied)
    }

    fn ingest_delta(
        &mut self,
        event: &UsageEvent,
        total_buckets: usize,
    ) -> Result<IngestOutcome, LedgerError> {
        if let Some(outcome) = self.check_identity(event)? {
            return Ok(outcome);
        }
        if event.event_id.is_none() && event.sequence.is_none() {
            self.diagnostics.undeduplicable += 1;
        }
        if !event.complete {
            self.diagnostics.incomplete_captures += 1;
        }
        self.latest_complete = event.complete;
        let values = categories(&event.counts);
        let bucket = self.bucket(event.at_ms(), total_buckets)?;
        for (index, value) in values.iter().enumerate() {
            match value {
                Some(value) => bucket.counts[index] = add(bucket.counts[index], *value)?,
                None => bucket.unknown_mask |= 1 << index,
            }
        }
        match event.reasoning_output {
            Some(value) => bucket.reasoning = add(bucket.reasoning, value)?,
            None => bucket.reasoning_unknown = true,
        }
        Ok(IngestOutcome::Applied)
    }
}

fn add(a: u64, b: u64) -> Result<u64, LedgerError> {
    a.checked_add(b)
        .filter(|total| *total <= MAX_TOKEN_COUNT)
        .ok_or(LedgerError::Overflow)
}

struct AggregateBuilder {
    range: TimeRange,
    coverage_gap: bool,
    totals: [u64; 4],
    unknown: [bool; 4],
    reasoning: u64,
    reasoning_unknown: bool,
    observed_at_ms: Option<u64>,
    executions: u64,
    unmeasured: u64,
    contained: u64,
    measured_streams: u64,
    overflow: bool,
    sources: BTreeSet<UsageSource>,
    diagnostics: UsageDiagnostics,
    touched: bool,
}

impl AggregateBuilder {
    fn new(range: TimeRange, coverage_start_ms: u64) -> Self {
        Self {
            range,
            coverage_gap: range.start_ms < coverage_start_ms
                || range.start_ms % BUCKET_MS != 0 || range.end_ms % BUCKET_MS != 0,
            totals: [0; 4],
            unknown: [false; 4],
            reasoning: 0,
            reasoning_unknown: false,
            observed_at_ms: None,
            executions: 0,
            unmeasured: 0,
            contained: 0,
            measured_streams: 0,
            overflow: false,
            sources: BTreeSet::new(),
            diagnostics: UsageDiagnostics::default(),
            touched: false,
        }
    }

    fn add(&mut self, record: &ExecutionRecord, contained: bool) {
        let in_range_streams: Vec<&StreamState> = record
            .streams
            .values()
            .filter(|stream| stream.buckets.range(self.bucket_bounds()).next().is_some())
            .collect();
        let started_in_range = self.range.contains(record.binding.started_at_ms);
        if in_range_streams.is_empty() && !started_in_range {
            return;
        }
        self.touched = true;
        if contained {
            self.contained += 1;
            return;
        }
        self.executions += 1;
        if record.streams.is_empty() {
            self.unmeasured += 1;
            return;
        }
        for stream in in_range_streams {
            self.measured_streams += 1;
            self.diagnostics.add(&stream.diagnostics);
            if let Some(source) = stream.source {
                self.sources.insert(source);
            }
            self.observed_at_ms = self.observed_at_ms.max(stream.last_observed_ms);
            if stream.cumulative {
                for (unknown, high) in self.unknown.iter_mut().zip(stream.high) {
                    *unknown |= high.is_none();
                }
                self.reasoning_unknown |= stream.reasoning_high.is_none();
                // A later complete running total supersedes earlier partial
                // captures; only the latest snapshot's capture state counts.
                if !stream.latest_complete {
                    self.diagnostics.incomplete_captures += 1;
                }
            }
            for bucket in stream
                .buckets
                .range(self.bucket_bounds())
                .map(|(_, bucket)| bucket)
            {
                for (index, total) in self.totals.iter_mut().enumerate() {
                    match add(*total, bucket.counts[index]) {
                        Ok(sum) => *total = sum,
                        Err(_) => self.overflow = true,
                    }
                    self.unknown[index] |= bucket.unknown_mask & (1 << index) != 0;
                }
                match add(self.reasoning, bucket.reasoning) {
                    Ok(sum) => self.reasoning = sum,
                    Err(_) => self.overflow = true,
                }
                self.reasoning_unknown |= bucket.reasoning_unknown;
            }
        }
    }

    fn bucket_bounds(&self) -> std::ops::Range<u64> {
        // Buckets are 15-minute aligned; a range is matched by bucket start,
        // which is exact for 15-minute aligned boundaries (every local day).
        let start = self.range.start_ms - self.range.start_ms % BUCKET_MS;
        let start = if start < self.range.start_ms {
            start.saturating_add(BUCKET_MS)
        } else {
            start
        };
        start.min(self.range.end_ms)..self.range.end_ms
    }

    fn finish(self) -> UsageAggregate {
        let measured = self.measured_streams > 0;
        // Overflow, or no measurement in a range the ledger did not cover:
        // nothing is known (absence before coverage is not zero usage).
        let counts = if self.overflow || (!measured && self.coverage_gap) {
            super::UNKNOWN_COUNTS
        } else {
            let value = |index: usize| (!self.unknown[index]).then_some(self.totals[index]);
            TokenCounts {
                input: value(0),
                output: value(1),
                cache_write: value(2),
                cache_read: value(3),
            }
        };
        let reasoning_output =
            (measured && !self.reasoning_unknown && !self.overflow).then_some(self.reasoning);
        let complete = !self.coverage_gap
            && !self.overflow
            && self.unmeasured == 0
            && !self.diagnostics.degrades()
            && categories(&counts).iter().all(Option::is_some);
        UsageAggregate {
            counts,
            reasoning_output,
            complete,
            observed_at_ms: self.observed_at_ms,
            executions: self.executions,
            unmeasured_executions: self.unmeasured,
            contained_executions: self.contained,
            coverage_gap: self.coverage_gap,
            sources: self.sources.into_iter().collect(),
            diagnostics: self.diagnostics,
        }
    }
}
