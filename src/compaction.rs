//! Background compaction for conversation context management
//!
//! When context reaches 80% of the limit, kicks off background summarization.
//! User continues chatting while summary is generated. When ready, seamlessly
//! swaps in the compacted context.

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

/// Manages background compaction of conversation context
pub struct CompactionManager {
    /// All messages in current context (may be compacted)
    messages: Vec<Message>,

    /// Active summary (if we've compacted before)
    active_summary: Option<Summary>,

    /// Background compaction task handle
    pending_task: Option<JoinHandle<Result<CompactionResult>>>,

    /// Turn index where pending compaction will cut off
    pending_cutoff: usize,

    /// Total turns seen (for tracking)
    total_turns: usize,

    /// Token budget
    token_budget: usize,

    /// Provider-reported input token usage from the latest request.
    /// Used to trigger compaction with real token counts instead of only heuristics.
    observed_input_tokens: Option<u64>,

    /// Full conversation history for RAG (never compacted)
    full_history: Vec<Message>,

    /// Last compaction event (if any)
    last_compaction: Option<CompactionEvent>,
}

impl CompactionManager {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            active_summary: None,
            pending_task: None,
            pending_cutoff: 0,
            total_turns: 0,
            token_budget: DEFAULT_TOKEN_BUDGET,
            observed_input_tokens: None,
            full_history: Vec::new(),
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

    /// Add a message to the conversation
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message.clone());
        self.full_history.push(message);
        self.total_turns += 1;
    }

    /// Get current token estimate
    pub fn token_estimate(&self) -> usize {
        let mut total_chars = 0;

        // Count summary if present
        if let Some(ref summary) = self.active_summary {
            total_chars += summary.text.len();
        }

        // Count all messages
        for msg in &self.messages {
            total_chars += Self::message_char_count(msg);
        }

        total_chars / CHARS_PER_TOKEN
    }

    /// Store provider-reported input token usage for compaction decisions.
    pub fn update_observed_input_tokens(&mut self, tokens: u64) {
        self.observed_input_tokens = Some(tokens);
    }

    /// Best-effort current token count for compaction decisions.
    /// Uses whichever is larger: char-based estimate or provider-reported usage.
    pub fn effective_token_count(&self) -> usize {
        let estimate = self.token_estimate();
        let observed = self
            .observed_input_tokens
            .and_then(|tokens| usize::try_from(tokens).ok())
            .unwrap_or(0);
        estimate.max(observed)
    }

    /// Get current context usage as percentage
    pub fn context_usage(&self) -> f32 {
        self.effective_token_count() as f32 / self.token_budget as f32
    }

    /// Check if we should start compaction
    pub fn should_compact(&self) -> bool {
        self.pending_task.is_none()
            && self.context_usage() >= COMPACTION_THRESHOLD
            && self.messages.len() > RECENT_TURNS_TO_KEEP
    }

    /// Start background compaction if needed
    pub fn maybe_start_compaction(&mut self, provider: Arc<dyn Provider>) {
        if !self.should_compact() {
            return;
        }

        // Calculate cutoff - keep last N turns verbatim
        let mut cutoff = self.messages.len().saturating_sub(RECENT_TURNS_TO_KEEP);
        if cutoff == 0 {
            return;
        }

        // Adjust cutoff to not split tool call/result pairs
        cutoff = self.safe_cutoff(cutoff);
        if cutoff == 0 {
            return;
        }

        // Snapshot messages to summarize
        let messages_to_summarize: Vec<Message> = self.messages[..cutoff].to_vec();
        let existing_summary = self.active_summary.clone();

        self.pending_cutoff = cutoff;

        // Spawn background task
        self.pending_task = Some(tokio::spawn(async move {
            generate_summary(provider, messages_to_summarize, existing_summary).await
        }));
    }

    /// Force immediate compaction (for manual /compact command).
    /// Returns Ok(()) if compaction started, Err with reason if not.
    pub fn force_compact(&mut self, provider: Arc<dyn Provider>) -> Result<(), String> {
        // Check if already compacting
        if self.pending_task.is_some() {
            return Err("Compaction already in progress".to_string());
        }

        // Need at least some messages to compact
        if self.messages.len() <= RECENT_TURNS_TO_KEEP {
            return Err(format!(
                "Not enough messages to compact (need more than {}, have {})",
                RECENT_TURNS_TO_KEEP,
                self.messages.len()
            ));
        }

        // Check minimum threshold
        if self.context_usage() < MANUAL_COMPACT_MIN_THRESHOLD {
            return Err(format!(
                "Context usage too low ({:.1}%) - nothing to compact",
                self.context_usage() * 100.0
            ));
        }

        // Calculate cutoff - keep last N turns verbatim
        let mut cutoff = self.messages.len().saturating_sub(RECENT_TURNS_TO_KEEP);
        if cutoff == 0 {
            return Err("No messages available to compact after keeping recent turns".to_string());
        }

        // Adjust cutoff to not split tool call/result pairs
        cutoff = self.safe_cutoff(cutoff);
        if cutoff == 0 {
            return Err("Cannot compact - would split tool call/result pairs".to_string());
        }

        // Snapshot messages to summarize
        let messages_to_summarize: Vec<Message> = self.messages[..cutoff].to_vec();
        let existing_summary = self.active_summary.clone();

        self.pending_cutoff = cutoff;

        // Spawn background task
        self.pending_task = Some(tokio::spawn(async move {
            generate_summary(provider, messages_to_summarize, existing_summary).await
        }));

        Ok(())
    }

    /// Find a safe cutoff point that doesn't split tool call/result pairs.
    /// Returns an adjusted cutoff that ensures all ToolResults in kept messages
    /// have their corresponding ToolUse in the kept messages too.
    fn safe_cutoff(&self, initial_cutoff: usize) -> usize {
        use std::collections::HashSet;

        let mut cutoff = initial_cutoff;

        // Collect tool_use_ids from ToolResults in the "kept" portion (after cutoff)
        let mut needed_tool_ids: HashSet<String> = HashSet::new();
        for msg in &self.messages[cutoff..] {
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
        for msg in &self.messages[cutoff..] {
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
        for (idx, msg) in self.messages[..cutoff].iter().enumerate().rev() {
            let mut found_any = false;
            for block in &msg.content {
                if let ContentBlock::ToolUse { id, .. } = block {
                    if missing.contains(id) {
                        found_any = true;
                    }
                }
            }
            if found_any {
                // Include this message and all after it in the kept portion
                cutoff = idx;
                // Recursively check if moving cutoff back created new orphans
                return self.safe_cutoff_from(cutoff);
            }
        }

        // If we couldn't find all tool calls, don't compact at all
        0
    }

    /// Helper for recursive safe_cutoff calculation
    fn safe_cutoff_from(&self, cutoff: usize) -> usize {
        if cutoff == 0 {
            return 0;
        }
        self.safe_cutoff(cutoff)
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
                // Create new summary
                let summary = Summary {
                    text: result.summary,
                    covers_up_to_turn: result.covers_up_to_turn,
                    original_turn_count: self.pending_cutoff,
                };

                // Remove compacted messages
                self.messages.drain(..self.pending_cutoff);

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

    /// Get messages for API call (with summary if compacted)
    pub fn messages_for_api(&mut self) -> Vec<Message> {
        // First check if pending compaction is done
        self.check_and_apply_compaction();

        match &self.active_summary {
            Some(summary) => {
                // Prepend summary as system-style context
                let summary_block = ContentBlock::Text {
                    text: format!(
                        "## Previous Conversation Summary\n\n{}\n\n---\n\n",
                        summary.text
                    ),
                    cache_control: None,
                };

                let mut result = Vec::with_capacity(self.messages.len() + 1);

                // Add summary as first user message with context
                result.push(Message {
                    role: Role::User,
                    content: vec![summary_block],
                });

                // Add remaining messages
                result.extend(self.messages.clone());

                result
            }
            None => self.messages.clone(),
        }
    }

    /// Get full history for RAG search
    pub fn full_history(&self) -> &[Message] {
        &self.full_history
    }

    /// Search full history by keyword
    pub fn search_history(&self, query: &str) -> Vec<SearchResult> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for (idx, msg) in self.full_history.iter().enumerate() {
            let text = Self::message_to_text(msg);
            if text.to_lowercase().contains(&query_lower) {
                // Find matching snippet
                let snippet = Self::extract_snippet(&text, &query_lower);
                results.push(SearchResult {
                    turn: idx,
                    role: msg.role.clone(),
                    snippet,
                });
            }
        }

        results
    }

    /// Get specific turns from history
    pub fn get_turns(&self, start: usize, end: usize) -> Vec<&Message> {
        self.full_history
            .iter()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect()
    }

    /// Check if compaction is in progress
    pub fn is_compacting(&self) -> bool {
        self.pending_task.is_some()
    }

    /// Get stats about current state
    pub fn stats(&self) -> CompactionStats {
        CompactionStats {
            total_turns: self.total_turns,
            active_messages: self.messages.len(),
            has_summary: self.active_summary.is_some(),
            is_compacting: self.is_compacting(),
            token_estimate: self.token_estimate(),
            effective_tokens: self.effective_token_count(),
            observed_input_tokens: self.observed_input_tokens,
            context_usage: self.context_usage(),
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
            })
            .sum()
    }

    fn message_to_text(msg: &Message) -> String {
        msg.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                ContentBlock::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn extract_snippet(text: &str, query: &str) -> String {
        let lower = text.to_lowercase();
        if let Some(pos) = lower.find(query) {
            let start = pos.saturating_sub(50);
            let end = (pos + query.len() + 50).min(text.len());
            let mut snippet = text[start..end].to_string();
            if start > 0 {
                snippet = format!("...{}", snippet);
            }
            if end < text.len() {
                snippet = format!("{}...", snippet);
            }
            snippet
        } else {
            text.chars().take(100).collect()
        }
    }
}

impl Default for CompactionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Search result from conversation history
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub turn: usize,
    pub role: Role,
    pub snippet: String,
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
                    // Truncate long tool results (respecting UTF-8 char boundaries)
                    let truncated = if content.len() > 500 {
                        format!("{}... (truncated)", crate::util::truncate_str(content, 500))
                    } else {
                        content.clone()
                    };
                    conversation_text.push_str(&format!("[Result: {}]\n", truncated));
                }
                ContentBlock::Reasoning { .. } => {}
            }
        }
        conversation_text.push('\n');
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
        }
    }

    #[test]
    fn test_new_manager() {
        let manager = CompactionManager::new();
        assert_eq!(manager.messages.len(), 0);
        assert!(manager.active_summary.is_none());
        assert!(!manager.is_compacting());
    }

    #[test]
    fn test_add_message() {
        let mut manager = CompactionManager::new();
        manager.add_message(make_text_message(Role::User, "Hello"));
        manager.add_message(make_text_message(Role::Assistant, "Hi there!"));

        assert_eq!(manager.messages.len(), 2);
        assert_eq!(manager.full_history.len(), 2);
        assert_eq!(manager.total_turns, 2);
    }

    #[test]
    fn test_token_estimate() {
        let mut manager = CompactionManager::new();
        // 100 chars = ~25 tokens
        manager.add_message(make_text_message(Role::User, &"x".repeat(100)));

        let estimate = manager.token_estimate();
        assert!(estimate > 20 && estimate < 30);
    }

    #[test]
    fn test_should_compact() {
        let mut manager = CompactionManager::new().with_budget(100); // Very small budget

        // Add enough messages to trigger compaction
        for i in 0..20 {
            manager.add_message(make_text_message(
                Role::User,
                &format!("Message {} with some content", i),
            ));
        }

        assert!(manager.should_compact());
    }

    #[test]
    fn test_context_usage_prefers_observed_tokens() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        manager.add_message(make_text_message(Role::User, "short message"));
        manager.update_observed_input_tokens(900);

        assert!(manager.context_usage() >= 0.90);
        assert!(manager.effective_token_count() >= 900);
    }

    #[test]
    fn test_should_compact_uses_observed_tokens() {
        let mut manager = CompactionManager::new().with_budget(1_000);

        // Keep messages short so estimate stays low; compaction should still trigger from observed usage.
        for _ in 0..12 {
            manager.add_message(make_text_message(Role::User, "x"));
        }
        manager.update_observed_input_tokens(850);

        assert!(manager.should_compact());
    }

    #[test]
    fn test_search_history() {
        let mut manager = CompactionManager::new();
        manager.add_message(make_text_message(Role::User, "Fix the authentication bug"));
        manager.add_message(make_text_message(Role::Assistant, "I'll look at auth.rs"));
        manager.add_message(make_text_message(Role::User, "Also check the database"));

        let results = manager.search_history("auth");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_get_turns() {
        let mut manager = CompactionManager::new();
        for i in 0..10 {
            manager.add_message(make_text_message(Role::User, &format!("Turn {}", i)));
        }

        let turns = manager.get_turns(3, 6);
        assert_eq!(turns.len(), 3);
    }

    #[test]
    fn test_messages_for_api_no_summary() {
        let mut manager = CompactionManager::new();
        manager.add_message(make_text_message(Role::User, "Hello"));
        manager.add_message(make_text_message(Role::Assistant, "Hi!"));

        let msgs = manager.messages_for_api();
        assert_eq!(msgs.len(), 2);
    }

    #[tokio::test]
    async fn test_force_compact_applies_summary() {
        let mut manager = CompactionManager::new().with_budget(1_000);
        for i in 0..30 {
            manager.add_message(make_text_message(
                Role::User,
                &format!("Turn {} {}", i, "x".repeat(120)),
            ));
        }

        let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
        manager
            .force_compact(provider)
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

        let msgs = manager.messages_for_api();
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
