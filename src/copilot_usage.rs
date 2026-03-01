//! Local Copilot usage tracking
//!
//! Tracks request counts and token usage locally since GitHub Copilot
//! doesn't expose a usage API. Data persists to ~/.jcode/copilot_usage.json.

use chrono::{Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

static TRACKER: Mutex<Option<CopilotUsageTracker>> = Mutex::new(None);

fn usage_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".jcode")
        .join("copilot_usage.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CopilotUsageTracker {
    pub today: DayUsage,
    pub month: MonthUsage,
    pub all_time: AllTimeUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DayUsage {
    pub date: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MonthUsage {
    pub month: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AllTimeUsage {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl CopilotUsageTracker {
    fn load() -> Self {
        let path = usage_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    fn save(&self) {
        let path = usage_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    fn roll_if_needed(&mut self) {
        let now = Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        let month = format!("{}-{:02}", now.year(), now.month());

        if self.today.date != today {
            self.today = DayUsage {
                date: today,
                ..Default::default()
            };
        }
        if self.month.month != month {
            self.month = MonthUsage {
                month,
                ..Default::default()
            };
        }
    }

    fn record(&mut self, input_tokens: u64, output_tokens: u64) {
        self.roll_if_needed();

        self.today.requests += 1;
        self.today.input_tokens += input_tokens;
        self.today.output_tokens += output_tokens;

        self.month.requests += 1;
        self.month.input_tokens += input_tokens;
        self.month.output_tokens += output_tokens;

        self.all_time.requests += 1;
        self.all_time.input_tokens += input_tokens;
        self.all_time.output_tokens += output_tokens;

        self.save();
    }
}

/// Record a completed Copilot request.
pub fn record_request(input_tokens: u64, output_tokens: u64) {
    let mut guard = TRACKER.lock().unwrap();
    let tracker = guard.get_or_insert_with(CopilotUsageTracker::load);
    tracker.record(input_tokens, output_tokens);
}

/// Get current usage snapshot.
pub fn get_usage() -> CopilotUsageTracker {
    let mut guard = TRACKER.lock().unwrap();
    let tracker = guard.get_or_insert_with(CopilotUsageTracker::load);
    tracker.roll_if_needed();
    tracker.clone()
}
