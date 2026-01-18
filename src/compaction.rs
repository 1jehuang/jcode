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

/// Default token budget (100k tokens)
const DEFAULT_TOKEN_BUDGET: usize = 100_000;

/// Trigger compaction at this percentage of budget
const COMPACTION_THRESHOLD: f32 = 0.80;

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

    /// Full conversation history for RAG (never compacted)
    full_history: Vec<Message>,
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
            full_history: Vec::new(),
        }
    }

    pub fn with_budget(mut self, budget: usize) -> Self {
        self.token_budget = budget;
        self
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

    /// Get current context usage as percentage
    pub fn context_usage(&self) -> f32 {
        self.token_estimate() as f32 / self.token_budget as f32
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
        let cutoff = self.messages.len().saturating_sub(RECENT_TURNS_TO_KEEP);
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

                self.pending_cutoff = 0;
            }
            Ok(Err(e)) => {
                eprintln!("[compaction] Failed to generate summary: {}", e);
                self.pending_cutoff = 0;
            }
            Err(e) => {
                eprintln!("[compaction] Task panicked: {}", e);
                self.pending_cutoff = 0;
            }
        }
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
            context_usage: self.context_usage(),
        }
    }

    fn message_char_count(msg: &Message) -> usize {
        msg.content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text, .. } => text.len(),
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
                    // Truncate long tool results
                    let truncated = if content.len() > 500 {
                        format!("{}... (truncated)", &content[..500])
                    } else {
                        content.clone()
                    };
                    conversation_text.push_str(&format!("[Result: {}]\n", truncated));
                }
            }
        }
        conversation_text.push('\n');
    }

    // Create summarization request
    let summary_request = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!("{}\n\n---\n\n{}", conversation_text, SUMMARY_PROMPT),
            cache_control: None,
        }],
    }];

    // Call provider (this uses remaining context budget)
    // For now, we'll use a simple completion
    // TODO: Add a simple complete method to Provider trait
    let response = provider
        .complete(
            &summary_request,
            &[],
            "You are a helpful assistant that summarizes conversations.",
            None,
        )
        .await?;

    // Collect response
    use futures::StreamExt;
    let mut summary = String::new();
    tokio::pin!(response);

    while let Some(event) = response.next().await {
        if let Ok(crate::message::StreamEvent::TextDelta(text)) = event {
            summary.push_str(&text);
        }
    }

    Ok(CompactionResult {
        summary,
        covers_up_to_turn: messages.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
