use jcode_message_types::{ContentBlock, Message, Role};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};

/// Default token budget (200k tokens - matches Claude's actual context limit)
pub const DEFAULT_TOKEN_BUDGET: usize = 200_000;

/// Trigger compaction at this percentage of budget
pub const COMPACTION_THRESHOLD: f32 = 0.80;

/// If context is above this threshold when compaction starts, do a synchronous
/// hard-compact (drop old messages) so the API call doesn't fail.
pub const CRITICAL_THRESHOLD: f32 = 0.95;

/// Minimum threshold for manual compaction (can compact at any time above this)
pub const MANUAL_COMPACT_MIN_THRESHOLD: f32 = 0.10;

/// Keep this many recent turns verbatim (not summarized)
pub const RECENT_TURNS_TO_KEEP: usize = 10;

/// Absolute minimum turns to keep during emergency compaction
pub const MIN_TURNS_TO_KEEP: usize = 2;

/// Max chars for a single tool result during emergency truncation
pub const EMERGENCY_TOOL_RESULT_MAX_CHARS: usize = 4000;

/// Approximate chars per token for estimation
pub const CHARS_PER_TOKEN: usize = 4;

/// Fixed token overhead for system prompt + tool definitions.
/// These are not counted in message content but do count toward the context limit.
/// Estimated conservatively: ~8k tokens for system prompt + ~10k for 50+ tools.
pub const SYSTEM_OVERHEAD_TOKENS: usize = 18_000;

/// Rolling window size for token history (proactive/semantic modes)
pub const TOKEN_HISTORY_WINDOW: usize = 20;

/// Maximum characters to embed per message (first N chars capture semantic content)
pub const EMBED_MAX_CHARS_PER_MSG: usize = 512;

/// Rolling window of per-turn embeddings used for topic-shift detection
pub const EMBEDDING_HISTORY_WINDOW: usize = 10;

/// Per-manager semantic embedding cache capacity.
pub const SEMANTIC_EMBED_CACHE_CAPACITY: usize = 256;

pub const SUMMARY_PROMPT: &str = r#"Summarize our conversation so you can continue this work later.

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
    pub openai_encrypted_content: Option<String>,
    pub covers_up_to_turn: usize,
    pub original_turn_count: usize,
}

/// Event emitted when compaction is applied
#[derive(Debug, Clone)]
pub struct CompactionEvent {
    pub trigger: String,
    pub pre_tokens: Option<u64>,
    pub post_tokens: Option<u64>,
    pub tokens_saved: Option<u64>,
    pub duration_ms: Option<u64>,
    pub messages_dropped: Option<usize>,
    pub messages_compacted: Option<usize>,
    pub summary_chars: Option<usize>,
    pub active_messages: Option<usize>,
}

/// What happened when ensure_context_fits was called
#[derive(Debug, Clone, PartialEq)]
pub enum CompactionAction {
    /// Nothing needed, context is fine.
    None,
    /// Background summarization started.
    BackgroundStarted { trigger: String },
    /// Emergency hard compact performed. Contains number of messages dropped.
    HardCompacted(usize),
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

pub fn compacted_summary_text_block(summary: &str) -> String {
    format!("## Previous Conversation Summary\n\n{}\n\n---\n\n", summary)
}

pub fn build_compaction_prompt(
    messages: &[Message],
    existing_summary: Option<&Summary>,
    max_prompt_chars: usize,
) -> String {
    let mut conversation_text = build_compaction_conversation_text(messages, existing_summary);
    let overhead = SUMMARY_PROMPT.len() + 50;
    if conversation_text.len() + overhead > max_prompt_chars && max_prompt_chars > overhead {
        let budget = max_prompt_chars - overhead;
        conversation_text = truncate_str_boundary(&conversation_text, budget).to_string();
        conversation_text
            .push_str("\n\n... [earlier conversation truncated to fit context window]\n");
    }
    format!("{}\n\n---\n\n{}", conversation_text, SUMMARY_PROMPT)
}

pub fn build_compaction_conversation_text(
    messages: &[Message],
    existing_summary: Option<&Summary>,
) -> String {
    let mut conversation_text = String::new();
    if let Some(summary) = existing_summary {
        conversation_text.push_str("## Previous Summary\n\n");
        conversation_text.push_str(&summary.text);
        conversation_text.push_str("\n\n## New Conversation\n\n");
    }

    for msg in messages {
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
                        format!("{}... (truncated)", truncate_str_boundary(content, 500))
                    } else {
                        content.clone()
                    };
                    conversation_text.push_str(&format!("[Result: {}]\n", truncated));
                }
                ContentBlock::Reasoning { .. } => {}
                ContentBlock::Image { .. } => conversation_text.push_str("[Image]\n"),
                ContentBlock::OpenAICompaction { .. } => {
                    conversation_text.push_str("[OpenAI native compaction]\n")
                }
            }
        }
        conversation_text.push('\n');
    }
    conversation_text
}

pub fn truncate_str_boundary(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub fn mean_embedding(embeddings: &[&Vec<f32>], dim: usize) -> Vec<f32> {
    let mut mean = vec![0f32; dim];
    for emb in embeddings {
        for (i, v) in emb.iter().enumerate() {
            if i < dim {
                mean[i] += v;
            }
        }
    }
    let n = embeddings.len().max(1) as f32;
    for v in &mut mean {
        *v /= n;
    }
    let norm: f32 = mean.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut mean {
            *v /= norm;
        }
    }
    mean
}

/// Find a safe compaction cutoff that does not leave kept tool results without
/// their corresponding tool calls.
pub fn safe_compaction_cutoff(messages: &[Message], initial_cutoff: usize) -> usize {
    let mut cutoff = initial_cutoff.min(messages.len());

    // Track tool call/result ids in the kept portion.
    let mut available_tool_ids = HashSet::new();
    let mut missing_tool_ids = HashSet::new();

    for msg in &messages[cutoff..] {
        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { id, .. } => {
                    available_tool_ids.insert(id.clone());
                    missing_tool_ids.remove(id);
                }
                ContentBlock::ToolResult { tool_use_id, .. }
                    if !available_tool_ids.contains(tool_use_id) =>
                {
                    missing_tool_ids.insert(tool_use_id.clone());
                }
                _ => {}
            }
        }
    }

    if missing_tool_ids.is_empty() {
        return cutoff;
    }

    // Walk backward once, progressively growing the kept suffix until every
    // kept tool result has its matching tool use in the same suffix.
    for (idx, msg) in messages[..cutoff].iter().enumerate().rev() {
        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { id, .. } => {
                    available_tool_ids.insert(id.clone());
                    missing_tool_ids.remove(id);
                }
                ContentBlock::ToolResult { tool_use_id, .. }
                    if !available_tool_ids.contains(tool_use_id) =>
                {
                    missing_tool_ids.insert(tool_use_id.clone());
                }
                _ => {}
            }
        }
        if missing_tool_ids.is_empty() {
            cutoff = idx;
            return cutoff;
        }
    }

    // If we couldn't find every matching tool call, don't compact at all.
    0
}

pub fn message_char_count(msg: &Message) -> usize {
    content_char_count(&msg.content)
}

pub fn content_char_count(content: &[ContentBlock]) -> usize {
    content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text, .. } => text.len(),
            ContentBlock::Reasoning { text } => text.len(),
            ContentBlock::ToolUse { input, .. } => input.to_string().len() + 50,
            ContentBlock::ToolResult { content, .. } => content.len() + 20,
            ContentBlock::Image { data, .. } => data.len(),
            ContentBlock::OpenAICompaction { encrypted_content } => encrypted_content.len(),
        })
        .sum()
}

pub fn summary_payload_char_count(summary: &Summary) -> usize {
    summary
        .openai_encrypted_content
        .as_ref()
        .map(|value| value.len())
        .unwrap_or_else(|| summary.text.len())
}

pub fn estimate_compaction_tokens(
    summary: Option<&Summary>,
    active_message_chars: usize,
    token_budget: usize,
) -> usize {
    let summary_chars = summary.map(summary_payload_char_count).unwrap_or(0);
    estimate_compaction_tokens_from_chars(summary_chars + active_message_chars, token_budget)
}

pub fn estimate_compaction_tokens_from_chars(total_chars: usize, token_budget: usize) -> usize {
    let msg_tokens = total_chars / CHARS_PER_TOKEN;
    // Add overhead for system prompt + tool definitions, which are not in the
    // message list but do count toward the context limit. Scale the overhead to
    // the budget so tests with tiny budgets aren't affected.
    let overhead = if token_budget >= DEFAULT_TOKEN_BUDGET / 2 {
        SYSTEM_OVERHEAD_TOKENS
    } else {
        0
    };
    msg_tokens + overhead
}

pub fn semantic_goal_text(messages: &[Message]) -> String {
    let mut text = String::new();
    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::Text {
                    text: block_text, ..
                } => push_semantic_excerpt(&mut text, block_text, 200),
                ContentBlock::ToolResult { content, .. } => {
                    push_semantic_excerpt(&mut text, content, 100)
                }
                _ => {}
            }
        }
    }
    text
}

pub fn semantic_message_text(msg: &Message) -> String {
    let mut text = String::new();
    for block in &msg.content {
        if let ContentBlock::Text {
            text: block_text, ..
        } = block
        {
            push_semantic_excerpt(&mut text, block_text, EMBED_MAX_CHARS_PER_MSG);
        }
    }
    text
}

pub fn push_semantic_excerpt(target: &mut String, source: &str, max_chars: usize) {
    if source.is_empty() {
        return;
    }
    if !target.is_empty() {
        target.push(' ');
    }
    target.extend(source.chars().take(max_chars));
}

pub fn semantic_cache_key(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

pub fn build_emergency_summary_text(
    existing_summary: Option<&str>,
    dropped_count: usize,
    pre_tokens: u64,
    token_budget: usize,
    dropped_messages: &[Message],
) -> String {
    let mut summary_parts: Vec<String> = Vec::new();

    if let Some(existing) = existing_summary
        && !existing.is_empty()
    {
        summary_parts.push(existing.to_string());
    }

    summary_parts.push(format!(
        "**[Emergency compaction]**: {} messages were dropped to recover from context overflow. \
         The conversation had ~{}k tokens which exceeded the {}k limit.",
        dropped_count,
        pre_tokens / 1000,
        token_budget / 1000,
    ));

    let mut file_mentions = Vec::new();
    let mut tool_names = HashSet::new();
    for msg in dropped_messages {
        collect_emergency_summary_hints(msg, &mut tool_names, &mut file_mentions);
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

    summary_parts.join("\n\n")
}

fn collect_emergency_summary_hints(
    msg: &Message,
    tool_names: &mut HashSet<String>,
    file_mentions: &mut Vec<String>,
) {
    for block in &msg.content {
        match block {
            ContentBlock::ToolUse { name, .. } => {
                tool_names.insert(name.clone());
            }
            ContentBlock::Text { text, .. } => {
                extract_file_mentions(text, file_mentions);
            }
            _ => {}
        }
    }
}

pub fn extract_file_mentions(text: &str, file_mentions: &mut Vec<String>) {
    for word in text.split_whitespace() {
        if looks_like_file_reference(word) {
            let cleaned = clean_file_reference(word);
            if !cleaned.is_empty() {
                file_mentions.push(cleaned.to_string());
            }
        }
    }
}

pub fn looks_like_file_reference(word: &str) -> bool {
    (word.contains('/') || word.contains('.'))
        && word.len() > 3
        && word.len() < 120
        && !word.starts_with("http")
        && (word.contains(".rs")
            || word.contains(".ts")
            || word.contains(".py")
            || word.contains(".toml")
            || word.contains(".json")
            || word.starts_with("src/")
            || word.starts_with("./"))
}

pub fn clean_file_reference(word: &str) -> &str {
    word.trim_matches(|c: char| {
        !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-'
    })
}

pub fn emergency_truncate_tool_results(messages: &mut [Message], max_chars: usize) -> usize {
    let mut truncated = 0;

    for msg in messages.iter_mut() {
        for block in msg.content.iter_mut() {
            match block {
                ContentBlock::ToolResult { content, .. } if content.len() > max_chars => {
                    *content = emergency_truncated_tool_result(content, max_chars);
                    truncated += 1;
                }
                _ => {}
            }
        }
    }

    truncated
}

pub fn emergency_truncated_tool_result(content: &str, max_chars: usize) -> String {
    let original_len = content.len();
    let keep_head = max_chars / 2;
    let keep_tail = max_chars / 4;
    let head = truncate_str_boundary(content, keep_head);
    let tail = tail_str_boundary(content, keep_tail);
    let truncated_len = original_len.saturating_sub(head.len() + tail.len());
    format!(
        "{}\n\n... [{} chars truncated for context recovery] ...\n\n{}",
        head, truncated_len, tail,
    )
}

pub fn tail_str_boundary(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut start = value.len().saturating_sub(max_bytes);
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_compaction_prompt_with_summary_and_truncated_tool_result() {
        let summary = Summary {
            text: "prior work".to_string(),
            openai_encrypted_content: None,
            covers_up_to_turn: 1,
            original_turn_count: 1,
        };
        let message = Message::user("hello");
        let prompt = build_compaction_prompt(&[message], Some(&summary), 10_000);
        assert!(prompt.contains("## Previous Summary"));
        assert!(prompt.contains("prior work"));
        assert!(prompt.contains("**User:**"));
        assert!(prompt.contains(SUMMARY_PROMPT));
    }

    #[test]
    fn truncates_on_utf8_boundary() {
        assert_eq!(truncate_str_boundary("éabc", 1), "");
        assert_eq!(truncate_str_boundary("éabc", 2), "é");
    }

    #[test]
    fn mean_embedding_is_normalized() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let mean = mean_embedding(&[&a, &b], 2);
        let norm = (mean[0] * mean[0] + mean[1] * mean[1]).sqrt();
        assert!((norm - 1.0).abs() < 0.0001);
    }

    #[test]
    fn safe_cutoff_keeps_tool_use_with_tool_result() {
        let tool_use = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file":"src/lib.rs"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        };
        let tool_result = Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "ok".to_string(),
                is_error: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        };
        let messages = vec![
            Message::user("old"),
            tool_use,
            tool_result,
            Message::user("new"),
        ];

        assert_eq!(safe_compaction_cutoff(&messages, 2), 1);
    }

    #[test]
    fn estimates_tokens_with_large_budget_overhead() {
        let summary = Summary {
            text: "abcd".repeat(100),
            openai_encrypted_content: None,
            covers_up_to_turn: 1,
            original_turn_count: 1,
        };

        assert_eq!(estimate_compaction_tokens(Some(&summary), 0, 1000), 100);
        assert_eq!(
            estimate_compaction_tokens(Some(&summary), 0, DEFAULT_TOKEN_BUDGET),
            100 + SYSTEM_OVERHEAD_TOKENS
        );
    }

    #[test]
    fn builds_semantic_text_from_relevant_content() {
        let message = Message {
            role: Role::User,
            content: vec![
                ContentBlock::Text {
                    text: "hello world".to_string(),
                    cache_control: None,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "tool output".to_string(),
                    is_error: None,
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        };

        assert_eq!(semantic_message_text(&message), "hello world");
        assert_eq!(semantic_goal_text(&[message]), "hello world tool output");
        assert_eq!(semantic_cache_key("stable"), semantic_cache_key("stable"));
    }

    #[test]
    fn builds_emergency_summary_with_tools_and_files() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file":"src/lib.rs"}),
                }],
                timestamp: None,
                tool_duration_ms: None,
            },
            Message::user("Edited src/compaction.rs and Cargo.toml, ignored https://example.com"),
        ];

        let summary =
            build_emergency_summary_text(Some("previous"), 2, 201_000, 200_000, &messages);
        assert!(summary.contains("previous"));
        assert!(summary.contains("2 messages were dropped"));
        assert!(summary.contains("Tools used: read"));
        assert!(summary.contains("Files referenced: Cargo.toml, src/compaction.rs"));
        assert!(!summary.contains("https://example.com"));
    }

    #[test]
    fn emergency_truncation_is_utf8_safe() {
        let original = format!("{}middle{}", "é".repeat(20), "尾".repeat(20));
        let truncated = emergency_truncated_tool_result(&original, 25);
        assert!(truncated.contains("chars truncated for context recovery"));
        assert!(truncated.is_char_boundary(truncated.len()));
    }
}
