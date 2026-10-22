//! Cross-process Anthropic OAuth usage cache.
//!
//! `jcode usage --json` runs as a fresh process on every poll (Jcode Desktop
//! polls once a minute), so the in-memory cache alone never absorbs repeat
//! requests and Anthropic's usage endpoint answers 429. Persisting the last
//! response and any error backoff lets every process share one fetch cadence,
//! and keeps the last good limits visible while the endpoint is throttled.

use super::{
    ANTHROPIC_CACHE_DURATION, ERROR_BACKOFF, ModelScopedUsageWindow, RATE_LIMIT_BACKOFF, UsageData,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
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
    /// Learned per-account polling interval survives successful retries.
    #[serde(default)]
    adaptive_interval_secs: u64,
    #[serde(default)]
    retry_not_before_ms: Option<i64>,
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

fn open_lock(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// The lock inode is separate from the JSON file, which atomic writes replace.
/// Never delete lock files: a second inode would permit concurrent owners.
fn map_lock(path: &Path) -> std::io::Result<File> {
    let lock = open_lock(&path.with_extension("lock"))?;
    lock.lock()?;
    Ok(lock)
}

/// Hold across the cache recheck and HTTP request. Closing the file releases the
/// lock on cancellation or process exit. Contention never blocks a Tokio worker.
pub(super) async fn lock_probe(key: &str) -> anyhow::Result<File> {
    let root = crate::storage::jcode_dir()?;
    let name = hex::encode(Sha256::digest(key.as_bytes()));
    let lock = open_lock(
        &root
            .join("anthropic_usage_locks")
            .join(format!("{name}.lock")),
    )?;
    // A stale holder (crashed probe, leaked test lock) must not hang the
    // caller forever: after the probe budget expires, degrade to the
    // failure-cache path instead of spinning.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    anyhow::bail!("usage probe lock contention timed out");
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

fn update(key: &str, change: impl FnOnce(&mut Entry)) -> Entry {
    let Some(path) = path() else {
        let mut entry = Entry::default();
        change(&mut entry);
        return entry;
    };
    let Ok(_lock) = map_lock(&path) else {
        let mut entry = Entry::default();
        change(&mut entry);
        return entry;
    };
    let mut map = load();
    let entry = map.entry(key.to_string()).or_default();
    change(entry);
    let result = entry.clone();
    // Drop logins whose only data is long expired.
    map.retain(|_, entry| {
        entry.adaptive_interval_secs > 0
            || entry.retry_not_before_ms.is_some_and(|at| at > now_ms())
            || entry
                .last_good
                .as_ref()
                .is_some_and(|good| age(good.fetched_unix_ms) < LAST_GOOD_MAX_AGE)
            || entry
                .error_unix_ms
                .is_some_and(|at| age(at) < RATE_LIMIT_BACKOFF)
    });
    let _ = crate::storage::write_json_secret(&path, &map);
    result
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
        last_success_at: Some(instant_for(snapshot.fetched_unix_ms)),
        refresh_after: None,
        last_error,
    }
}

/// Data another process fetched recently enough to reuse without a request.
/// A recent failure returns the last good limits when there are any, else
/// the error itself, so throttled logins keep backing off across processes.
fn interval(entry: &Entry) -> Duration {
    Duration::from_secs(
        entry
            .adaptive_interval_secs
            .clamp(ANTHROPIC_CACHE_DURATION.as_secs(), 3600),
    )
}

fn deadline(entry: &Entry) -> Option<i64> {
    let success = entry.last_good.as_ref().map(|good| {
        good.fetched_unix_ms
            .saturating_add(interval(entry).as_millis() as i64)
    });
    let failure = entry
        .error
        .as_ref()
        .zip(entry.error_unix_ms)
        .map(|(error, at)| {
            let delay = if is_rate_limit(error) {
                interval(entry).max(RATE_LIMIT_BACKOFF)
            } else {
                ERROR_BACKOFF
            };
            at.saturating_add(delay.as_millis() as i64)
        });
    success
        .into_iter()
        .chain(failure)
        .chain(entry.retry_not_before_ms)
        .max()
}

fn entry_usage(entry: &Entry) -> UsageData {
    let mut usage =
        entry
            .last_good
            .as_ref()
            .filter(|good| age(good.fetched_unix_ms) < LAST_GOOD_MAX_AGE)
            .map(|good| to_usage(good, entry.error.clone()))
            .unwrap_or_else(|| UsageData {
                last_error: Some(entry.error.clone().unwrap_or_else(|| {
                    "Usage limits unavailable until next scheduled probe".into()
                })),
                ..Default::default()
            });
    if let Some(at) = entry.error_unix_ms {
        usage.fetched_at = Some(instant_for(at));
    }
    if let Some(at) = deadline(entry) {
        usage.refresh_after = Instant::now().checked_add(Duration::from_millis(
            at.saturating_sub(now_ms()).max(0) as u64,
        ));
    }
    usage
}

pub(super) fn fresh(key: &str) -> Option<UsageData> {
    let entry = load().remove(key)?;
    (deadline(&entry)? > now_ms()).then(|| entry_usage(&entry))
}

/// Last good limits for a failed request, if any are recent enough to show.
#[cfg(test)]
pub(super) fn last_good(key: &str) -> Option<UsageData> {
    let good = load().remove(key)?.last_good?;
    (age(good.fetched_unix_ms) < LAST_GOOD_MAX_AGE).then(|| to_usage(&good, None))
}

pub(super) fn store_success(key: &str, data: &UsageData) -> UsageData {
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
    let entry = update(key, |entry| {
        // Migrate an old rate-limit record before clearing its error.
        if entry.adaptive_interval_secs == 0 && entry.error.as_deref().is_some_and(is_rate_limit) {
            entry.adaptive_interval_secs = RATE_LIMIT_BACKOFF.as_secs() * 2;
        }
        entry.last_good = Some(snapshot);
        entry.error = None;
        entry.error_unix_ms = None;
        // Do not clear learned slowdown or a provider's outstanding deadline.
    });
    entry_usage(&entry)
}

/// Parse bounded delta-seconds or HTTP-date hints. Invalid values never shorten policy.
fn retry_after_ms(value: &str, now: i64) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return None;
    }
    if value.bytes().all(|b| b.is_ascii_digit()) {
        let seconds = value.parse::<i64>().ok()?;
        return now.checked_add(seconds.checked_mul(1000)?);
    }
    let date = chrono::DateTime::parse_from_rfc2822(value)
        .ok()?
        .timestamp_millis();
    (date >= now).then_some(date)
}

#[cfg(test)]
pub(super) fn store_error(key: &str, error: &str) {
    store_error_with_hint(key, error, None);
}

pub(super) fn store_error_with_hint(key: &str, error: &str, hint: Option<&str>) -> UsageData {
    let now = now_ms();
    let entry = update(key, |entry| {
        if is_rate_limit(error) {
            entry.adaptive_interval_secs = interval(entry).as_secs().saturating_mul(2).min(3600);
        }
        let delay = if is_rate_limit(error) {
            interval(entry)
        } else {
            ERROR_BACKOFF
        };
        let policy = now.saturating_add(delay.as_millis() as i64);
        let hinted = hint
            .and_then(|hint| retry_after_ms(hint, now))
            .map(|at| at.max(now.saturating_add(RATE_LIMIT_BACKOFF.as_millis() as i64)));
        entry.retry_not_before_ms = Some(
            entry
                .retry_not_before_ms
                .unwrap_or(0)
                .max(policy)
                .max(hinted.unwrap_or(0)),
        );
        entry.error = Some(error.to_string());
        entry.error_unix_ms = Some(now);
    });
    entry_usage(&entry)
}

/// Last good limits only, for shipping to another machine. Backoff and error
/// state stay local because they describe this machine's request history.
pub fn export_last_good() -> Vec<u8> {
    let map: HashMap<String, Entry> = load()
        .into_iter()
        .filter_map(|(key, entry)| {
            let good = entry
                .last_good
                .filter(|good| age(good.fetched_unix_ms) < LAST_GOOD_MAX_AGE)?;
            Some((
                key,
                Entry {
                    last_good: Some(good),
                    ..Default::default()
                },
            ))
        })
        .collect();
    serde_json::to_vec(&map).unwrap_or_default()
}

/// Adopt imported limits only where they are newer than what this machine has.
pub fn import_last_good(bytes: &[u8]) {
    let Ok(incoming) = serde_json::from_slice::<HashMap<String, Entry>>(bytes) else {
        return;
    };
    let Some(path) = path() else { return };
    let Ok(_lock) = map_lock(&path) else { return };
    let mut map = load();
    for (key, entry) in incoming {
        let Some(good) = entry.last_good else {
            continue;
        };
        let local = map.entry(key).or_default();
        if local
            .last_good
            .as_ref()
            .is_none_or(|have| have.fetched_unix_ms < good.fetched_unix_ms)
        {
            local.last_good = Some(good);
        }
    }
    let _ = crate::storage::write_json_secret(&path, &map);
}

/// Forget persisted data for a login after a limit reset.
pub(super) fn invalidate(matches: impl Fn(&str) -> bool) {
    let Some(path) = path() else { return };
    let Ok(_lock) = map_lock(&path) else { return };
    let mut map = load();
    for (key, entry) in &mut map {
        if matches(key) {
            // A quota reset invalidates ratios, not provider polling policy.
            entry.retry_not_before_ms = deadline(entry);
            entry.last_good = None;
            if entry.adaptive_interval_secs == 0
                && entry.error.as_deref().is_some_and(is_rate_limit)
            {
                entry.adaptive_interval_secs = RATE_LIMIT_BACKOFF.as_secs();
            }
        }
    }
    let _ = crate::storage::write_json_secret(&path, &map);
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKER_ENV: &str = "JCODE_USAGE_CACHE_TEST_WORKER";

    // Run this test in isolated copies of the real test executable. It never
    // reads a user's credentials or contacts a provider.
    #[test]
    fn subprocess_worker() {
        let Ok(mode) = std::env::var(WORKER_ENV) else {
            return;
        };
        let root = crate::storage::jcode_dir().unwrap();
        let started = Instant::now();
        while !root.join("start-workers").exists() {
            assert!(started.elapsed() < Duration::from_secs(15));
            std::thread::sleep(Duration::from_millis(5));
        }
        if mode == "writes" {
            let index = std::env::var("JCODE_USAGE_CACHE_TEST_INDEX").unwrap();
            for entry in 0..20 {
                store_success(&format!("label:worker-{index}-{entry}"), &sample());
            }
        } else {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let key = "label:subprocess-flight".to_string();
                let data = super::super::flight::fetch(key.clone(), || async {
                    use std::io::Write;
                    let mut counter = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(root.join("probes"))
                        .unwrap();
                    counter.write_all(b"probe\n").unwrap();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    super::super::cache_anthropic_usage_failure(
                        key,
                        "Usage API error (429 Too Many Requests)".into(),
                    )
                })
                .await
                .unwrap();
                assert_eq!(data.five_hour, 0.4);
                assert!(data.last_error.unwrap().contains("429"));
            });
        }
    }

    fn run_workers(mode: &str, count: usize) {
        let root = crate::storage::jcode_dir().unwrap();
        let mut workers = Vec::new();
        for index in 0..count {
            workers.push(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "usage::disk_cache::tests::subprocess_worker",
                        "--nocapture",
                    ])
                    .env(WORKER_ENV, mode)
                    .env("JCODE_USAGE_CACHE_TEST_INDEX", index.to_string())
                    .env("JCODE_HOME", &root)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
        }
        std::fs::write(root.join("start-workers"), b"ready").unwrap();
        for worker in workers {
            let output = worker.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "fixture worker failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn subprocess_updates_preserve_every_account() {
        with_home(|| {
            run_workers("writes", 6);
            let map = load();
            assert_eq!(map.len(), 120);
            for index in 0..6 {
                for entry in 0..20 {
                    assert!(map.contains_key(&format!("label:worker-{index}-{entry}")));
                }
            }
        });
    }

    #[test]
    fn subprocess_probes_coalesce_and_share_rate_limit_backoff() {
        with_home(|| {
            let mut map = HashMap::new();
            map.insert(
                "label:subprocess-flight".to_string(),
                Entry {
                    last_good: Some(Snapshot {
                        fetched_unix_ms: now_ms() - 1_000_000,
                        five_hour: 0.4,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            );
            crate::storage::write_json_secret(&path().unwrap(), &map).unwrap();
            run_workers("probe", 6);
            assert_eq!(
                std::fs::read_to_string(crate::storage::jcode_dir().unwrap().join("probes"))
                    .unwrap(),
                "probe\n"
            );
            let data = fresh("label:subprocess-flight").unwrap();
            assert_eq!(data.five_hour, 0.4);
            assert!(data.last_error.unwrap().contains("429"));
        });
    }

    #[test]
    fn equal_timestamp_failure_takes_precedence_over_success() {
        with_home(|| {
            let at = now_ms();
            let map = HashMap::from([(
                "label:equal".to_string(),
                Entry {
                    last_good: Some(Snapshot {
                        fetched_unix_ms: at,
                        ..Default::default()
                    }),
                    error: Some("Usage API error (429 Too Many Requests)".into()),
                    error_unix_ms: Some(at),
                    ..Default::default()
                },
            )]);
            crate::storage::write_json_secret(&path().unwrap(), &map).unwrap();
            assert!(
                fresh("label:equal")
                    .unwrap()
                    .last_error
                    .unwrap()
                    .contains("429")
            );
        });
    }

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
    fn anthropic_normal_polling_is_fifteen_minutes_and_openai_stays_five() {
        let at = Instant::now() - Duration::from_secs(600);
        let anthropic = UsageData {
            fetched_at: Some(at),
            ..Default::default()
        };
        let openai = super::super::OpenAIUsageData {
            fetched_at: Some(at),
            ..Default::default()
        };
        assert!(!anthropic.is_stale());
        assert!(openai.is_stale());
    }

    #[test]
    fn slowdown_survives_success_and_restart_with_matching_ram_deadline() {
        with_home(|| {
            let key = "label:adaptive";
            store_error(key, "429");
            assert_eq!(load()[key].adaptive_interval_secs, 1800);
            let ram = store_success(key, &sample());
            let disk = fresh(key).unwrap();
            assert!(ram.last_error.is_none());
            assert!(ram.last_success_at.is_some());
            let remaining = ram.refresh_after.unwrap().duration_since(Instant::now());
            assert!(remaining > Duration::from_secs(1798));
            assert!(remaining <= Duration::from_secs(1800));
            assert!(
                ram.refresh_after
                    .unwrap()
                    .max(disk.refresh_after.unwrap())
                    .duration_since(ram.refresh_after.unwrap().min(disk.refresh_after.unwrap()))
                    < Duration::from_millis(20)
            );
            store_error(key, "429");
            assert_eq!(load()[key].adaptive_interval_secs, 3600);
            store_success(key, &sample());
            store_error(key, "429");
            assert_eq!(load()[key].adaptive_interval_secs, 3600);
        });
    }

    #[test]
    fn retry_hints_respect_policy_floor_and_can_extend_beyond_adaptive_cap() {
        with_home(|| {
            for hint in ["0", "62", "garbage", "-1", "1.5", "18446744073709551615"] {
                let key = format!("label:hint-{hint}");
                let ram = store_error_with_hint(&key, "429", Some(hint));
                let left = ram.refresh_after.unwrap().duration_since(Instant::now());
                assert!(left > Duration::from_secs(1798), "hint {hint}");
                assert!(left <= Duration::from_secs(1800), "hint {hint}");
                let disk = fresh(&key).unwrap();
                assert!(
                    ram.refresh_after
                        .unwrap()
                        .max(disk.refresh_after.unwrap())
                        .duration_since(
                            ram.refresh_after.unwrap().min(disk.refresh_after.unwrap())
                        )
                        < Duration::from_millis(20)
                );
            }
            let now = now_ms();
            assert_eq!(retry_after_ms("62", now), Some(now + 62_000));
            let future = chrono::DateTime::from_timestamp_millis(now + 7_200_000).unwrap();
            let hint = future.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
            let date_ms = retry_after_ms(&hint, now).unwrap();
            assert!((7_199_000..=7_200_000).contains(&(date_ms - now)));
            let data = store_error_with_hint("label:date", "429", Some(&hint));
            assert!(
                data.refresh_after.unwrap().duration_since(Instant::now())
                    > Duration::from_secs(7198)
            );
            store_success("label:date", &sample());
            assert!(
                fresh("label:date")
                    .unwrap()
                    .refresh_after
                    .unwrap()
                    .duration_since(Instant::now())
                    > Duration::from_secs(7198)
            );
            for invalid in [
                "",
                "yesterday",
                "-62",
                "0.1",
                "999999999999999999999999",
                "Sun, 18 Oct 2020 00:15:00 GMT",
                "62\nInjected",
            ] {
                assert!(retry_after_ms(invalid, now).is_none(), "{invalid}");
            }
        });
    }

    #[test]
    fn reset_and_expired_windows_never_bypass_provider_deadline() {
        with_home(|| {
            let key = "label:reset";
            let mut data = sample();
            data.five_hour_resets_at = Some("2020-01-01T00:00:00Z".into());
            let ram = store_success(key, &data);
            assert!(!ram.is_stale());
            assert!(!fresh(key).unwrap().is_stale());
            let before = load()[key].last_good.as_ref().unwrap().fetched_unix_ms + 900_000;
            invalidate(|candidate| candidate == key);
            assert_eq!(load()[key].retry_not_before_ms, Some(before));
            let cleared = fresh(key).unwrap();
            assert!(!cleared.is_stale());
            assert!(cleared.last_error.is_some());
            assert!(cleared.last_success_at.is_none());
            assert_eq!(cleared.five_hour, 0.0);
        });
    }

    #[test]
    fn legacy_rate_limit_history_is_retained_after_success() {
        with_home(|| {
            let key = "label:legacy";
            let entry: Entry = serde_json::from_value(serde_json::json!({
                "last_good": null,
                "error": "429",
                "error_unix_ms": now_ms()
            }))
            .unwrap();
            crate::storage::write_json_secret(&path().unwrap(), &HashMap::from([(key, entry)]))
                .unwrap();
            assert!(!fresh(key).unwrap().is_stale());
            store_success(key, &sample());
            assert_eq!(load()[key].adaptive_interval_secs, 1800);
        });
    }

    #[test]
    fn old_good_provenance_is_not_resurrected_by_a_long_retry_hint() {
        with_home(|| {
            let key = "label:old-good";
            store_success(key, &sample());
            update(key, |entry| {
                entry.last_good.as_mut().unwrap().fetched_unix_ms -= 86_401_000
            });
            let data = store_error_with_hint(key, "429", Some("100000"));
            assert!(data.last_success_at.is_none());
            assert_eq!(data.five_hour, 0.0);
            assert!(data.last_error.is_some());
        });
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
            assert!(
                throttled
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.contains("429"))
            );
            // The backoff timestamp, rather than the old successful fetch,
            // is used for retry timing. The returned data must still signal
            // that the displayed limits were not freshly confirmed.
            assert!(throttled.fetched_at.unwrap().elapsed() < Duration::from_secs(1));
            assert_eq!(last_good("label:a").unwrap().seven_day, 0.2);
        });
    }

    #[test]
    fn errors_without_data_back_off_and_invalidate_preserves_cooldown() {
        with_home(|| {
            store_error("label:b", "Usage API error (429 Too Many Requests)");
            let throttled = fresh("label:b").unwrap();
            assert!(throttled.last_error.unwrap().contains("429"));
            invalidate(|key| key == "label:b");
            assert!(
                fresh("label:b")
                    .unwrap()
                    .last_error
                    .unwrap()
                    .contains("429")
            );
        });
    }

    #[test]
    fn expired_backoff_allows_one_new_probe_and_failure_keeps_limits_unknown() {
        with_home(|| {
            let key = "label:expired-backoff-fixture";
            let map = HashMap::from([(
                key.to_string(),
                Entry {
                    last_good: Some(Snapshot {
                        fetched_unix_ms: now_ms() - 1_000_000,
                        five_hour: 0.4,
                        ..Default::default()
                    }),
                    error: Some("Usage API error (429 Too Many Requests)".into()),
                    error_unix_ms: Some(now_ms() - 901_000),
                    ..Default::default()
                },
            )]);
            crate::storage::write_json_secret(&path().unwrap(), &map).unwrap();
            assert!(fresh(key).is_none());
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let data = super::super::flight::fetch(key.into(), || async {
                    super::super::cache_anthropic_usage_failure(
                        key.into(),
                        "Usage API error (429 Too Many Requests)".into(),
                    )
                })
                .await
                .unwrap();
                assert_eq!(data.five_hour, 0.4);
                let snapshot =
                    super::super::anthropic_snapshot_from_usage("fixture".into(), None, &data);
                assert!(snapshot.error.is_some());
                assert_eq!(snapshot.five_hour_ratio, None);
                assert!(!snapshot.exhausted);
                super::super::flight::fetch(key.into(), || async {
                    panic!("renewed backoff must be reused")
                })
                .await
                .unwrap();
            });
        });
    }
}
