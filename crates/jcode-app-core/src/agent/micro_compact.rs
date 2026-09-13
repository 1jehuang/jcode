//! Integration shim for jcode-micro-compact.
//!
//! Runs micro-compaction before each provider request to reclaim tokens
//! from old tool results without LLM summarization.

use std::time::Duration;

use jcode_micro_compact::{
    CachedMcConfig, CachedMcPath, TimeBasedTrigger, estimate_message_tokens, is_compactable,
};

use crate::message::{ContentBlock, Message, Role};

/// Default TTL between micro-compaction runs (60 seconds).
const DEFAULT_MC_TTL: Duration = Duration::from_secs(60);

/// State for micro-compaction decisions across turns.
#[derive(Debug)]
pub struct MicroCompactState {
    trigger: TimeBasedTrigger,
    cached_path: CachedMcPath,
}

impl MicroCompactState {
    pub fn new() -> Self {
        Self {
            trigger: TimeBasedTrigger::new(DEFAULT_MC_TTL),
            cached_path: CachedMcPath {
                cache_key: String::new(),
                compacted_indices: Vec::new(),
                tokens_saved: 0,
                message_count: 0,
            },
        }
    }

    /// Check if micro-compaction should run right now.
    pub fn should_compact(&self) -> bool {
        self.trigger.should_compact()
    }

    /// Mark that micro-compaction has just run.
    pub fn record_compaction(&mut self) {
        self.trigger.record_compaction();
    }

    /// Check if the cached MC path is still valid for the current message count.
    pub fn cached_path_valid(&self, message_count: usize, last_hash: &str) -> bool {
        self.cached_path.is_valid(message_count, last_hash)
    }
}

/// Replace compactable tool results in old messages with short placeholders.
///
/// Only compacts messages older than `max_age_msgs` from the end.
/// Returns the number of messages that were compacted.
pub fn compact_old_tool_results(messages: &mut [Message], max_age_msgs: usize) -> usize {
    if messages.len() <= max_age_msgs {
        return 0;
    }

    let cutoff = messages.len().saturating_sub(max_age_msgs);
    let mut compacted = 0;

    for msg in &mut messages[..cutoff] {
        if msg.role != Role::Assistant {
            continue;
        }
        for block in &mut msg.content {
            if let ContentBlock::ToolResult { content, .. } = block {
                // Compact large tool results (>2KB) - these are likely file reads, bash output, etc.
                if content.len() > 2048 {
                    let original_len = content.len();
                    *content = format!(
                        "[Tool output compacted by micro-compact: {} bytes reclaimed]",
                        original_len
                    );
                    compacted += 1;
                }
            }
        }
    }

    compacted
}

/// Estimate token savings from micro-compacting old messages.
pub fn estimate_savings(messages: &[Message], max_age_msgs: usize) -> usize {
    if messages.len() <= max_age_msgs {
        return 0;
    }
    let cutoff = messages.len().saturating_sub(max_age_msgs);
    estimate_message_tokens(&messages[..cutoff])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_old_tool_results_skips_recent() {
        let mut messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "1".into(),
                    content: "x".repeat(3000),
                    is_error: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "2".into(),
                    content: "y".repeat(3000),
                    is_error: None,
                }],
            },
        ];
        let compacted = compact_old_tool_results(&mut messages, 1);
        assert_eq!(compacted, 1);
        assert!(messages[0].content[0].to_string().contains("compacted"));
        assert!(messages[1].content[0].to_string().contains("yyyy"));
    }
}
