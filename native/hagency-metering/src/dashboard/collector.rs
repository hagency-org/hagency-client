//! Collector: the daemon-side owner of dashboard telemetry state.
//!
//! The owner daemon feeds trusted bindings, runtime usage observations,
//! provider quota payloads, resource registrations, hosting status and
//! activities into one [`DashboardCollector`], then serves
//! [`DashboardCollector::snapshot`] over IPC. Every mutation advances the
//! revision so subscribers can detect gaps. The collector never executes work
//! or writes server state; it only aggregates observations it is given.

use super::ledger::{
    DailyUsage, ExecutionBinding, GroupBy, IngestOutcome, LedgerError, UsageAggregate, UsageEvent,
    UsageLedger, UsageQuery,
};
use super::period::{PeriodError, TimeRange, UsagePeriod};
use super::quota::{AccountFreshness, QuotaBook, QuotaError, RefreshPolicy};
use super::{
    Activity, DASHBOARD_API_VERSION, DashboardSnapshot, HostingStatus, OwnerIdentity,
    ResourceState, ResourceUsage, UsageSummary,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_RESOURCES: usize = 1024;
pub const MAX_ACTIVITIES: usize = 256;
pub const SNAPSHOT_ACTIVITY_LIMIT: usize = 100;
pub const MAX_TEXT_BYTES: usize = 1024;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CollectorError {
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Quota(#[from] QuotaError),
    #[error(transparent)]
    Period(#[from] PeriodError),
    #[error("resource or activity field is empty or exceeds its bound")]
    InvalidField,
    #[error("account is not registered, or cannot be shared by this resource")]
    UnknownAccount,
    #[error("collector capacity exceeded")]
    Capacity,
}

/// Host-owned resource configuration (pool / rented Agent group).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRegistration {
    pub id: String,
    pub name: String,
    /// Registered quota-book account ID; verified, or this resource's own
    /// unverified account.
    #[serde(default)]
    pub account_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub agent_count: u64,
    /// The Agent's configured total budget, if any. Never a provider quota.
    #[serde(default)]
    pub budget_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotOptions {
    #[serde(default)]
    pub period: UsagePeriod,
    /// Owner's local UTC offset at `now`, supplied by the host.
    #[serde(default)]
    pub utc_offset_minutes: i32,
}

/// Drill-down report for the Usage destination (filters by account, model,
/// requester; calendar series). Not part of the snapshot DTO.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub range: TimeRange,
    pub total: UsageAggregate,
    pub by_account: Vec<(Option<String>, UsageAggregate)>,
    pub by_resource: Vec<(Option<String>, UsageAggregate)>,
    pub by_model: Vec<(Option<String>, UsageAggregate)>,
    pub by_requester: Vec<(Option<String>, UsageAggregate)>,
    pub daily: Vec<DailyUsage>,
    pub accounts: Vec<AccountFreshness>,
}

#[derive(Clone, Debug)]
pub struct DashboardCollector {
    revision: u64,
    owner: Option<OwnerIdentity>,
    hosting: HostingStatus,
    resources: BTreeMap<String, ResourceRegistration>,
    ledger: UsageLedger,
    quotas: QuotaBook,
    activities: BTreeMap<String, Activity>,
}

fn text(value: &str) -> Result<(), CollectorError> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES {
        return Err(CollectorError::InvalidField);
    }
    Ok(())
}

impl DashboardCollector {
    pub fn new(ledger: UsageLedger, policy: RefreshPolicy) -> Self {
        Self {
            revision: 0,
            owner: None,
            hosting: HostingStatus::default(),
            resources: BTreeMap::new(),
            ledger,
            quotas: QuotaBook::new(policy),
            activities: BTreeMap::new(),
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn touch(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    pub fn owner(&self) -> Option<&OwnerIdentity> {
        self.owner.as_ref()
    }

    /// Set the owner. A different owner (user or server) clears all private
    /// state and installs `ledger` (that owner's restored ledger), so late
    /// data cannot appear on another account's dashboard. Returns whether
    /// the owner identity changed.
    pub fn set_owner(
        &mut self,
        owner: Option<OwnerIdentity>,
        ledger: impl FnOnce() -> UsageLedger,
    ) -> bool {
        let same_identity = match (&self.owner, &owner) {
            (Some(a), Some(b)) => a.user_id == b.user_id && a.server == b.server,
            (None, None) => true,
            _ => false,
        };
        if same_identity {
            if self.owner != owner {
                self.owner = owner;
                self.touch();
            }
            return false;
        }
        let policy = self.quotas.policy();
        self.owner = owner;
        self.hosting = HostingStatus::default();
        self.resources.clear();
        self.ledger = ledger();
        self.quotas = QuotaBook::new(policy);
        self.activities.clear();
        self.touch();
        true
    }

    pub fn set_hosting(&mut self, hosting: HostingStatus) {
        if self.hosting != hosting {
            self.hosting = hosting;
            self.touch();
        }
    }

    pub fn hosting(&self) -> &HostingStatus {
        &self.hosting
    }

    pub fn upsert_resource(
        &mut self,
        resource: ResourceRegistration,
    ) -> Result<(), CollectorError> {
        for field in [
            &resource.id,
            &resource.name,
            &resource.provider,
            &resource.model,
        ] {
            text(field)?;
        }
        if let Some(account) = &resource.account_id
            && !self.quotas.usable_by(account, &resource.id)
        {
            return Err(CollectorError::UnknownAccount);
        }
        if !self.resources.contains_key(&resource.id) && self.resources.len() >= MAX_RESOURCES {
            return Err(CollectorError::Capacity);
        }
        if self.resources.get(&resource.id) != Some(&resource) {
            self.resources.insert(resource.id.clone(), resource);
            self.touch();
        }
        Ok(())
    }

    pub fn remove_resource(&mut self, id: &str) -> bool {
        let removed = self.resources.remove(id).is_some();
        if removed {
            self.touch();
        }
        removed
    }

    /// Record a trusted execution binding (from the daemon's launch record).
    pub fn bind_execution(&mut self, binding: ExecutionBinding) -> Result<bool, CollectorError> {
        if let Some(account) = &binding.account_id
            && !self.quotas.usable_by(account, &binding.resource_id)
        {
            return Err(CollectorError::UnknownAccount);
        }
        let added = self.ledger.bind_execution(binding)?;
        if added {
            self.touch();
        }
        Ok(added)
    }

    pub fn ingest_usage(&mut self, event: &UsageEvent) -> Result<IngestOutcome, CollectorError> {
        let outcome = self.ledger.ingest(event)?;
        if matches!(outcome, IngestOutcome::Applied | IngestOutcome::Conflict) {
            self.touch();
        }
        Ok(outcome)
    }

    pub fn ledger(&self) -> &UsageLedger {
        &self.ledger
    }

    /// Mutable ledger access (import/merge/prune). Advances the revision.
    pub fn ledger_mut(&mut self) -> &mut UsageLedger {
        self.touch();
        &mut self.ledger
    }

    pub fn quotas(&self) -> &QuotaBook {
        &self.quotas
    }

    /// Mutable quota access for provider observations. Advances the revision.
    pub fn quotas_mut(&mut self) -> &mut QuotaBook {
        self.touch();
        &mut self.quotas
    }

    /// Insert or update an activity by ID; the oldest are evicted.
    pub fn record_activity(&mut self, activity: Activity) -> Result<(), CollectorError> {
        text(&activity.id)?;
        text(&activity.title)?;
        text(&activity.state)?;
        if activity.detail.len() > MAX_TEXT_BYTES {
            return Err(CollectorError::InvalidField);
        }
        if self.activities.get(&activity.id) == Some(&activity) {
            return Ok(());
        }
        self.activities.insert(activity.id.clone(), activity);
        while self.activities.len() > MAX_ACTIVITIES {
            let oldest = self
                .activities
                .values()
                .min_by(|a, b| a.at_ms.cmp(&b.at_ms).then_with(|| a.id.cmp(&b.id)))
                .map(|activity| activity.id.clone());
            if let Some(oldest) = oldest {
                self.activities.remove(&oldest);
            }
        }
        self.touch();
        Ok(())
    }

    pub fn remove_activity(&mut self, id: &str) -> bool {
        let removed = self.activities.remove(id).is_some();
        if removed {
            self.touch();
        }
        removed
    }

    /// Build the shared DTO for the requested period.
    pub fn snapshot(
        &self,
        now_ms: u64,
        options: SnapshotOptions,
    ) -> Result<DashboardSnapshot, CollectorError> {
        let range = options.period.resolve(now_ms, options.utc_offset_minutes)?;
        let resources = self
            .resources
            .values()
            .map(|resource| {
                let mut query = UsageQuery::range(range);
                query.resource_id = Some(resource.id.clone());
                let usage = self.ledger.aggregate(&query);
                ResourceUsage {
                    id: resource.id.clone(),
                    name: resource.name.clone(),
                    account_id: resource.account_id.clone(),
                    provider: resource.provider.clone(),
                    model: resource.model.clone(),
                    agent_count: Some(resource.agent_count),
                    budget_tokens: resource.budget_tokens,
                    state: resource_state(&usage).as_str().to_owned(),
                    counts: usage.counts,
                }
            })
            .collect();
        let total = self.ledger.aggregate(&UsageQuery::range(range));
        let mut activities: Vec<Activity> = self.activities.values().cloned().collect();
        activities.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| a.id.cmp(&b.id)));
        activities.truncate(SNAPSHOT_ACTIVITY_LIMIT);
        Ok(DashboardSnapshot {
            version: DASHBOARD_API_VERSION,
            revision: self.revision,
            observed_at_ms: now_ms,
            owner: self.owner.clone(),
            hosting: self.hosting.clone(),
            accounts: self.quotas.project(now_ms).into_iter().map(|mut account| {
                let mut query = UsageQuery::range(range);
                query.account_id = Some(account.id.clone());
                let usage = self.ledger.aggregate(&query);
                account.usage = Some(UsageSummary { counts: usage.counts, complete: usage.complete,
                    observed_at_ms: usage.observed_at_ms, estimated_cost_usd: None });
                account
            }).collect(),
            resources,
            inventory_available:true,
            inventory_complete:true,
            usage: UsageSummary {
                counts: total.counts,
                complete: total.complete,
                observed_at_ms: total.observed_at_ms,
                estimated_cost_usd: None,
            },
            usage_breakdown: None,
            activities,
        })
    }

    /// Detailed usage for the Usage destination, with the same range rules.
    pub fn usage_report(
        &self,
        now_ms: u64,
        options: SnapshotOptions,
        filter: Option<UsageQuery>,
    ) -> Result<UsageReport, CollectorError> {
        let range = options.period.resolve(now_ms, options.utc_offset_minutes)?;
        let query = match filter {
            Some(filter) => UsageQuery { range, ..filter },
            None => UsageQuery::range(range),
        };
        let group = |by| self.ledger.aggregate_by(&query, by).into_iter().collect();
        let daily = if range == TimeRange::ALL {
            Vec::new()
        } else {
            self.ledger
                .daily_series(&query, options.utc_offset_minutes)?
        };
        let accounts = self
            .quotas
            .project(now_ms)
            .iter()
            .filter_map(|account| self.quotas.freshness(&account.id, now_ms))
            .collect();
        Ok(UsageReport {
            range,
            total: self.ledger.aggregate(&query),
            by_account: group(GroupBy::Account),
            by_resource: group(GroupBy::Resource),
            by_model: group(GroupBy::Model),
            by_requester: group(GroupBy::Requester),
            daily,
            accounts,
        })
    }
}

/// Completeness state of a resource's measured usage.
pub fn resource_state(usage: &UsageAggregate) -> ResourceState {
    if usage.executions == 0 && usage.contained_executions == 0 {
        // Before the ledger's coverage began, absence is not evidence of idleness.
        if usage.coverage_gap {
            ResourceState::Partial
        } else {
            ResourceState::Idle
        }
    } else if !usage.measured() && usage.contained_executions == 0 {
        ResourceState::Unmeasured
    } else if usage.complete {
        ResourceState::Measured
    } else {
        ResourceState::Partial
    }
}
