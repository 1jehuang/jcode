#![cfg_attr(test, allow(clippy::await_holding_lock))]

//! Conversation search tool - RAG for compacted conversation history

use super::{Tool, ToolContext, ToolOutput};
use crate::compaction::CompactionManager;
use crate::message::{Message, Role};
use crate::session::Session;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Deserialize)]
struct SearchInput {
    /// Search query (keyword search)
    #[serde(default)]
    query: Option<String>,

    /// Get specific turns by range
    #[serde(default)]
    turns: Option<TurnRange>,

    /// Get stats about conversation
    #[serde(default)]
    stats: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct TurnRange {
    start: usize,
    end: usize,
}

pub struct ConversationSearchTool {
    compaction: Arc<RwLock<CompactionManager>>,
}

impl ConversationSearchTool {
    pub fn new(compaction: Arc<RwLock<CompactionManager>>) -> Self {
        Self { compaction }
    }
}

#[async_trait]
impl Tool for ConversationSearchTool {
    fn name(&self) -> &str {
        "conversation_search"
    }

    fn description(&self) -> &str {
        "Search conversation history."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "query": {
                    "type": "string",
                    "description": "Search query."
                },
                "turns": {
                    "type": "object",
                    "properties": {
                        "start": {"type": "integer", "description": "Start turn."},
                        "end": {"type": "integer", "description": "End turn."}
                    },
                    "required": ["start", "end"],
                    "description": "Turn range."
                },
                "stats": {
                    "type": "boolean",
                    "description": "Return stats."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: SearchInput = serde_json::from_value(input)?;
        let manager = self.compaction.read().await;
        let session_messages = load_session_messages(&ctx.session_id);
        if session_messages.is_none() {
            crate::logging::warn(&format!(
                "[tool:conversation_search] failed to load session history for session {}",
                ctx.session_id
            ));
        }

        let mut output = String::new();

        // Handle stats request
        if params.stats == Some(true) {
            let stats = manager.stats();
            output.push_str(&format!(
                "## Conversation Stats\n\n\
                 - Total turns: {}\n\
                 - Active messages in context: {}\n\
                 - Has summary: {}\n\
                 - Compaction in progress: {}\n\
                 - Estimated tokens: {}\n\
                 - Context usage: {:.1}%\n",
                stats.total_turns,
                stats.active_messages,
                stats.has_summary,
                stats.is_compacting,
                stats.token_estimate,
                stats.context_usage * 100.0
            ));
        }

        // Handle keyword search
        if let Some(query) = params.query {
            let results = session_messages
                .as_deref()
                .map(|messages| search_messages(messages, &query))
                .unwrap_or_default();

            if results.is_empty() {
                output.push_str(&format!(
                    "## Search Results\n\nNo results found for '{}'\n",
                    query
                ));
            } else {
                output.push_str(&format!(
                    "## Search Results for '{}'\n\nFound {} matches:\n\n",
                    query,
                    results.len()
                ));

                for result in results.iter().take(10) {
                    let role = match result.role {
                        Role::User => "User",
                        Role::Assistant => "Assistant",
                    };
                    output.push_str(&format!(
                        "**Turn {} ({}):**\n{}\n\n",
                        result.turn, role, result.snippet
                    ));
                }

                if results.len() > 10 {
                    crate::logging::warn(&format!(
                        "[tool:conversation_search] truncating displayed search results for session {} query={} total_results={}",
                        ctx.session_id,
                        query,
                        results.len()
                    ));
                    output.push_str(&format!("... and {} more results\n", results.len() - 10));
                }
            }
        }

        // Handle turn range request
        if let Some(range) = params.turns {
            let turns = session_messages.as_deref().map(|messages| {
                messages
                    .iter()
                    .skip(range.start)
                    .take(range.end.saturating_sub(range.start))
                    .collect::<Vec<_>>()
            });

            if turns.as_ref().map(|t| t.is_empty()).unwrap_or(true) {
                output.push_str(&format!(
                    "## Turns {}-{}\n\nNo turns found in that range.\n",
                    range.start, range.end
                ));
            } else if let Some(turns) = turns {
                output.push_str(&format!("## Turns {}-{}\n\n", range.start, range.end));

                for (idx, msg) in turns.iter().enumerate() {
                    let turn_num = range.start + idx;
                    let role = match msg.role {
                        Role::User => "User",
                        Role::Assistant => "Assistant",
                    };

                    output.push_str(&format!("**Turn {} ({}):**\n", turn_num, role));

                    for block in &msg.content {
                        match block {
                            crate::message::ContentBlock::Text { text, .. } => {
                                // Truncate very long messages
                                if text.len() > 1000 {
                                    output.push_str(crate::util::truncate_str(text, 1000));
                                    output.push_str("... (truncated)\n");
                                } else {
                                    output.push_str(text);
                                    output.push('\n');
                                }
                            }
                            crate::message::ContentBlock::ToolUse { name, .. } => {
                                output.push_str(&format!("[Tool call: {}]\n", name));
                            }
                            crate::message::ContentBlock::ToolResult { content, .. } => {
                                let preview = if content.len() > 200 {
                                    format!("{}...", crate::util::truncate_str(content, 200))
                                } else {
                                    content.clone()
                                };
                                output.push_str(&format!("[Tool result: {}]\n", preview));
                            }
                            crate::message::ContentBlock::Reasoning { .. }
                            | crate::message::ContentBlock::ReasoningTrace { .. }
                            | crate::message::ContentBlock::AnthropicThinking { .. }
                            | crate::message::ContentBlock::OpenAIReasoning { .. }
                            | crate::message::ContentBlock::ToolReference { .. }
                            | crate::message::ContentBlock::ProviderNative { .. } => {}
                            crate::message::ContentBlock::Image { .. } => {
                                output.push_str("[Image]\n");
                            }
                            crate::message::ContentBlock::OpenAICompaction { .. } => {
                                output.push_str("[OpenAI native compaction]\n");
                            }
                        }
                    }
                    output.push('\n');
                }
            }
        }

        if output.is_empty() {
            output = "Please provide a 'query' to search, 'turns' range to retrieve, \
                      or 'stats': true to see conversation statistics."
                .to_string();
        }

        Ok(ToolOutput::new(output).with_title("conversation_search"))
    }
}

/// Search result from conversation history
struct SearchResult {
    turn: usize,
    role: Role,
    snippet: String,
}

fn load_session_messages(session_id: &str) -> Option<Vec<Message>> {
    let session = Session::load(session_id).ok()?;
    Some(
        session
            .messages
            .into_iter()
            .map(|msg| msg.to_message())
            .collect(),
    )
}

fn search_messages(messages: &[Message], query: &str) -> Vec<SearchResult> {
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();

    for (idx, msg) in messages.iter().enumerate() {
        let text = message_to_text(msg);
        let lower = text.to_lowercase();
        if lower.contains(&query_lower) {
            let snippet = snippet_from_lower(&text, &lower, &query_lower);
            results.push(SearchResult {
                turn: idx,
                role: msg.role.clone(),
                snippet,
            });
        }
    }

    results
}

fn message_to_text(msg: &Message) -> String {
    msg.content
        .iter()
        .filter_map(|block| match block {
            crate::message::ContentBlock::Text { text, .. } => Some(text.clone()),
            crate::message::ContentBlock::ToolResult { content, .. } => Some(content.clone()),
            crate::message::ContentBlock::OpenAICompaction { .. } => {
                Some("[OpenAI native compaction]".to_string())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
fn extract_snippet(text: &str, query: &str) -> String {
    snippet_from_lower(text, &text.to_lowercase(), query)
}

/// Build a snippet around the first match of `query` in `lower`, which must be
/// `text.to_lowercase()`: the exact copy the search matched against, so a hit
/// found by the search (including context-sensitive final sigma) is always
/// found here too.
fn snippet_from_lower(text: &str, lower: &str, query: &str) -> String {
    let Some(pos) = lower.find(query) else {
        return text.chars().take(100).collect();
    };
    let (match_start, match_end) = map_lower_range(text, pos, pos + query.len());
    // 50 bytes of context each side, widened to whole characters so the
    // slice never splits a multi-byte char like '·'.
    let mut start = match_start.saturating_sub(50);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (match_end + 50).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let mut snippet = text[start..end].to_string();
    if start > 0 {
        snippet = format!("...{}", snippet);
    }
    if end < text.len() {
        snippet = format!("{}...", snippet);
    }
    snippet
}

/// Map the byte range `lo..hi` of `text.to_lowercase()` back to a byte range
/// of `text` covering the same characters.
///
/// `str::to_lowercase` lowercases each char like `char::to_lowercase`, except
/// 'Σ', which becomes 'σ' or word-final 'ς'. Both are 2 bytes, so each char's
/// lowercased byte length can be computed on its own. Walking `text` while
/// summing those lengths gives the mapping with O(1) extra memory, and the
/// walk stops as soon as it passes the end of the match.
fn map_lower_range(text: &str, lo: usize, hi: usize) -> (usize, usize) {
    let mut lower_offset = 0;
    let mut start = None;
    for (idx, ch) in text.char_indices() {
        if lower_offset >= hi {
            return (start.unwrap_or(idx), idx);
        }
        let lowered_len: usize = ch.to_lowercase().map(char::len_utf8).sum();
        if start.is_none() && lower_offset + lowered_len > lo {
            start = Some(idx);
        }
        lower_offset += lowered_len;
    }
    (start.unwrap_or(text.len()), text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::CompactionManager;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn create_test_tool() -> ConversationSearchTool {
        let manager = Arc::new(RwLock::new(CompactionManager::new()));
        ConversationSearchTool::new(manager)
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::storage::lock_test_env()
    }

    fn setup_session(messages: Vec<Message>) -> (ToolContext, std::path::PathBuf, Option<String>) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("jcode-test-{}", nonce));
        let _ = std::fs::create_dir_all(base.join("sessions"));

        let previous_home = std::env::var("JCODE_HOME").ok();
        crate::env::set_var("JCODE_HOME", &base);

        let session_id = format!("test-session-{}", nonce);
        let mut session = Session::create_with_id(session_id.clone(), None, None);
        for msg in messages {
            session.add_message(msg.role.clone(), msg.content.clone());
        }
        session.save().unwrap();

        let ctx = ToolContext {
            session_id,
            message_id: "test-message".to_string(),
            tool_call_id: "test-tool-call".to_string(),
            working_dir: None,
            stdin_request_tx: None,
            decision_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        };

        (ctx, base, previous_home)
    }

    fn restore_env(base: std::path::PathBuf, previous_home: Option<String>) {
        if let Some(prev) = previous_home {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn test_tool_name() {
        let tool = create_test_tool();
        assert_eq!(tool.name(), "conversation_search");
    }

    #[tokio::test]
    async fn test_stats() {
        let _guard = env_lock();
        let tool = create_test_tool();
        let (ctx, base, previous_home) = setup_session(Vec::new());
        let input = json!({"stats": true});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("Conversation Stats"));
        assert!(result.output.contains("Total turns"));
        restore_env(base, previous_home);
    }

    #[tokio::test]
    async fn test_empty_search() {
        let _guard = env_lock();
        let tool = create_test_tool();
        let (ctx, base, previous_home) = setup_session(Vec::new());
        let input = json!({"query": "nonexistent"});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("No results found"));
        restore_env(base, previous_home);
    }

    #[tokio::test]
    async fn test_empty_turns() {
        let _guard = env_lock();
        let tool = create_test_tool();
        let (ctx, base, previous_home) = setup_session(Vec::new());
        let input = json!({"turns": {"start": 0, "end": 5}});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("No turns found"));
        restore_env(base, previous_home);
    }

    /// Snippet windows used to be cut at raw byte offsets and panicked when
    /// they landed inside a multi-byte char (seen live on a '·' in tool
    /// output). Every offset around the match must slice cleanly.
    #[test]
    fn snippet_never_splits_multibyte_chars() {
        // Pad right next to the match on each side so the 50-byte window
        // edges sweep every byte offset of the 7-byte "a · b " pattern,
        // including the middle of the 2-byte '·'.
        for before in 0..7 {
            for after in 0..7 {
                let text = format!(
                    "{}{}apiduck{}{}",
                    "a · b ".repeat(20),
                    "x".repeat(before),
                    "y".repeat(after),
                    " · end".repeat(20)
                );
                let snippet = extract_snippet(&text, "apiduck");
                assert!(snippet.contains("apiduck"), "{before}/{after}: {snippet}");
                assert!(snippet.starts_with("..."), "{before}/{after}");
                assert!(snippet.ends_with("..."), "{before}/{after}");
            }
        }
        // Lowercasing can change byte lengths ('İ' is 2 bytes, 3 lowercased),
        // so an offset found in the lowercased copy is not an offset in the
        // original text. The match must still map back to it.
        let text = format!("{}Needle{}", "İ".repeat(100), "İ".repeat(40));
        let snippet = extract_snippet(&text, "needle");
        assert!(snippet.contains("Needle"), "{snippet}");
        assert_eq!(extract_snippet("no match here", "zzz"), "no match here");
    }

    #[test]
    fn snippet_finds_word_final_sigma_like_search() {
        // The search lowercases the whole message, so "ΟΔΟΣ" becomes "οδος"
        // with a word-final 'ς'. Lowercasing char by char would give "οδοσ"
        // instead, and the snippet would fall back to the first 100 chars.
        let text = format!("{} ΟΔΟΣ {}", "x".repeat(200), "y".repeat(200));
        let query = "ος".to_lowercase();
        assert!(text.to_lowercase().contains(&query));
        let snippet = extract_snippet(&text, &query);
        assert!(snippet.contains("ΟΔΟΣ"), "{snippet}");
        assert!(snippet.starts_with("...") && snippet.ends_with("..."));
    }

    #[test]
    fn snippet_from_large_message_is_short_and_contains_hit() {
        // 4 MiB of mixed-width text with the hit in the middle. The offset
        // mapping walks chars with O(1) extra state instead of building a
        // per-byte table, so this stays cheap in memory.
        let half = "ab·İΣ ".repeat(4 * 1024 * 1024 / 2 / 9);
        let text = format!("{half}HiddenNeedle{half}");
        let snippet = extract_snippet(&text, "hiddenneedle");
        assert!(snippet.contains("HiddenNeedle"), "{snippet}");
        assert!(snippet.len() < 200, "{}", snippet.len());
    }
}
