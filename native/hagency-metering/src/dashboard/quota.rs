//! Provider-native quota observations projected into [`ProviderAccount`]s.
//!
//! Inputs are typed, sanitized payloads that the trusted host already
//! received through a runtime's documented interface; nothing here reads
//! credentials, auth files or process environments, and no model turn is
//! generated to refresh a quota. Window lengths and reset times keep their
//! provider semantics. Nothing is inferred from a plan name, and no absolute
//! token allowance is hardcoded.
//!
//! Supported sources (each optional; absence stays unknown):
//!
//! - Codex App Server `account/rateLimits/read` (full replacement of Codex
//!   buckets) and `account/rateLimits/updated` (merge of one bucket).
//!   `usedPercent`, `windowDurationMins`, `resetsAt` (Unix seconds).
//! - Claude Code status-line `rate_limits` (`five_hour`, `seven_day`,
//!   `spend_limit`; `used_percentage` 0-100, `resets_at` Unix seconds). Only
//!   present for subscribers after the first API response; each window may
//!   be absent.
//! - Claude Agent SDK `rate_limit_event` (`status`, optional `resetsAt`).
//!   It does not name a window, so it only sets the account's limited state
//!   and an event-scoped reset; `utilization` has no documented unit and is
//!   not projected.
//! - Claude SDK `/usage` report `rate_limits.limits` rows (`kind`, `percent`,
//!   ISO-8601 `resets_at`), experimental upstream.
//! - Octos: no universal upstream subscription quota; register the account
//!   as unsupported unless an upstream adapter supplies windows.

use super::period::{parse_rfc3339_ms, unix_seconds_to_ms};
use super::{AccountState, ProviderAccount, QuotaWindow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_ACCOUNTS: usize = 256;
pub const MAX_WINDOWS_PER_ACCOUNT: usize = 32;
pub const MAX_LABEL_BYTES: usize = 128;
/// Default age after which an observation is shown as stale.
pub const DEFAULT_STALE_AFTER_MS: u64 = 15 * 60_000;
pub const DEFAULT_REFRESH_INTERVAL_MS: u64 = 5 * 60_000;
pub const DEFAULT_MAX_BACKOFF_MS: u64 = 60 * 60_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum QuotaError {
    #[error("quota observation names an unregistered account")]
    UnknownAccount,
    #[error("an identifier or label is empty or exceeds its bound")]
    InvalidField,
    #[error("account is registered with a different identity")]
    IdentityConflict,
    #[error("quota capacity exceeded")]
    Capacity,
}

/// Provider account identity. Grouping across resources happens only for
/// `Verified`; an unverified account is scoped to its single resource and is
/// never merged by email, display name, model or credential path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountIdentity {
    /// `subject` is an opaque, host-verified, stable provider account
    /// identifier obtained through native auth custody. It is hashed into
    /// the account ID and is never exposed in snapshots.
    Verified { provider: String, subject: String },
    #[serde(rename_all = "camelCase")]
    Unverified {
        provider: String,
        resource_id: String,
    },
}

impl AccountIdentity {
    pub fn provider(&self) -> &str {
        match self {
            Self::Verified { provider, .. } | Self::Unverified { provider, .. } => provider,
        }
    }
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }
    /// Stable dashboard account ID. Verified: `provider:` + 16 hex digits of
    /// SHA-256 over provider and subject. Unverified: per resource.
    pub fn account_id(&self) -> String {
        match self {
            Self::Verified { provider, subject } => {
                let digest = Sha256::digest(format!("hagency.account.v1\0{provider}\0{subject}"));
                let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
                format!("{provider}:{hex}")
            }
            Self::Unverified {
                provider,
                resource_id,
            } => format!("unverified:{provider}:{resource_id}"),
        }
    }
    fn validate(&self) -> Result<(), QuotaError> {
        let fields = match self {
            Self::Verified { provider, subject } => [provider, subject],
            Self::Unverified {
                provider,
                resource_id,
            } => [provider, resource_id],
        };
        for field in fields {
            label(field)?;
        }
        Ok(())
    }
}

fn label(value: &str) -> Result<(), QuotaError> {
    if value.is_empty() || value.len() > MAX_LABEL_BYTES || value.chars().any(char::is_control) {
        return Err(QuotaError::InvalidField);
    }
    Ok(())
}

fn clean_label(value: &str) -> Option<String> {
    let trimmed = value.trim();
    label(trimmed).ok().map(|_| trimmed.to_owned())
}

/// Whether this account's runtime/auth mode can report quota at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaCapability {
    /// A documented source exists; data may still be absent until observed.
    #[default]
    Supported,
    /// The mode (e.g. API key, Bedrock, Octos without upstream adapter)
    /// exposes no plan quota. Never shown as zero usage.
    Unsupported,
}

/// Provenance of a quota window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaSource {
    CodexRateLimitsRead,
    CodexRateLimitsUpdated,
    ClaudeStatusLine,
    ClaudeRateLimitEvent,
    ClaudeUsageReport,
    UpstreamAdapter,
}

impl QuotaSource {
    /// Sources that replace one another's windows as a set.
    fn family(self) -> u8 {
        match self {
            Self::CodexRateLimitsRead | Self::CodexRateLimitsUpdated => 0,
            Self::ClaudeStatusLine => 1,
            Self::ClaudeRateLimitEvent => 2,
            Self::ClaudeUsageReport => 3,
            Self::UpstreamAdapter => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRegistration {
    pub identity: AccountIdentity,
    pub label: String,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub capability: QuotaCapability,
    #[serde(default)]
    pub runtime_version: Option<String>,
}

// ---- Codex App Server payloads (camelCase as documented) ----

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexRateLimitWindow {
    pub used_percent: Option<f64>,
    pub window_duration_mins: Option<u64>,
    /// Unix seconds.
    pub resets_at: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexRateLimitSnapshot {
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub primary: Option<CodexRateLimitWindow>,
    pub secondary: Option<CodexRateLimitWindow>,
    pub plan_type: Option<String>,
    pub rate_limit_reached_type: Option<String>,
}

/// `account/rateLimits/read` result.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexRateLimitsRead {
    pub rate_limits: Option<CodexRateLimitSnapshot>,
    pub rate_limits_by_limit_id: Option<BTreeMap<String, CodexRateLimitSnapshot>>,
}

/// `account/rateLimits/updated` notification params.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexRateLimitsUpdated {
    pub rate_limits: Option<CodexRateLimitSnapshot>,
}

// ---- Claude Code payloads (snake_case as documented) ----

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeStatusWindow {
    pub used_percentage: Option<f64>,
    /// Unix seconds.
    pub resets_at: Option<f64>,
}

/// Status-line `rate_limits` object.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeStatusLineRateLimits {
    pub five_hour: Option<ClaudeStatusWindow>,
    pub seven_day: Option<ClaudeStatusWindow>,
    pub spend_limit: Option<ClaudeStatusWindow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeRateLimitStatus {
    Allowed,
    AllowedWarning,
    Rejected,
}

/// SDK `rate_limit_event.rate_limit_info`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeRateLimitInfo {
    pub status: ClaudeRateLimitStatus,
    /// Unix seconds when present.
    #[serde(default)]
    pub resets_at: Option<f64>,
    #[serde(default)]
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeUsageScopeLabel {
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeUsageScope {
    pub model: Option<ClaudeUsageScopeLabel>,
    pub surface: Option<ClaudeUsageScopeLabel>,
}

/// One `SDKUsageReport.rate_limits.limits` row.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeUsageLimit {
    pub kind: String,
    pub group: Option<String>,
    pub percent: Option<f64>,
    /// ISO 8601 with offset.
    pub resets_at: Option<String>,
    pub scope: Option<ClaudeUsageScope>,
    pub severity: Option<String>,
}

/// `SDKUsageReport.rate_limits`. `limits: None` means no rows to report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeUsageReportRateLimits {
    pub limits: Option<Vec<ClaudeUsageLimit>>,
}

/// One provider-neutral window from an upstream adapter (e.g. an Octos
/// upstream provider) that reports its own semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterWindow {
    pub id: String,
    pub label: String,
    pub used_percent: Option<f64>,
    pub resets_at_ms: Option<u64>,
    pub window_minutes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObservedWindow {
    window: QuotaWindow,
    source: QuotaSource,
    observed_at_ms: u64,
    reached: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshPolicy {
    pub interval_ms: u64,
    pub stale_after_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            interval_ms: DEFAULT_REFRESH_INTERVAL_MS,
            stale_after_ms: DEFAULT_STALE_AFTER_MS,
            max_backoff_ms: DEFAULT_MAX_BACKOFF_MS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountQuota {
    registration: AccountRegistration,
    windows: BTreeMap<String, ObservedWindow>,
    last_observed_ms: Option<u64>,
    /// Latest SDK rate-limit status, with its observation time.
    event_status: Option<(ClaudeRateLimitStatus, u64)>,
    /// Plan reported by the provider (overrides the registered hint).
    reported_plan: Option<String>,
    refresh_failures: u32,
    next_refresh_ms: u64,
    last_error: Option<String>,
}

/// Freshness/capability detail beyond the DTO's `state` string.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountFreshness {
    pub account_id: String,
    pub state: AccountState,
    pub verified: bool,
    pub capability: QuotaCapability,
    pub age_ms: Option<u64>,
    pub sources: Vec<QuotaSource>,
    pub expired_windows: u32,
    pub last_error: Option<String>,
    pub next_refresh_ms: u64,
}

/// Per physical account quota state, shared by every resource on it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuotaBook {
    accounts: BTreeMap<String, AccountQuota>,
    policy: RefreshPolicy,
}

impl QuotaBook {
    pub fn new(policy: RefreshPolicy) -> Self {
        Self {
            accounts: BTreeMap::new(),
            policy,
        }
    }

    pub fn policy(&self) -> RefreshPolicy {
        self.policy
    }

    /// Register (or update label/plan/capability of) an account; returns its ID.
    pub fn register(&mut self, registration: AccountRegistration) -> Result<String, QuotaError> {
        registration.identity.validate()?;
        label(&registration.label)?;
        if let Some(plan) = &registration.plan {
            label(plan)?;
        }
        let id = registration.identity.account_id();
        match self.accounts.get_mut(&id) {
            Some(existing) => {
                if existing.registration.identity != registration.identity {
                    return Err(QuotaError::IdentityConflict);
                }
                existing.registration = registration;
            }
            None => {
                if self.accounts.len() >= MAX_ACCOUNTS {
                    return Err(QuotaError::Capacity);
                }
                self.accounts.insert(
                    id.clone(),
                    AccountQuota {
                        registration,
                        windows: BTreeMap::new(),
                        last_observed_ms: None,
                        event_status: None,
                        reported_plan: None,
                        refresh_failures: 0,
                        next_refresh_ms: 0,
                        last_error: None,
                    },
                );
            }
        }
        Ok(id)
    }

    pub fn remove(&mut self, account_id: &str) -> bool {
        self.accounts.remove(account_id).is_some()
    }

    pub fn contains(&self, account_id: &str) -> bool {
        self.accounts.contains_key(account_id)
    }

    pub fn identity(&self, account_id: &str) -> Option<&AccountIdentity> {
        self.accounts
            .get(account_id)
            .map(|account| &account.registration.identity)
    }

    /// Whether `resource_id` may attribute to this account: any verified
    /// account, or the unverified account scoped to that same resource.
    pub fn usable_by(&self, account_id: &str, resource_id: &str) -> bool {
        match self.identity(account_id) {
            Some(AccountIdentity::Verified { .. }) => true,
            Some(AccountIdentity::Unverified {
                resource_id: owner, ..
            }) => owner == resource_id,
            None => false,
        }
    }

    pub fn is_verified(&self, account_id: &str) -> bool {
        self.accounts
            .get(account_id)
            .is_some_and(|account| account.registration.identity.is_verified())
    }

    fn account(&mut self, account_id: &str) -> Result<&mut AccountQuota, QuotaError> {
        self.accounts
            .get_mut(account_id)
            .ok_or(QuotaError::UnknownAccount)
    }

    /// Full Codex rate-limit read: replaces every Codex window.
    pub fn apply_codex_read(
        &mut self,
        account_id: &str,
        observed_at_ms: u64,
        read: &CodexRateLimitsRead,
    ) -> Result<(), QuotaError> {
        let policy = self.policy;
        let account = self.account(account_id)?;
        let mut windows = Vec::new();
        let mut plan = None;
        match &read.rate_limits_by_limit_id {
            Some(buckets) if !buckets.is_empty() => {
                for (key, bucket) in buckets {
                    plan = plan.or(bucket.plan_type.clone());
                    codex_windows(Some(key), bucket, &mut windows);
                }
            }
            _ => {
                if let Some(bucket) = &read.rate_limits {
                    plan = bucket.plan_type.clone();
                    codex_windows(None, bucket, &mut windows);
                }
            }
        }
        account.replace_family(QuotaSource::CodexRateLimitsRead, windows, observed_at_ms)?;
        if let Some(plan) = plan.as_deref().and_then(clean_label) {
            account.reported_plan = Some(plan);
        }
        account.refreshed(observed_at_ms, policy);
        Ok(())
    }

    /// Codex rate-limit notification: merges the named bucket only.
    pub fn apply_codex_updated(
        &mut self,
        account_id: &str,
        observed_at_ms: u64,
        update: &CodexRateLimitsUpdated,
    ) -> Result<(), QuotaError> {
        let account = self.account(account_id)?;
        let Some(bucket) = &update.rate_limits else {
            return Ok(());
        };
        let mut windows = Vec::new();
        codex_windows(None, bucket, &mut windows);
        let prefix = format!("codex.{}.", bucket_key(None, bucket));
        let newer = account.windows.iter().any(|(id, observed)| {
            id.starts_with(&prefix) && observed.observed_at_ms > observed_at_ms
        });
        if newer {
            return Ok(()); // out-of-order older notification
        }
        account.windows.retain(|id, observed| {
            observed.source.family() != QuotaSource::CodexRateLimitsUpdated.family()
                || !id.starts_with(&prefix)
        });
        for (window, reached) in windows {
            account.insert(
                window,
                QuotaSource::CodexRateLimitsUpdated,
                observed_at_ms,
                reached,
            )?;
        }
        if let Some(plan) = bucket.plan_type.as_deref().and_then(clean_label) {
            account.reported_plan = Some(plan);
        }
        account.last_observed_ms = account.last_observed_ms.max(Some(observed_at_ms));
        Ok(())
    }

    /// Claude status-line `rate_limits`: replaces the status-line window set
    /// (Claude Code drops a window once it resets). `None` means the object
    /// was absent, which is not evidence of zero usage; nothing changes.
    pub fn apply_claude_status_line(
        &mut self,
        account_id: &str,
        observed_at_ms: u64,
        limits: Option<&ClaudeStatusLineRateLimits>,
    ) -> Result<(), QuotaError> {
        let policy = self.policy;
        let account = self.account(account_id)?;
        let Some(limits) = limits else {
            return Ok(());
        };
        let mut windows = Vec::new();
        for (id, title, minutes, window) in [
            (
                "claude.five_hour",
                "5-hour limit",
                Some(300),
                &limits.five_hour,
            ),
            (
                "claude.seven_day",
                "7-day limit",
                Some(10_080),
                &limits.seven_day,
            ),
            (
                "claude.spend_limit",
                "Spend limit",
                None,
                &limits.spend_limit,
            ),
        ] {
            if let Some(window) = window {
                let used = percent(window.used_percentage, id != "claude.spend_limit");
                windows.push((
                    QuotaWindow {
                        id: id.to_owned(),
                        label: title.to_owned(),
                        used_percent: used,
                        resets_at_ms: window.resets_at.and_then(unix_seconds_to_ms),
                        window_minutes: minutes,
                    },
                    used.is_some_and(|used| used >= 100.0),
                ));
            }
        }
        account.replace_family(QuotaSource::ClaudeStatusLine, windows, observed_at_ms)?;
        account.refreshed(observed_at_ms, policy);
        Ok(())
    }

    /// Claude SDK `rate_limit_event`. Records the limit status and, when
    /// given, the reset time of the event's (unnamed) limit.
    pub fn apply_claude_rate_limit_event(
        &mut self,
        account_id: &str,
        observed_at_ms: u64,
        info: &ClaudeRateLimitInfo,
    ) -> Result<(), QuotaError> {
        let account = self.account(account_id)?;
        if account
            .event_status
            .is_some_and(|(_, previous)| previous > observed_at_ms)
        {
            return Ok(()); // out-of-order older event
        }
        account.event_status = Some((info.status, observed_at_ms));
        let windows = match info.resets_at.and_then(unix_seconds_to_ms) {
            Some(resets_at_ms) if info.status != ClaudeRateLimitStatus::Allowed => vec![(
                QuotaWindow {
                    id: "claude.rate_limit_event".to_owned(),
                    label: "Current rate limit".to_owned(),
                    used_percent: None,
                    resets_at_ms: Some(resets_at_ms),
                    window_minutes: None,
                },
                info.status == ClaudeRateLimitStatus::Rejected,
            )],
            _ => Vec::new(),
        };
        account.replace_family(QuotaSource::ClaudeRateLimitEvent, windows, observed_at_ms)?;
        account.last_observed_ms = account.last_observed_ms.max(Some(observed_at_ms));
        Ok(())
    }

    /// Claude SDK usage report. `None` (`rate_limits: null`) is unknown and
    /// changes nothing; an empty `limits` array clears the report's windows.
    pub fn apply_claude_usage_report(
        &mut self,
        account_id: &str,
        observed_at_ms: u64,
        report: Option<&ClaudeUsageReportRateLimits>,
    ) -> Result<(), QuotaError> {
        let policy = self.policy;
        let account = self.account(account_id)?;
        let Some(limits) = report.and_then(|report| report.limits.as_ref()) else {
            return Ok(());
        };
        let mut windows = Vec::new();
        let mut used_ids = BTreeMap::new();
        for row in limits.iter().take(MAX_WINDOWS_PER_ACCOUNT) {
            let Some(kind) = clean_label(&row.kind) else {
                continue;
            };
            let scope = row.scope.as_ref().and_then(|scope| {
                scope
                    .model
                    .as_ref()
                    .or(scope.surface.as_ref())
                    .and_then(|label| label.display_name.as_deref())
                    .and_then(clean_label)
            });
            let mut id = match &scope {
                Some(scope) => format!("claude.usage.{kind}.{scope}"),
                None => format!("claude.usage.{kind}"),
            };
            let seen = used_ids.entry(id.clone()).or_insert(0u32);
            *seen += 1;
            if *seen > 1 {
                id = format!("{id}#{seen}");
            }
            let title = match &scope {
                Some(scope) => format!("{} ({scope})", kind.replace('_', " ")),
                None => kind.replace('_', " "),
            };
            let used = percent(row.percent, true);
            windows.push((
                QuotaWindow {
                    id,
                    label: title,
                    used_percent: used,
                    resets_at_ms: row.resets_at.as_deref().and_then(parse_rfc3339_ms),
                    window_minutes: None,
                },
                row.severity.as_deref() == Some("critical")
                    && used.is_some_and(|used| used >= 100.0),
            ));
        }
        account.replace_family(QuotaSource::ClaudeUsageReport, windows, observed_at_ms)?;
        account.refreshed(observed_at_ms, policy);
        Ok(())
    }

    /// Windows from an upstream adapter that reports its own semantics.
    pub fn apply_adapter_windows(
        &mut self,
        account_id: &str,
        observed_at_ms: u64,
        windows: &[AdapterWindow],
    ) -> Result<(), QuotaError> {
        let policy = self.policy;
        let account = self.account(account_id)?;
        let mut projected = Vec::new();
        for window in windows.iter().take(MAX_WINDOWS_PER_ACCOUNT) {
            let (Some(id), Some(title)) = (clean_label(&window.id), clean_label(&window.label))
            else {
                return Err(QuotaError::InvalidField);
            };
            let used = percent(window.used_percent, false);
            projected.push((
                QuotaWindow {
                    id: format!("adapter.{id}"),
                    label: title,
                    used_percent: used,
                    resets_at_ms: window.resets_at_ms,
                    window_minutes: window.window_minutes.filter(|minutes| *minutes > 0),
                },
                used.is_some_and(|used| used >= 100.0),
            ));
        }
        account.replace_family(QuotaSource::UpstreamAdapter, projected, observed_at_ms)?;
        account.refreshed(observed_at_ms, policy);
        Ok(())
    }

    /// Record a failed provider refresh. `retry_after_ms` (e.g. from a rate
    /// limit) is honored; otherwise backoff doubles up to the policy max.
    pub fn record_refresh_failure(
        &mut self,
        account_id: &str,
        now_ms: u64,
        reason: &str,
        retry_after_ms: Option<u64>,
    ) -> Result<(), QuotaError> {
        let policy = self.policy;
        let account = self.account(account_id)?;
        account.refresh_failures = account.refresh_failures.saturating_add(1);
        let exponent = account.refresh_failures.min(16);
        let backoff = policy
            .interval_ms
            .saturating_mul(1u64 << exponent)
            .min(policy.max_backoff_ms);
        let wait = retry_after_ms.map_or(backoff, |retry| retry.max(backoff));
        account.next_refresh_ms = now_ms.saturating_add(wait);
        let reason: String = reason
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_LABEL_BYTES)
            .collect();
        account.last_error = (!reason.is_empty()).then_some(reason);
        Ok(())
    }

    /// Supported accounts whose next poll is due. One entry per physical
    /// account, so resources sharing it share one poll.
    pub fn due_for_refresh(&self, now_ms: u64) -> Vec<String> {
        self.accounts
            .iter()
            .filter(|(_, account)| {
                account.registration.capability == QuotaCapability::Supported
                    && account.next_refresh_ms <= now_ms
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn freshness(&self, account_id: &str, now_ms: u64) -> Option<AccountFreshness> {
        let account = self.accounts.get(account_id)?;
        let mut sources: Vec<_> = account
            .windows
            .values()
            .map(|window| window.source)
            .collect();
        sources.sort();
        sources.dedup();
        Some(AccountFreshness {
            account_id: account_id.to_owned(),
            state: account.state(now_ms, self.policy),
            verified: account.registration.identity.is_verified(),
            capability: account.registration.capability,
            age_ms: account.last_observed_ms.map(|at| now_ms.saturating_sub(at)),
            sources,
            expired_windows: account.expired(now_ms),
            last_error: account.last_error.clone(),
            next_refresh_ms: account.next_refresh_ms,
        })
    }

    /// Project every account into dashboard DTOs at `now_ms`. Windows whose
    /// reset has passed keep their reset time but lose `used_percent`, since
    /// post-reset usage is unknown until the provider reports again.
    pub fn project(&self, now_ms: u64) -> Vec<ProviderAccount> {
        self.accounts
            .iter()
            .map(|(id, account)| {
                let mut windows: Vec<QuotaWindow> = account
                    .windows
                    .values()
                    .map(|observed| {
                        let mut window = observed.window.clone();
                        if window.resets_at_ms.is_some_and(|reset| reset <= now_ms) {
                            window.used_percent = None;
                        }
                        window
                    })
                    .collect();
                windows.sort_by(|a, b| {
                    a.window_minutes
                        .unwrap_or(u64::MAX)
                        .cmp(&b.window_minutes.unwrap_or(u64::MAX))
                        .then_with(|| a.id.cmp(&b.id))
                });
                ProviderAccount {
                    usage: None,
                    id: id.clone(),
                    label: account.registration.label.clone(),
                    provider: account.registration.identity.provider().to_owned(),
                    plan: account
                        .reported_plan
                        .clone()
                        .or_else(|| account.registration.plan.clone()),
                    state: account.state(now_ms, self.policy).as_str().to_owned(),
                    observed_at_ms: account.last_observed_ms,
                    quota_windows: windows,
                }
            })
            .collect()
    }
}

fn percent(value: Option<f64>, capped: bool) -> Option<f64> {
    let value = value.filter(|value| value.is_finite() && *value >= 0.0)?;
    // Spend limits may exceed 100; plan windows are 0-100 by definition.
    if capped && value > 100.0 {
        return Some(100.0);
    }
    (value <= 1000.0).then_some(value)
}

fn bucket_key(key: Option<&str>, bucket: &CodexRateLimitSnapshot) -> String {
    key.or(bucket.limit_id.as_deref())
        .and_then(clean_label)
        .unwrap_or_else(|| "default".to_owned())
}

fn codex_windows(
    key: Option<&str>,
    bucket: &CodexRateLimitSnapshot,
    out: &mut Vec<(QuotaWindow, bool)>,
) {
    let limit = bucket_key(key, bucket);
    let name = bucket.limit_name.as_deref().and_then(clean_label);
    // `rateLimitReachedType` is bucket-level. Attribute it to the windows
    // that are full; only if none reports 100% does it cover the bucket.
    let full = |window: &Option<CodexRateLimitWindow>| {
        window
            .as_ref()
            .and_then(|window| percent(window.used_percent, true))
            .is_some_and(|used| used >= 100.0)
    };
    let any_full = full(&bucket.primary) || full(&bucket.secondary);
    let reached = bucket.rate_limit_reached_type.is_some() && !any_full;
    for (slot, window) in [
        ("primary", &bucket.primary),
        ("secondary", &bucket.secondary),
    ] {
        let Some(window) = window else { continue };
        let minutes = window.window_duration_mins.filter(|minutes| *minutes > 0);
        let title = match (&name, minutes) {
            (Some(name), Some(minutes)) => format!("{name} · {}", duration_label(minutes)),
            (Some(name), None) => format!("{name} · {slot}"),
            (None, Some(minutes)) => format!("{} limit", duration_label(minutes)),
            (None, None) => format!("{slot} limit"),
        };
        let used = percent(window.used_percent, true);
        out.push((
            QuotaWindow {
                id: format!("codex.{limit}.{slot}"),
                label: title,
                used_percent: used,
                resets_at_ms: window.resets_at.and_then(unix_seconds_to_ms),
                window_minutes: minutes,
            },
            reached || used.is_some_and(|used| used >= 100.0),
        ));
    }
}

/// Human label for a provider-reported window length.
pub fn duration_label(minutes: u64) -> String {
    match minutes {
        m if m % 10_080 == 0 => format!("{}-week", m / 10_080),
        m if m % 1440 == 0 => format!("{}-day", m / 1440),
        m if m % 60 == 0 => format!("{}-hour", m / 60),
        m => format!("{m}-minute"),
    }
}

impl AccountQuota {
    fn insert(
        &mut self,
        window: QuotaWindow,
        source: QuotaSource,
        observed_at_ms: u64,
        reached: bool,
    ) -> Result<(), QuotaError> {
        if let Some(existing) = self.windows.get(&window.id) {
            if existing.observed_at_ms > observed_at_ms {
                return Ok(()); // never let an older observation overwrite
            }
        } else if self.windows.len() >= MAX_WINDOWS_PER_ACCOUNT {
            return Err(QuotaError::Capacity);
        }
        self.windows.insert(
            window.id.clone(),
            ObservedWindow {
                window,
                source,
                observed_at_ms,
                reached,
            },
        );
        Ok(())
    }

    fn replace_family(
        &mut self,
        source: QuotaSource,
        windows: Vec<(QuotaWindow, bool)>,
        observed_at_ms: u64,
    ) -> Result<(), QuotaError> {
        let family = source.family();
        let newest = self
            .windows
            .values()
            .filter(|window| window.source.family() == family)
            .map(|window| window.observed_at_ms)
            .max();
        if newest.is_some_and(|newest| newest > observed_at_ms) {
            return Ok(()); // out-of-order older full read
        }
        let others = self
            .windows
            .values()
            .filter(|window| window.source.family() != family)
            .count();
        if others + windows.len() > MAX_WINDOWS_PER_ACCOUNT {
            return Err(QuotaError::Capacity); // refused before any mutation
        }
        self.windows
            .retain(|_, window| window.source.family() != family);
        for (window, reached) in windows {
            self.insert(window, source, observed_at_ms, reached)?;
        }
        self.last_observed_ms = self.last_observed_ms.max(Some(observed_at_ms));
        Ok(())
    }

    fn refreshed(&mut self, observed_at_ms: u64, policy: RefreshPolicy) {
        self.refresh_failures = 0;
        self.last_error = None;
        self.next_refresh_ms = self
            .next_refresh_ms
            .max(observed_at_ms.saturating_add(policy.interval_ms));
    }

    fn expired(&self, now_ms: u64) -> u32 {
        self.windows
            .values()
            .filter(|window| {
                window
                    .window
                    .resets_at_ms
                    .is_some_and(|reset| reset <= now_ms)
            })
            .count() as u32
    }

    fn state(&self, now_ms: u64, policy: RefreshPolicy) -> AccountState {
        if !self.registration.identity.is_verified() {
            return AccountState::Unverified;
        }
        if self.registration.capability == QuotaCapability::Unsupported && self.windows.is_empty() {
            return AccountState::Unsupported;
        }
        let Some(last) = self.last_observed_ms else {
            return AccountState::Unknown;
        };
        let live = |window: &&ObservedWindow| {
            !window
                .window
                .resets_at_ms
                .is_some_and(|reset| reset <= now_ms)
        };
        let rejected = self.event_status.is_some_and(|(status, at)| {
            status == ClaudeRateLimitStatus::Rejected
                && self
                    .windows
                    .get("claude.rate_limit_event")
                    .is_none_or(|window| live(&window))
                && now_ms.saturating_sub(at) <= policy.stale_after_ms
        });
        if rejected
            || self
                .windows
                .values()
                .filter(live)
                .any(|window| window.reached)
        {
            return AccountState::Limited;
        }
        if now_ms.saturating_sub(last) > policy.stale_after_ms || self.expired(now_ms) > 0 {
            return AccountState::Stale;
        }
        if self.windows.is_empty() {
            // Observed, but the provider reported no window: nothing to show.
            return AccountState::Unknown;
        }
        if self
            .windows
            .values()
            .any(|window| window.window.used_percent.is_none())
        {
            return AccountState::Partial;
        }
        AccountState::Fresh
    }
}
