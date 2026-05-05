//! Cross-session search tool - RAG across all past sessions
//!
//! The tool is optimized for agent recall rather than raw grep output:
//! - current session, system reminders, and tool-only messages are hidden by default
//! - session metadata is searchable and returned as first-class results
//! - snapshot + journal persistence is searched so recent messages are visible
//! - results are grouped by session by default to avoid duplicate floods

use super::{Tool, ToolContext, ToolOutput};
use crate::message::ContentBlock;
use crate::session::{Session, StoredMessage, session_journal_path_from_snapshot};
use crate::storage;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use jcode_session_types::{
    SessionSearchQueryProfile as QueryProfile,
    score_session_search_text_match as score_message_match,
    session_search_path_matches_query as path_matches_query,
    session_search_raw_matches_query as raw_matches_query,
    session_search_working_dir_matches as working_dir_matches,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Max session snapshots/journals to deserialize after raw pre-filtering.
const MAX_DESERIALIZE: usize = 500;

/// Number of parallel threads for file scanning/loading.
const SCAN_THREADS: usize = 8;

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 50;
const DEFAULT_MAX_PER_SESSION: usize = 1;
const MAX_MAX_PER_SESSION: usize = 20;
const DEFAULT_MAX_SCAN_SESSIONS: usize = 1000;
const MAX_MAX_SCAN_SESSIONS: usize = 10_000;
const MAX_CONTEXT_MESSAGES: usize = 5;

#[derive(Debug, Deserialize)]
struct SearchInput {
    query: String,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    /// Include the active session in results. Defaults to false because this tool
    /// is meant for recalling past sessions and otherwise tends to find itself.
    #[serde(default)]
    include_current: Option<bool>,
    /// Include raw tool calls/results. Defaults to false because they usually
    /// crowd out the conclusions the agent is trying to recall.
    #[serde(default)]
    include_tools: Option<bool>,
    /// Include system/display messages and system reminders. Defaults to false.
    #[serde(default)]
    include_system: Option<bool>,
    /// Maximum number of hits from a single session. Defaults to 1 for diversity.
    #[serde(default)]
    max_per_session: Option<i64>,
    /// Restrict matches to user, assistant, or metadata results.
    #[serde(default)]
    role: Option<String>,
    /// Restrict sessions by provider key/source label substring.
    #[serde(default)]
    provider: Option<String>,
    /// Restrict sessions by model substring.
    #[serde(default)]
    model: Option<String>,
    /// Restrict to sessions updated/messages at or after this RFC3339 timestamp or YYYY-MM-DD date.
    #[serde(default)]
    after: Option<String>,
    /// Restrict to sessions updated/messages at or before this RFC3339 timestamp or YYYY-MM-DD date.
    #[serde(default)]
    before: Option<String>,
    /// Restrict Jcode sessions by saved/bookmarked flag.
    #[serde(default)]
    saved: Option<bool>,
    /// Restrict Jcode sessions by debug flag.
    #[serde(default)]
    debug: Option<bool>,
    /// Restrict Jcode sessions by canary flag.
    #[serde(default)]
    canary: Option<bool>,
    /// Restrict source: jcode, claude, codex, pi, opencode, or all.
    #[serde(default)]
    source: Option<String>,
    /// Include external session sources discovered by the session picker. Defaults to true.
    #[serde(default)]
    include_external: Option<bool>,
    /// Number of preceding messages to include around each hit.
    #[serde(default)]
    context_before: Option<i64>,
    /// Number of following messages to include around each hit.
    #[serde(default)]
    context_after: Option<i64>,
    /// Bound the number of recent sessions scanned per source.
    #[serde(default)]
    max_scan_sessions: Option<i64>,
}

pub struct SessionSearchTool;

impl SessionSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SessionSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct SearchOptions {
    current_session_id: String,
    working_dir_filter: Option<String>,
    limit: usize,
    max_per_session: usize,
    include_current: bool,
    include_tools: bool,
    include_system: bool,
    include_external: bool,
    role_filter: Option<RoleFilter>,
    provider_filter: Option<String>,
    model_filter: Option<String>,
    source_filter: Option<String>,
    saved_filter: Option<bool>,
    debug_filter: Option<bool>,
    canary_filter: Option<bool>,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    context_before: usize,
    context_after: usize,
    max_scan_sessions: usize,
}

impl SearchOptions {
    #[cfg(test)]
    fn for_test(current_session_id: impl Into<String>) -> Self {
        Self {
            current_session_id: current_session_id.into(),
            working_dir_filter: None,
            limit: DEFAULT_LIMIT,
            max_per_session: DEFAULT_MAX_PER_SESSION,
            include_current: false,
            include_tools: false,
            include_system: false,
            include_external: true,
            role_filter: None,
            provider_filter: None,
            model_filter: None,
            source_filter: None,
            saved_filter: None,
            debug_filter: None,
            canary_filter: None,
            after: None,
            before: None,
            context_before: 0,
            context_after: 0,
            max_scan_sessions: DEFAULT_MAX_SCAN_SESSIONS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoleFilter {
    User,
    Assistant,
    Metadata,
}

impl RoleFilter {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "metadata" | "session" => Some(Self::Metadata),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchResultKind {
    Metadata,
    Message,
}

impl SearchResultKind {
    fn label(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Message => "message",
        }
    }
}

#[derive(Debug, Clone)]
struct SearchResult {
    source: String,
    session_id: String,
    short_name: Option<String>,
    title: Option<String>,
    working_dir: Option<String>,
    provider_key: Option<String>,
    model: Option<String>,
    updated_at: DateTime<Utc>,
    kind: SearchResultKind,
    role: String,
    message_index: Option<usize>,
    message_id: Option<String>,
    message_timestamp: Option<DateTime<Utc>>,
    snippet: String,
    score: f64,
    matched_terms: Vec<String>,
    exact_match: bool,
    context: Vec<ResultContextLine>,
}

#[derive(Debug, Clone)]
struct ResultContextLine {
    message_index: usize,
    role: String,
    timestamp: Option<DateTime<Utc>>,
    text: String,
}

#[derive(Debug, Clone)]
struct ExternalSessionRecord {
    source: &'static str,
    session_id: String,
    short_name: Option<String>,
    title: Option<String>,
    working_dir: Option<String>,
    provider_key: Option<String>,
    model: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    path: PathBuf,
    messages: Vec<ExternalMessageRecord>,
}

#[derive(Debug, Clone)]
struct ExternalMessageRecord {
    role: String,
    text: String,
    timestamp: Option<DateTime<Utc>>,
    id: Option<String>,
}

#[derive(Debug, Default)]
struct SearchReport {
    results: Vec<SearchResult>,
    scanned_jcode_sessions: usize,
    candidate_jcode_sessions: usize,
    scanned_external_sessions: usize,
    external_sources: Vec<&'static str>,
    read_errors: usize,
    parse_errors: usize,
    truncated: bool,
}

#[derive(Debug, Clone)]
struct SessionFileCandidate {
    snapshot_path: PathBuf,
    journal_path: PathBuf,
    session_id_hint: String,
    mtime: SystemTime,
}

#[derive(Default)]
struct RawFilterOutcome {
    candidates: Vec<SessionFileCandidate>,
    read_errors: usize,
}

#[derive(Default)]
struct SearchWorkerOutcome {
    results: Vec<SearchResult>,
    parse_errors: usize,
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }

    fn description(&self) -> &str {
        "Search past chat sessions. Current session, tool-only messages, and system reminders are hidden by default."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "query": {
                    "type": "string",
                    "description": "Search query. Use distinctive keywords; stop-word-only queries are rejected."
                },
                "working_dir": {
                    "type": "string",
                    "description": "Restrict results to sessions whose working directory matches this path or path prefix. Matching is normalized and case-insensitive."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_LIMIT,
                    "description": "Max results."
                },
                "include_current": {
                    "type": "boolean",
                    "description": "Include the current active session. Defaults to false."
                },
                "include_tools": {
                    "type": "boolean",
                    "description": "Include raw tool calls and tool results. Defaults to false to reduce log noise."
                },
                "include_system": {
                    "type": "boolean",
                    "description": "Include system reminders and display/system messages. Defaults to false."
                },
                "max_per_session": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_MAX_PER_SESSION,
                    "description": "Maximum hits to return from one session. Defaults to 1 for result diversity."
                },
                "role": {
                    "type": "string",
                    "enum": ["user", "assistant", "metadata"],
                    "description": "Restrict results to a role or to metadata-only hits."
                },
                "provider": {
                    "type": "string",
                    "description": "Restrict by provider/source substring, e.g. openai, claude, codex, pi, opencode."
                },
                "model": {
                    "type": "string",
                    "description": "Restrict by model substring."
                },
                "after": {
                    "type": "string",
                    "description": "Only include sessions/messages at or after this RFC3339 timestamp or YYYY-MM-DD date."
                },
                "before": {
                    "type": "string",
                    "description": "Only include sessions/messages at or before this RFC3339 timestamp or YYYY-MM-DD date."
                },
                "saved": {
                    "type": "boolean",
                    "description": "Restrict Jcode sessions by saved/bookmarked flag."
                },
                "debug": {
                    "type": "boolean",
                    "description": "Restrict Jcode sessions by debug/test flag."
                },
                "canary": {
                    "type": "boolean",
                    "description": "Restrict Jcode sessions by canary flag."
                },
                "source": {
                    "type": "string",
                    "enum": ["all", "jcode", "claude", "codex", "pi", "opencode"],
                    "description": "Restrict session source. Defaults to all available sources."
                },
                "include_external": {
                    "type": "boolean",
                    "description": "Include external session sources discovered by the session picker. Defaults to true."
                },
                "context_before": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": MAX_CONTEXT_MESSAGES,
                    "description": "Number of preceding messages to include around each hit."
                },
                "context_after": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": MAX_CONTEXT_MESSAGES,
                    "description": "Number of following messages to include around each hit."
                },
                "max_scan_sessions": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_MAX_SCAN_SESSIONS,
                    "description": "Bound the number of recent sessions scanned per source."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: SearchInput = serde_json::from_value(input)?;
        let limit = match validate_bounded_usize(params.limit, DEFAULT_LIMIT, 1, MAX_LIMIT, "limit")
        {
            Ok(limit) => limit,
            Err(message) => return Ok(ToolOutput::new(message).with_title("session_search")),
        };
        let max_per_session = match validate_bounded_usize(
            params.max_per_session,
            DEFAULT_MAX_PER_SESSION,
            1,
            MAX_MAX_PER_SESSION,
            "max_per_session",
        ) {
            Ok(max_per_session) => max_per_session.min(limit),
            Err(message) => return Ok(ToolOutput::new(message).with_title("session_search")),
        };
        let context_before = match validate_bounded_usize(
            params.context_before,
            0,
            0,
            MAX_CONTEXT_MESSAGES,
            "context_before",
        ) {
            Ok(value) => value,
            Err(message) => return Ok(ToolOutput::new(message).with_title("session_search")),
        };
        let context_after = match validate_bounded_usize(
            params.context_after,
            0,
            0,
            MAX_CONTEXT_MESSAGES,
            "context_after",
        ) {
            Ok(value) => value,
            Err(message) => return Ok(ToolOutput::new(message).with_title("session_search")),
        };
        let max_scan_sessions = match validate_bounded_usize(
            params.max_scan_sessions,
            DEFAULT_MAX_SCAN_SESSIONS,
            1,
            MAX_MAX_SCAN_SESSIONS,
            "max_scan_sessions",
        ) {
            Ok(value) => value,
            Err(message) => return Ok(ToolOutput::new(message).with_title("session_search")),
        };
        let role_filter = match parse_role_filter(params.role.as_deref()) {
            Ok(value) => value,
            Err(message) => return Ok(ToolOutput::new(message).with_title("session_search")),
        };
        let source_filter = match normalize_source_filter(params.source.as_deref()) {
            Ok(value) => value,
            Err(message) => return Ok(ToolOutput::new(message).with_title("session_search")),
        };
        let after = match parse_datetime_filter(params.after.as_deref(), "after") {
            Ok(value) => value,
            Err(message) => return Ok(ToolOutput::new(message).with_title("session_search")),
        };
        let before = match parse_datetime_filter(params.before.as_deref(), "before") {
            Ok(value) => value,
            Err(message) => return Ok(ToolOutput::new(message).with_title("session_search")),
        };

        let query = QueryProfile::new(&params.query);
        if query.is_empty() {
            return Ok(ToolOutput::new("Query cannot be empty.").with_title("session_search"));
        }
        if !query.is_actionable() {
            return Ok(ToolOutput::new(format!(
                "Query '{}' is too generic after removing common stop words. Add at least one distinctive keyword.",
                params.query.trim()
            ))
            .with_title("session_search"));
        }

        let sessions_dir = storage::jcode_dir()?.join("sessions");

        let options = SearchOptions {
            current_session_id: ctx.session_id.clone(),
            working_dir_filter: params.working_dir.clone(),
            limit,
            max_per_session,
            include_current: params.include_current.unwrap_or(false),
            include_tools: params.include_tools.unwrap_or(false),
            include_system: params.include_system.unwrap_or(false),
            include_external: params.include_external.unwrap_or(true),
            role_filter,
            provider_filter: normalize_optional_filter(params.provider),
            model_filter: normalize_optional_filter(params.model),
            source_filter,
            saved_filter: params.saved,
            debug_filter: params.debug,
            canary_filter: params.canary,
            after,
            before,
            context_before,
            context_after,
            max_scan_sessions,
        };

        let report = tokio::task::spawn_blocking({
            let session_id = ctx.session_id.clone();
            let query = query.clone();
            let options = options.clone();
            move || search_sessions_blocking(&sessions_dir, &query, &options, &session_id)
        })
        .await??;

        if report.results.is_empty() {
            return Ok(ToolOutput::new(no_results_message(&params.query, &options))
                .with_title("session_search"));
        }

        Ok(
            ToolOutput::new(format_results(&params.query, &report, &options))
                .with_title("session_search"),
        )
    }
}

fn validate_bounded_usize(
    value: Option<i64>,
    default: usize,
    min: usize,
    max: usize,
    name: &str,
) -> std::result::Result<usize, String> {
    let Some(value) = value else {
        return Ok(default);
    };
    if value < min as i64 || value > max as i64 {
        return Err(format!(
            "{name} must be between {min} and {max}; received {value}."
        ));
    }
    Ok(value as usize)
}

fn parse_role_filter(raw: Option<&str>) -> std::result::Result<Option<RoleFilter>, String> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    RoleFilter::parse(raw)
        .map(Some)
        .ok_or_else(|| format!("role must be one of user, assistant, or metadata; received {raw}."))
}

fn normalize_optional_filter(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn normalize_source_filter(raw: Option<&str>) -> std::result::Result<Option<String>, String> {
    let Some(source) = raw.map(str::trim).filter(|source| !source.is_empty()) else {
        return Ok(None);
    };
    let normalized = source.to_ascii_lowercase();
    match normalized.as_str() {
        "all" => Ok(None),
        "jcode" | "claude" | "claude-code" | "codex" | "pi" | "opencode" => {
            Ok(Some(normalized.replace("claude-code", "claude")))
        }
        _ => Err(format!(
            "source must be one of all, jcode, claude, codex, pi, or opencode; received {source}."
        )),
    }
}

fn parse_datetime_filter(
    raw: Option<&str>,
    name: &str,
) -> std::result::Result<Option<DateTime<Utc>>, String> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(Some(dt.with_timezone(&Utc)));
    }
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let Some(naive) = date.and_hms_opt(0, 0, 0) else {
            return Err(format!("{name} has an invalid date: {raw}."));
        };
        return Ok(Some(DateTime::from_naive_utc_and_offset(naive, Utc)));
    }
    Err(format!(
        "{name} must be an RFC3339 timestamp or YYYY-MM-DD date; received {raw}."
    ))
}

/// Synchronous search across session files with parallel raw pre-filtering and
/// journal-aware session loading.
fn search_sessions_blocking(
    sessions_dir: &Path,
    query: &QueryProfile,
    options: &SearchOptions,
    log_session_id: &str,
) -> Result<SearchReport> {
    let mut report = SearchReport::default();
    if !query.is_actionable() {
        return Ok(report);
    }

    if source_matches_filter("jcode", options) {
        let mut files = collect_session_files(sessions_dir)?;
        if !files.is_empty() {
            files.sort_unstable_by(|a, b| b.mtime.cmp(&a.mtime));
            if files.len() > options.max_scan_sessions {
                files.truncate(options.max_scan_sessions);
                report.truncated = true;
            }
            report.scanned_jcode_sessions = files.len();

            if !options.include_current {
                files.retain(|candidate| candidate.session_id_hint != options.current_session_id);
            }

            if !files.is_empty() {
                let raw_filter_outcomes = filter_candidates_parallel(&files, query);
                report.read_errors += raw_filter_outcomes
                    .iter()
                    .map(|outcome| outcome.read_errors)
                    .sum::<usize>();
                let mut candidates: Vec<SessionFileCandidate> = raw_filter_outcomes
                    .into_iter()
                    .flat_map(|outcome| outcome.candidates)
                    .collect();
                candidates.sort_unstable_by(|a, b| b.mtime.cmp(&a.mtime));
                report.candidate_jcode_sessions = candidates.len();
                if candidates.len() > MAX_DESERIALIZE {
                    candidates.truncate(MAX_DESERIALIZE);
                    report.truncated = true;
                }

                let search_outcomes = score_candidates_parallel(&candidates, query, options);
                report.parse_errors += search_outcomes
                    .iter()
                    .map(|outcome| outcome.parse_errors)
                    .sum::<usize>();
                report.results.extend(
                    search_outcomes
                        .into_iter()
                        .flat_map(|outcome| outcome.results),
                );
            }
        }
    }

    if options.include_external {
        let external_report = search_external_sessions(query, options);
        report.scanned_external_sessions += external_report.scanned_external_sessions;
        report
            .external_sources
            .extend(external_report.external_sources);
        report.read_errors += external_report.read_errors;
        report.parse_errors += external_report.parse_errors;
        report.truncated |= external_report.truncated;
        report.results.extend(external_report.results);
    }

    if report.read_errors > 0 || report.parse_errors > 0 {
        crate::logging::warn(&format!(
            "[tool:session_search] skipped unreadable or invalid session files in session {} (read_errors={} parse_errors={})",
            log_session_id, report.read_errors, report.parse_errors
        ));
    }

    report.results.sort_unstable_by(compare_results);
    report.results = group_and_limit_results(report.results, options);
    Ok(report)
}

fn collect_session_files(sessions_dir: &Path) -> Result<Vec<SessionFileCandidate>> {
    let mut files = Vec::new();
    if !sessions_dir.exists() {
        return Ok(files);
    }
    for entry in std::fs::read_dir(sessions_dir)?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Some(stem) = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
        else {
            continue;
        };
        let journal_path = session_journal_path_from_snapshot(&path);
        let snapshot_mtime = modified_time_or_epoch(&path);
        let journal_mtime = modified_time_or_epoch(&journal_path);
        files.push(SessionFileCandidate {
            snapshot_path: path,
            journal_path,
            session_id_hint: stem,
            mtime: snapshot_mtime.max(journal_mtime),
        });
    }
    Ok(files)
}

fn modified_time_or_epoch(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn filter_candidates_parallel(
    files: &[SessionFileCandidate],
    query: &QueryProfile,
) -> Vec<RawFilterOutcome> {
    if files.is_empty() {
        return Vec::new();
    }
    let thread_count = SCAN_THREADS.min(files.len());
    let chunk_size = files.len().div_ceil(thread_count);

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in files.chunks(chunk_size) {
            handles.push(scope.spawn(move || {
                let mut outcome = RawFilterOutcome::default();
                for candidate in chunk {
                    if path_matches_query(&candidate.session_id_hint, query) {
                        outcome.candidates.push(candidate.clone());
                        continue;
                    }

                    let Some(raw) = read_candidate_raw(candidate, &mut outcome.read_errors) else {
                        continue;
                    };
                    if raw_matches_query(&raw, query) {
                        outcome.candidates.push(candidate.clone());
                    }
                }
                outcome
            }));
        }
        handles
            .into_iter()
            .map(|handle| match handle.join() {
                Ok(outcome) => outcome,
                Err(_) => {
                    crate::logging::warn(
                        "session_search raw pre-filter worker panicked; skipping that worker's candidates",
                    );
                    RawFilterOutcome::default()
                }
            })
            .collect()
    })
}

fn read_candidate_raw(
    candidate: &SessionFileCandidate,
    read_errors: &mut usize,
) -> Option<Vec<u8>> {
    let mut raw = match std::fs::read(&candidate.snapshot_path) {
        Ok(data) => data,
        Err(_) => {
            *read_errors += 1;
            return None;
        }
    };

    if candidate.journal_path.exists() {
        match std::fs::read(&candidate.journal_path) {
            Ok(journal) => {
                raw.push(b'\n');
                raw.extend_from_slice(&journal);
            }
            Err(_) => *read_errors += 1,
        }
    }

    Some(raw)
}

fn score_candidates_parallel(
    candidates: &[SessionFileCandidate],
    query: &QueryProfile,
    options: &SearchOptions,
) -> Vec<SearchWorkerOutcome> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let thread_count = SCAN_THREADS.min(candidates.len());
    let chunk_size = candidates.len().div_ceil(thread_count);

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in candidates.chunks(chunk_size) {
            handles.push(scope.spawn(move || {
                let mut outcome = SearchWorkerOutcome::default();
                for candidate in chunk {
                    match Session::load_from_path(&candidate.snapshot_path) {
                        Ok(session) => {
                            append_session_results(&mut outcome.results, &session, query, options)
                        }
                        Err(_) => outcome.parse_errors += 1,
                    }
                }
                outcome
            }));
        }
        handles
            .into_iter()
            .map(|handle| match handle.join() {
                Ok(outcome) => outcome,
                Err(_) => {
                    crate::logging::warn(
                        "session_search scoring worker panicked; skipping that worker's results",
                    );
                    SearchWorkerOutcome::default()
                }
            })
            .collect()
    })
}

fn search_external_sessions(query: &QueryProfile, options: &SearchOptions) -> SearchReport {
    let mut report = SearchReport::default();
    let mut records = Vec::new();

    if source_matches_filter("claude", options) {
        if let Ok(sessions) =
            crate::import::list_claude_code_sessions_lazy(options.max_scan_sessions)
        {
            report.external_sources.push("claude");
            for session in sessions.into_iter().take(options.max_scan_sessions) {
                let path = PathBuf::from(&session.full_path);
                let messages = load_claude_external_messages(&path, options);
                let created_at = session.created.unwrap_or_else(Utc::now);
                let updated_at = session.modified.or(session.created).unwrap_or(created_at);
                let title = session
                    .summary
                    .filter(|summary| !summary.trim().is_empty())
                    .unwrap_or_else(|| truncate_title_text(&session.first_prompt, 72));
                records.push(ExternalSessionRecord {
                    source: "claude",
                    session_id: session.session_id.clone(),
                    short_name: Some(format!(
                        "claude {}",
                        &session.session_id[..session.session_id.len().min(8)]
                    )),
                    title: Some(title),
                    working_dir: session.project_path,
                    provider_key: Some("claude-code".to_string()),
                    model: None,
                    created_at,
                    updated_at,
                    path,
                    messages,
                });
            }
        }
    }

    collect_external_jsonl_source(
        &mut records,
        &mut report,
        "codex",
        ".codex/sessions",
        options,
        load_codex_external_session,
    );
    collect_external_jsonl_source(
        &mut records,
        &mut report,
        "pi",
        ".pi/agent/sessions",
        options,
        load_pi_external_session,
    );
    collect_opencode_external_sessions(&mut records, &mut report, options);

    if records.len() > options.max_scan_sessions.saturating_mul(5) {
        records.truncate(options.max_scan_sessions.saturating_mul(5));
        report.truncated = true;
    }

    report.scanned_external_sessions = records.len();
    for record in records {
        append_external_session_results(&mut report.results, &record, query, options);
    }
    report.external_sources.sort_unstable();
    report.external_sources.dedup();
    report
}

fn collect_external_jsonl_source(
    records: &mut Vec<ExternalSessionRecord>,
    report: &mut SearchReport,
    source: &'static str,
    root_relative: &str,
    options: &SearchOptions,
    loader: fn(&Path, &SearchOptions) -> Result<Option<ExternalSessionRecord>>,
) {
    if !source_matches_filter(source, options) {
        return;
    }
    let Ok(root) = crate::storage::user_home_path(root_relative) else {
        return;
    };
    if !root.exists() {
        return;
    }
    report.external_sources.push(source);
    for path in collect_recent_files_recursive(&root, "jsonl", options.max_scan_sessions) {
        match loader(&path, options) {
            Ok(Some(record)) => records.push(record),
            Ok(None) => {}
            Err(_) => report.parse_errors += 1,
        }
    }
}

fn collect_opencode_external_sessions(
    records: &mut Vec<ExternalSessionRecord>,
    report: &mut SearchReport,
    options: &SearchOptions,
) {
    if !source_matches_filter("opencode", options) {
        return;
    }
    let Ok(root) = crate::storage::user_home_path(".local/share/opencode/storage/session") else {
        return;
    };
    if !root.exists() {
        return;
    }
    report.external_sources.push("opencode");
    for path in collect_recent_files_recursive(&root, "json", options.max_scan_sessions) {
        match load_opencode_external_session(&path, options) {
            Ok(Some(record)) => records.push(record),
            Ok(None) => {}
            Err(_) => report.parse_errors += 1,
        }
    }
}

fn append_external_session_results(
    results: &mut Vec<SearchResult>,
    session: &ExternalSessionRecord,
    query: &QueryProfile,
    options: &SearchOptions,
) {
    if !external_session_matches_filters(session, options) {
        return;
    }
    if let Some(filter) = options.working_dir_filter.as_deref()
        && !session
            .working_dir
            .as_deref()
            .is_some_and(|working_dir| working_dir_matches(working_dir, filter))
    {
        return;
    }

    if role_filter_allows_metadata(options)
        && datetime_matches(session.updated_at, options)
        && let Some(match_score) = score_message_match(&external_metadata_text(session), query)
    {
        results.push(SearchResult {
            source: session.source.to_string(),
            session_id: format!("{}:{}", session.source, session.session_id),
            short_name: session.short_name.clone(),
            title: session.title.clone(),
            working_dir: session.working_dir.clone(),
            provider_key: session.provider_key.clone(),
            model: session.model.clone(),
            updated_at: session.updated_at,
            kind: SearchResultKind::Metadata,
            role: "metadata".to_string(),
            message_index: None,
            message_id: None,
            message_timestamp: None,
            snippet: match_score.snippet,
            score: match_score.score + 1.5,
            matched_terms: match_score.matched_terms,
            exact_match: match_score.exact_match,
            context: Vec::new(),
        });
    }

    for (message_index, msg) in session.messages.iter().enumerate() {
        if !role_filter_allows_external_message(&msg.role, options) {
            continue;
        }
        if !datetime_matches(msg.timestamp.unwrap_or(session.updated_at), options) {
            continue;
        }
        let Some(match_score) = score_message_match(&msg.text, query) else {
            continue;
        };
        results.push(SearchResult {
            source: session.source.to_string(),
            session_id: format!("{}:{}", session.source, session.session_id),
            short_name: session.short_name.clone(),
            title: session.title.clone(),
            working_dir: session.working_dir.clone(),
            provider_key: session.provider_key.clone(),
            model: session.model.clone(),
            updated_at: session.updated_at,
            kind: SearchResultKind::Message,
            role: msg.role.clone(),
            message_index: Some(message_index),
            message_id: msg.id.clone(),
            message_timestamp: msg.timestamp,
            snippet: match_score.snippet,
            score: match_score.score,
            matched_terms: match_score.matched_terms,
            exact_match: match_score.exact_match,
            context: build_external_context(&session.messages, message_index, options),
        });
    }
}

fn external_metadata_text(session: &ExternalSessionRecord) -> String {
    let mut fields = vec![
        format!("Source: {}", session.source),
        format!("Session ID: {}:{}", session.source, session.session_id),
        format!("Created: {}", format_datetime(session.created_at)),
        format!("Updated: {}", format_datetime(session.updated_at)),
        format!("Path: {}", session.path.display()),
    ];
    if let Some(title) = &session.title {
        fields.push(format!("Title: {title}"));
    }
    if let Some(working_dir) = &session.working_dir {
        fields.push(format!("Working directory: {working_dir}"));
    }
    if let Some(provider_key) = &session.provider_key {
        fields.push(format!("Provider: {provider_key}"));
    }
    if let Some(model) = &session.model {
        fields.push(format!("Model: {model}"));
    }
    fields.join("\n")
}

fn build_external_context(
    messages: &[ExternalMessageRecord],
    hit_index: usize,
    options: &SearchOptions,
) -> Vec<ResultContextLine> {
    if options.context_before == 0 && options.context_after == 0 {
        return Vec::new();
    }
    let start = hit_index.saturating_sub(options.context_before);
    let end = (hit_index + options.context_after + 1).min(messages.len());
    (start..end)
        .filter(|&idx| idx != hit_index)
        .filter_map(|idx| {
            let msg = &messages[idx];
            (!msg.text.trim().is_empty()).then(|| ResultContextLine {
                message_index: idx,
                role: msg.role.clone(),
                timestamp: msg.timestamp,
                text: truncate_context_text(&msg.text),
            })
        })
        .collect()
}

fn collect_recent_files_recursive(root: &Path, extension: &str, limit: usize) -> Vec<PathBuf> {
    fn walk(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, extension, out);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case(extension))
                .unwrap_or(false)
            {
                out.push(path);
            }
        }
    }

    let mut files = Vec::new();
    walk(root, extension, &mut files);
    files.sort_by(|a, b| modified_time_or_epoch(b).cmp(&modified_time_or_epoch(a)));
    files.truncate(limit);
    files
}

fn load_claude_external_messages(
    path: &Path,
    options: &SearchOptions,
) -> Vec<ExternalMessageRecord> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(|line| line.ok())
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .filter_map(|value| {
            let entry_type = value
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if entry_type != "user" && entry_type != "assistant" {
                return None;
            }
            let message = value.get("message")?;
            let role = message
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or(entry_type)
                .to_string();
            let text = extract_external_text(
                message.get("content").unwrap_or(&Value::Null),
                options.include_tools,
            );
            if text.trim().is_empty() {
                return None;
            }
            Some(ExternalMessageRecord {
                role,
                text,
                timestamp: parse_timestamp_value(value.get("timestamp")),
                id: value
                    .get("uuid")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
        })
        .collect()
}

fn load_codex_external_session(
    path: &Path,
    options: &SearchOptions,
) -> Result<Option<ExternalSessionRecord>> {
    let file = File::open(path)?;
    let mut lines = BufReader::new(file).lines();
    let Some(first_line) = lines.next() else {
        return Ok(None);
    };
    let header: Value = serde_json::from_str(&first_line?)?;
    let meta = if header.get("type").and_then(|v| v.as_str()) == Some("session_meta") {
        header.get("payload").unwrap_or(&header)
    } else {
        &header
    };
    let session_id = meta.get("id").and_then(|v| v.as_str()).unwrap_or_default();
    if session_id.is_empty() {
        return Ok(None);
    }
    let created_at = parse_timestamp_value(meta.get("timestamp"))
        .or_else(|| parse_timestamp_value(header.get("timestamp")))
        .unwrap_or_else(Utc::now);
    let mut updated_at = modified_datetime(path).unwrap_or(created_at);
    let working_dir = meta.get("cwd").and_then(|v| v.as_str()).map(str::to_string);
    let mut messages = Vec::new();
    for line in lines.map_while(|line| line.ok()) {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let line_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let (role, content_value) = if line_type == "message" {
            let Some(role) = value.get("role").and_then(|v| v.as_str()) else {
                continue;
            };
            (role, value.get("content").unwrap_or(&Value::Null))
        } else if line_type == "response_item" {
            let Some(payload) = value.get("payload") else {
                continue;
            };
            if payload.get("type").and_then(|v| v.as_str()) != Some("message") {
                continue;
            }
            let Some(role) = payload.get("role").and_then(|v| v.as_str()) else {
                continue;
            };
            (role, payload.get("content").unwrap_or(&Value::Null))
        } else {
            continue;
        };
        if role != "user" && role != "assistant" {
            continue;
        }
        let text = extract_external_text(content_value, options.include_tools);
        if text.trim().is_empty() {
            continue;
        }
        let timestamp = parse_timestamp_value(value.get("timestamp"));
        if let Some(ts) = timestamp {
            updated_at = updated_at.max(ts);
        }
        messages.push(ExternalMessageRecord {
            role: role.to_string(),
            text,
            timestamp,
            id: value.get("id").and_then(|v| v.as_str()).map(str::to_string),
        });
    }
    Ok(Some(ExternalSessionRecord {
        source: "codex",
        session_id: session_id.to_string(),
        short_name: Some(format!("codex {}", &session_id[..session_id.len().min(8)])),
        title: Some(format!(
            "Codex session {}",
            &session_id[..session_id.len().min(8)]
        )),
        working_dir,
        provider_key: Some("openai-codex".to_string()),
        model: None,
        created_at,
        updated_at,
        path: path.to_path_buf(),
        messages,
    }))
}

fn load_pi_external_session(
    path: &Path,
    options: &SearchOptions,
) -> Result<Option<ExternalSessionRecord>> {
    let file = File::open(path)?;
    let mut lines = BufReader::new(file).lines();
    let Some(first_line) = lines.next() else {
        return Ok(None);
    };
    let header: Value = serde_json::from_str(&first_line?)?;
    if header.get("type").and_then(|v| v.as_str()) != Some("session") {
        return Ok(None);
    }
    let session_id = header
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if session_id.is_empty() {
        return Ok(None);
    }
    let created_at = parse_timestamp_value(header.get("timestamp")).unwrap_or_else(Utc::now);
    let mut updated_at = modified_datetime(path).unwrap_or(created_at);
    let working_dir = header
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mut provider_key = Some("pi".to_string());
    let mut model = None;
    let mut messages = Vec::new();
    for line in lines.map_while(|line| line.ok()) {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if let Some(ts) = parse_timestamp_value(value.get("timestamp")) {
            updated_at = updated_at.max(ts);
        }
        match value.get("type").and_then(|v| v.as_str()) {
            Some("model_change") => {
                provider_key = value
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or(provider_key);
                model = value
                    .get("modelId")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or(model);
            }
            Some("message") => {
                let Some(message) = value.get("message") else {
                    continue;
                };
                let role = message
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if role != "user" && role != "assistant" {
                    continue;
                }
                let text = extract_external_text(
                    message.get("content").unwrap_or(&Value::Null),
                    options.include_tools,
                );
                if text.trim().is_empty() {
                    continue;
                }
                messages.push(ExternalMessageRecord {
                    role: role.to_string(),
                    text,
                    timestamp: parse_timestamp_value(value.get("timestamp")),
                    id: value.get("id").and_then(|v| v.as_str()).map(str::to_string),
                });
            }
            _ => {}
        }
    }
    Ok(Some(ExternalSessionRecord {
        source: "pi",
        session_id: session_id.to_string(),
        short_name: Some(format!("pi {}", &session_id[..session_id.len().min(8)])),
        title: Some(format!(
            "Pi session {}",
            &session_id[..session_id.len().min(8)]
        )),
        working_dir,
        provider_key,
        model,
        created_at,
        updated_at,
        path: path.to_path_buf(),
        messages,
    }))
}

fn load_opencode_external_session(
    path: &Path,
    options: &SearchOptions,
) -> Result<Option<ExternalSessionRecord>> {
    let value: Value = serde_json::from_reader(File::open(path)?)?;
    let session_id = value.get("id").and_then(|v| v.as_str()).unwrap_or_default();
    if session_id.is_empty() {
        return Ok(None);
    }
    let created_at = value
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(|v| v.as_i64())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let updated_at = value
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(|v| v.as_i64())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .or_else(|| modified_datetime(path))
        .unwrap_or(created_at);
    let working_dir = value
        .get("directory")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(|title| truncate_title_text(title, 72))
        .unwrap_or_else(|| {
            format!(
                "OpenCode session {}",
                &session_id[..session_id.len().min(8)]
            )
        });
    let messages_root = crate::storage::user_home_path(format!(
        ".local/share/opencode/storage/message/{}",
        session_id
    ))?;
    let mut provider_key = Some("opencode".to_string());
    let mut model = None;
    let mut messages = Vec::new();
    if messages_root.exists() {
        for msg_path in
            collect_recent_files_recursive(&messages_root, "json", options.max_scan_sessions)
        {
            let Ok(msg_value) = serde_json::from_reader::<_, Value>(File::open(&msg_path)?) else {
                continue;
            };
            let role = msg_value
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if role != "user" && role != "assistant" {
                continue;
            }
            if model.is_none() {
                model = msg_value
                    .get("modelID")
                    .or_else(|| msg_value.get("model").and_then(|m| m.get("modelID")))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            provider_key = msg_value
                .get("providerID")
                .or_else(|| msg_value.get("model").and_then(|m| m.get("providerID")))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(provider_key);
            let text = msg_value
                .get("summary")
                .or_else(|| msg_value.get("content"))
                .map(|value| extract_external_text(value, options.include_tools))
                .unwrap_or_default();
            if text.trim().is_empty() {
                continue;
            }
            messages.push(ExternalMessageRecord {
                role: role.to_string(),
                text,
                timestamp: None,
                id: msg_value
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            });
        }
    }
    Ok(Some(ExternalSessionRecord {
        source: "opencode",
        session_id: session_id.to_string(),
        short_name: Some(format!(
            "opencode {}",
            &session_id[..session_id.len().min(8)]
        )),
        title: Some(title),
        working_dir,
        provider_key,
        model,
        created_at,
        updated_at,
        path: path.to_path_buf(),
        messages,
    }))
}

fn extract_external_text(value: &Value, include_tools: bool) -> String {
    fn visit(value: &Value, include_tools: bool, out: &mut Vec<String>) {
        match value {
            Value::String(text) => {
                if !text.trim().is_empty() {
                    out.push(text.trim().to_string());
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, include_tools, out);
                }
            }
            Value::Object(map) => {
                let block_type = map.get("type").and_then(|v| v.as_str()).unwrap_or_default();
                if !include_tools
                    && matches!(block_type, "tool_use" | "tool_result" | "function_call")
                {
                    return;
                }
                if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        out.push(text.trim().to_string());
                    }
                } else if include_tools
                    && let Some(content) = map.get("content").and_then(|v| v.as_str())
                    && !content.trim().is_empty()
                {
                    out.push(content.trim().to_string());
                }
                for (key, nested) in map {
                    if matches!(key.as_str(), "type" | "text" | "content") {
                        continue;
                    }
                    visit(nested, include_tools, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    visit(value, include_tools, &mut out);
    out.join("\n")
}

fn parse_timestamp_value(value: Option<&Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(|v| v.as_str())
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

fn modified_datetime(path: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .map(DateTime::<Utc>::from)
}

fn truncate_title_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        format!(
            "{}…",
            trimmed
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn append_session_results(
    results: &mut Vec<SearchResult>,
    session: &Session,
    query: &QueryProfile,
    options: &SearchOptions,
) {
    if !options.include_current && session.id == options.current_session_id {
        return;
    }
    if !jcode_session_matches_filters(session, options) {
        return;
    }

    if let Some(filter) = options.working_dir_filter.as_deref()
        && !session
            .working_dir
            .as_deref()
            .is_some_and(|working_dir| working_dir_matches(working_dir, filter))
    {
        return;
    }

    if role_filter_allows_metadata(options)
        && datetime_matches(session.updated_at, options)
        && let Some(match_score) = score_message_match(&metadata_text(session), query)
    {
        results.push(SearchResult {
            source: "jcode".to_string(),
            session_id: session.id.clone(),
            short_name: session.short_name.clone(),
            title: session.title.clone(),
            working_dir: session.working_dir.clone(),
            provider_key: session.provider_key.clone(),
            model: session.model.clone(),
            updated_at: session.updated_at,
            kind: SearchResultKind::Metadata,
            role: "metadata".to_string(),
            message_index: None,
            message_id: None,
            message_timestamp: None,
            snippet: match_score.snippet,
            score: match_score.score + 2.0,
            matched_terms: match_score.matched_terms,
            exact_match: match_score.exact_match,
            context: Vec::new(),
        });
    }

    for (message_index, msg) in session.messages.iter().enumerate() {
        if !options.include_system && is_system_like_message(msg) {
            continue;
        }
        if is_tool_only_message(msg) && !options.include_tools {
            continue;
        }
        if !role_filter_allows_message(msg, options) {
            continue;
        }
        if !datetime_matches(msg.timestamp.unwrap_or(session.updated_at), options) {
            continue;
        }

        let text = searchable_message_text(msg, options.include_tools);
        if text.is_empty() {
            continue;
        }

        let Some(match_score) = score_message_match(&text, query) else {
            continue;
        };

        let mut score = match_score.score;
        if is_tool_only_message(msg) {
            score *= 0.4;
        }

        results.push(SearchResult {
            source: "jcode".to_string(),
            session_id: session.id.clone(),
            short_name: session.short_name.clone(),
            title: session.title.clone(),
            working_dir: session.working_dir.clone(),
            provider_key: session.provider_key.clone(),
            model: session.model.clone(),
            updated_at: session.updated_at,
            kind: SearchResultKind::Message,
            role: role_label(msg).to_string(),
            message_index: Some(message_index),
            message_id: Some(msg.id.clone()),
            message_timestamp: msg.timestamp,
            snippet: match_score.snippet,
            score,
            matched_terms: match_score.matched_terms,
            exact_match: match_score.exact_match,
            context: build_jcode_context(&session.messages, message_index, options),
        });
    }
}

fn metadata_text(session: &Session) -> String {
    let mut fields = vec![
        format!("Session ID: {}", session.id),
        format!("Updated: {}", format_datetime(session.updated_at)),
        format!("Created: {}", format_datetime(session.created_at)),
    ];

    if let Some(short_name) = &session.short_name {
        fields.push(format!("Short name: {short_name}"));
    }
    if let Some(title) = &session.title {
        fields.push(format!("Title: {title}"));
    }
    if let Some(working_dir) = &session.working_dir {
        fields.push(format!("Working directory: {working_dir}"));
    }
    if let Some(save_label) = &session.save_label {
        fields.push(format!("Save label: {save_label}"));
    }
    if let Some(provider_key) = &session.provider_key {
        fields.push(format!("Provider: {provider_key}"));
    }
    if let Some(model) = &session.model {
        fields.push(format!("Model: {model}"));
    }

    fields.join("\n")
}

fn source_matches_filter(source: &str, options: &SearchOptions) -> bool {
    options
        .source_filter
        .as_deref()
        .map(|filter| source.eq_ignore_ascii_case(filter))
        .unwrap_or(true)
}

fn jcode_session_matches_filters(session: &Session, options: &SearchOptions) -> bool {
    if !source_matches_filter("jcode", options) {
        return false;
    }
    if !provider_matches(session.provider_key.as_deref(), "jcode", options) {
        return false;
    }
    if !field_filter_matches(session.model.as_deref(), options.model_filter.as_deref()) {
        return false;
    }
    if options
        .saved_filter
        .is_some_and(|expected| session.saved != expected)
    {
        return false;
    }
    if options
        .debug_filter
        .is_some_and(|expected| session.is_debug != expected)
    {
        return false;
    }
    if options
        .canary_filter
        .is_some_and(|expected| session.is_canary != expected)
    {
        return false;
    }
    true
}

fn external_session_matches_filters(
    session: &ExternalSessionRecord,
    options: &SearchOptions,
) -> bool {
    if !source_matches_filter(session.source, options) {
        return false;
    }
    if !provider_matches(session.provider_key.as_deref(), session.source, options) {
        return false;
    }
    if !field_filter_matches(session.model.as_deref(), options.model_filter.as_deref()) {
        return false;
    }
    if options.saved_filter == Some(true)
        || options.debug_filter == Some(true)
        || options.canary_filter == Some(true)
    {
        return false;
    }
    true
}

fn provider_matches(provider_key: Option<&str>, source: &str, options: &SearchOptions) -> bool {
    let Some(filter) = options.provider_filter.as_deref() else {
        return true;
    };
    field_filter_matches(provider_key, Some(filter)) || source.to_ascii_lowercase().contains(filter)
}

fn field_filter_matches(value: Option<&str>, filter: Option<&str>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    value
        .map(|value| value.to_ascii_lowercase().contains(filter))
        .unwrap_or(false)
}

fn datetime_matches(value: DateTime<Utc>, options: &SearchOptions) -> bool {
    if options.after.is_some_and(|after| value < after) {
        return false;
    }
    if options.before.is_some_and(|before| value > before) {
        return false;
    }
    true
}

fn role_filter_allows_metadata(options: &SearchOptions) -> bool {
    options
        .role_filter
        .map(|role| role == RoleFilter::Metadata)
        .unwrap_or(true)
}

fn role_filter_allows_message(msg: &StoredMessage, options: &SearchOptions) -> bool {
    let Some(role_filter) = options.role_filter else {
        return true;
    };
    match role_filter {
        RoleFilter::User => msg.role == crate::message::Role::User,
        RoleFilter::Assistant => msg.role == crate::message::Role::Assistant,
        RoleFilter::Metadata => false,
    }
}

fn role_filter_allows_external_message(role: &str, options: &SearchOptions) -> bool {
    let Some(role_filter) = options.role_filter else {
        return true;
    };
    match role_filter {
        RoleFilter::User => role.eq_ignore_ascii_case("user"),
        RoleFilter::Assistant => role.eq_ignore_ascii_case("assistant"),
        RoleFilter::Metadata => false,
    }
}

fn build_jcode_context(
    messages: &[StoredMessage],
    hit_index: usize,
    options: &SearchOptions,
) -> Vec<ResultContextLine> {
    if options.context_before == 0 && options.context_after == 0 {
        return Vec::new();
    }
    let start = hit_index.saturating_sub(options.context_before);
    let end = (hit_index + options.context_after + 1).min(messages.len());
    (start..end)
        .filter(|&idx| idx != hit_index)
        .filter_map(|idx| {
            let msg = &messages[idx];
            if !options.include_system && is_system_like_message(msg) {
                return None;
            }
            if !options.include_tools && is_tool_only_message(msg) {
                return None;
            }
            let text = searchable_message_text(msg, options.include_tools);
            if text.trim().is_empty() {
                return None;
            }
            Some(ResultContextLine {
                message_index: idx,
                role: role_label(msg).to_string(),
                timestamp: msg.timestamp,
                text: truncate_context_text(&text),
            })
        })
        .collect()
}

fn truncate_context_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 320 {
        trimmed.to_string()
    } else {
        format!("{}...", trimmed.chars().take(320).collect::<String>())
    }
}

fn searchable_message_text(msg: &StoredMessage, include_tools: bool) -> String {
    msg.content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.clone()),
            ContentBlock::ToolResult { content, .. } if include_tools => Some(content.clone()),
            ContentBlock::ToolUse { name, input, .. } if include_tools => {
                let input = input.to_string();
                Some(if input == "null" {
                    format!("[tool call: {name}]")
                } else {
                    format!("[tool call: {name}] {input}")
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_system_like_message(msg: &StoredMessage) -> bool {
    msg.display_role.is_some()
        || msg
            .content
            .iter()
            .find_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.trim_start()),
                _ => None,
            })
            .is_some_and(|text| text.starts_with("<system-reminder>"))
}

fn is_tool_only_message(msg: &StoredMessage) -> bool {
    let mut has_text = false;
    let mut has_tool = false;

    for block in &msg.content {
        match block {
            ContentBlock::Text { text, .. } if !text.trim().is_empty() => has_text = true,
            ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. } => has_tool = true,
            _ => {}
        }
    }

    has_tool && !has_text
}

fn role_label(msg: &StoredMessage) -> &'static str {
    if let Some(display_role) = msg.display_role {
        return match display_role {
            crate::session::StoredDisplayRole::System => "system",
            crate::session::StoredDisplayRole::BackgroundTask => "background",
        };
    }

    match msg.role {
        crate::message::Role::User => "user",
        crate::message::Role::Assistant => "assistant",
    }
}

fn compare_results(a: &SearchResult, b: &SearchResult) -> std::cmp::Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| b.updated_at.cmp(&a.updated_at))
        .then_with(|| a.session_id.cmp(&b.session_id))
        .then_with(|| a.message_index.cmp(&b.message_index))
}

fn group_and_limit_results(
    results: Vec<SearchResult>,
    options: &SearchOptions,
) -> Vec<SearchResult> {
    let mut grouped = Vec::new();
    let mut per_session: HashMap<String, usize> = HashMap::new();

    for result in results {
        let count = per_session.entry(result.session_id.clone()).or_default();
        if *count >= options.max_per_session {
            continue;
        }
        *count += 1;
        grouped.push(result);
        if grouped.len() >= options.limit {
            break;
        }
    }

    grouped
}

fn format_results(query: &str, report: &SearchReport, options: &SearchOptions) -> String {
    let results = &report.results;
    let mut output = format!(
        "## Found {} results for '{}'\n\n",
        results.len(),
        query.trim()
    );

    output.push_str(&format!(
        "_Defaults: current session {}, external sources {}, tool calls/results {}, system reminders {}. Max per session: {}._\n\n",
        if options.include_current { "included" } else { "excluded" },
        if options.include_external { "included" } else { "hidden" },
        if options.include_tools { "included" } else { "hidden" },
        if options.include_system { "included" } else { "hidden" },
        options.max_per_session,
    ));

    output.push_str(&format!(
        "_Scanned: {} Jcode sessions ({} candidates), {} external sessions{}{}._\n\n",
        report.scanned_jcode_sessions,
        report.candidate_jcode_sessions,
        report.scanned_external_sessions,
        if report.external_sources.is_empty() {
            String::new()
        } else {
            format!(" from {}", report.external_sources.join(", "))
        },
        if report.truncated {
            "; scan truncated"
        } else {
            ""
        },
    ));

    for (i, result) in results.iter().enumerate() {
        let session_name = result
            .short_name
            .as_deref()
            .or(result.title.as_deref())
            .unwrap_or(&result.session_id);
        output.push_str(&format!("### Result {} - {}\n", i + 1, session_name));
        output.push_str(&format!("- Source: `{}`\n", result.source));
        output.push_str(&format!("- Session ID: `{}`\n", result.session_id));
        if let Some(title) = &result.title {
            output.push_str(&format!("- Title: {}\n", title));
        }
        if let Some(dir) = &result.working_dir {
            output.push_str(&format!("- Working dir: `{}`\n", dir));
        }
        if let Some(provider_key) = &result.provider_key {
            output.push_str(&format!("- Provider: `{}`\n", provider_key));
        }
        if let Some(model) = &result.model {
            output.push_str(&format!("- Model: `{}`\n", model));
        }
        output.push_str(&format!(
            "- Updated: {}\n- Match: {}",
            format_datetime(result.updated_at),
            result.kind.label(),
        ));
        if let Some(index) = result.message_index {
            output.push_str(&format!(" #{}", index + 1));
        }
        output.push_str(&format!(" ({})", result.role));
        if let Some(message_id) = &result.message_id {
            output.push_str(&format!(", id `{}`", message_id));
        }
        if let Some(timestamp) = result.message_timestamp {
            output.push_str(&format!(", at {}", format_datetime(timestamp)));
        }
        output.push('\n');
        output.push_str(&format!(
            "- Why: {}{}\n",
            if result.exact_match {
                "exact phrase; "
            } else {
                ""
            },
            format_matched_terms(&result.matched_terms),
        ));
        output.push_str("\n");
        output.push_str(&markdown_code_block(&result.snippet));
        if !result.context.is_empty() {
            output.push_str("\n\nContext:\n");
            for context in &result.context {
                output.push_str(&format!(
                    "- #{} {}{}\n",
                    context.message_index + 1,
                    context.role,
                    context
                        .timestamp
                        .map(|ts| format!(" at {}", format_datetime(ts)))
                        .unwrap_or_default()
                ));
                output.push_str(&markdown_code_block(&context.text));
                output.push('\n');
            }
        }
        output.push_str("\n\n");
    }

    output
}

fn no_results_message(query: &str, options: &SearchOptions) -> String {
    let mut output = format!("No results found for '{}' in past sessions.", query.trim());
    let mut hints = Vec::new();
    if !options.include_current {
        hints.push(
            "current session is excluded by default; retry with include_current=true if needed",
        );
    }
    if !options.include_tools {
        hints.push(
            "tool calls/results are hidden by default; retry with include_tools=true for raw logs",
        );
    }
    if !options.include_system {
        hints.push("system reminders are hidden by default; retry with include_system=true for internal context");
    }
    if options.working_dir_filter.is_some() {
        hints.push("the working_dir filter may be too narrow");
    }
    if !hints.is_empty() {
        output.push_str("\n\nSearch notes:\n");
        for hint in hints {
            output.push_str("- ");
            output.push_str(hint);
            output.push('\n');
        }
    }
    output
}

fn format_matched_terms(terms: &[String]) -> String {
    if terms.is_empty() {
        return "matched exact phrase".to_string();
    }
    let rendered = terms
        .iter()
        .take(8)
        .map(|term| format!("`{term}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if terms.len() > 8 {
        format!("matched terms {rendered}, ...")
    } else {
        format!("matched terms {rendered}")
    }
}

fn format_datetime(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn markdown_code_block(text: &str) -> String {
    let longest_backtick_run = longest_repeated_char_run(text, '`');
    let fence_len = if longest_backtick_run >= 3 {
        longest_backtick_run + 1
    } else {
        3
    };
    let fence = "`".repeat(fence_len);
    format!("{fence}text\n{text}\n{fence}")
}

fn longest_repeated_char_run(text: &str, needle: char) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for ch in text.chars() {
        if ch == needle {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

#[cfg(test)]
#[path = "session_search_tests.rs"]
mod session_search_tests;
