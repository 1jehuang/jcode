// ---------------------------------------------------------------------------//
// Session Cost Tracker — Ported from Claude Code's cost-tracker.ts         //
// ---------------------------------------------------------------------------//!
//! Persistent cost tracking across session resumes. When a user resumes a
//! session, costs are restored from disk so the total reflects all work
//! done in that session (not just since the last launch).
//!
//! # Architecture (from Claude Code)
//!
//! ```text
//! Session starts
//!     |
//! restoreCostStateForSession()  <--- reads from project config
//!     |
//! Agent runs, costs accumulate
//!     |
//! saveCurrentSessionCosts()    ---> writes to project config
//!     |
//! (crash / restart / resume)
//!     |
//! restoreCostStateForSession()  <--- restores full history
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Per-model usage breakdown matching Claude Code's ModelUsage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
}

/// Complete session cost state — persisted to disk on every significant update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCostState {
    /// Total cost in USD (across all models)
    pub total_cost_usd: f64,
    /// Total wall-clock duration in milliseconds
    pub total_duration_ms: u64,
    /// Total API latency in milliseconds
    pub total_api_duration_ms: u64,
    /// Total tool execution time in milliseconds
    pub total_tool_duration_ms: u64,
    /// Lines of code added
    pub lines_added: u64,
    /// Lines of code removed
    pub lines_removed: u64,
    /// Per-model breakdown
    #[serde(default)]
    pub model_usage: std::collections::HashMap<String, ModelUsage>,
    /// Session ID for validation on restore
    pub session_id: String,
    /// Last updated timestamp
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Number of auto-continue loops completed
    #[serde(default)]
    pub auto_continue_count: u32,
    /// Number of compactions performed
    #[serde(default)]
    pub compaction_count: u32,
}

impl Default for SessionCostState {
    fn default() -> Self {
        Self {
            total_cost_usd: 0.0,
            total_duration_ms: 0,
            total_api_duration_ms: 0,
            total_tool_duration_ms: 0,
            lines_added: 0,
            lines_removed: 0,
            model_usage: std::collections::HashMap::new(),
            session_id: String::new(),
            updated_at: chrono::Utc::now(),
            auto_continue_count: 0,
            compaction_count: 0,
        }
    }
}

impl SessionCostState {
    /// Create a new empty state for a given session.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            ..Default::default()
        }
    }

    /// Record API response cost and token usage for a model call.
    pub fn record_model_call(
        &mut self,
        model: &str,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
        cache_read: u64,
        cache_creation: u64,
    ) {
        self.total_cost_usd += cost_usd;
        let entry = self.model_usage.entry(model.to_string()).or_default();
        entry.input_tokens += input_tokens;
        entry.output_tokens += output_tokens;
        entry.cache_read_input_tokens += cache_read;
        entry.cache_creation_input_tokens += cache_creation;
        entry.cost_usd += cost_usd;
    }

    /// Record tool execution time.
    pub fn record_tool_duration(&mut self, duration_ms: u64) {
        self.total_tool_duration_ms += duration_ms;
    }

    /// Record code changes (lines added/removed).
    pub fn record_code_changes(&mut self, added: i64, removed: i64) {
        if added > 0 {
            self.lines_added += added as u64;
        }
        if removed > 0 {
            self.lines_removed += removed as u64;
        }
    }

    /// Record a compaction event.
    pub fn record_compaction(&mut self) {
        self.compaction_count += 1;
    }

    /// Get total tokens across all models.
    pub fn total_input_tokens(&self) -> u64 {
        self.model_usage.values().map(|m| m.input_tokens).sum()
    }

    pub fn total_output_tokens(&self) -> u64 {
        self.model_usage.values().map(|m| m.output_tokens).sum()
    }

    pub fn total_cache_read_tokens(&self) -> u64 {
        self.model_usage
            .values()
            .map(|m| m.cache_read_input_tokens)
            .sum()
    }

    /// Format a human-readable cost summary (matching Claude Code's formatTotalCost).
    pub fn format_summary(&self) -> String {
        let cost_str = if self.total_cost_usd > 0.5 {
            format!("${:.2}", self.total_cost_usd)
        } else {
            format!("${:.4}", self.total_cost_usd)
        };

        let mut lines = vec![
            format!("Total cost:            {}", cost_str),
            format!(
                "Total duration (API):  {}",
                format_duration(self.total_api_duration_ms)
            ),
            format!(
                "Total code changes:    {} line(s) added, {} line(s) removed",
                self.lines_added, self.lines_removed
            ),
        ];

        // Per-model breakdown (if multiple models used)
        if self.model_usage.len() > 1 || !self.model_usage.is_empty() {
            lines.push("Usage by model:".to_string());
            for (model, usage) in &self.model_usage {
                let model_cost = if usage.cost_usd > 0.5 {
                    format!("${:.2}", usage.cost_usd)
                } else {
                    format!("${:.4}", usage.cost_usd)
                };
                lines.push(format!(
                    "  {:20} {} in, {} out, {} cache read, {} cache write ({})",
                    model,
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache_read_input_tokens,
                    usage.cache_creation_input_tokens,
                    model_cost
                ));
            }
        }

        lines.join("\n")
    }
}

/// Thread-safe, persistent session cost tracker.
pub struct SessionCostTracker {
    state: Arc<Mutex<SessionCostState>>,
    storage_path: PathBuf,
}

impl SessionCostTracker {
    /// Create a new tracker that persists to the given path.
    pub fn new(storage_path: PathBuf) -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionCostState::default())),
            storage_path,
        }
    }

    /// Restore state from disk for a specific session ID.
    /// Returns `true` if state was successfully restored and session IDs match.
    ///
    /// This is the key feature ported from Claude Code's
    /// `restoreCostStateForSession()` — it allows resuming a session without
    /// losing the accumulated cost history.
    pub fn restore_for_session(&self, session_id: &str) -> bool {
        match std::fs::read_to_string(&self.storage_path) {
            Ok(content) => match serde_json::from_str::<SessionCostState>(&content) {
                Ok(stored) if stored.session_id == session_id => {
                    if let Ok(mut guard) = self.state.lock() {
                        *guard = stored;
                        guard.updated_at = chrono::Utc::now();
                        true
                    } else {
                        false
                    }
                }
                _ => false, // No data or session ID mismatch
            },
            Err(_) => false, // File doesn't exist yet
        }
    }

    /// Save current state to disk. Call before session switch or shutdown.
    ///
    /// Ported from Claude Code's `saveCurrentSessionCosts()`.
    pub fn save(&self) -> std::io::Result<()> {
        let state = self.state.lock().map(|s| s.clone());
        match state {
            Ok(s) => {
                let dir = self.storage_path.parent().unwrap_or(Path::new("."));
                std::fs::create_dir_all(dir)?;
                let content = serde_json::to_string_pretty(&s)?;
                std::fs::write(&self.storage_path, content)?;
                Ok(())
            }
            Err(_) => Err(std::io::Error::other(
                "Failed to lock cost state",
            )),
        }
    }

    /// Get a snapshot of current state (for display/logging).
    pub fn snapshot(&self) -> SessionCostState {
        self.state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Record a model API call (thread-safe).
    pub fn record_call(
        &self,
        model: &str,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
        cache_read: u64,
        cache_creation: u64,
    ) {
        if let Ok(mut s) = self.state.lock() {
            s.record_model_call(model, cost_usd, input_tokens, output_tokens, cache_read, cache_creation);
            s.updated_at = chrono::Utc::now();
        }
    }

    /// Reset to a fresh state for a new session.
    pub fn reset(&self, session_id: impl Into<String>) {
        if let Ok(mut s) = self.state.lock() {
            *s = SessionCostState::new(session_id);
        }
    }
}

/// Format milliseconds into human-readable duration string.
fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let secs = ms / 1000;
        let mins = secs / 60;
        let remaining_secs = secs % 60;
        format!("{}m {}s", mins, remaining_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_recording() {
        let state = SessionCostState::new("test-session");
        // Can't record on non-mut directly, test through tracker
        let tmp_dir = tempfile::tempdir().unwrap();
        let path = tmp_dir.path().join("cost_state.json");
        let tracker = SessionCostTracker::new(path);

        tracker.record_call("claude-sonnet-4", 0.01, 1000, 500, 200, 50);
        tracker.record_call("claude-sonnet-4", 0.02, 2000, 800, 400, 100);
        tracker.record_code_changes(10, 5);

        let snap = tracker.snapshot();
        assert!((snap.total_cost_usd - 0.03).abs() < 0.001);
        assert_eq!(snap.lines_added, 10);
        assert_eq!(snap.lines_removed, 5);
        assert_eq!(snap.total_input_tokens(), 3000);
    }

    #[test]
    fn test_persistence_roundtrip() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let path = tmp_dir.path().join("cost_state.json");

        // Write initial state
        let tracker1 = SessionCostTracker::new(path.clone());
        tracker1.reset("session-alpha");
        tracker1.record_call("model-a", 0.05, 1000, 500, 100, 50);
        tracker1.save().unwrap();

        // Restore in new instance
        let tracker2 = SessionCostTracker::new(path.clone());
        assert!(tracker2.restore_for_session("session-alpha"));
        let snap = tracker2.snapshot();
        assert!((snap.total_cost_usd - 0.05).abs() < 0.001);
        assert_eq!(snap.session_id, "session-alpha");

        // Wrong session ID should not restore
        let tracker3 = SessionCostTracker::new(path.clone());
        assert!(!tracker3.restore_for_session("wrong-session"));
    }

    #[test]
    fn test_format_summary() {
        let state = SessionCostState::new("test");
        // Use internal fields for testing
        let mut state = state;
        state.total_cost_usd = 1.234;
        state.lines_added = 42;
        state.lines_removed = 7;
        state.record_model_call("sonnet", 1.234, 50000, 25000, 10000, 5000);

        let summary = state.format_summary();
        assert!(summary.contains("$1.23"));
        assert!(summary.contains("42 line(s) added"));
        assert!(summary.contains("Usage by model"));
    }
}
