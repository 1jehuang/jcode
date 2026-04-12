//! Cross-session search tool - RAG across all past sessions
//!
//! Performance notes:
//! - Phase 1: collect file paths, sort by mtime (newest first)
//! - Phase 2: parallel raw-text pre-filter (case-insensitive grep on file bytes, no JSON parse)
//! - Phase 3: only deserialize files that passed the pre-filter
//! - Parallel I/O via std::thread::scope for file scanning
//! - Single .to_lowercase() per text block, early termination

use super::{Tool, ToolContext, ToolOutput};
use crate::session::Session;
use crate::storage;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;

/// Max files to deserialize even if more pass the raw-text filter.
const MAX_DESERIALIZE: usize = 200;

/// Max candidate results to collect before we stop scanning files.
const MAX_CANDIDATES: usize = 500;

/// Number of parallel threads for file scanning.
const SCAN_THREADS: usize = 8;

#[derive(Debug, Deserialize)]
struct SearchInput {
    query: String,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
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

struct SearchResult {
    session_id: String,
    short_name: Option<String>,
    working_dir: Option<String>,
    role: String,
    snippet: String,
    score: f64,
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
        self.normalized.is_empty() && self.terms.is_empty()
    }
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }

    fn description(&self) -> &str {
        "Search past chat sessions."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query."
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: SearchInput = serde_json::from_value(input)?;
        let limit = params.limit.unwrap_or(10);
        let query = QueryProfile::new(&params.query);
        let wd_filter = params.working_dir.clone();

        if query.is_empty() {
            return Ok(ToolOutput::new("Query cannot be empty."));
        }

        let sessions_dir = storage::jcode_dir()?.join("sessions");
        if !sessions_dir.exists() {
            return Ok(ToolOutput::new("No past sessions found."));
        }

        let results = tokio::task::spawn_blocking(move || {
            search_sessions_blocking(&sessions_dir, &query, wd_filter.as_deref(), limit)
        })
        .await??;

        if results.is_empty() {
            return Ok(ToolOutput::new(format!(
                "No results found for '{}' in past sessions.",
                params.query
            )));
        }

        let mut output = format!(
            "## Found {} results for '{}'\n\n",
            results.len(),
            params.query
        );

        for (i, result) in results.iter().enumerate() {
            let session_name = result.short_name.as_deref().unwrap_or(&result.session_id);
            let dir = result
                .working_dir
                .as_deref()
                .map(|d| format!(" ({})", d))
                .unwrap_or_default();

            output.push_str(&format!(
                "### Result {} - Session: {}{}\n**{}:**\n```\n{}\n```\n\n",
                i + 1,
                session_name,
                dir,
                result.role,
                result.snippet
            ));
        }

        Ok(ToolOutput::new(output).with_title("session_search"))
    }
}

/// Synchronous search across session files with parallel I/O.
fn search_sessions_blocking(
    sessions_dir: &std::path::Path,
    query: &QueryProfile,
    wd_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Phase 1: Collect file paths with mtime, sort newest-first
    let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in std::fs::read_dir(sessions_dir)?.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((path, mtime));
        }
    }
    files.sort_unstable_by(|a, b| b.1.cmp(&a.1));

    // Phase 2: Parallel scan - split files across threads for raw-text pre-filter + deserialization
    let wd_lower = wd_filter.map(|w| w.to_lowercase());
    let thread_count = SCAN_THREADS.min(files.len().max(1));
    let chunk_size = (files.len() + thread_count - 1) / thread_count.max(1);

    let all_results: Vec<Vec<SearchResult>> = std::thread::scope(|s| {
        let mut handles = Vec::new();

        for chunk in files.chunks(chunk_size) {
            let wd_lower = &wd_lower;
            let wd_filter = wd_filter;

            handles.push(s.spawn(move || {
                let mut results: Vec<SearchResult> = Vec::new();
                let mut deserialized = 0usize;

                for (path, _mtime) in chunk {
                    if results.len() >= MAX_CANDIDATES / thread_count {
                        break;
                    }

                    let raw = match std::fs::read(path) {
                        Ok(data) => data,
                        Err(_) => continue,
                    };

                    if !raw_matches_query(&raw, query) {
                        continue;
                    }

                    if let Some(wd) = wd_lower {
                        if !contains_case_insensitive_bytes(&raw, wd.as_bytes()) {
                            continue;
                        }
                    }

                    deserialized += 1;
                    if deserialized > MAX_DESERIALIZE / thread_count {
                        break;
                    }

                    let raw_str = match std::str::from_utf8(&raw) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };

                    let session: Session = match serde_json::from_str(raw_str) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };

                    if let Some(wd_filter) = wd_filter {
                        match &session.working_dir {
                            Some(session_wd) if session_wd.contains(wd_filter) => {}
                            _ => continue,
                        }
                    }

                    for msg in &session.messages {
                        let text = searchable_message_text(msg);
                        if text.is_empty() {
                            continue;
                        }

                        let Some((snippet, score)) = score_message_match(&text, query) else {
                            continue;
                        };

                        let role = match msg.role {
                            crate::message::Role::User => "user",
                            crate::message::Role::Assistant => "assistant",
                        };

                        results.push(SearchResult {
                            session_id: session.id.clone(),
                            short_name: session.short_name.clone(),
                            working_dir: session.working_dir.clone(),
                            role: role.to_string(),
                            snippet,
                            score,
                        });
                    }
                }

                results
            }));
        }

        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    // Merge results from all threads
    let mut results: Vec<SearchResult> = all_results.into_iter().flatten().collect();

    // Sort by score descending, take top `limit`
    results.sort_unstable_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);

    Ok(results)
}

fn searchable_message_text(msg: &crate::session::StoredMessage) -> String {
    msg.content
        .iter()
        .filter_map(|block| match block {
            crate::message::ContentBlock::Text { text, .. } => Some(text.clone()),
            crate::message::ContentBlock::ToolResult { content, .. } => Some(content.clone()),
            crate::message::ContentBlock::ToolUse { name, input, .. } => {
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

fn score_message_match(text: &str, query: &QueryProfile) -> Option<(String, f64)> {
    let text_lower = text.to_lowercase();
    let exact_pos = (!query.normalized.is_empty())
        .then(|| text_lower.find(&query.normalized))
        .flatten();

    let mut matched_terms = 0usize;
    let mut total_term_hits = 0usize;
    let mut first_term_pos = None;

    for term in &query.terms {
        if let Some(pos) = text_lower.find(term) {
            matched_terms += 1;
            total_term_hits += text_lower.matches(term).count();
            first_term_pos.get_or_insert(pos);
        }
    }

    if exact_pos.is_none() && matched_terms < query.min_term_matches {
        return None;
    }

    let anchor = exact_pos.or(first_term_pos);
    let snippet = extract_snippet(text, anchor, query, 200);
    let coverage = if query.terms.is_empty() {
        0.0
    } else {
        matched_terms as f64 / query.terms.len() as f64
    };
    let score = if exact_pos.is_some() { 4.0 } else { 0.0 }
        + coverage * 3.0
        + matched_terms as f64 * 0.25
        + (total_term_hits as f64 / (text.len() as f64 + 1.0)) * 200.0;

    Some((snippet, score))
}

fn raw_matches_query(raw: &[u8], query: &QueryProfile) -> bool {
    if query.is_empty() {
        return false;
    }

    if !query.normalized.is_empty()
        && contains_case_insensitive_bytes(raw, query.normalized.as_bytes())
    {
        return true;
    }

    let matched_terms = query
        .terms
        .iter()
        .filter(|term| contains_case_insensitive_bytes(raw, term.as_bytes()))
        .count();
    matched_terms >= query.min_term_matches
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

        let keep = token.chars().count() >= 2
            || token
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_alphanumeric());
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
/// entire file. Only handles ASCII case folding, which is fine for searching
/// session JSON (keys and most content are ASCII or the query itself is ASCII).
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

/// Extract a snippet around the first match. Takes pre-computed lowercase text
/// to avoid re-lowercasing.
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
mod tests {
    use super::*;
    use crate::message::{ContentBlock, Role};
    use crate::session::Session;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn with_temp_home<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp dir");
        let previous_home = std::env::var("JCODE_HOME").ok();
        crate::env::set_var("JCODE_HOME", temp.path());
        std::fs::create_dir_all(temp.path().join("sessions")).expect("create sessions dir");

        let result = f(temp.path());

        if let Some(previous_home) = previous_home {
            crate::env::set_var("JCODE_HOME", previous_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }

        result
    }

    fn save_test_session(messages: Vec<(Role, Vec<ContentBlock>)>) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut session = Session::create_with_id(format!("test-session-{nonce}"), None, None);
        session.short_name = Some("search-test".to_string());
        session.working_dir = Some("/tmp/project".to_string());
        for (role, content) in messages {
            session.add_message(role, content);
        }
        session.save().expect("save test session");
    }

    #[test]
    fn token_overlap_matches_when_exact_phrase_is_absent() {
        with_temp_home(|home| {
            save_test_session(vec![(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "Try reconnecting your AirPods after the Bluetooth audio drops."
                        .to_string(),
                    cache_control: None,
                }],
            )]);

            let query = QueryProfile::new("airpods reconnect bluetooth");
            let results = search_sessions_blocking(&home.join("sessions"), &query, None, 10)
                .expect("search succeeds");

            assert!(!results.is_empty(), "expected token-overlap match");
            assert!(results[0].snippet.to_lowercase().contains("airpods"));
        });
    }

    #[test]
    fn tool_use_input_is_searchable() {
        with_temp_home(|home| {
            save_test_session(vec![(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "websearch".to_string(),
                    input: json!({
                        "query": "best time post hackernews visibility upvotes"
                    }),
                }],
            )]);

            let query = QueryProfile::new("hackernews visibility upvotes");
            let results = search_sessions_blocking(&home.join("sessions"), &query, None, 10)
                .expect("search succeeds");

            assert!(!results.is_empty(), "expected tool input match");
            assert!(results[0].snippet.to_lowercase().contains("hackernews"));
        });
    }
}
