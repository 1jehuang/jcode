//! Core Session Types
//!
//! Defines the fundamental data structures for session management.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Session export format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExport {
    pub session_id: String,
    pub version: String,
    pub exported_at: DateTime<Utc>,
    pub messages: Vec<ExportedMessage>,
    pub metadata: HashMap<String, String>,
}

/// Exported message format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub metadata: Option<HashMap<String, String>>,
}

/// Session import request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionImport {
    pub session_id: Option<String>, // Optional: generate new if None
    pub messages: Vec<ExportedMessage>,
    pub metadata: HashMap<String, String>,
}

/// Import result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub session_id: String,
    pub messages_imported: usize,
    pub warnings: Vec<String>,
}

/// Session cost tracker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCostTracker {
    pub session_id: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub api_calls: u64,
    pub start_time: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
}

impl SessionCostTracker {
    pub fn new(session_id: &str) -> Self {
        let now = Utc::now();
        Self {
            session_id: session_id.to_string(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
            api_calls: 0,
            start_time: now,
            last_updated: now,
        }
    }

    pub fn record_api_call(&mut self, input_tokens: u64, output_tokens: u64, cost_usd: f64) {
        self.total_input_tokens += input_tokens;
        self.total_output_tokens += output_tokens;
        self.total_cost_usd += cost_usd;
        self.api_calls += 1;
        self.last_updated = Utc::now();
    }

    pub fn get_total_tokens(&self) -> u64 {
        self.total_input_tokens + self.total_output_tokens
    }

    pub fn get_summary(&self) -> CostSummary {
        CostSummary {
            session_id: self.session_id.clone(),
            total_tokens: self.get_total_tokens(),
            total_cost_usd: self.total_cost_usd,
            api_calls: self.api_calls,
            duration_hours: (Utc::now() - self.start_time).num_seconds() as f64 / 3600.0,
        }
    }
}

/// Cost summary for reporting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub session_id: String,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub api_calls: u64,
    pub duration_hours: f64,
}

/// Session garbage collection config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcConfig {
    pub max_age_days: u64,
    pub max_sessions: usize,
    pub min_keep_sessions: usize,
    pub dry_run: bool,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            max_age_days: 30,
            max_sessions: 1000,
            min_keep_sessions: 10,
            dry_run: false,
        }
    }
}

/// GC result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcResult {
    pub sessions_scanned: usize,
    pub sessions_deleted: usize,
    pub space_freed_bytes: u64,
    pub deleted_session_ids: Vec<String>,
}

/// Runtime session manager state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeState {
    pub active_sessions: Vec<String>,
    pub total_memory_mb: f64,
    pub uptime_seconds: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_tracker() {
        let mut tracker = SessionCostTracker::new("test-session");
        
        tracker.record_api_call(100, 200, 0.001);
        tracker.record_api_call(150, 250, 0.0015);
        
        assert_eq!(tracker.total_input_tokens, 250);
        assert_eq!(tracker.total_output_tokens, 450);
        assert_eq!(tracker.get_total_tokens(), 700);
        assert_eq!(tracker.api_calls, 2);
    }

    #[test]
    fn test_cost_summary() {
        let tracker = SessionCostTracker::new("test-session");
        let summary = tracker.get_summary();
        
        assert_eq!(summary.session_id, "test-session");
        assert_eq!(summary.total_tokens, 0);
        assert_eq!(summary.api_calls, 0);
    }
}
