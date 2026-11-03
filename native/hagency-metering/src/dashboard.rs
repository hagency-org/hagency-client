//! Shared owner-dashboard contract (ADR 0012, API version 1).
//!
//! The DTOs in this module are the wire/view model shared by the owner daemon,
//! the standalone controller and the embedded Hagency Agent page. They carry
//! *observations*, never authority: the presence of a field does not establish
//! that a runtime, framework or provider supports it. Unknown is represented
//! as `None`/`null`, never as zero, and the default snapshot is explicitly
//! unconfigured rather than ready.
//!
//! Enum values serialize as `snake_case`; struct fields as `camelCase`.
//!
//! Submodules provide the typed pipeline that populates a snapshot from real
//! runtime observations supplied by the trusted host (daemon):
//!
//! - [`ledger`]: execution-bound usage ingestion, de-duplication, cumulative
//!   and delta normalization, subagent containment and bounded persistence.
//! - [`quota`]: provider-native quota windows (Codex App Server rate limits,
//!   Claude status-line/SDK/usage-report data, Octos without universal quota).
//! - [`period`]: daily, weekly, monthly and custom ranges.
//! - [`collector`]: combines ledger, quota book and host state into a
//!   revisioned [`DashboardSnapshot`].

pub mod collector;
pub mod ledger;
pub mod period;
pub mod quota;

use crate::TokenCounts;
use serde::{Deserialize, Serialize};

/// Shared dashboard contract version carried in snapshots and IPC frames.
pub const DASHBOARD_API_VERSION: u32 = 1;

/// All four categories unknown. `TokenCounts` deliberately has no `Default`,
/// because zero is a measurement and unknown is not.
pub const UNKNOWN_COUNTS: TokenCounts = TokenCounts {
    input: None,
    output: None,
    cache_write: None,
    cache_read: None,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshot {
    pub version: u32,
    pub revision: u64,
    pub observed_at_ms: u64,
    pub owner: Option<OwnerIdentity>,
    pub hosting: HostingStatus,
    pub accounts: Vec<ProviderAccount>,
    pub resources: Vec<ResourceUsage>,
    /// Whether the most recent local inventory read succeeded. Rows may be
    /// retained from an older read when false; empty is not confirmed empty.
    #[serde(default)]
    pub inventory_available:bool,
    /// False when the inventory is partial (including a bounded page limit).
    #[serde(default)]
    pub inventory_complete:bool,
    pub usage: UsageSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_breakdown: Option<UsageBreakdown>,
    pub activities: Vec<Activity>,
}

impl DashboardSnapshot {
    /// No owner, hosting unconfigured, no accounts/resources and unknown
    /// usage. Never claims readiness or measured consumption.
    pub fn unconfigured(observed_at_ms: u64) -> Self {
        Self {
            version: DASHBOARD_API_VERSION,
            revision: 0,
            observed_at_ms,
            owner: None,
            hosting: HostingStatus::default(),
            accounts: Vec::new(),
            resources: Vec::new(),
            inventory_available:false,
            inventory_complete:false,
            usage: UsageSummary::default(),
            usage_breakdown: None,
            activities: Vec::new(),
        }
    }

    /// Whether a consumer built for [`DASHBOARD_API_VERSION`] can render this.
    pub fn is_compatible(&self) -> bool {
        self.version == DASHBOARD_API_VERSION
    }
}

/// Exact trusted execution dimensions, never name-based attribution.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageFilter {
    pub account_id: Option<String>,
    pub resource_id: Option<String>,
    pub model: Option<String>,
    pub requester_id: Option<String>,
}

impl UsageFilter {
    pub fn is_empty(&self) -> bool {
        self.account_id.is_none() && self.resource_id.is_none() && self.model.is_none() && self.requester_id.is_none()
    }
    pub fn query(&self, range: period::TimeRange) -> ledger::UsageQuery {
        ledger::UsageQuery { range, account_id: self.account_id.clone(), resource_id: self.resource_id.clone(),
            model: self.model.clone(), requester_id: self.requester_id.clone(), runtime: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBreakdown {
    pub filter: UsageFilter,
    pub report: collector::UsageReport,
    /// Choices come from the whole requested period so narrowing one field
    /// does not make another selected filter disappear.
    pub models: Vec<String>,
    pub requesters: Vec<String>,
}

impl Default for DashboardSnapshot {
    fn default() -> Self {
        Self::unconfigured(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerIdentity {
    pub user_id: String,
    pub display_name: String,
    pub server: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostingStatus {
    pub state: HostingState,
    pub detail: Option<String>,
    /// `None` when not observed; `Some(0)` only when observed idle.
    pub active_runs: Option<u64>,
    /// `None` when not observed; `Some(0)` only when observed empty.
    pub pending_approvals: Option<u64>,
}

impl Default for HostingStatus {
    fn default() -> Self {
        Self {
            state: HostingState::Unconfigured,
            detail: None,
            active_runs: None,
            pending_approvals: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostingState {
    #[default]
    Unconfigured,
    Starting,
    Running,
    Paused,
    Draining,
    Stopped,
    Offline,
    Failed,
}

impl HostingState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Draining => "draining",
            Self::Stopped => "stopped",
            Self::Offline => "offline",
            Self::Failed => "failed",
        }
    }
}

/// One provider account (plan card). `state` uses [`AccountState`] strings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAccount {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub plan: Option<String>,
    pub state: String,
    pub observed_at_ms: Option<u64>,
    pub quota_windows: Vec<QuotaWindow>,
    /// Ledger-attributed usage for the same selected period as the snapshot.
    #[serde(default)]
    pub usage: Option<UsageSummary>,
}

/// A provider-native window. `window_minutes` is only what the provider
/// reported; it is never inferred from a label or plan name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub id: String,
    pub label: String,
    pub used_percent: Option<f64>,
    pub resets_at_ms: Option<u64>,
    pub window_minutes: Option<u64>,
}

/// One hosted resource (pool/rented Agent group) and its measured usage.
/// `state` uses [`ResourceState`] strings; `account_id` is `None` unless the
/// provider account identity was verified by the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceUsage {
    pub id: String,
    pub name: String,
    pub account_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub agent_count: Option<u64>,
    pub budget_tokens: Option<u64>,
    pub state: String,
    pub counts: TokenCounts,
}

impl ResourceUsage {
    /// Token cache-hit ratio over this resource's measured scope.
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        cache_hit_ratio(&self.counts)
    }
}

/// Measured consumption over the selected period. `complete` is true only
/// when every contributing measurement was complete and de-duplicable.
/// `estimated_cost_usd` is an API-equivalent estimate, never a subscription
/// charge; it stays `None` unless the host supplies a priced estimate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub counts: TokenCounts,
    pub complete: bool,
    pub observed_at_ms: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
}

impl UsageSummary {
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        cache_hit_ratio(&self.counts)
    }
}

impl Default for UsageSummary {
    fn default() -> Self {
        Self {
            counts: UNKNOWN_COUNTS,
            complete: false,
            observed_at_ms: None,
            estimated_cost_usd: None,
        }
    }
}

/// `state` uses [`ActivityState`] strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub id: String,
    pub at_ms: u64,
    pub title: String,
    pub detail: String,
    pub state: String,
}

/// Token cache-hit ratio: `cache_read / (input + cache_read + cache_write)`,
/// computed from already-aggregated counts (`input` is fresh input). Any
/// unknown category or a zero denominator yields `None`.
pub fn cache_hit_ratio(counts: &TokenCounts) -> Option<f64> {
    let read = counts.cache_read?;
    let denominator =
        u128::from(counts.input?) + u128::from(read) + u128::from(counts.cache_write?);
    (denominator != 0).then(|| read as f64 / denominator as f64)
}

macro_rules! string_states {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($(#[$vmeta])* $variant),+ }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
            /// Unknown strings (e.g. from a newer daemon) return `None`.
            pub fn parse(value: &str) -> Option<Self> {
                match value { $($text => Some(Self::$variant),)+ _ => None }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_states! {
    /// Values for [`ProviderAccount::state`]. Describes the quota observation,
    /// not account authentication or executor readiness.
    AccountState {
        /// Provider data observed within its freshness limit.
        Fresh => "fresh",
        /// Last observation older than the capability's freshness limit.
        Stale => "stale",
        /// Some windows observed; others the provider reports are absent.
        Partial => "partial",
        /// The provider reported an active limit (rejected/reached).
        Limited => "limited",
        /// The runtime/auth mode exposes no quota data (e.g. API key, Octos).
        Unsupported => "unsupported",
        /// Supported in principle, but nothing observed yet.
        Unknown => "unknown",
        /// Provider identity could not be verified; never merged with others.
        Unverified => "unverified",
    }
}

string_states! {
    /// Values for [`ResourceUsage::state`]: completeness of measured usage.
    ResourceState {
        /// All contributing measurements complete.
        Measured => "measured",
        /// Some measurements incomplete, missing categories or ambiguous.
        Partial => "partial",
        /// Execution bound but no usage measurement received.
        Unmeasured => "unmeasured",
        /// No execution of this resource in the selected period.
        Idle => "idle",
    }
}

string_states! {
    /// Values for [`Activity::state`].
    ActivityState {
        Running => "running",
        Pending => "pending",
        Succeeded => "succeeded",
        Failed => "failed",
        Uncertain => "uncertain",
        Info => "info",
        Warning => "warning",
    }
}
