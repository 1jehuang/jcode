//! Micro-compaction integration for the agent turn loop.
//!
//! Before each API call, replaces old tool results (Read, Bash, Grep, etc.)
//! with short placeholders to reclaim context-window tokens without LLM
//! summarization.

use std::collections::HashMap;
use std::time::Duration;

use jcode_micro_compact::{is_compactable, TimeBasedTrigger};
use jcode_message_types::ContentBlock;
use jcode_session_types::StoredMessage;

use crate::logging;

/// Default TTL between micro-compaction runs (60 seconds).
const DEFAULT_MC_TTL: Duration = Duration::from_secs(60);

/// Minimum content length (chars) before a tool result is worth compacting.
/// Results shorter than this are already cheap enough to keep as-is.
const MIN_CONTENT_LEN: usize = 200;

/// Create a new `TimeBasedTrigger` with the default TTL.
pub fn new_trigger() -> TimeBasedTrigger {
    TimeBasedTrigger::new(DEFAULT_MC_TTL)
}

/// Maybe micro-compact the session's message history.
///
/// Checks the time-based trigger; if it fires, scans for compactable tool
/// results and replaces their content with short placeholders. The session's
/// provider-message cache is invalidated via `replace_messages` so the next
/// `messages_for_provider()` call reflects the compacted history.
pub fn maybe_compact(session: &mut crate::session::Session, trigger: &mut TimeBasedTrigger) {
    if !trigger.should_compact() {
        return;
    }

    // Clone messages so we can modify and replace (invalidates provider cache).
    let mut messages = session.messages.clone();
    let compacted = compact_messages(&mut messages);

    if compacted > 0 {
        session.replace_messages(messages);
        logging::info(&format!(
            "Micro-compacted {} tool results",
            compacted
        ));
    }

    // Always record the compaction point so we don't re-check every turn.
    trigger.record_compaction();
}

/// Replace compactable tool-result content blocks with short placeholders.
///
/// Returns the number of blocks that were compacted.
fn compact_messages(messages: &mut [StoredMessage]) -> usize {
    // Build tool_use_id -> tool_name map from assistant ToolUse blocks.
    let mut tool_map: HashMap<String, String> = HashMap::new();
    for msg in messages.iter() {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                tool_map.insert(id.clone(), name.clone());
            }
        }
    }

    let mut compacted = 0usize;
    for msg in messages.iter_mut() {
        for block in msg.content.iter_mut() {
            if let ContentBlock::ToolResult {
                tool_use_id, content, ..
            } = block
            {
                let tool_name = match tool_map.get(tool_use_id.as_str()) {
                    Some(name) => name.as_str(),
                    None => continue,
                };
                if !is_compactable(tool_name) {
                    continue;
                }
                if content.len() <= MIN_CONTENT_LEN {
                    continue;
                }
                *content = format!(
                    "[micro-compacted {} result ({} chars)]",
                    tool_name,
                    content.len()
                );
                compacted += 1;
            }
        }
    }

    compacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_session_types::{Role, StoredMessage};

    fn tool_use_msg(id: &str, name: &str) -> StoredMessage {
        StoredMessage {
            id: format!("msg-{id}"),
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: serde_json::json!({}),
                thought_signature: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }
    }

    fn tool_result_msg(tool_use_id: &str, content: &str) -> StoredMessage {
        StoredMessage {
            id: format!("result-{tool_use_id}"),
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }
    }

    #[test]
    fn compacts_large_read_result() {
        let long_content = "x".repeat(500);
        let mut messages = vec![
            tool_use_msg("t1", "Read"),
            tool_result_msg("t1", &long_content),
        ];
        let compacted = compact_messages(&mut messages);
        assert_eq!(compacted, 1);
        match &messages[1].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("micro-compacted"));
                assert!(content.contains("Read"));
                assert!(content.contains("500 chars"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn skips_short_content() {
        let short = "short";
        let mut messages = vec![
            tool_use_msg("t1", "Read"),
            tool_result_msg("t1", short),
        ];
        let compacted = compact_messages(&mut messages);
        assert_eq!(compacted, 0);
    }

    #[test]
    fn skips_non_compactable_tool() {
        let long = "y".repeat(500);
        let mut messages = vec![
            tool_use_msg("t1", "Agent"),
            tool_result_msg("t1", &long),
        ];
        let compacted = compact_messages(&mut messages);
        assert_eq!(compacted, 0);
    }
}
