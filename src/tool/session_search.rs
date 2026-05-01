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
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
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
    session_id: String,
    short_name: Option<String>,
    title: Option<String>,
    working_dir: Option<String>,
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

#[derive(Debug, Clone)]
struct QueryProfile {
    normalized: String,
    terms: Vec<String>,
    min_term_matches: usize,
}

impl QueryProfile {
    fn new(query: &str) -> Self {
        let normalized = query.trim().to_lowercase();
        let terms = tokenize_query(&normalized);
        let min_term_matches = minimum_term_matches(terms.len());
        Self {
            normalized,
            terms,
            min_term_matches,
        }
    }

    fn is_empty(&self) -> bool {
        self.normalized.is_empty()
    }

    fn is_actionable(&self) -> bool {
        !self.is_empty() && !self.terms.is_empty()
    }
}

#[derive(Debug)]
struct MatchScore {
    snippet: String,
    score: f64,
    matched_terms: Vec<String>,
    exact_match: bool,
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
        if !sessions_dir.exists() {
            return Ok(ToolOutput::new("No past sessions found.").with_title("session_search"));
        }

        let options = SearchOptions {
            current_session_id: ctx.session_id.clone(),
            working_dir_filter: params.working_dir.clone(),
            limit,
            max_per_session,
            include_current: params.include_current.unwrap_or(false),
            include_tools: params.include_tools.unwrap_or(false),
            include_system: params.include_system.unwrap_or(false),
        };

        let results = tokio::task::spawn_blocking({
            let session_id = ctx.session_id.clone();
            let query = query.clone();
            let options = options.clone();
            move || search_sessions_blocking(&sessions_dir, &query, &options, &session_id)
        })
        .await??;

        if results.is_empty() {
            return Ok(ToolOutput::new(no_results_message(&params.query, &options))
                .with_title("session_search"));
        }

        Ok(
            ToolOutput::new(format_results(&params.query, &results, &options))
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

/// Synchronous search across session files with parallel raw pre-filtering and
/// journal-aware session loading.
fn search_sessions_blocking(
    sessions_dir: &Path,
    query: &QueryProfile,
    options: &SearchOptions,
    log_session_id: &str,
) -> Result<Vec<SearchResult>> {
    if !query.is_actionable() {
        return Ok(Vec::new());
    }

    let mut files = collect_session_files(sessions_dir)?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    files.sort_unstable_by(|a, b| b.mtime.cmp(&a.mtime));

    if !options.include_current {
        files.retain(|candidate| candidate.session_id_hint != options.current_session_id);
    }
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let raw_filter_outcomes = filter_candidates_parallel(&files, query);
    let read_errors: usize = raw_filter_outcomes
        .iter()
        .map(|outcome| outcome.read_errors)
        .sum();
    let mut candidates: Vec<SessionFileCandidate> = raw_filter_outcomes
        .into_iter()
        .flat_map(|outcome| outcome.candidates)
        .collect();
    candidates.sort_unstable_by(|a, b| b.mtime.cmp(&a.mtime));
    candidates.truncate(MAX_DESERIALIZE);

    let search_outcomes = score_candidates_parallel(&candidates, query, options);
    let parse_errors: usize = search_outcomes
        .iter()
        .map(|outcome| outcome.parse_errors)
        .sum();

    if read_errors > 0 || parse_errors > 0 {
        crate::logging::warn(&format!(
            "[tool:session_search] skipped unreadable or invalid session files in session {} (read_errors={} parse_errors={})",
            log_session_id, read_errors, parse_errors
        ));
    }

    let mut results: Vec<SearchResult> = search_outcomes
        .into_iter()
        .flat_map(|outcome| outcome.results)
        .collect();

    results.sort_unstable_by(compare_results);
    Ok(group_and_limit_results(results, options))
}

fn collect_session_files(sessions_dir: &Path) -> Result<Vec<SessionFileCandidate>> {
    let mut files = Vec::new();
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

fn append_session_results(
    results: &mut Vec<SearchResult>,
    session: &Session,
    query: &QueryProfile,
    options: &SearchOptions,
) {
    if !options.include_current && session.id == options.current_session_id {
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

    if let Some(match_score) = score_message_match(&metadata_text(session), query) {
        results.push(SearchResult {
            session_id: session.id.clone(),
            short_name: session.short_name.clone(),
            title: session.title.clone(),
            working_dir: session.working_dir.clone(),
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
        });
    }

    for (message_index, msg) in session.messages.iter().enumerate() {
        if !options.include_system && is_system_like_message(msg) {
            continue;
        }
        if is_tool_only_message(msg) && !options.include_tools {
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
            session_id: session.id.clone(),
            short_name: session.short_name.clone(),
            title: session.title.clone(),
            working_dir: session.working_dir.clone(),
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

fn score_message_match(text: &str, query: &QueryProfile) -> Option<MatchScore> {
    if !query.is_actionable() {
        return None;
    }

    let text_lower = text.to_lowercase();
    let exact_pos = (!query.normalized.is_empty())
        .then(|| text_lower.find(&query.normalized))
        .flatten();

    let mut matched_terms = Vec::new();
    let mut total_term_hits = 0usize;
    let mut first_term_pos = None;

    for term in &query.terms {
        if let Some(pos) = text_lower.find(term) {
            matched_terms.push(term.clone());
            total_term_hits += text_lower.matches(term).count();
            first_term_pos = Some(first_term_pos.map_or(pos, |current: usize| current.min(pos)));
        }
    }

    if exact_pos.is_none() && matched_terms.len() < query.min_term_matches {
        return None;
    }

    let anchor = exact_pos.or(first_term_pos);
    let snippet = extract_snippet(text, anchor, query, 280);
    let coverage = matched_terms.len() as f64 / query.terms.len() as f64;
    let score = if exact_pos.is_some() { 4.0 } else { 0.0 }
        + coverage * 3.0
        + matched_terms.len() as f64 * 0.25
        + (total_term_hits as f64 / (text.len() as f64 + 1.0)) * 200.0;

    Some(MatchScore {
        snippet,
        score,
        matched_terms,
        exact_match: exact_pos.is_some(),
    })
}

fn raw_matches_query(raw: &[u8], query: &QueryProfile) -> bool {
    if !query.is_actionable() {
        return false;
    }

    if query.normalized.is_ascii() {
        if contains_case_insensitive_bytes(raw, query.normalized.as_bytes()) {
            return true;
        }
        let matched_terms = query
            .terms
            .iter()
            .filter(|term| contains_case_insensitive_bytes(raw, term.as_bytes()))
            .count();
        return matched_terms >= query.min_term_matches;
    }

    let Ok(raw_text) = std::str::from_utf8(raw) else {
        return false;
    };
    normalized_text_matches(&raw_text.to_lowercase(), query)
}

fn path_matches_query(path_text: &str, query: &QueryProfile) -> bool {
    normalized_text_matches(&path_text.to_lowercase(), query)
}

fn normalized_text_matches(text_lower: &str, query: &QueryProfile) -> bool {
    if !query.is_actionable() {
        return false;
    }
    if text_lower.contains(&query.normalized) {
        return true;
    }
    query
        .terms
        .iter()
        .filter(|term| text_lower.contains(term.as_str()))
        .count()
        >= query.min_term_matches
}

fn tokenize_query(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut seen = HashSet::new();

    for token in query.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }

        let token = token.to_lowercase();
        if is_stop_word(&token) {
            continue;
        }

        let keep = token.chars().count() >= 2 || token.chars().all(|c| c.is_ascii_digit());
        if keep && seen.insert(token.clone()) {
            terms.push(token);
        }
    }

    terms
}

fn is_stop_word(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "but"
            | "by"
            | "for"
            | "from"
            | "how"
            | "i"
            | "in"
            | "into"
            | "is"
            | "it"
            | "my"
            | "of"
            | "on"
            | "or"
            | "our"
            | "that"
            | "the"
            | "their"
            | "this"
            | "to"
            | "we"
            | "what"
            | "when"
            | "where"
            | "which"
            | "with"
            | "you"
            | "your"
    )
}

fn minimum_term_matches(term_count: usize) -> usize {
    match term_count {
        0 => 0,
        1 => 1,
        2 => 2,
        3..=5 => 2,
        _ => 3,
    }
}

/// Fast case-insensitive byte search. Avoids allocating a lowercase copy of the
/// entire file for the common ASCII-query case.
fn contains_case_insensitive_bytes(haystack: &[u8], needle_lower: &[u8]) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.len() < needle_lower.len() {
        return false;
    }
    let end = haystack.len() - needle_lower.len();
    'outer: for i in 0..=end {
        for (j, &nb) in needle_lower.iter().enumerate() {
            let hb = haystack[i + j];
            let hb_lower = if hb.is_ascii_uppercase() {
                hb | 0x20
            } else {
                hb
            };
            if hb_lower != nb {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

fn working_dir_matches(session_wd: &str, filter: &str) -> bool {
    let session_norm = normalize_path_for_match(session_wd);
    let filter_norm = normalize_path_for_match(filter);
    if filter_norm.is_empty() {
        return true;
    }

    if session_norm == filter_norm {
        return true;
    }

    let filter_with_sep = format!("{filter_norm}/");
    if session_norm.starts_with(&filter_with_sep) {
        return true;
    }

    // If the user supplied only a project name or path fragment, keep substring
    // matching as a fallback. This preserves the previous loose behavior while
    // making absolute path filters deterministic above.
    !filter_norm.contains('/') && session_norm.contains(&filter_norm)
}

fn normalize_path_for_match(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
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

fn format_results(query: &str, results: &[SearchResult], options: &SearchOptions) -> String {
    let mut output = format!(
        "## Found {} results for '{}'\n\n",
        results.len(),
        query.trim()
    );

    output.push_str(&format!(
        "_Defaults: current session {}, tool calls/results {}, system reminders {}. Max per session: {}._\n\n",
        if options.include_current { "included" } else { "excluded" },
        if options.include_tools { "included" } else { "hidden" },
        if options.include_system { "included" } else { "hidden" },
        options.max_per_session,
    ));

    for (i, result) in results.iter().enumerate() {
        let session_name = result
            .short_name
            .as_deref()
            .or(result.title.as_deref())
            .unwrap_or(&result.session_id);
        output.push_str(&format!("### Result {} - {}\n", i + 1, session_name));
        output.push_str(&format!("- Session ID: `{}`\n", result.session_id));
        if let Some(title) = &result.title {
            output.push_str(&format!("- Title: {}\n", title));
        }
        if let Some(dir) = &result.working_dir {
            output.push_str(&format!("- Working dir: `{}`\n", dir));
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

/// Extract a snippet around the first match.
fn extract_snippet(
    text: &str,
    anchor: Option<usize>,
    query: &QueryProfile,
    max_len: usize,
) -> String {
    if let Some(pos) = anchor {
        let focus_len = if !query.normalized.is_empty() {
            query.normalized.len()
        } else {
            query.terms.first().map(|term| term.len()).unwrap_or(0)
        };
        let start = pos.saturating_sub(max_len / 2);
        let end = (pos + focus_len + max_len / 2).min(text.len());

        let start = floor_char_boundary(text, start);
        let end = ceil_char_boundary(text, end);

        let start = text[..start]
            .rfind(char::is_whitespace)
            .map(|p| p + 1)
            .unwrap_or(start);
        let end = text[end..]
            .find(char::is_whitespace)
            .map(|p| end + p)
            .unwrap_or(end);

        let mut snippet = text[start..end].to_string();
        if start > 0 {
            snippet = format!("...{}", snippet);
        }
        if end < text.len() {
            snippet = format!("{}...", snippet);
        }
        snippet
    } else {
        text.chars().take(max_len).collect()
    }
}

fn floor_char_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut idx = i;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut idx = i;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

#[cfg(test)]
#[path = "session_search_tests.rs"]
mod session_search_tests;
