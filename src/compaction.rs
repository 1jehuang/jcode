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

/// If context is above this threshold when compaction starts, do a synchronous
/// hard-compact (drop old messages) so the API call doesn't fail.
const CRITICAL_THRESHOLD: f32 = 0.95;

/// Minimum threshold for manual compaction (can compact at any time above this)
const MANUAL_COMPACT_MIN_THRESHOLD: f32 = 0.10;

/// Keep this many recent turns verbatim (not summarized)
const RECENT_TURNS_TO_KEEP: usize = 10;

/// Absolute minimum turns to keep during emergency compaction
const MIN_TURNS_TO_KEEP: usize = 2;

/// Max chars for a single tool result during emergency truncation
const EMERGENCY_TOOL_RESULT_MAX_CHARS: usize = 4000;

/// Approximate chars per token for estimation
const CHARS_PER_TOKEN: usize = 4;

/// Fixed token overhead for system prompt + tool definitions.
/// These are not counted in message content but do count toward the context limit.
/// Estimated conservatively: ~8k tokens for system prompt + ~10k for 50+ tools.
const SYSTEM_OVERHEAD_TOKENS: usize = 18_000;

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

/// What happened when ensure_context_fits was called
#[derive(Debug, Clone, PartialEq)]
pub enum CompactionAction {
    /// Nothing needed — context is fine
    None,
    /// Background summarization started (context 80-95%)
    BackgroundStarted,
    /// Emergency hard compact performed (context >= 95%)
    /// Contains number of messages dropped
    HardCompacted(usize),
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

        let msg_tokens = total_chars / CHARS_PER_TOKEN;
        // Add overhead for system prompt + tool definitions, which are not in the message list
        // but do count toward the context limit. Scale the overhead to the budget so
        // tests with tiny budgets aren't affected.
        let overhead = if self.token_budget >= DEFAULT_TOKEN_BUDGET / 2 {
            SYSTEM_OVERHEAD_TOKENS
        } else {
            0
        };
        msg_tokens + overhead
    }

    /// Get current token estimate (backward compat — uses 0 messages, only summary + observed)
    pub fn token_estimate(&self) -> usize {
        let mut total_chars = 0;
        if let Some(ref summary) = self.active_summary {
            total_chars += summary.text.len();
        }
        let msg_tokens = total_chars / CHARS_PER_TOKEN;
        let overhead = if self.token_budget >= DEFAULT_TOKEN_BUDGET / 2 {
            SYSTEM_OVERHEAD_TOKENS
        } else {
            0
        };
        msg_tokens + overhead
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
        let msg_count = messages_to_summarize.len();
        let existing_summary = self.active_summary.clone();

        self.pending_cutoff = cutoff;

        // Spawn background task
        self.pending_task = Some(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let result = generate_summary(provider, messages_to_summarize, existing_summary).await;
            crate::logging::info(&format!(
                "Compaction finished in {:.2}s ({} messages summarized)",
                start.elapsed().as_secs_f64(),
                msg_count,
            ));
            result
        }));
    }

    /// Ensure context fits before an API call.
    ///
    /// Starts background compaction if above 80%. If context is critically full
    /// (>=95%), also performs an immediate hard-compact (drops old messages) so
    /// the next API call doesn't fail with "prompt too long".
    pub fn ensure_context_fits(
        &mut self,
        all_messages: &[Message],
        provider: Arc<dyn Provider>,
    ) -> CompactionAction {
        let was_compacting = self.is_compacting();
        self.maybe_start_compaction_with(all_messages, provider);
        let bg_started = !was_compacting && self.is_compacting();

        let usage = self.context_usage_with(all_messages);
        if usage >= CRITICAL_THRESHOLD {
            crate::logging::warn(&format!(
                "[compaction] Context at {:.1}% (critical threshold {:.0}%) — performing synchronous hard compact",
                usage * 100.0,
                CRITICAL_THRESHOLD * 100.0,
            ));
            match self.hard_compact_with(all_messages) {
                Ok(dropped) => {
                    let post_usage = self.context_usage_with(all_messages);
                    crate::logging::info(&format!(
                        "[compaction] Hard compact dropped {} messages, context now at {:.1}%",
                        dropped,
                        post_usage * 100.0,
                    ));
                    return CompactionAction::HardCompacted(dropped);
                }
                Err(reason) => {
                    crate::logging::error(&format!(
                        "[compaction] Hard compact failed at critical threshold: {}",
                        reason
                    ));
                }
            }
        }

        if bg_started {
            CompactionAction::BackgroundStarted
        } else {
            CompactionAction::None
        }
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
        let msg_count = messages_to_summarize.len();
        let existing_summary = self.active_summary.clone();

        self.pending_cutoff = cutoff;

        self.pending_task = Some(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let result = generate_summary(provider, messages_to_summarize, existing_summary).await;
            crate::logging::info(&format!(
                "Compaction finished in {:.2}s ({} messages summarized)",
                start.elapsed().as_secs_f64(),
                msg_count,
            ));
            result
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

    /// Get the character count of the active summary (0 if none)
    pub fn summary_chars(&self) -> usize {
        self.active_summary
            .as_ref()
            .map(|s| s.text.len())
            .unwrap_or(0)
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
    ///
    /// When the remaining turns (after keeping `RECENT_TURNS_TO_KEEP`) still
    /// exceed the token budget, progressively keeps fewer turns down to
    /// `MIN_TURNS_TO_KEEP`.
    pub fn hard_compact_with(&mut self, all_messages: &[Message]) -> Result<usize, String> {
        let active = self.active_messages(all_messages);

        if active.len() <= MIN_TURNS_TO_KEEP {
            return Err(format!(
                "Not enough messages to compact (have {}, need more than {})",
                active.len(),
                MIN_TURNS_TO_KEEP
            ));
        }

        let pre_tokens = self.effective_token_count_with(all_messages) as u64;

        let mut turns_to_keep = RECENT_TURNS_TO_KEEP.min(active.len().saturating_sub(1));
        let mut cutoff;
        loop {
            cutoff = active.len().saturating_sub(turns_to_keep);
            cutoff = Self::safe_cutoff_static(active, cutoff);

            if cutoff > 0 {
                let remaining: usize = active[cutoff..]
                    .iter()
                    .map(Self::message_char_count)
                    .sum();
                let remaining_tokens = remaining / CHARS_PER_TOKEN;
                if remaining_tokens <= self.token_budget {
                    break;
                }
            }

            if turns_to_keep <= MIN_TURNS_TO_KEEP {
                cutoff = active.len().saturating_sub(MIN_TURNS_TO_KEEP);
                cutoff = Self::safe_cutoff_static(active, cutoff);
                break;
            }
            turns_to_keep = (turns_to_keep / 2).max(MIN_TURNS_TO_KEEP);
        }

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

    /// Emergency truncation: shorten large tool results in active messages.
    ///
    /// When hard compaction isn't sufficient (the remaining few turns are
    /// individually too large), this truncates tool result content so the
    /// conversation can fit within the token budget.
    ///
    /// Returns the number of tool results that were truncated.
    pub fn emergency_truncate_with(&mut self, all_messages: &mut [Message]) -> usize {
        let start = self.compacted_count.min(all_messages.len());
        let active = &mut all_messages[start..];
        let mut truncated = 0;

        for msg in active.iter_mut() {
            for block in msg.content.iter_mut() {
                if let ContentBlock::ToolResult { content, .. } = block {
                    if content.len() > EMERGENCY_TOOL_RESULT_MAX_CHARS {
                        let original_len = content.len();
                        let keep_head = EMERGENCY_TOOL_RESULT_MAX_CHARS / 2;
                        let keep_tail = EMERGENCY_TOOL_RESULT_MAX_CHARS / 4;
                        let head = &content[..keep_head];
                        let tail_start = original_len.saturating_sub(keep_tail);
                        let tail = &content[tail_start..];
                        *content = format!(
                            "{}\n\n... [{} chars truncated for context recovery] ...\n\n{}",
                            head,
                            original_len - keep_head - keep_tail,
                            tail,
                        );
                        truncated += 1;
                    }
                }
            }
        }

        if truncated > 0 {
            self.observed_input_tokens = None;
        }
        truncated
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
        // 100 chars = ~25 tokens (plus 18k overhead for full budget)
        let messages = vec![make_text_message(Role::User, &"x".repeat(100))];
        let estimate = manager.token_estimate_with(&messages);
        // With DEFAULT_TOKEN_BUDGET and 18k overhead: 25 + 18000 = 18025
        assert!(estimate >= 18_000 && estimate < 19_000);
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

    // ── ensure_context_fits tests ──────────────────────────────

    #[tokio::test]
    async fn test_guard_below_80_does_nothing() {
        let mut manager = CompactionManager::new().with_budget(10_000);
        let mut messages = Vec::new();
        for i in 0..15 {
            messages.push(make_text_message(Role::User, &format!("msg {}", i)));
            manager.notify_message_added();
        }
        // Char estimate is tiny, observed tokens well below 80%
        manager.update_observed_input_tokens(5_000);

        let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
        let action = manager.ensure_context_fits(&messages, provider);
        assert_eq!(action, CompactionAction::None, "should do nothing below 80%");
        assert!(!manager.is_compacting(), "should NOT start background compaction below 80%");
        assert_eq!(manager.compacted_count, 0);
    }

    #[tokio::test]
    async fn test_guard_between_80_and_95_starts_background_only() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(make_text_message(Role::User, &format!("msg {}", i)));
            manager.notify_message_added();
        }
        // 85% usage — above 80% threshold but below 95% critical
        manager.update_observed_input_tokens(850);

        let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
        let action = manager.ensure_context_fits(&messages, provider);
        assert_eq!(action, CompactionAction::BackgroundStarted, "should start background compaction at 85%");
        assert!(manager.is_compacting(), "SHOULD start background compaction at 85%");
        assert_eq!(manager.compacted_count, 0, "compacted_count should stay 0 (no hard compact)");
    }

    #[tokio::test]
    async fn test_guard_at_95_triggers_hard_compact() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(make_text_message(
                Role::User,
                &format!("message {} with padding {}", i, "x".repeat(50)),
            ));
            manager.notify_message_added();
        }
        // 96% usage — above critical threshold
        manager.update_observed_input_tokens(960);

        let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
        let action = manager.ensure_context_fits(&messages, provider);
        assert!(matches!(action, CompactionAction::HardCompacted(_)), "SHOULD hard-compact at 96%");
        assert!(manager.compacted_count > 0, "compacted_count should increase after hard compact");
        assert!(manager.active_summary.is_some(), "should have an emergency summary");
    }

    #[tokio::test]
    async fn test_guard_at_100_percent_drops_messages() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        let mut messages = Vec::new();
        for i in 0..30 {
            messages.push(make_text_message(
                Role::User,
                &format!("turn {} content {}", i, "y".repeat(80)),
            ));
            manager.notify_message_added();
        }
        // Over 100% — simulates the exact bug scenario
        manager.update_observed_input_tokens(1_050);

        let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
        let action = manager.ensure_context_fits(&messages, provider);
        assert!(matches!(action, CompactionAction::HardCompacted(_)), "MUST hard-compact when over 100%");

        let api_messages = manager.messages_for_api_with(&messages);
        assert!(api_messages.len() < messages.len(), "API messages should be fewer after hard compact");
        // First message should be the emergency summary
        match &api_messages[0].content[0] {
            ContentBlock::Text { text, .. } => {
                assert!(text.contains("Previous Conversation Summary"));
                assert!(text.contains("Emergency compaction"));
            }
            _ => panic!("expected text summary block"),
        }
    }

    // ── hard_compact_with edge cases ────────────────────────────────

    #[test]
    fn test_hard_compact_too_few_messages() {
        let mut manager = CompactionManager::new().with_budget(100);
        let messages = vec![
            make_text_message(Role::User, "hello"),
            make_text_message(Role::Assistant, "hi"),
        ];
        manager.notify_message_added();
        manager.notify_message_added();

        let result = manager.hard_compact_with(&messages);
        assert!(result.is_err(), "should fail with only 2 messages (MIN_TURNS_TO_KEEP)");
    }

    #[test]
    fn test_hard_compact_preserves_recent_turns() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        let mut messages = Vec::new();
        for i in 0..25 {
            messages.push(make_text_message(
                Role::User,
                &format!("turn {}", i),
            ));
            manager.notify_message_added();
        }
        manager.update_observed_input_tokens(950);

        let dropped = manager.hard_compact_with(&messages).expect("should compact");
        assert!(dropped > 0, "should drop some messages");
        assert!(dropped < 25, "should not drop ALL messages");

        let api_messages = manager.messages_for_api_with(&messages);
        // Should have summary + recent turns
        assert!(api_messages.len() >= 2, "should keep at least MIN_TURNS_TO_KEEP + summary");
        assert!(api_messages.len() <= 15, "should have dropped a significant number");
    }

    // ── safe_cutoff_static: tool call/result pair integrity ─────────

    #[test]
    fn test_safe_cutoff_preserves_tool_pairs() {
        // Messages: [user, assistant(tool_use), user(tool_result), assistant, user]
        // If cutoff tries to split between tool_use and tool_result, it should back up
        let messages = vec![
            make_text_message(Role::User, "do something"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                }],
                timestamp: None,
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool_1".to_string(),
                    content: "file1.txt\nfile2.txt".to_string(),
                    is_error: Some(false),
                }],
                timestamp: None,
            },
            make_text_message(Role::Assistant, "I see the files"),
            make_text_message(Role::User, "thanks"),
        ];

        // Try to cut between tool_use (index 1) and tool_result (index 2)
        let cutoff = CompactionManager::safe_cutoff_static(&messages, 2);
        // Should move back to include the tool_use at index 1
        assert!(
            cutoff <= 1,
            "cutoff should back up to include tool_use (got {})",
            cutoff
        );
    }

    #[test]
    fn test_safe_cutoff_no_tool_pairs() {
        let messages = vec![
            make_text_message(Role::User, "hello"),
            make_text_message(Role::Assistant, "hi"),
            make_text_message(Role::User, "how are you"),
            make_text_message(Role::Assistant, "fine"),
        ];

        let cutoff = CompactionManager::safe_cutoff_static(&messages, 2);
        assert_eq!(cutoff, 2, "no tool pairs, cutoff should stay unchanged");
    }

    // ── emergency_truncate_with ─────────────────────────────────────

    #[test]
    fn test_emergency_truncate_large_tool_results() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        let big_result = "x".repeat(10_000); // Way over EMERGENCY_TOOL_RESULT_MAX_CHARS (4000)
        let mut messages = vec![
            make_text_message(Role::User, "run something"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "cat bigfile"}),
                }],
                timestamp: None,
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool_1".to_string(),
                    content: big_result.clone(),
                    is_error: Some(false),
                }],
                timestamp: None,
            },
            make_text_message(Role::Assistant, "that's a big file"),
        ];
        for _ in &messages {
            manager.notify_message_added();
        }

        let truncated = manager.emergency_truncate_with(&mut messages);
        assert_eq!(truncated, 1, "should truncate exactly 1 tool result");

        // Check the truncated content
        if let ContentBlock::ToolResult { content, .. } = &messages[2].content[0] {
            assert!(content.len() < big_result.len(), "content should be shorter");
            assert!(content.contains("truncated for context recovery"), "should have truncation marker");
        } else {
            panic!("expected tool result");
        }
    }

    #[test]
    fn test_emergency_truncate_skips_small_results() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        let mut messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool_1".to_string(),
                    content: "small output".to_string(),
                    is_error: Some(false),
                }],
                timestamp: None,
            },
        ];
        manager.notify_message_added();

        let truncated = manager.emergency_truncate_with(&mut messages);
        assert_eq!(truncated, 0, "should not truncate small results");
    }

    // ── Double compaction ───────────────────────────────────────────

    #[test]
    fn test_hard_compact_twice() {
        let mut manager = CompactionManager::new().with_budget(500);
        let mut messages = Vec::new();
        for i in 0..30 {
            messages.push(make_text_message(
                Role::User,
                &format!("turn {} {}", i, "z".repeat(40)),
            ));
            manager.notify_message_added();
        }
        manager.update_observed_input_tokens(480);

        // First hard compact
        let dropped1 = manager.hard_compact_with(&messages).expect("first compact should work");
        assert!(dropped1 > 0);
        let count_after_first = manager.compacted_count;

        // Simulate more messages arriving after first compact
        for i in 30..45 {
            messages.push(make_text_message(
                Role::User,
                &format!("turn {} {}", i, "z".repeat(40)),
            ));
            manager.notify_message_added();
        }
        manager.update_observed_input_tokens(490);

        // Second hard compact
        let dropped2 = manager.hard_compact_with(&messages).expect("second compact should work");
        assert!(dropped2 > 0);
        assert!(manager.compacted_count > count_after_first, "compacted_count should increase");

        // Summary should mention both compactions
        let api_messages = manager.messages_for_api_with(&messages);
        assert!(api_messages.len() < messages.len());
        match &api_messages[0].content[0] {
            ContentBlock::Text { text, .. } => {
                assert!(text.contains("Emergency compaction"));
            }
            _ => panic!("expected summary"),
        }
    }

    // ── messages_for_api_with after compaction ──────────────────────

    #[test]
    fn test_messages_for_api_with_summary_prepended() {
        let mut manager = CompactionManager::new().with_budget(500);
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(make_text_message(Role::User, &format!("turn {}", i)));
            manager.notify_message_added();
        }
        manager.update_observed_input_tokens(490);

        manager.hard_compact_with(&messages).expect("should compact");

        let api_msgs = manager.messages_for_api_with(&messages);
        // First message should be the summary
        assert_eq!(api_msgs[0].role, Role::User);
        match &api_msgs[0].content[0] {
            ContentBlock::Text { text, .. } => {
                assert!(text.starts_with("## Previous Conversation Summary"));
            }
            _ => panic!("expected text"),
        }
        // Remaining should be recent turns from original messages
        assert!(api_msgs.len() < messages.len());
    }

    // ── context_usage accuracy ──────────────────────────────────────

    #[test]
    fn test_context_usage_with_both_estimate_and_observed() {
        let mut manager = CompactionManager::new().with_budget(200_000);
        // Build messages totalling ~50k chars = ~12.5k token estimate
        let mut messages = Vec::new();
        for i in 0..50 {
            messages.push(make_text_message(
                Role::User,
                &format!("{} {}", i, "a".repeat(1000)),
            ));
            manager.notify_message_added();
        }

        // Without observed tokens, usage should be based on char estimate
        let usage_no_observed = manager.context_usage_with(&messages);
        assert!(usage_no_observed < 0.2, "char estimate should be low: {}", usage_no_observed);

        // With observed tokens at 160k, should use observed (higher) value
        manager.update_observed_input_tokens(160_000);
        let usage_with_observed = manager.context_usage_with(&messages);
        assert!(
            usage_with_observed >= 0.79,
            "should use observed tokens: {}",
            usage_with_observed
        );
    }

    #[test]
    fn test_context_usage_after_compaction_resets_observed() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(make_text_message(Role::User, &format!("msg {} pad {}", i, "x".repeat(50))));
            manager.notify_message_added();
        }
        manager.update_observed_input_tokens(960);

        // Hard compact should reset observed_input_tokens
        manager.hard_compact_with(&messages).expect("should compact");
        assert!(
            manager.observed_input_tokens.is_none(),
            "observed_input_tokens should be cleared after hard compact"
        );

        // After compaction, usage should be based on char estimate of remaining messages only
        let post_usage = manager.context_usage_with(&messages);
        // The remaining messages are small, so usage should be well below the critical threshold
        assert!(
            post_usage < CRITICAL_THRESHOLD,
            "post-compaction usage should be below critical: {}",
            post_usage
        );
    }
}
