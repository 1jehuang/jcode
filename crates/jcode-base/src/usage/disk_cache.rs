//! Cross-process Anthropic OAuth usage cache.
//!
//! `jcode usage --json` runs as a fresh process on every poll (Jcode Desktop
//! polls once a minute), so the in-memory cache alone never absorbs repeat
//! requests and Anthropic's usage endpoint answers 429. Persisting the last
//! response and any error backoff lets every process share one fetch cadence,
//! and keeps the last good limits visible while the endpoint is throttled.

use super::{CACHE_DURATION, ERROR_BACKOFF, ModelScopedUsageWindow, RATE_LIMIT_BACKOFF, UsageData};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Last good limits stay useful while the endpoint is throttled. Rolled-over
/// windows are zeroed by `display_snapshot`, so older data is never shown as
/// current usage past its reset time.
const LAST_GOOD_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct Snapshot {
    fetched_unix_ms: i64,
    five_hour: f32,
    five_hour_resets_at: Option<String>,
    seven_day: f32,
    seven_day_resets_at: Option<String>,
    seven_day_opus: Option<f32>,
    #[serde(default)]
    model_scoped: Vec<(String, f32, Option<String>)>,
    extra_usage_enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct Entry {
    last_good: Option<Snapshot>,
    /// Latest failure and when it happened, for cross-process backoff.
    error: Option<String>,
    error_unix_ms: Option<i64>,
}

fn path() -> Option<PathBuf> {
    Some(
        crate::storage::jcode_dir()
            .ok()?
            .join("anthropic_usage_cache.json"),
    )
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn age(unix_ms: i64) -> Duration {
    Duration::from_millis(now_ms().saturating_sub(unix_ms).max(0) as u64)
}

fn instant_for(unix_ms: i64) -> Instant {
    let now = Instant::now();
    now.checked_sub(age(unix_ms)).unwrap_or(now)
}

fn load() -> HashMap<String, Entry> {
    path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn update(key: &str, change: impl FnOnce(&mut Entry)) {
    let Some(path) = path() else { return };
    let mut map = load();
    change(map.entry(key.to_string()).or_default());
    // Drop logins whose only data is long expired.
    map.retain(|_, entry| {
        entry
            .last_good
            .as_ref()
            .is_some_and(|good| age(good.fetched_unix_ms) < LAST_GOOD_MAX_AGE)
            || entry
                .error_unix_ms
                .is_some_and(|at| age(at) < RATE_LIMIT_BACKOFF)
    });
    let _ = crate::storage::write_json_secret(&path, &map);
}

fn is_rate_limit(error: &str) -> bool {
    error.contains("429") || error.contains("rate limit") || error.contains("Rate limited")
}

fn to_usage(snapshot: &Snapshot, last_error: Option<String>) -> UsageData {
    UsageData {
        five_hour: snapshot.five_hour,
        five_hour_resets_at: snapshot.five_hour_resets_at.clone(),
        seven_day: snapshot.seven_day,
        seven_day_resets_at: snapshot.seven_day_resets_at.clone(),
        seven_day_opus: snapshot.seven_day_opus,
        model_scoped: snapshot
            .model_scoped
            .iter()
            .map(
                |(model_name, utilization, resets_at)| ModelScopedUsageWindow {
                    model_name: model_name.clone(),
                    utilization: *utilization,
                    resets_at: resets_at.clone(),
                },
            )
            .collect(),
        extra_usage_enabled: snapshot.extra_usage_enabled,
        fetched_at: Some(instant_for(snapshot.fetched_unix_ms)),
        last_error,
    }
}

/// Data another process fetched recently enough to reuse without a request.
/// A recent failure returns the last good limits when there are any, else
/// the error itself, so throttled logins keep backing off across processes.
pub(super) fn fresh(key: &str) -> Option<UsageData> {
    let entry = load().remove(key)?;
    let good = entry
        .last_good
        .as_ref()
        .filter(|good| age(good.fetched_unix_ms) < LAST_GOOD_MAX_AGE);
    if let (Some(error), Some(at)) = (&entry.error, entry.error_unix_ms) {
        let backoff = if is_rate_limit(error) {
            RATE_LIMIT_BACKOFF
        } else {
            ERROR_BACKOFF
        };
        if age(at) < backoff && good.is_none_or(|good| good.fetched_unix_ms < at) {
            return Some(match good {
                // Report stale-but-real limits and note why they are stale.
                Some(good) => {
                    let mut usage = to_usage(good, None);
                    usage.fetched_at = Some(instant_for(at));
                    usage
                }
                None => UsageData {
                    fetched_at: Some(instant_for(at)),
                    last_error: Some(error.clone()),
                    ..Default::default()
                },
            });
        }
    }
    let good = good?;
    let usage = to_usage(good, None);
    (age(good.fetched_unix_ms) < CACHE_DURATION && !usage.is_stale()).then_some(usage)
}

/// Last good limits for a failed request, if any are recent enough to show.
pub(super) fn last_good(key: &str) -> Option<UsageData> {
    let good = load().remove(key)?.last_good?;
    (age(good.fetched_unix_ms) < LAST_GOOD_MAX_AGE).then(|| to_usage(&good, None))
}

pub(super) fn store_success(key: &str, data: &UsageData) {
    let snapshot = Snapshot {
        fetched_unix_ms: now_ms(),
        five_hour: data.five_hour,
        five_hour_resets_at: data.five_hour_resets_at.clone(),
        seven_day: data.seven_day,
        seven_day_resets_at: data.seven_day_resets_at.clone(),
        seven_day_opus: data.seven_day_opus,
        model_scoped: data
            .model_scoped
            .iter()
            .map(|w| (w.model_name.clone(), w.utilization, w.resets_at.clone()))
            .collect(),
        extra_usage_enabled: data.extra_usage_enabled,
    };
    update(key, |entry| {
        entry.last_good = Some(snapshot);
        entry.error = None;
        entry.error_unix_ms = None;
    });
}

pub(super) fn store_error(key: &str, error: &str) {
    update(key, |entry| {
        entry.error = Some(error.to_string());
        entry.error_unix_ms = Some(now_ms());
    });
}

/// Move persisted data from `from` to `to` (a refreshed token of the same
/// login), unless `to` already has its own entry.
pub(super) fn rename(from: &str, to: &str) {
    let Some(path) = path() else { return };
    let mut map = load();
    if map.contains_key(to) {
        return;
    }
    let Some(entry) = map.remove(from) else {
        return;
    };
    map.insert(to.to_string(), entry);
    let _ = crate::storage::write_json_secret(&path, &map);
}

/// Forget persisted data for a login after a limit reset.
pub(super) fn invalidate(matches: impl Fn(&str) -> bool) {
    let Some(path) = path() else { return };
    let mut map = load();
    let before = map.len();
    map.retain(|key, _| !matches(key));
    if map.len() != before {
        let _ = crate::storage::write_json_secret(&path, &map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_home<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::storage::lock_test_env();
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", dir.path());
        let result = f();
        match previous {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        result
    }

    fn sample() -> UsageData {
        UsageData {
            five_hour: 0.4,
            seven_day: 0.2,
            five_hour_resets_at: Some("2999-01-01T00:00:00Z".into()),
            ..Default::default()
        }
    }

    #[test]
    fn success_is_shared_and_rate_limit_keeps_last_good_limits() {
        with_home(|| {
            assert!(fresh("label:a").is_none());
            store_success("label:a", &sample());
            let shared = fresh("label:a").unwrap();
            assert_eq!(shared.five_hour, 0.4);
            assert!(shared.last_error.is_none());

            store_error("label:a", "Usage API error (429 Too Many Requests)");
            let throttled = fresh("label:a").expect("backoff is shared");
            assert_eq!(throttled.five_hour, 0.4);
            assert!(throttled.last_error.is_none());
            assert_eq!(last_good("label:a").unwrap().seven_day, 0.2);
        });
    }

    #[test]
    fn errors_without_data_back_off_and_invalidate_clears() {
        with_home(|| {
            store_error("label:b", "Usage API error (429 Too Many Requests)");
            let throttled = fresh("label:b").unwrap();
            assert!(throttled.last_error.unwrap().contains("429"));
            invalidate(|key| key == "label:b");
            assert!(fresh("label:b").is_none());
        });
    }
}
