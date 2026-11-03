//! Read-only Codex App Server account rate-limit metadata.
//!
//! Captured only from the actual process's own `account/rateLimits/read`
//! response and `account/rateLimits/updated` notifications. The values are
//! account-wide quota windows, never per-turn usage or billing evidence, and
//! they grant no scope or authority. Only a fixed, bounded numeric/label
//! projection is retained; credit details, reset-credit rows and any
//! unrecognized fields are dropped.
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_BUCKETS: usize = 16;
const MAX_LABEL: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct RateLimitWindow {
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at_seconds: Option<u64>,
}
impl RateLimitWindow {
    pub fn used_percent(&self) -> Option<f64> {
        self.used_percent
    }
    pub fn window_minutes(&self) -> Option<u64> {
        self.window_minutes
    }
    /// Unix seconds, as documented by the App Server.
    pub fn resets_at_seconds(&self) -> Option<u64> {
        self.resets_at_seconds
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RateLimitBucket {
    limit_id: Option<String>,
    limit_name: Option<String>,
    primary: Option<RateLimitWindow>,
    secondary: Option<RateLimitWindow>,
    plan_type: Option<String>,
    reached_type: Option<String>,
}
impl RateLimitBucket {
    pub fn limit_id(&self) -> Option<&str> {
        self.limit_id.as_deref()
    }
    pub fn limit_name(&self) -> Option<&str> {
        self.limit_name.as_deref()
    }
    pub fn primary(&self) -> Option<&RateLimitWindow> {
        self.primary.as_ref()
    }
    pub fn secondary(&self) -> Option<&RateLimitWindow> {
        self.secondary.as_ref()
    }
    pub fn plan_type(&self) -> Option<&str> {
        self.plan_type.as_deref()
    }
    pub fn reached_type(&self) -> Option<&str> {
        self.reached_type.as_deref()
    }
}

/// How the snapshot was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateLimitOrigin {
    /// Full `account/rateLimits/read` response: replaces every bucket.
    Read,
    /// `account/rateLimits/updated` notification: one bucket.
    Updated,
}

/// One sanitized rate-limit observation from this actual process. No
/// constructor is public; hosts cannot fabricate it.
#[derive(Clone, Debug, PartialEq)]
pub struct RateLimitSnapshot {
    origin: RateLimitOrigin,
    /// Monotonic per process; increases with every accepted observation.
    revision: u64,
    /// Wall-clock receipt time (Unix ms) on this host.
    received_at_ms: u64,
    /// The bucket from `rateLimits` (backward-compatible single view).
    primary_bucket: Option<RateLimitBucket>,
    /// `rateLimitsByLimitId`, when present.
    by_limit_id: BTreeMap<String, RateLimitBucket>,
}
impl RateLimitSnapshot {
    pub fn origin(&self) -> RateLimitOrigin {
        self.origin
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn received_at_ms(&self) -> u64 {
        self.received_at_ms
    }
    pub fn bucket(&self) -> Option<&RateLimitBucket> {
        self.primary_bucket.as_ref()
    }
    pub fn by_limit_id(&self) -> &BTreeMap<String, RateLimitBucket> {
        &self.by_limit_id
    }
}

fn label(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= MAX_LABEL && !s.chars().any(char::is_control))
        .map(str::to_owned)
}

fn window(value: &Value) -> Option<RateLimitWindow> {
    let object = value.as_object()?;
    let used_percent = object
        .get("usedPercent")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 1000.0);
    let window_minutes = object
        .get("windowDurationMins")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0 && *v <= 10 * 366 * 24 * 60);
    let resets_at_seconds = object
        .get("resetsAt")
        .and_then(Value::as_u64)
        .filter(|v| *v <= 100_000_000_000);
    Some(RateLimitWindow {
        used_percent,
        window_minutes,
        resets_at_seconds,
    })
}

fn bucket(value: &Value) -> Option<RateLimitBucket> {
    value.as_object()?;
    Some(RateLimitBucket {
        limit_id: label(&value["limitId"]),
        limit_name: label(&value["limitName"]),
        primary: window(&value["primary"]),
        secondary: window(&value["secondary"]),
        plan_type: label(&value["planType"]),
        reached_type: label(&value["rateLimitReachedType"]),
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Stable public quota-subject digest shared by idle and execution observers.
/// Missing/malformed subjects never fall back to an email, model, or path.
pub fn quota_account_digest(subject:&str)->Option<String> {
    if subject.is_empty() || subject.len()>128
        || !subject.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'_'|b'-')) {return None;}
    hagency_core::canonical::digest(&serde_json::json!({"provider":"openai.chatgpt","subject":subject})).ok()
}

/// Per-process tracker owned by the session driver.
#[derive(Default)]
pub(super) struct RateLimitTracker {
    latest: Option<RateLimitSnapshot>,
    revision: u64,
    account_digest: Option<String>,
}
impl RateLimitTracker {
    pub(super) fn latest(&self) -> Option<&RateLimitSnapshot> {
        self.latest.as_ref()
    }
    /// Digest of the runtime-reported ChatGPT `accountId`, identical to the
    /// managed qualification provider digest. The raw subject is never kept.
    pub(super) fn account_digest(&self) -> Option<&str> {
        self.account_digest.as_deref()
    }
    /// Never fails the session: malformed metadata is simply not retained.
    pub(super) fn observe(&mut self, origin: RateLimitOrigin, value: &Value) {
        let Some(primary_bucket) = bucket(&value["rateLimits"]) else {
            return;
        };
        if origin == RateLimitOrigin::Read {
            self.account_digest=value["accountId"].as_str().and_then(quota_account_digest);
        }
        let mut by_limit_id = BTreeMap::new();
        if origin == RateLimitOrigin::Read
            && let Some(map) = value["rateLimitsByLimitId"].as_object()
        {
            for (key, value) in map.iter().take(MAX_BUCKETS) {
                if let (Some(key), Some(bucket)) = (label(&Value::String(key.clone())), bucket(value)) {
                    by_limit_id.insert(key, bucket);
                }
            }
        }
        self.revision += 1;
        self.latest = Some(RateLimitSnapshot {
            origin,
            revision: self.revision,
            received_at_ms: now_ms(),
            primary_bucket: Some(primary_bucket),
            by_limit_id,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projects_documented_fields_and_drops_everything_else() {
        let mut tracker = RateLimitTracker::default();
        tracker.observe(
            RateLimitOrigin::Read,
            &json!({"accountId":"secret_subject","rateLimits":{"limitId":"codex","primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":1793581200},"secondary":null,"credits":{"balance":"9"}},
                "rateLimitsByLimitId":{"codex_other":{"limitId":"codex_other","limitName":"Other","primary":{"usedPercent":42.5,"windowDurationMins":60,"resetsAt":1793581200}}},
                "rateLimitResetCredits":{"availableCount":2}}),
        );
        let snapshot = tracker.latest().unwrap();
        assert_eq!(snapshot.revision(), 1);
        let bucket = snapshot.bucket().unwrap();
        assert_eq!(bucket.limit_id(), Some("codex"));
        assert_eq!(bucket.primary().unwrap().window_minutes(), Some(300));
        assert!(bucket.secondary().is_none());
        assert_eq!(snapshot.by_limit_id()["codex_other"].primary().unwrap().used_percent(), Some(42.5));
        assert!(!format!("{snapshot:?}").contains("secret_subject"));
        assert_eq!(tracker.account_digest().map(str::len), Some(64));
        assert_eq!(tracker.account_digest(),quota_account_digest("secret_subject").as_deref());
        // Malformed metadata is ignored, never fatal and never clears data.
        tracker.observe(RateLimitOrigin::Updated, &json!({"rateLimits":"bad"}));
        assert_eq!(tracker.latest().unwrap().revision(), 1);
        tracker.observe(
            RateLimitOrigin::Updated,
            &json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":-3,"windowDurationMins":0}}}),
        );
        let updated = tracker.latest().unwrap();
        assert_eq!(updated.origin(), RateLimitOrigin::Updated);
        let primary = updated.bucket().unwrap().primary().unwrap();
        assert_eq!((primary.used_percent(), primary.window_minutes()), (None, None));
    }
}
