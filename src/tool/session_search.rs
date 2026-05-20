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
use chrono::{DateTime, NaiveDate, Utc};
use jcode_import_core::{
    ExternalMessageRecord, ExternalSessionRecord, ImportCoreResult, collect_recent_files_recursive,
    load_claude_external_messages, load_codex_external_session, load_opencode_external_session,
    load_pi_external_session,
};
use jcode_session_types::{
    SessionSearchContextLine as ResultContextLine, SessionSearchQueryProfile as QueryProfile,
    SessionSearchRenderOptions, SessionSearchReport as SearchReport,
    SessionSearchResult as SearchResult, SessionSearchResultKind as SearchResultKind,
    format_session_search_no_results, format_session_search_results,
    score_session_search_text_match as score_message_match,
    session_search_datetime_matches as session_datetime_matches,
    session_search_field_filter_matches as field_filter_matches,
    session_search_format_datetime as format_datetime,
    session_search_path_matches_query as path_matches_query,
    session_search_raw_matches_query as raw_matches_query,
    session_search_truncate_title_text as truncate_title_text,
    session_search_working_dir_matches as working_dir_matches,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
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

    if source_matches_filter("claude", options)
        && let Ok(sessions) =
            crate::import::list_claude_code_sessions_lazy(options.max_scan_sessions)
    {
        report.external_sources.push("claude");
        for session in sessions.into_iter().take(options.max_scan_sessions) {
            let path = PathBuf::from(&session.full_path);
            let messages = load_claude_external_messages(&path, options.include_tools);
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
    loader: fn(&Path, bool) -> ImportCoreResult<Option<ExternalSessionRecord>>,
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
        match loader(&path, options.include_tools) {
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
    let Ok(messages_base) = crate::storage::user_home_path(".local/share/opencode/storage/message")
    else {
        return;
    };
    for path in collect_recent_files_recursive(&root, "json", options.max_scan_sessions) {
        match load_opencode_external_session(
            &path,
            &messages_base,
            options.include_tools,
            options.max_scan_sessions,
        ) {
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
        && session_datetime_matches(session.updated_at, options.after, options.before)
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
        if !session_datetime_matches(
            msg.timestamp.unwrap_or(session.updated_at),
            options.after,
            options.before,
        ) {
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
        && session_datetime_matches(session.updated_at, options.after, options.before)
        && let Some(match_score) = score_message_match(&metadata_text(session), query)
    {
        results.push(SearchResult {
            source: "jcode".to_string(),
            session_id: session.id.clone(),
            short_name: session.short_name.clone(),
            title: session.display_title().map(ToOwned::to_owned),
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
        if !session_datetime_matches(
            msg.timestamp.unwrap_or(session.updated_at),
            options.after,
            options.before,
        ) {
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
            title: session.display_title().map(ToOwned::to_owned),
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
    if let Some(title) = session.display_title() {
        fields.push(format!("Title: {title}"));
    }
    if let Some(generated_title) = &session.title
        && session.custom_title.is_some()
        && Some(generated_title.as_str()) != session.display_title()
    {
        fields.push(format!("Generated title: {generated_title}"));
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

fn render_options(options: &SearchOptions) -> SessionSearchRenderOptions {
    SessionSearchRenderOptions {
        include_current: options.include_current,
        include_external: options.include_external,
        include_tools: options.include_tools,
        include_system: options.include_system,
        max_per_session: options.max_per_session,
        has_working_dir_filter: options.working_dir_filter.is_some(),
    }
}

fn format_results(query: &str, report: &SearchReport, options: &SearchOptions) -> String {
    format_session_search_results(query, report, &render_options(options))
}

fn no_results_message(query: &str, options: &SearchOptions) -> String {
    format_session_search_no_results(query, &render_options(options))
}

#[cfg(test)]
#[path = "session_search_tests.rs"]
mod session_search_tests;
