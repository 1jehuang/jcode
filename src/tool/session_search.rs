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
use serde_json::{json, Value};
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

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }

    fn description(&self) -> &str {
        "Search across all past chat sessions to find relevant context, code snippets, \
         or previous discussions. Use this when you need to recall something from a \
         previous conversation that might be helpful for the current task."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search term to find in past sessions"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional: only search sessions from this directory"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results to return (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: SearchInput = serde_json::from_value(input)?;
        let limit = params.limit.unwrap_or(10);
        let query_lower = params.query.to_lowercase();
        let wd_filter = params.working_dir.clone();

        let sessions_dir = storage::jcode_dir()?.join("sessions");
        if !sessions_dir.exists() {
            return Ok(ToolOutput::new("No past sessions found."));
        }

        let results = tokio::task::spawn_blocking(move || {
            search_sessions_blocking(&sessions_dir, &query_lower, wd_filter.as_deref(), limit)
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
    query_lower: &str,
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

                    if !contains_case_insensitive_bytes(&raw, query_lower.as_bytes()) {
                        continue;
                    }

                    if let Some(ref wd) = wd_lower {
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
                        for block in &msg.content {
                            let text = match block {
                                crate::message::ContentBlock::Text { text, .. } => text,
                                crate::message::ContentBlock::ToolResult { content, .. } => {
                                    content
                                }
                                _ => continue,
                            };

                            let text_lower = text.to_lowercase();
                            if !text_lower.contains(query_lower) {
                                continue;
                            }

                            let snippet =
                                extract_snippet(text, &text_lower, query_lower, 200);
                            let role = match msg.role {
                                crate::message::Role::User => "user",
                                crate::message::Role::Assistant => "assistant",
                            };

                            let match_count = text_lower.matches(query_lower).count();
                            let score = match_count as f64 / (text.len() as f64 + 1.0);

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
                }

                results
            }));
        }

        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    // Merge results from all threads
    let mut results: Vec<SearchResult> =
        all_results.into_iter().flatten().collect();

    // Sort by score descending, take top `limit`
    results.sort_unstable_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);

    Ok(results)
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
fn extract_snippet(text: &str, text_lower: &str, query: &str, max_len: usize) -> String {
    if let Some(pos) = text_lower.find(query) {
        let start = pos.saturating_sub(max_len / 2);
        let end = (pos + query.len() + max_len / 2).min(text.len());

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
