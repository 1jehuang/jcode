//! Cross-session search tool - RAG across all past sessions

use super::{Tool, ToolContext, ToolOutput};
use crate::session::Session;
use crate::storage;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct SearchInput {
    /// Search query
    query: String,
    /// Only search sessions from this working directory
    #[serde(default)]
    working_dir: Option<String>,
    /// Maximum results to return
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

/// A search result from a past session
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

        // Get sessions directory
        let sessions_dir = storage::jcode_dir()?.join("sessions");
        if !sessions_dir.exists() {
            return Ok(ToolOutput::new("No past sessions found."));
        }

        let mut results: Vec<SearchResult> = Vec::new();

        // Iterate through session files
        let entries = std::fs::read_dir(&sessions_dir)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                // Try to load the session
                if let Ok(session) = storage::read_json::<Session>(&path) {
                    // Filter by working directory if specified
                    if let Some(ref wd_filter) = params.working_dir {
                        if let Some(ref session_wd) = session.working_dir {
                            if !session_wd.contains(wd_filter) {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    }

                    // Search through messages
                    for msg in &session.messages {
                        for block in &msg.content {
                            let text = match block {
                                crate::message::ContentBlock::Text { text, .. } => text,
                                crate::message::ContentBlock::ToolResult { content, .. } => content,
                                _ => continue,
                            };

                            if text.to_lowercase().contains(&query_lower) {
                                // Extract relevant snippet around the match
                                let snippet = extract_snippet(text, &query_lower, 200);
                                let role = match msg.role {
                                    crate::message::Role::User => "user",
                                    crate::message::Role::Assistant => "assistant",
                                };

                                // Simple scoring: shorter snippets with more matches score higher
                                let match_count = text.to_lowercase().matches(&query_lower).count();
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
                }
            }
        }

        if results.is_empty() {
            return Ok(ToolOutput::new(format!(
                "No results found for '{}' in past sessions.",
                params.query
            )));
        }

        // Sort by score descending
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top results
        let results: Vec<_> = results.into_iter().take(limit).collect();

        // Format output
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

/// Extract a snippet around the first match
fn extract_snippet(text: &str, query: &str, max_len: usize) -> String {
    let text_lower = text.to_lowercase();
    if let Some(pos) = text_lower.find(query) {
        let start = pos.saturating_sub(max_len / 2);
        let end = (pos + query.len() + max_len / 2).min(text.len());

        // Find word boundaries
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
