//! Local Copilot usage tracking
//!
//! Tracks request counts and token usage locally since GitHub Copilot
//! doesn't expose a usage API. Data persists to ~/.jcode/copilot_usage.json.

use chrono::{Datelike, Utc};
use std::path::PathBuf;
use std::sync::Mutex;

static TRACKER: Mutex<Option<CopilotUsageTracker>> = Mutex::new(None);

fn usage_path() -> PathBuf {
    crate::storage::jcode_dir()
        .unwrap_or_else(|_| PathBuf::from(".").join(".jcode"))
        .join("copilot_usage.json")
}

pub use jcode_usage_types::{AllTimeUsage, CopilotUsageTracker, DayUsage, MonthUsage};

fn roll_if_needed(tracker: &mut CopilotUsageTracker) {
    let now = Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    let month = format!("{}-{:02}", now.year(), now.month());

    if tracker.today.date != today {
        tracker.today = DayUsage {
            date: today,
            ..Default::default()
        };
    }
    if tracker.month.month != month {
        tracker.month = MonthUsage {
            month,
            ..Default::default()
        };
    }
}

fn record_usage(
    tracker: &mut CopilotUsageTracker,
    input_tokens: u64,
    output_tokens: u64,
    is_premium: bool,
) {
    roll_if_needed(tracker);

    tracker.today.requests += 1;
    tracker.today.input_tokens += input_tokens;
    tracker.today.output_tokens += output_tokens;
    if is_premium {
        tracker.today.premium_requests += 1;
    }

    tracker.month.requests += 1;
    tracker.month.input_tokens += input_tokens;
    tracker.month.output_tokens += output_tokens;
    if is_premium {
        tracker.month.premium_requests += 1;
    }

    tracker.all_time.requests += 1;
    tracker.all_time.input_tokens += input_tokens;
    tracker.all_time.output_tokens += output_tokens;
    if is_premium {
        tracker.all_time.premium_requests += 1;
    }

    save_tracker(tracker);
}
fn load_tracker() -> CopilotUsageTracker {
    let path = usage_path();
    crate::storage::read_json(&path).unwrap_or_default()
}

fn save_tracker(tracker: &CopilotUsageTracker) {
    let path = usage_path();
    let _ = crate::storage::write_json(&path, tracker);
}

/// Record a completed Copilot request.
pub fn record_request(input_tokens: u64, output_tokens: u64, is_premium: bool) {
    let mut guard = match TRACKER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let tracker = guard.get_or_insert_with(load_tracker);
    record_usage(tracker, input_tokens, output_tokens, is_premium);
}

/// Get current usage snapshot.
pub fn get_usage() -> CopilotUsageTracker {
    let mut guard = match TRACKER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let tracker = guard.get_or_insert_with(load_tracker);
    roll_if_needed(tracker);
    tracker.clone()
}

#[cfg(test)]
mod tests {
    use super::{
        AllTimeUsage, CopilotUsageTracker, DayUsage, MonthUsage, TRACKER, load_tracker,
        save_tracker, usage_path,
    };
    use std::ffi::OsString;

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::storage::lock_test_env()
    }

    struct EnvVarGuard {
        key: &'static str,
        prev: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prev = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.prev {
                crate::env::set_var(self.key, prev);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    fn clear_tracker() {
        if let Ok(mut tracker) = TRACKER.lock() {
            *tracker = None;
        }
    }

    #[test]
    fn usage_path_respects_jcode_home() {
        let _env_lock = lock_env();
        clear_tracker();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("JCODE_HOME", temp.path().as_os_str());

        assert_eq!(usage_path(), temp.path().join("copilot_usage.json"));
    }

    #[test]
    fn save_and_load_roundtrip_under_jcode_home() {
        let _env_lock = lock_env();
        clear_tracker();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("JCODE_HOME", temp.path().as_os_str());

        let tracker = CopilotUsageTracker {
            today: DayUsage {
                date: "2026-03-06".to_string(),
                requests: 2,
                premium_requests: 1,
                input_tokens: 100,
                output_tokens: 50,
            },
            month: MonthUsage {
                month: "2026-03".to_string(),
                requests: 2,
                premium_requests: 1,
                input_tokens: 100,
                output_tokens: 50,
            },
            all_time: AllTimeUsage {
                requests: 2,
                premium_requests: 1,
                input_tokens: 100,
                output_tokens: 50,
            },
        };

        save_tracker(&tracker);
        let loaded = load_tracker();

        assert_eq!(loaded.today.date, "2026-03-06");
        assert_eq!(loaded.today.requests, 2);
        assert_eq!(loaded.all_time.output_tokens, 50);
    }
}
