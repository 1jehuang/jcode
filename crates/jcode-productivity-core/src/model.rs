//! Core data model for the productivity report.
//!
//! `SessionSummary` is the compact, cache-friendly per-session aggregate that we
//! persist between runs so re-scanning ~100k transcripts stays fast. The global
//! [`ProductivityReport`] is computed by folding all summaries together.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Compact per-session aggregate. Designed to be cheap to (de)serialize so we
/// can cache one of these per transcript file and only re-parse changed files.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionSummary {
    /// First and last activity timestamps (RFC3339, UTC), if known.
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    /// Session created/updated timestamps from the file header.
    pub created_at: Option<String>,
    pub updated_at: Option<String>,

    /// Project identity (basename of working_dir) and full path.
    pub project: Option<String>,
    pub working_dir: Option<String>,
    pub provider_key: Option<String>,
    pub model: Option<String>,

    /// Message counts.
    pub user_msgs: u32,
    pub assistant_msgs: u32,

    /// Characters the human typed (sum of user text blocks). Rough "words" proxy.
    pub user_chars: u64,
    /// Characters the assistant produced in text blocks.
    pub assistant_chars: u64,

    /// Tool invocation histogram. Batch inner calls are expanded into this map
    /// so individual tools (read/edit/bash/...) get credit.
    pub tools: BTreeMap<String, u32>,
    /// Images attached/produced in the transcript.
    pub images: u32,

    /// Token usage rollups.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,

    /// Local-time activity histograms (hour-of-day, weekday Mon=0).
    pub hour_hist: [u32; 24],
    pub weekday_hist: [u32; 7],

    /// Calendar dates (local, YYYY-MM-DD) on which this session was active.
    pub active_dates: Vec<String>,
}

impl SessionSummary {
    pub fn total_tool_calls(&self) -> u64 {
        self.tools.values().map(|v| *v as u64).sum()
    }
}

/// A single named, sortable metric used for "top N" lists in the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tally {
    pub name: String,
    pub count: u64,
}

/// The fully-computed report. Serializable so callers can render markdown, a PNG
/// dashboard, or future JSON exports from the same source of truth.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProductivityReport {
    /// Window the report covers ("all time" today).
    pub generated_at: String,

    // Volume ---------------------------------------------------------------
    pub total_sessions: u64,
    pub total_messages: u64,
    pub user_prompts: u64,
    pub assistant_messages: u64,
    pub total_tool_calls: u64,
    pub total_images: u64,

    // Tokens ---------------------------------------------------------------
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,

    // Human effort ---------------------------------------------------------
    pub user_chars: u64,
    pub user_words: u64,
    pub assistant_chars: u64,

    // Time -----------------------------------------------------------------
    pub first_day: Option<String>,
    pub last_day: Option<String>,
    pub active_days: u64,
    pub span_days: u64,
    pub current_streak: u64,
    pub longest_streak: u64,
    pub busiest_day: Option<Tally>,
    pub hour_hist: [u32; 24],
    pub weekday_hist: [u32; 7],
    pub peak_hour: u8,
    pub chronotype: String,

    // Breakdowns -----------------------------------------------------------
    pub top_projects: Vec<Tally>,
    pub distinct_projects: u64,
    pub top_tools: Vec<Tally>,
    pub top_models: Vec<Tally>,
    pub top_providers: Vec<Tally>,

    // Derived activity buckets --------------------------------------------
    pub code_edits: u64,
    pub commands_run: u64,
    pub searches: u64,
    pub web_actions: u64,
    pub longest_session_msgs: u64,
    pub avg_session_msgs: f64,

    // Shareable flavor -----------------------------------------------------
    pub archetype: String,
    pub archetype_blurb: String,
    pub power_score: u64,
    pub badges: Vec<String>,

    // Meta -----------------------------------------------------------------
    pub scanned_files: u64,
    pub parse_errors: u64,
    pub scan_secs: f64,
    pub cache_hits: u64,
}
