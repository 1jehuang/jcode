//! MicroCompact - Incremental Tool Result Cleanup
//!
//! Ported from Claude Code's `services/compact/microCompact.ts` (v2.1.88).
//!
//! ## Overview
//!
//! MicroCompact reduces token usage on each API call by clearing old tool results
//! that the model no longer needs. Unlike full compaction (which summarizes and
//! replaces messages), MicroCompact only clears individual tool_result content blocks
//! while preserving message structure.
//!
//! ## Modes (ported from Claude Code)
//!
//! ### 1. Time-Based MicroCompact (`TimeBasedMC`)
//! When the time gap since the last assistant message exceeds a threshold, it means
//! the server-side prompt cache has likely expired. At that point, we clear old tool
//! results to reduce the payload size for what will be a full cache-miss request anyway.
//!
//! Trigger condition:
//! ```text
//! now - last_assistant_timestamp > gap_threshold_minutes → clear old results
//! ```
//!
//! ### 2. Cached MicroCompact (future: Cache Editing API)
//! Uses provider-specific cache editing APIs (e.g., Anthropic's `cache_edits`) to
//! delete tool results from the cached prefix without invalidating the cache.
//! This is provider-dependent and will be implemented when supported providers add the API.
//!
//! ## Compactable Tools
//!
//! Only these tools' results are eligible for clearing:
//! - `read`, `write`, `edit`, `multiedit` — file operations
//! - `bash` — shell commands
//! - `grep`, `glob` — search operations
//! - `webfetch`, `websearch` — web access
//!
//! Tools like `memory`, `communicate`, `todo` are NEVER cleared because their results
//! carry persistent state needed by the model.
//!
//! ## Integration Point
//!
//! Called in `turn_loops.rs` just before building the API payload, after memory injection.
//! The function mutates messages in-place and returns stats about what was cleared.

use jcode_message_types::{ContentBlock, Message, Role};
use std::collections::HashSet;
use std::time::SystemTime;

// Re-export CHARS_PER_TOKEN from the crate root
pub use crate::CHARS_PER_TOKEN;

/// Message shown in place of cleared tool result content.
/// Matches Claude Code's TIME_BASED_MC_CLEARED_MESSAGE.
pub const MC_CLEARED_MESSAGE: &str = "[Old tool result content cleared]";

/// Default time gap threshold in minutes for time-based trigger.
/// Matches Claude Code default of 10 minutes.
pub const DEFAULT_GAP_THRESHOLD_MINUTES: u64 = 10;

/// Default number of recent tool results to keep (even when clearing).
/// Always keep at least the last N results so the model has working context.
pub const DEFAULT_KEEP_RECENT: usize = 5;

/// Approximate token size for image/document blocks (conservative estimate).
const IMAGE_TOKEN_SIZE: usize = 2000;

/// Configuration for time-based microcompact.
#[derive(Debug, Clone)]
pub struct TimeBasedConfig {
    /// Enable/disable time-based trigger.
    pub enabled: bool,
    /// Minimum gap (in minutes) since last assistant message to trigger.
    pub gap_threshold_minutes: u64,
    /// Number of most recent tool results to preserve.
    pub keep_recent: usize,
}

impl Default for TimeBasedConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            gap_threshold_minutes: DEFAULT_GAP_THRESHOLD_MINUTES,
            keep_recent: DEFAULT_KEEP_RECENT,
        }
    }
}

/// Result of a microcompact operation.
#[derive(Debug, Clone)]
pub enum MicroCompactResult {
    /// No compaction was needed (under threshold, or nothing to clear).
    NoOp,
    /// Tool result contents were cleared.
    Cleared {
        /// Number of tool results whose content was replaced.
        tools_cleared: usize,
        /// Estimated tokens saved by this operation.
        tokens_saved: usize,
        /// What triggered this compaction.
        trigger: MicroCompactTrigger,
    },
}

/// What caused the microcompact to fire.
#[derive(Debug, Clone)]
pub enum MicroCompactTrigger {
    /// Time-based: gap since last assistant message exceeded threshold.
    TimeBased { gap_minutes: f64 },
    // Future: CacheEdits { deleted_tool_ids: Vec<String> },
}

/// The main microcompactor struct.
///
/// Holds configuration and tracks state across calls within a session.
#[derive(Debug, Clone)]
pub struct MicroCompactor {
    /// Set of tool names whose results can be compacted.
    compactable_tools: HashSet<String>,

    /// Time-based trigger configuration.
    time_config: TimeBasedConfig,
}

impl MicroCompactor {
    /// Create a new MicroCompactor with default settings.
    pub fn new() -> Self {
        let mut compactable_tools = HashSet::new();
        // File operation tools
        compactable_tools.insert("read".to_string());
        compactable_tools.insert("write".to_string());
        compactable_tools.insert("edit".to_string());
        compactable_tools.insert("multiedit".to_string());
        compactable_tools.insert("patch".to_string());
        compactable_tools.insert("apply_patch".to_string());
        // Search tools
        compactable_tools.insert("grep".to_string());
        compactable_tools.insert("glob".to_string());
        compactable_tools.insert("agentgrep".to_string());
        compactable_tools.insert("ls".to_string());
        // Shell tools
        compactable_tools.insert("bash".to_string());
        // Web tools
        compactable_tools.insert("webfetch".to_string());
        compactable_tools.insert("websearch".to_string());

        Self {
            compactable_tools,
            time_config: TimeBasedConfig::default(),
        }
    }

    /// Create with custom time-based config.
    pub fn with_time_config(config: TimeBasedConfig) -> Self {
        let mut self_ = Self::new();
        self_.time_config = config;
        self_
    }

    /// Run microcompact on the given messages.
    ///
    /// This is the main entry point called before each API request.
    /// It checks triggers and applies compaction if needed.
    ///
    /// # Arguments
    /// * `messages` - The conversation history (mutated in-place if cleared)
    /// * `now` - Current timestamp (for testing; uses SystemTime::now() if None)
    ///
    /// # Returns
    /// A `MicroCompactResult` describing what happened.
    pub fn run(
        &self,
        messages: &mut Vec<Message>,
        now: Option<SystemTime>,
    ) -> MicroCompactResult {
        // Try time-based trigger first
        if let Some(result) = self.maybe_time_based(messages, now) {
            return result;
        }

        MicroCompactResult::NoOp
    }

    /// Check and apply time-based microcompact.
    ///
    /// Fires when the gap between "now" and the last assistant message timestamp
    /// exceeds the configured threshold. When the cache has expired (long gap),
    /// clearing old tool results reduces the payload without any cache downside.
    fn maybe_time_based(
        &self,
        messages: &mut Vec<Message>,
        now: Option<SystemTime>,
    ) -> Option<MicroCompactResult> {
        if !self.time_config.enabled {
            return None;
        }

        let now = now.unwrap_or(SystemTime::now());
        let gap_minutes = self.compute_gap_minutes(messages, now)?;

        if gap_minutes < self.time_config.gap_threshold_minutes as f64 {
            return None;
        }

        // Find all compactable tool IDs in order
        let compactable_ids = self.collect_compactable_tool_ids(messages);

        if compactable_ids.is_empty() {
            return None;
        }

        // Keep the most recent N, clear the rest
        let keep_recent = self.time_config.keep_recent.max(1); // floor at 1
        let keep_set: HashSet<String> =
            compactable_ids.iter().rev().take(keep_recent).cloned().collect();
        let clear_set: HashSet<String> = compactable_ids
            .iter()
            .filter(|id| !keep_set.contains(*id))
            .cloned()
            .collect();

        if clear_set.is_empty() {
            return None;
        }

        // Apply clearing and count savings
        let tokens_saved = self.clear_tool_results(messages, &clear_set);
        let tools_cleared = clear_set.len();

        if tokens_saved == 0 {
            return None;
        }

        tracing::info!(
            "[MicroCompact] gap {:.1}min > {}min, cleared {} tool results (~{} tokens)",
            gap_minutes,
            self.time_config.gap_threshold_minutes,
            tools_cleared,
            tokens_saved
        );

        Some(MicroCompactResult::Cleared {
            tools_cleared,
            tokens_saved,
            trigger: MicroCompactTrigger::TimeBased { gap_minutes },
        })
    }

    /// Compute minutes elapsed since the last assistant message.
    fn compute_gap_minutes(&self, messages: &[Message], now: SystemTime) -> Option<f64> {
        let last_ts: chrono::DateTime<chrono::Utc> = messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .last()
            .and_then(|m| m.timestamp)?;

        // Convert DateTime<Utc> to SystemTime for duration calculation
        let last_system_time: SystemTime = last_ts.into();
        let duration = now.duration_since(last_system_time).ok()?;
        Some(duration.as_secs_f64() / 60.0)
    }

    /// Collect tool_use block IDs from assistant messages for compactable tools only,
    /// in encounter order.
    fn collect_compactable_tool_ids(&self, messages: &[Message]) -> Vec<String> {
        let mut ids = Vec::new();
        for msg in messages {
            if msg.role != Role::Assistant {
                continue;
            }
            for block in &msg.content {
                if let ContentBlock::ToolUse { name, id, .. } = block {
                    if self.compactable_tools.contains(name) {
                        ids.push(id.clone());
                    }
                }
            }
        }
        ids
    }

    /// Clear tool result content for the given tool_use_ids.
    /// Returns estimated token savings.
    fn clear_tool_results(
        &self,
        messages: &mut Vec<Message>,
        clear_set: &HashSet<String>,
    ) -> usize {
        let mut tokens_saved = 0;

        for msg in messages.iter_mut() {
            if msg.role != Role::User {
                continue;
            }

            for block in msg.content.iter_mut() {
                if let &mut ContentBlock::ToolResult {
                    ref tool_use_id,
                    ref mut content,
                    ..
                } = block
                {
                    if clear_set.contains(tool_use_id) && *content != MC_CLEARED_MESSAGE {
                        tokens_saved += estimate_content_tokens(content);
                        *content = MC_CLEARED_MESSAGE.to_string();
                    }
                }
            }
        }

        tokens_saved
    }
}

/// Roughly estimate token count for a content string.
/// Pads by 4/3 to be conservative (matching Claude Code approach).
fn estimate_content_tokens(content: &str) -> usize {
    let chars = content.len();
    (chars * 4 / 3 + CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN
}

/// Estimate tokens for an arbitrary ContentBlock (used for bookkeeping).
pub fn estimate_block_tokens(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text { text, .. } => estimate_content_tokens(text),
        ContentBlock::ToolResult { content, .. } => estimate_content_tokens(content),
        ContentBlock::ToolUse { name, input, .. } => {
            // Count name + input JSON
            let input_str = serde_json::to_string(input).unwrap_or_default();
            estimate_content_tokens(&(name.clone() + &input_str))
        }
        ContentBlock::Image { .. } => IMAGE_TOKEN_SIZE,
        ContentBlock::Reasoning { text } => estimate_content_tokens(text),
        _ => 0,
    }
}

impl Default for MicroCompactor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_message_types::ContentBlock;

    fn make_assistant_msg(tool_name: &str, tool_id: &str, ts_secs: u64) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: tool_id.to_string(),
                name: tool_name.to_string(),
                input: serde_json::json!({}),
            }],
            timestamp: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(ts_secs)),
            ..Default::default()
        }
    }

    fn make_user_result(tool_id: &str, content: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_id.to_string(),
                content: content.to_string(),
                is_error: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn test_no_op_when_gap_under_threshold() {
        let mc = MicroCompactor::new();
        let mut messages = vec![
            make_assistant_msg("read", "tu1", 100), // recent
            make_user_result("tu1", "file contents here"),
        ];
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(101); // 1 min gap

        assert!(matches!(mc.run(&mut messages, Some(now)), MicroCompactResult::NoOp));
        assert_eq!(messages[1].content[0], ContentBlock::ToolResult {
            tool_use_id: "tu1".into(),
            content: "file contents here".into(),
            is_error: None,
        });
    }

    #[test]
    fn test_clears_old_results_when_gap_exceeds_threshold() {
        let mc = MicroCompactor::with_time_config(TimeBasedConfig {
            enabled: true,
            gap_threshold_minutes: 5, // 5 min threshold for testing
            keep_recent: 2,
        });

        let mut messages = vec![
            // Old turn (15 min ago)
            make_assistant_msg("read", "tu_old", 0),
            make_user_result("tu_old", "old file content worth many tokens"),
            // Recent turn (1 min ago)
            make_assistant_msg("grep", "tu_recent", 840), // 14 min after epoch
            make_user_result("tu_recent", "recent grep output"),
        ];

        // Now at 20 min past epoch → gap to tu_old is 20min > 5min threshold
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1200);

        match mc.run(&mut messages, Some(now)) {
            MicroCompactResult::Cleared { tools_cleared, tokens_saved, .. } => {
                assert_eq!(tools_cleared, 1);
                assert!(tokens_saved > 0);
            }
            other => panic!("Expected Cleared, got {:?}", other),
        }

        // Old should be cleared, recent preserved
        match &messages[1].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content.as_str(), MC_CLEARED_MESSAGE);
            }
            _ => panic!("Expected ToolResult"),
        }
        match &messages[3].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content.as_str(), "recent grep output");
            }
            _ => panic!("Expected ToolResult"),
        }
    }

    #[test]
    fn test_non_compactable_tools_not_cleared() {
        let mc = MicroCompactor::with_time_config(TimeBasedConfig {
            enabled: true,
            gap_threshold_minutes: 1,
            keep_recent: 0,
        });

        let mut messages = vec![
            make_assistant_msg("memory", "mem1", 0),
            make_user_result("mem1", "important memory data"),
        ];

        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(120);

        // Memory tool is not in compactable set → NoOp
        assert!(matches!(mc.run(&mut messages, Some(now)), MicroCompactResult::NoOp));
    }

    #[test]
    fn test_keep_recent_floor_at_one() {
        let mc = MicroCompactor::with_time_config(TimeBasedConfig {
            enabled: true,
            gap_threshold_minutes: 1,
            keep_recent: 0, // floor should be 1
        });

        let mut messages = vec![
            make_assistant_msg("read", "tu1", 0),
            make_user_result("tu1", "old"),
            make_assistant_msg("read", "tu2", 60),
            make_user_result("tu2", "recent"),
        ];

        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(300);

        match mc.run(&mut messages, Some(now)) {
            MicroCompactResult::Cleared { tools_cleared, .. } => {
                // Only tu1 cleared, tu2 kept (floor at 1)
                assert_eq!(tools_cleared, 1);
            }
            other => panic!("Expected Cleared, got {:?}", other),
        }
    }
}
