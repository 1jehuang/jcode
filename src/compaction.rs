//! Background compaction for conversation context management
//!
//! When context reaches 80% of the limit, kicks off background summarization.
//! User continues chatting while summary is generated. When ready, seamlessly
//! swaps in the compacted context.
//!
//! The CompactionManager does NOT store its own copy of messages. Instead,
//! callers pass `&[Message]` references when needed. The manager tracks how
//! many messages from the front have been compacted via `compacted_count`.

#![allow(dead_code)]

use crate::message::{ContentBlock, Message, Role};
use crate::provider::Provider;
use anyhow::Result;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Default token budget (200k tokens - matches Claude's actual context limit)
const DEFAULT_TOKEN_BUDGET: usize = 200_000;

/// Trigger compaction at this percentage of budget
const COMPACTION_THRESHOLD: f32 = 0.80;

/// Minimum threshold for manual compaction (can compact at any time above this)
const MANUAL_COMPACT_MIN_THRESHOLD: f32 = 0.10;

/// Keep this many recent turns verbatim (not summarized)
const RECENT_TURNS_TO_KEEP: usize = 10;

/// Approximate chars per token for estimation
const CHARS_PER_TOKEN: usize = 4;

const SUMMARY_PROMPT: &str = r#"Summarize our conversation so you can continue this work later.

Write in natural language with these sections:
- **Context:** What we're working on and why (1-2 sentences)
- **What we did:** Key actions taken, files changed, problems solved
- **Current state:** What works, what's broken, what's next
- **User preferences:** Specific requirements or decisions they made

Be concise but preserve important details. You can search the full conversation later if you need exact error messages or code snippets."#;

/// A completed summary covering turns up to a certain point
#[derive(Debug, Clone)]
pub struct Summary {
    pub text: String,
    pub covers_up_to_turn: usize,
    pub original_turn_count: usize,
}

/// Event emitted when compaction is applied
#[derive(Debug, Clone)]
pub struct CompactionEvent {
    pub trigger: String,
    pub pre_tokens: Option<u64>,
}

/// Result from background compaction task
struct CompactionResult {
    summary: String,
    covers_up_to_turn: usize,
}

/// Manages background compaction of conversation context.
///
/// Does NOT own message data. The caller owns the messages and passes
/// references into methods that need them. After compaction, the manager
/// records `compacted_count` — the number of leading messages that have
/// been summarized and should be skipped when building API payloads.
pub struct CompactionManager {
    /// Number of leading messages that have been compacted into the summary.
    /// When building API messages, skip the first `compacted_count` messages.
    compacted_count: usize,

    /// Active summary (if we've compacted before)
    active_summary: Option<Summary>,

    /// Background compaction task handle
    pending_task: Option<JoinHandle<Result<CompactionResult>>>,

    /// Turn index (relative to uncompacted messages) where pending compaction will cut off
    pending_cutoff: usize,

    /// Total turns seen (for tracking)
    total_turns: usize,

    /// Token budget
    token_budget: usize,

    /// Provider-reported input token usage from the latest request.
    /// Used to trigger compaction with real token counts instead of only heuristics.
    observed_input_tokens: Option<u64>,

    /// Last compaction event (if any)
    last_compaction: Option<CompactionEvent>,
}

impl CompactionManager {
    pub fn new() -> Self {
        Self {
            compacted_count: 0,
            active_summary: None,
            pending_task: None,
            pending_cutoff: 0,
            total_turns: 0,
            token_budget: DEFAULT_TOKEN_BUDGET,
            observed_input_tokens: None,
            last_compaction: None,
        }
    }

    /// Reset all compaction state
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn with_budget(mut self, budget: usize) -> Self {
        self.token_budget = budget;
        self
    }

    /// Update the token budget (e.g., when model changes)
    pub fn set_budget(&mut self, budget: usize) {
        self.token_budget = budget;
    }

    /// Get current token budget
    pub fn token_budget(&self) -> usize {
        self.token_budget
    }

    /// Notify the manager that a message was added.
    /// This just increments the turn counter — no data is stored.
    pub fn notify_message_added(&mut self) {
        self.total_turns += 1;
    }

    /// Backward-compatible alias for `notify_message_added`.
    /// Accepts (and ignores) the message — callers that haven't been
    /// updated yet can still call `add_message(msg)`.
    pub fn add_message(&mut self, _message: Message) {
        self.notify_message_added();
    }

    /// Get the active (uncompacted) messages from a full message list.
    /// Skips the first `compacted_count` messages.
    fn active_messages<'a>(&self, all_messages: &'a [Message]) -> &'a [Message] {
        if self.compacted_count <= all_messages.len() {
            &all_messages[self.compacted_count..]
        } else {
            // Edge case: messages were cleared/replaced with fewer items
            all_messages
        }
    }

    /// Get current token estimate using the caller's message list
    pub fn token_estimate_with(&self, all_messages: &[Message]) -> usize {
        let mut total_chars = 0;

        if let Some(ref summary) = self.active_summary {
            total_chars += summary.text.len();
        }

        for msg in self.active_messages(all_messages) {
            total_chars += Self::message_char_count(msg);
        }

        total_chars / CHARS_PER_TOKEN
    }

    /// Get current token estimate (backward compat — uses 0 messages, only summary + observed)
    pub fn token_estimate(&self) -> usize {
        let mut total_chars = 0;
        if let Some(ref summary) = self.active_summary {
            total_chars += summary.text.len();
        }
        total_chars / CHARS_PER_TOKEN
    }

    /// Store provider-reported input token usage for compaction decisions.
    pub fn update_observed_input_tokens(&mut self, tokens: u64) {
        self.observed_input_tokens = Some(tokens);
    }

    /// Best-effort current token count using the caller's messages.
    pub fn effective_token_count_with(&self, all_messages: &[Message]) -> usize {
        let estimate = self.token_estimate_with(all_messages);
        let observed = self
            .observed_input_tokens
            .and_then(|tokens| usize::try_from(tokens).ok())
            .unwrap_or(0);
        estimate.max(observed)
    }

    /// Best-effort token count without message data (uses only observed tokens)
    pub fn effective_token_count(&self) -> usize {
        let estimate = self.token_estimate();
        let observed = self
            .observed_input_tokens
            .and_then(|tokens| usize::try_from(tokens).ok())
            .unwrap_or(0);
        estimate.max(observed)
    }

    /// Get current context usage as percentage (using caller's messages)
    pub fn context_usage_with(&self, all_messages: &[Message]) -> f32 {
        self.effective_token_count_with(all_messages) as f32 / self.token_budget as f32
    }

    /// Get current context usage (without messages, uses observed tokens only)
    pub fn context_usage(&self) -> f32 {
        self.effective_token_count() as f32 / self.token_budget as f32
    }

    /// Check if we should start compaction
    pub fn should_compact_with(&self, all_messages: &[Message]) -> bool {
        let active = self.active_messages(all_messages);
        self.pending_task.is_none()
            && self.context_usage_with(all_messages) >= COMPACTION_THRESHOLD
            && active.len() > RECENT_TURNS_TO_KEEP
    }

    /// Start background compaction if needed
    pub fn maybe_start_compaction_with(
        &mut self,
        all_messages: &[Message],
        provider: Arc<dyn Provider>,
    ) {
        if !self.should_compact_with(all_messages) {
            return;
        }

        let active = self.active_messages(all_messages);

        // Calculate cutoff within active messages — keep last N turns verbatim
        let mut cutoff = active.len().saturating_sub(RECENT_TURNS_TO_KEEP);
        if cutoff == 0 {
            return;
        }

        // Adjust cutoff to not split tool call/result pairs
        cutoff = Self::safe_cutoff_static(active, cutoff);
        if cutoff == 0 {
            return;
        }

        // Snapshot messages to summarize (must clone for the async task)
        let messages_to_summarize: Vec<Message> = active[..cutoff].to_vec();
        let existing_summary = self.active_summary.clone();

        self.pending_cutoff = cutoff;

        // Spawn background task
        self.pending_task = Some(tokio::spawn(async move {
            generate_summary(provider, messages_to_summarize, existing_summary).await
        }));
    }

    /// Backward-compatible wrapper
    pub fn maybe_start_compaction(&mut self, _provider: Arc<dyn Provider>) {
        // Without messages, we can only check observed tokens
        // This is a no-op if no messages are provided
        // Callers should migrate to maybe_start_compaction_with
    }

    /// Force immediate compaction (for manual /compact command).
    pub fn force_compact_with(
        &mut self,
        all_messages: &[Message],
        provider: Arc<dyn Provider>,
    ) -> Result<(), String> {
        if self.pending_task.is_some() {
            return Err("Compaction already in progress".to_string());
        }

        let active = self.active_messages(all_messages);

        if active.len() <= RECENT_TURNS_TO_KEEP {
            return Err(format!(
                "Not enough messages to compact (need more than {}, have {})",
                RECENT_TURNS_TO_KEEP,
                active.len()
            ));
        }

        if self.context_usage_with(all_messages) < MANUAL_COMPACT_MIN_THRESHOLD {
            return Err(format!(
                "Context usage too low ({:.1}%) - nothing to compact",
                self.context_usage_with(all_messages) * 100.0
            ));
        }

        let mut cutoff = active.len().saturating_sub(RECENT_TURNS_TO_KEEP);
        if cutoff == 0 {
            return Err("No messages available to compact after keeping recent turns".to_string());
        }

        cutoff = Self::safe_cutoff_static(active, cutoff);
        if cutoff == 0 {
            return Err("Cannot compact - would split tool call/result pairs".to_string());
        }

        let messages_to_summarize: Vec<Message> = active[..cutoff].to_vec();
        let existing_summary = self.active_summary.clone();

        self.pending_cutoff = cutoff;

        self.pending_task = Some(tokio::spawn(async move {
            generate_summary(provider, messages_to_summarize, existing_summary).await
        }));

        Ok(())
    }

    /// Backward-compatible force_compact (for callers that still have their own message vec).
    /// This variant works with the old API where CompactionManager had its own messages.
    /// Callers should migrate to force_compact_with.
    pub fn force_compact(&mut self, _provider: Arc<dyn Provider>) -> Result<(), String> {
        Err("force_compact requires messages — use force_compact_with(messages, provider)".to_string())
    }

    /// Find a safe cutoff point that doesn't split tool call/result pairs.
    /// Static version that works on a message slice.
    fn safe_cutoff_static(messages: &[Message], initial_cutoff: usize) -> usize {
        use std::collections::HashSet;

        let mut cutoff = initial_cutoff;

        // Collect tool_use_ids from ToolResults in the "kept" portion (after cutoff)
        let mut needed_tool_ids: HashSet<String> = HashSet::new();
        for msg in &messages[cutoff..] {
            for block in &msg.content {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    needed_tool_ids.insert(tool_use_id.clone());
                }
            }
        }

        if needed_tool_ids.is_empty() {
            return cutoff;
        }

        // Collect tool_use_ids from ToolUse blocks in the "kept" portion
        let mut available_tool_ids: HashSet<String> = HashSet::new();
        for msg in &messages[cutoff..] {
            for block in &msg.content {
                if let ContentBlock::ToolUse { id, .. } = block {
                    available_tool_ids.insert(id.clone());
                }
            }
        }

        // Find missing tool calls (results exist but calls don't in kept portion)
        let missing: HashSet<_> = needed_tool_ids
            .difference(&available_tool_ids)
            .cloned()
            .collect();

        if missing.is_empty() {
            return cutoff;
        }

        // Move cutoff backwards to include messages with missing tool calls
        for (idx, msg) in messages[..cutoff].iter().enumerate().rev() {
            let mut found_any = false;
            for block in &msg.content {
                if let ContentBlock::ToolUse { id, .. } = block {
                    if missing.contains(id) {
                        found_any = true;
                    }
                }
            }
            if found_any {
                cutoff = idx;
                return Self::safe_cutoff_static(messages, cutoff);
            }
        }

        // If we couldn't find all tool calls, don't compact at all
        0
    }

    /// Check if background compaction is done and apply it
    pub fn check_and_apply_compaction(&mut self) {
        let task = match self.pending_task.take() {
            Some(task) => task,
            None => return,
        };

        // Check if done without blocking
        if !task.is_finished() {
            // Not done yet, put it back
            self.pending_task = Some(task);
            return;
        }

        // Get result
        match futures::executor::block_on(task) {
            Ok(Ok(result)) => {
                let pre_tokens = self.effective_token_count() as u64;
                let summary = Summary {
                    text: result.summary,
                    covers_up_to_turn: result.covers_up_to_turn,
                    original_turn_count: self.pending_cutoff,
                };

                // Advance the compacted count — these messages are now summarized
                self.compacted_count += self.pending_cutoff;

                // Store summary
                self.active_summary = Some(summary);
                self.last_compaction = Some(CompactionEvent {
                    trigger: "background".to_string(),
                    pre_tokens: Some(pre_tokens),
                });
                self.observed_input_tokens = None;

                self.pending_cutoff = 0;
            }
            Ok(Err(e)) => {
                crate::logging::error(&format!("[compaction] Failed to generate summary: {}", e));
                self.pending_cutoff = 0;
            }
            Err(e) => {
                crate::logging::error(&format!("[compaction] Task panicked: {}", e));
                self.pending_cutoff = 0;
            }
        }
    }

    /// Take the last compaction event (if any)
    pub fn take_compaction_event(&mut self) -> Option<CompactionEvent> {
        self.last_compaction.take()
    }

    /// Get messages for API call (with summary if compacted).
    /// Takes the full message list from the caller.
    pub fn messages_for_api_with(&mut self, all_messages: &[Message]) -> Vec<Message> {
        self.check_and_apply_compaction();

        let active = self.active_messages(all_messages);

        match &self.active_summary {
            Some(summary) => {
                let summary_block = ContentBlock::Text {
                    text: format!(
                        "## Previous Conversation Summary\n\n{}\n\n---\n\n",
                        summary.text
                    ),
                    cache_control: None,
                };

                let mut result = Vec::with_capacity(active.len() + 1);

                result.push(Message {
                    role: Role::User,
                    content: vec![summary_block],
                    timestamp: None,
                });

                // Clone only the active (non-compacted) messages
                result.extend(active.iter().cloned());

                result
            }
            None => active.to_vec(),
        }
    }

    /// Backward-compatible messages_for_api (no messages available).
    /// Returns only summary if present, or empty vec.
    pub fn messages_for_api(&mut self) -> Vec<Message> {
        self.check_and_apply_compaction();

        // Without caller messages, we can only return the summary
        match &self.active_summary {
            Some(summary) => {
                let summary_block = ContentBlock::Text {
                    text: format!(
                        "## Previous Conversation Summary\n\n{}\n\n---\n\n",
                        summary.text
                    ),
                    cache_control: None,
                };
                vec![Message {
                    role: Role::User,
                    content: vec![summary_block],
                    timestamp: None,
                }]
            }
            None => Vec::new(),
        }
    }

    /// Check if compaction is in progress
    pub fn is_compacting(&self) -> bool {
        self.pending_task.is_some()
    }

    /// Get the number of compacted (summarized) messages
    pub fn compacted_count(&self) -> usize {
        self.compacted_count
    }

    /// Get stats about current state (without message data)
    pub fn stats(&self) -> CompactionStats {
        CompactionStats {
            total_turns: self.total_turns,
            active_messages: 0, // unknown without messages
            has_summary: self.active_summary.is_some(),
            is_compacting: self.is_compacting(),
            token_estimate: self.token_estimate(),
            effective_tokens: self.effective_token_count(),
            observed_input_tokens: self.observed_input_tokens,
            context_usage: self.context_usage(),
        }
    }

    /// Get stats with full message data
    pub fn stats_with(&self, all_messages: &[Message]) -> CompactionStats {
        let active = self.active_messages(all_messages);
        CompactionStats {
            total_turns: self.total_turns,
            active_messages: active.len(),
            has_summary: self.active_summary.is_some(),
            is_compacting: self.is_compacting(),
            token_estimate: self.token_estimate_with(all_messages),
            effective_tokens: self.effective_token_count_with(all_messages),
            observed_input_tokens: self.observed_input_tokens,
            context_usage: self.context_usage_with(all_messages),
        }
    }

    fn message_char_count(msg: &Message) -> usize {
        msg.content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text, .. } => text.len(),
                ContentBlock::Reasoning { text } => text.len(),
                ContentBlock::ToolUse { input, .. } => input.to_string().len() + 50,
                ContentBlock::ToolResult { content, .. } => content.len() + 20,
                ContentBlock::Image { data, .. } => data.len(),
            })
            .sum()
    }

    /// Poll for compaction completion and return an event if one was applied.
    pub fn poll_compaction_event(&mut self) -> Option<CompactionEvent> {
        self.check_and_apply_compaction();
        self.take_compaction_event()
    }

    /// Emergency hard compaction: drop old messages without summarizing.
    /// Takes the caller's full message list to inspect content.
    pub fn hard_compact_with(&mut self, all_messages: &[Message]) -> Result<usize, String> {
        let active = self.active_messages(all_messages);

        if active.len() <= RECENT_TURNS_TO_KEEP {
            return Err(format!(
                "Not enough messages to compact (have {}, need more than {})",
                active.len(),
                RECENT_TURNS_TO_KEEP
            ));
        }

        let pre_tokens = self.effective_token_count_with(all_messages) as u64;

        let mut cutoff = active.len().saturating_sub(RECENT_TURNS_TO_KEEP);
        cutoff = Self::safe_cutoff_static(active, cutoff);
        if cutoff == 0 {
            return Err("Cannot compact — would split tool call/result pairs".to_string());
        }

        let dropped_count = cutoff;

        let mut summary_parts: Vec<String> = Vec::new();

        if let Some(ref existing) = self.active_summary {
            summary_parts.push(existing.text.clone());
        }

        summary_parts.push(format!(
            "**[Emergency compaction]**: {} messages were dropped to recover from context overflow. \
             The conversation had ~{}k tokens which exceeded the {}k limit.",
            dropped_count,
            pre_tokens / 1000,
            self.token_budget / 1000,
        ));

        let mut file_mentions = Vec::new();
        let mut tool_names = std::collections::HashSet::new();
        for msg in &active[..cutoff] {
            for block in &msg.content {
                match block {
                    ContentBlock::ToolUse { name, .. } => {
                        tool_names.insert(name.clone());
                    }
                    ContentBlock::Text { text, .. } => {
                        for word in text.split_whitespace() {
                            if (word.contains('/') || word.contains('.'))
                                && word.len() > 3
                                && word.len() < 120
                                && !word.starts_with("http")
                            {
                                if word.contains(".rs")
                                    || word.contains(".ts")
                                    || word.contains(".py")
                                    || word.contains(".toml")
                                    || word.contains(".json")
                                    || word.starts_with("src/")
                                    || word.starts_with("./")
                                {
                                    let cleaned = word.trim_matches(|c: char| {
                                        !c.is_alphanumeric()
                                            && c != '/'
                                            && c != '.'
                                            && c != '_'
                                            && c != '-'
                                    });
                                    file_mentions.push(cleaned.to_string());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if !tool_names.is_empty() {
            let mut tools: Vec<_> = tool_names.into_iter().collect();
            tools.sort();
            summary_parts.push(format!("Tools used: {}", tools.join(", ")));
        }

        file_mentions.sort();
        file_mentions.dedup();
        if !file_mentions.is_empty() {
            file_mentions.truncate(30);
            summary_parts.push(format!("Files referenced: {}", file_mentions.join(", ")));
        }

        let summary = Summary {
            text: summary_parts.join("\n\n"),
            covers_up_to_turn: cutoff,
            original_turn_count: cutoff,
        };

        self.compacted_count += cutoff;
        self.active_summary = Some(summary);
        self.last_compaction = Some(CompactionEvent {
            trigger: "hard_compact".to_string(),
            pre_tokens: Some(pre_tokens),
        });
        self.observed_input_tokens = None;

        Ok(dropped_count)
    }

    /// Backward-compatible hard_compact
    pub fn hard_compact(&mut self) -> Result<usize, String> {
        Err("hard_compact requires messages — use hard_compact_with(messages)".to_string())
    }
}

impl Default for CompactionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Stats about compaction state
#[derive(Debug, Clone)]
pub struct CompactionStats {
    pub total_turns: usize,
    pub active_messages: usize,
    pub has_summary: bool,
    pub is_compacting: bool,
    pub token_estimate: usize,
    pub effective_tokens: usize,
    pub observed_input_tokens: Option<u64>,
    pub context_usage: f32,
}

/// Generate summary using the provider
async fn generate_summary(
    provider: Arc<dyn Provider>,
    messages: Vec<Message>,
    existing_summary: Option<Summary>,
) -> Result<CompactionResult> {
    // Build the conversation text for summarization
    let mut conversation_text = String::new();

    // Include existing summary if present
    if let Some(ref summary) = existing_summary {
        conversation_text.push_str("## Previous Summary\n\n");
        conversation_text.push_str(&summary.text);
        conversation_text.push_str("\n\n## New Conversation\n\n");
    }

    // Add messages
    for msg in &messages {
        let role_str = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };

        conversation_text.push_str(&format!("**{}:**\n", role_str));

        for block in &msg.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    conversation_text.push_str(text);
                    conversation_text.push('\n');
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    conversation_text.push_str(&format!("[Tool: {} - {}]\n", name, input));
                }
                ContentBlock::ToolResult { content, .. } => {
                    let truncated = if content.len() > 500 {
                        format!("{}... (truncated)", crate::util::truncate_str(content, 500))
                    } else {
                        content.clone()
                    };
                    conversation_text.push_str(&format!("[Result: {}]\n", truncated));
                }
                ContentBlock::Reasoning { .. } => {}
                ContentBlock::Image { .. } => {
                    conversation_text.push_str("[Image]\n");
                }
            }
        }
        conversation_text.push('\n');
    }

    // Truncate conversation text if it would exceed the provider's context limit.
    let max_prompt_chars = provider.context_window().saturating_sub(4000) * CHARS_PER_TOKEN;
    let overhead = SUMMARY_PROMPT.len() + 50;
    if conversation_text.len() + overhead > max_prompt_chars && max_prompt_chars > overhead {
        let budget = max_prompt_chars - overhead;
        conversation_text = crate::util::truncate_str(&conversation_text, budget).to_string();
        conversation_text
            .push_str("\n\n... [earlier conversation truncated to fit context window]\n");
    }

    // Generate summary using simple completion
    let prompt = format!("{}\n\n---\n\n{}", conversation_text, SUMMARY_PROMPT);
    let summary = provider
        .complete_simple(
            &prompt,
            "You are a helpful assistant that summarizes conversations.",
        )
        .await?;

    Ok(CompactionResult {
        summary,
        covers_up_to_turn: messages.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{EventStream, Provider};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct MockSummaryProvider;

    #[async_trait::async_trait]
    impl Provider for MockSummaryProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            Ok(Box::pin(futures::stream::empty()))
        }

        fn name(&self) -> &str {
            "mock-summary"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(MockSummaryProvider)
        }

        async fn complete_simple(&self, prompt: &str, _system: &str) -> Result<String> {
            Ok(format!("summary({} chars)", prompt.len()))
        }
    }

    fn make_text_message(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            timestamp: None,
        }
    }

    #[test]
    fn test_new_manager() {
        let manager = CompactionManager::new();
        assert_eq!(manager.compacted_count, 0);
        assert!(manager.active_summary.is_none());
        assert!(!manager.is_compacting());
    }

    #[test]
    fn test_notify_message_added() {
        let mut manager = CompactionManager::new();
        manager.notify_message_added();
        manager.notify_message_added();
        assert_eq!(manager.total_turns, 2);
    }

    #[test]
    fn test_token_estimate() {
        let manager = CompactionManager::new();
        // 100 chars = ~25 tokens
        let messages = vec![make_text_message(Role::User, &"x".repeat(100))];
        let estimate = manager.token_estimate_with(&messages);
        assert!(estimate > 20 && estimate < 30);
    }

    #[test]
    fn test_should_compact() {
        let mut manager = CompactionManager::new().with_budget(100); // Very small budget

        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(make_text_message(
                Role::User,
                &format!("Message {} with some content", i),
            ));
            manager.notify_message_added();
        }

        assert!(manager.should_compact_with(&messages));
    }

    #[test]
    fn test_context_usage_prefers_observed_tokens() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        let messages = vec![make_text_message(Role::User, "short message")];
        manager.notify_message_added();
        manager.update_observed_input_tokens(900);

        assert!(manager.context_usage_with(&messages) >= 0.90);
        assert!(manager.effective_token_count_with(&messages) >= 900);
    }

    #[test]
    fn test_should_compact_uses_observed_tokens() {
        let mut manager = CompactionManager::new().with_budget(1_000);

        let mut messages = Vec::new();
        for _ in 0..12 {
            messages.push(make_text_message(Role::User, "x"));
            manager.notify_message_added();
        }
        manager.update_observed_input_tokens(850);

        assert!(manager.should_compact_with(&messages));
    }

    #[test]
    fn test_messages_for_api_no_summary() {
        let mut manager = CompactionManager::new();
        let messages = vec![
            make_text_message(Role::User, "Hello"),
            make_text_message(Role::Assistant, "Hi!"),
        ];
        manager.notify_message_added();
        manager.notify_message_added();

        let msgs = manager.messages_for_api_with(&messages);
        assert_eq!(msgs.len(), 2);
    }

    #[tokio::test]
    async fn test_force_compact_applies_summary() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        let mut messages = Vec::new();
        for i in 0..30 {
            messages.push(make_text_message(
                Role::User,
                &format!("Turn {} {}", i, "x".repeat(120)),
            ));
            manager.notify_message_added();
        }

        let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
        manager
            .force_compact_with(&messages, provider)
            .expect("manual compaction should start");

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            manager.check_and_apply_compaction();
            if manager.stats().has_summary {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(
            manager.stats().has_summary,
            "summary should be applied after compaction task completes"
        );

        // After compaction, compacted_count should be > 0
        assert!(manager.compacted_count > 0);

        let msgs = manager.messages_for_api_with(&messages);
        assert!(msgs.len() < 30);
        let first = msgs.first().expect("summary message missing");
        assert_eq!(first.role, Role::User);
        match &first.content[0] {
            ContentBlock::Text { text, .. } => {
                assert!(text.contains("Previous Conversation Summary"));
            }
            _ => panic!("expected text summary block"),
        }
    }
}
