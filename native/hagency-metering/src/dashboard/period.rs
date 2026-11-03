//! Calendar consumption periods. These are local reporting ranges only; they
//! never imply that a provider supplies a matching allowance window.
//!
//! No timezone database is bundled. The trusted host passes the UTC offset in
//! effect for the owner's local timezone at `now`; across a daylight-saving
//! transition the earlier boundary can therefore differ from wall-clock local
//! midnight by the size of the transition. Use [`UsagePeriod::Range`] with
//! host-computed boundaries when exact DST boundaries matter.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MS_PER_MINUTE: u64 = 60_000;
pub const MS_PER_DAY: u64 = 86_400_000;
/// Every real-world UTC offset is a multiple of 15 minutes, so 15-minute
/// buckets keep local day boundaries exact.
pub const BUCKET_MS: u64 = 15 * MS_PER_MINUTE;
/// Largest accepted UTC offset magnitude (UTC-12:00 .. UTC+14:00 in practice).
pub const MAX_UTC_OFFSET_MINUTES: i32 = 18 * 60;
/// Upper bound on per-day series length.
pub const MAX_SERIES_DAYS: u64 = 400;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PeriodError {
    #[error("range start must be before its end")]
    EmptyRange,
    #[error("UTC offset is outside the supported range")]
    InvalidOffset,
    #[error("time is outside the supported calendar range")]
    OutOfRange,
    #[error("series exceeds the bounded day count")]
    TooLong,
}

/// Half-open millisecond interval `[start_ms, end_ms)` in Unix time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl TimeRange {
    pub fn new(start_ms: u64, end_ms: u64) -> Result<Self, PeriodError> {
        if start_ms >= end_ms {
            return Err(PeriodError::EmptyRange);
        }
        Ok(Self { start_ms, end_ms })
    }
    pub const ALL: Self = Self {
        start_ms: 0,
        end_ms: u64::MAX,
    };
    pub fn contains(&self, at_ms: u64) -> bool {
        self.start_ms <= at_ms && at_ms < self.end_ms
    }
    /// Whether any part of `[start, end)` intersects this range.
    pub fn overlaps(&self, start_ms: u64, end_ms: u64) -> bool {
        start_ms < self.end_ms && self.start_ms < end_ms
    }
}

/// Requested reporting period. Serialized as `{"kind":"day"}`,
/// `{"kind":"range","startMs":..,"endMs":..}` and so on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UsagePeriod {
    /// Local calendar day containing `now`.
    #[default]
    Day,
    /// ISO week (Monday start) containing `now`.
    Week,
    /// Calendar month containing `now`.
    Month,
    /// Explicit host/owner-selected range.
    #[serde(rename_all = "camelCase")]
    Range { start_ms: u64, end_ms: u64 },
    /// Everything the ledger holds (coverage starts at the ledger origin).
    All,
}

impl UsagePeriod {
    pub fn resolve(self, now_ms: u64, utc_offset_minutes: i32) -> Result<TimeRange, PeriodError> {
        let offset_ms = offset_ms(utc_offset_minutes)?;
        let local_now = i128::from(now_ms) + offset_ms;
        let local_day = local_now.div_euclid(i128::from(MS_PER_DAY));
        let (start_day, end_day) = match self {
            Self::Range { start_ms, end_ms } => return TimeRange::new(start_ms, end_ms),
            Self::All => return Ok(TimeRange::ALL),
            Self::Day => (local_day, local_day + 1),
            Self::Week => {
                // 1970-01-01 was a Thursday; Monday-based weekday index.
                let weekday = (local_day + 3).rem_euclid(7);
                (local_day - weekday, local_day - weekday + 7)
            }
            Self::Month => {
                let (year, month, _) = civil_from_days(local_day as i64);
                let start = days_from_civil(year, month, 1);
                let (next_year, next_month) = if month == 12 {
                    (year + 1, 1)
                } else {
                    (year, month + 1)
                };
                (
                    i128::from(start),
                    i128::from(days_from_civil(next_year, next_month, 1)),
                )
            }
        };
        let to_utc = |day: i128| -> Result<u64, PeriodError> {
            u64::try_from(day * i128::from(MS_PER_DAY) - offset_ms)
                .map_err(|_| PeriodError::OutOfRange)
        };
        TimeRange::new(to_utc(start_day)?, to_utc(end_day)?)
    }
}

pub(crate) fn offset_ms(utc_offset_minutes: i32) -> Result<i128, PeriodError> {
    if utc_offset_minutes.abs() > MAX_UTC_OFFSET_MINUTES {
        return Err(PeriodError::InvalidOffset);
    }
    Ok(i128::from(utc_offset_minutes) * i128::from(MS_PER_MINUTE))
}

/// Local-day start (Unix ms) for `at_ms` at the given offset.
pub fn local_day_start(at_ms: u64, utc_offset_minutes: i32) -> Result<u64, PeriodError> {
    let offset = offset_ms(utc_offset_minutes)?;
    let day = (i128::from(at_ms) + offset).div_euclid(i128::from(MS_PER_DAY));
    u64::try_from(day * i128::from(MS_PER_DAY) - offset).map_err(|_| PeriodError::OutOfRange)
}

/// Proleptic Gregorian date to days since 1970-01-01 (H. Hinnant's algorithm).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Days since 1970-01-01 to `(year, month, day)`.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Parse an RFC 3339 / ISO 8601 timestamp with an explicit `Z` or `±HH:MM`
/// offset (as used by Claude usage reports) into Unix milliseconds.
/// Timestamps without an offset are rejected rather than guessed.
pub fn parse_rfc3339_ms(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if value.len() > 64 || bytes.len() < 20 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<u32> {
        let part = value.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    if bytes[4] != b'-' || bytes[7] != b'-' || !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year.into(), month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut index = 19;
    let mut millis = 0u64;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        let fraction = &value[start..index];
        if fraction.is_empty() {
            return None;
        }
        let padded: String = fraction.chars().chain("000".chars()).take(3).collect();
        millis = padded.parse().ok()?;
    }
    let offset_minutes: i64 = match &value[index..] {
        "Z" | "z" => 0,
        rest if rest.len() == 6
            && matches!(rest.as_bytes()[0], b'+' | b'-')
            && rest.as_bytes()[3] == b':' =>
        {
            let hours = num(index + 1..index + 3)?;
            let minutes = num(index + 4..index + 6)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let magnitude = i64::from(hours * 60 + minutes);
            if rest.as_bytes()[0] == b'-' {
                -magnitude
            } else {
                magnitude
            }
        }
        _ => return None,
    };
    let days = days_from_civil(year.into(), month, day);
    let seconds =
        days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second.min(59))
            - offset_minutes * 60;
    let ms = i128::from(seconds) * 1000 + i128::from(millis);
    u64::try_from(ms).ok()
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

/// Provider timestamps documented as Unix seconds (Codex `resetsAt`, Claude
/// status-line `resets_at`). Negative, non-finite or absurd values are unknown.
pub fn unix_seconds_to_ms(seconds: f64) -> Option<u64> {
    if !seconds.is_finite() || seconds < 0.0 || seconds > 1.0e11 {
        return None;
    }
    Some((seconds * 1000.0).round() as u64)
}
