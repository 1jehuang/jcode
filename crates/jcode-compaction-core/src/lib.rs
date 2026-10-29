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

/// Max chars to keep for an inline image payload during emergency recovery.
/// Images are usually base64 screenshots; at hard-threshold time the useful
/// state should be represented by nearby tool text/summary, not by replaying a
/// huge raw image in the recent tail.
pub const EMERGENCY_IMAGE_MAX_CHARS: usize = 1024;

/// Approximate maximum request body size (in base64 characters) we aim to keep
/// the transcript under when recovering from a provider "request too large" /
/// 413 payload error. Anthropic rejects requests whose serialized body exceeds
/// roughly 32 MB; this distinct failure mode is driven almost entirely by inline
/// base64 images, which the normal token-budget accounting deliberately
/// undercounts (see `IMAGE_TOKEN_COST`). We target a conservative budget well
/// under the hard provider cap so a single retry reliably fits.
pub const PAYLOAD_IMAGE_CHAR_BUDGET: usize = 12 * 1024 * 1024;

/// Approximate chars per token for estimation
pub const CHARS_PER_TOKEN: usize = 4;

/// Approximate token cost charged for a single inline image.
///
/// Image content blocks carry base64-encoded payloads that are often hundreds
/// of kilobytes. Counting that raw base64 length as message text (len / 4)
/// massively overestimates the real context cost: providers tokenize images by
/// resolution, not by transport-encoded byte length, and a typical screenshot
/// costs on the order of ~1-2k tokens regardless of base64 size. Using the raw
/// length caused the token estimate to balloon far above the real
/// provider-observed input, spuriously tripping the compaction threshold and
/// driving repeated back-to-back ("triple") compactions that could not bring
/// the estimate down because the images stayed in the recent kept turns.
///
/// We charge a flat, slightly conservative per-image token budget instead.
pub const IMAGE_TOKEN_COST: usize = 1_600;

/// Approximate token cost charged for an OpenAI native compaction payload.
///
/// The payload is opaque ciphertext whose length bears no relation to the
/// tokens the provider replays: a 1.1M-char payload was observed costing far
/// less than 30k tokens while `len / 4` reported ~285k, so compaction appeared
/// to *grow* the context. Charge a flat, conservative budget instead.
pub const OPENAI_COMPACTION_TOKEN_COST: usize = 8_000;

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
        // Provider-native search call inputs, so a result can name its query.
        let mut native_call_inputs: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::new();
        for block in &msg.content {
            match block {
                ContentBlock::ProviderNative { provider, item } => {
                    use jcode_message_types::provider_native;
                    let Some(display) = provider_native::provider_native_display(provider, item)
                    else {
                        continue;
                    };
                    if display.output.is_none() {
                        if let Some(input) = display.input {
                            native_call_inputs.insert(display.id, input);
                        }
                        continue;
                    }
                    if let Some(text) = provider_native::provider_native_text_fallback(
                        provider,
                        item,
                        native_call_inputs.get(&display.id),
                    ) {
                        let truncated = if text.len() > 500 {
                            format!("{}... (truncated)", truncate_str_boundary(&text, 500))
                        } else {
                            text
                        };
                        conversation_text.push_str(&format!("[Result: {}]\n", truncated));
                    }
                }
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
                ContentBlock::Reasoning { .. }
                | ContentBlock::ReasoningTrace { .. }
                | ContentBlock::AnthropicThinking { .. }
                | ContentBlock::OpenAIReasoning { .. }
                | ContentBlock::ToolReference { .. } => {}
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
            ContentBlock::ReasoningTrace { text } => text.len(),
            ContentBlock::AnthropicThinking {
                thinking,
                signature,
            } => thinking.len() + signature.len(),
            ContentBlock::OpenAIReasoning {
                id,
                summary,
                encrypted_content,
                status,
            } => {
                id.len()
                    + summary.iter().map(String::len).sum::<usize>()
                    + encrypted_content.as_ref().map(String::len).unwrap_or(0)
                    + status.as_ref().map(String::len).unwrap_or(0)
            }
            ContentBlock::ToolUse { input, .. } => input.to_string().len() + 50,
            ContentBlock::ToolResult { content, .. } => content.len() + 20,
            // Charge a flat token cost for images instead of the raw base64
            // payload length. See IMAGE_TOKEN_COST: counting base64 length here
            // overestimates context by ~100x and triggers spurious repeated
            // compactions.
            ContentBlock::Image { .. } => IMAGE_TOKEN_COST * CHARS_PER_TOKEN,
            ContentBlock::OpenAICompaction { .. } => OPENAI_COMPACTION_TOKEN_COST * CHARS_PER_TOKEN,
            // The provider expands a reference into the full definition, but
            // that definition is already accounted for by the tool catalog.
            ContentBlock::ToolReference { tool_name, .. } => tool_name.len() + 20,
            // Search results are billed as input tokens on replay; the raw JSON
            // (mostly encrypted page content) is a reasonable size proxy.
            ContentBlock::ProviderNative { item, .. } => item.to_string().len(),
        })
        .sum()
}

pub fn summary_payload_char_count(summary: &Summary) -> usize {
    summary
        .openai_encrypted_content
        .as_ref()
        .map(|_| OPENAI_COMPACTION_TOKEN_COST * CHARS_PER_TOKEN)
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

/// Best-effort context size (tokens) from a provider usage report.
///
/// Providers disagree on what `input_tokens` means:
/// - **Split accounting** (Anthropic-style): `input_tokens` is only the
///   *uncached* remainder; cache reads/writes are separate counters, so the
///   real context size is `input + cache_read + cache_creation`.
/// - **Subset accounting** (OpenAI-style): `input_tokens` (`prompt_tokens`)
///   already includes cached tokens; `cached_tokens` is a subset and must NOT
///   be added again.
///
/// This is the single source of truth for that heuristic. Both the sidebar
/// context figure and the compaction manager's observed-token feed must use it
/// so the two never disagree (issue #441). When in doubt, avoid over-counting
/// unless there is strong evidence of split accounting.
pub fn effective_context_tokens_from_usage(
    provider_name: &str,
    input_tokens: u64,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
) -> u64 {
    let cache_read = cache_read_input_tokens.unwrap_or(0);
    let cache_creation = cache_creation_input_tokens.unwrap_or(0);
    let provider_name = provider_name.to_lowercase();

    // OpenAI cache writes, like reads, are subsets of the inclusive input count.
    if provider_name.contains("openai") || provider_name.contains("codex") {
        return input_tokens;
    }

    let split_cache_accounting = provider_name.contains("anthropic")
        || provider_name.contains("claude")
        || cache_creation > 0
        || cache_read > input_tokens;

    if split_cache_accounting {
        input_tokens
            .saturating_add(cache_read)
            .saturating_add(cache_creation)
    } else {
        input_tokens
    }
}

/// Previously reusable cache boundary, not the full context size. With split
/// accounting the uncached input tail was never written and cannot be expected
/// as a cache read on the next request. Cache creations, however, are reusable.
pub fn reusable_cache_tokens_from_usage(
    provider_name: &str,
    input_tokens: u64,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
) -> u64 {
    let effective = effective_context_tokens_from_usage(
        provider_name,
        input_tokens,
        cache_read_input_tokens,
        cache_creation_input_tokens,
    );
    let provider = provider_name.to_lowercase();
    if provider.contains("openai") || provider.contains("codex") {
        return input_tokens;
    }
    if provider.contains("anthropic") || provider.contains("claude") || effective > input_tokens {
        cache_read_input_tokens
            .unwrap_or(0)
            .saturating_add(cache_creation_input_tokens.unwrap_or(0))
    } else {
        input_tokens
    }
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

pub fn emergency_truncate_large_payloads(
    messages: &mut [Message],
    max_tool_result_chars: usize,
    max_image_chars: usize,
) -> usize {
    let mut truncated = 0;

    for msg in messages.iter_mut() {
        for block in msg.content.iter_mut() {
            match block {
                ContentBlock::ToolResult { content, .. }
                    if content.len() > max_tool_result_chars =>
                {
                    *content = emergency_truncated_tool_result(content, max_tool_result_chars);
                    truncated += 1;
                }
                ContentBlock::Image { media_type, data } if data.len() > max_image_chars => {
                    let original_len = data.len();
                    let media_type = media_type.clone();
                    *block = ContentBlock::Text {
                        text: format!(
                            "[Image omitted during emergency context recovery: media_type={media_type}, original_base64_chars={original_len}. Rely on adjacent browser/tool text, screenshots saved to disk, or re-open/re-screenshot if visual details are needed.]"
                        ),
                        cache_control: None,
                    };
                    truncated += 1;
                }
                _ => {}
            }
        }
    }

    truncated
}

/// Whether a provider error is a deterministic rejection of an inline image
/// (a 400 `invalid_request_error` naming an image block: unsupported
/// `media_type`, undecodable data, dimension or animation limits). The same
/// history would be rejected on every retry, so the agent strips images and
/// retries once instead of leaving the session permanently broken (#1712).
pub fn is_image_rejection_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    if is_request_payload_too_large_error(&lower) {
        return false;
    }
    let names_image_block = lower.contains(".image.source")
        || lower.contains(".image.")
        || lower.contains("image.source")
        || lower.contains("could not process image")
        || lower.contains("invalid image")
        || lower.contains("unsupported image")
        || lower.contains("image_parse_error")
        || lower.contains("invalid_image");
    let is_client_rejection = lower.contains("invalid_request_error")
        || lower.contains("invalid_image")
        || lower.contains("image_parse_error")
        || contains_independent_status_code(&lower, "400");
    names_image_block && is_client_rejection
}

/// Replace every inline image with a short text note. Used after a provider
/// rejected an image so the retried request carries none. Returns the number
/// of images replaced.
pub fn strip_all_images_in_contents(contents: &mut [&mut Vec<ContentBlock>]) -> usize {
    let mut stripped = 0;
    for content in contents.iter_mut() {
        for block in content.iter_mut() {
            if let ContentBlock::Image { media_type, .. } = block {
                let media_type = media_type.clone();
                *block = ContentBlock::Text {
                    text: format!(
                        "[Image omitted: the provider rejected an image in this conversation (media_type={media_type}). Re-open or re-screenshot it in PNG/JPEG if visual details are needed.]"
                    ),
                    cache_control: None,
                };
                stripped += 1;
            }
        }
    }
    stripped
}

/// Whether a provider error indicates the *serialized request body* was too
/// large (HTTP 413), as distinct from exceeding the model's token context
/// window. Anthropic surfaces this as `request_too_large` / "Request exceeds the
/// maximum size" / "413 Payload Too Large"; OpenAI and gateways use similar
/// wording. This failure mode is dominated by inline base64 images, which the
/// token-budget accounting deliberately undercounts, so it needs a dedicated
/// byte-size recovery rather than ordinary context compaction.
pub fn is_request_payload_too_large_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("request_too_large")
        || lower.contains("request too large")
        || lower.contains("payload too large")
        || lower.contains("request entity too large")
        || lower.contains("request exceeds the maximum size")
        || lower.contains("exceeds the maximum size")
        || contains_independent_status_code(&lower, "413")
}

/// Whether `haystack` contains `code` as a standalone status code rather than as
/// a fragment of a longer number (so "413" matches but "4130"/"version 4131"
/// does not). Mirrors the failover classifier's guard.
fn contains_independent_status_code(haystack: &str, code: &str) -> bool {
    let bytes = haystack.as_bytes();
    haystack.match_indices(code).any(|(start, _)| {
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_digit();
        let end = start + code.len();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_digit();
        before_ok && after_ok
    })
}

/// Strip oversized inline images from `messages`, oldest-first, until the total
/// remaining base64 image payload fits within `target_total_chars`.
///
/// Unlike [`emergency_truncate_large_payloads`] (which is driven by the token
/// budget and replaces *every* image past a tiny per-image cap), this is the
/// byte-size recovery path for HTTP 413 "request too large" errors: it preserves
/// as many of the most recent images as the request size budget allows and only
/// drops the older ones. Each stripped image is replaced with a text marker so
/// the model still knows an image existed and where to recover it from.
///
/// Returns the number of images that were replaced with text markers.
pub fn emergency_strip_large_images(messages: &mut [Message], target_total_chars: usize) -> usize {
    let mut contents: Vec<&mut Vec<ContentBlock>> =
        messages.iter_mut().map(|m| &mut m.content).collect();
    strip_large_images_in_contents(&mut contents, target_total_chars)
}

/// Core of [`emergency_strip_large_images`], operating directly on a slice of
/// content-block vectors so it can be reused for both provider `Message`s and
/// the session's stored-message representation (which share `ContentBlock`).
pub fn strip_large_images_in_contents(
    contents: &mut [&mut Vec<ContentBlock>],
    target_total_chars: usize,
) -> usize {
    // Collect (content_index, block_index, payload_len) for every inline image,
    // in transcript order (oldest first).
    let mut images: Vec<(usize, usize, usize)> = Vec::new();
    let mut total: usize = 0;
    for (ci, content) in contents.iter().enumerate() {
        for (bi, block) in content.iter().enumerate() {
            if let ContentBlock::Image { data, .. } = block {
                images.push((ci, bi, data.len()));
                total = total.saturating_add(data.len());
            }
        }
    }

    if total <= target_total_chars {
        return 0;
    }

    let mut stripped = 0;
    // Drop oldest images first until we're under budget (always keep trying even
    // if a single huge recent image alone exceeds the budget — better to ship a
    // request the provider might still trim than to give up entirely).
    for (ci, bi, payload_len) in images {
        if total <= target_total_chars {
            break;
        }
        let block = &mut contents[ci][bi];
        if let ContentBlock::Image { media_type, data } = block {
            let original_len = data.len();
            let media_type = media_type.clone();
            *block = ContentBlock::Text {
                text: format!(
                    "[Image omitted during request-size recovery: media_type={media_type}, original_base64_chars={original_len}. The request body exceeded the provider size limit; older images were dropped. Rely on adjacent browser/tool text, screenshots saved to disk, or re-open/re-screenshot if visual details are needed.]"
                ),
                cache_control: None,
            };
            total = total.saturating_sub(payload_len);
            stripped += 1;
        }
    }

    stripped
}

/// Inline tool-result images (computer-use and browser screenshots) are decayed
/// once the transcript holds more than this many of them. See
/// [`decay_old_tool_images_in_contents`].
pub const TOOL_IMAGE_DECAY_TRIGGER_COUNT: usize = 12;

/// Inline tool-result images are also decayed once their combined base64
/// payload exceeds this many characters.
pub const TOOL_IMAGE_DECAY_TRIGGER_CHARS: usize = 48 * 1024 * 1024;

/// After decay, at most this many of the newest tool-result images keep their
/// inline data. The gap to the trigger count is hysteresis: decay rewrites
/// older messages (invalidating provider prompt caches and forcing a full
/// snapshot write), so it only runs every few screenshots instead of on each.
pub const TOOL_IMAGE_DECAY_KEEP_COUNT: usize = 6;

/// After decay, the retained tool-result images fit within this many base64
/// characters, except that the newest image is always kept regardless of size.
pub const TOOL_IMAGE_DECAY_KEEP_CHARS: usize = 24 * 1024 * 1024;

/// Bound the inline image payload that tool results (screenshots from the
/// computer and browser tools) accumulate in a transcript.
///
/// Only images in messages that carry a `ToolResult` are considered, so images
/// the user pasted are untouched. Nothing happens until the tool images exceed
/// [`TOOL_IMAGE_DECAY_TRIGGER_COUNT`] or [`TOOL_IMAGE_DECAY_TRIGGER_CHARS`].
/// Then every tool image except the newest [`TOOL_IMAGE_DECAY_KEEP_COUNT`]
/// (that also fit within [`TOOL_IMAGE_DECAY_KEEP_CHARS`]) is replaced with a
/// short text marker. The newest image always survives so the model still sees
/// the screenshot it just asked for.
///
/// Returns the number of images replaced.
pub fn decay_old_tool_images_in_contents(contents: &mut [&mut Vec<ContentBlock>]) -> usize {
    let mut images: Vec<(usize, usize, usize)> = Vec::new();
    let mut total: usize = 0;
    for (ci, content) in contents.iter().enumerate() {
        if !content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        {
            continue;
        }
        for (bi, block) in content.iter().enumerate() {
            if let ContentBlock::Image { data, .. } = block {
                images.push((ci, bi, data.len()));
                total = total.saturating_add(data.len());
            }
        }
    }

    if images.len() <= TOOL_IMAGE_DECAY_TRIGGER_COUNT && total <= TOOL_IMAGE_DECAY_TRIGGER_CHARS {
        return 0;
    }

    let mut kept_count = 0usize;
    let mut kept_chars = 0usize;
    let mut stripped = 0usize;
    for (ci, bi, payload_len) in images.into_iter().rev() {
        let keep = kept_count == 0
            || (kept_count < TOOL_IMAGE_DECAY_KEEP_COUNT
                && kept_chars.saturating_add(payload_len) <= TOOL_IMAGE_DECAY_KEEP_CHARS);
        if keep {
            kept_count += 1;
            kept_chars = kept_chars.saturating_add(payload_len);
            continue;
        }
        let block = &mut contents[ci][bi];
        if let ContentBlock::Image { media_type, data } = block {
            let text = format!(
                "[Older tool screenshot omitted from history to bound transcript size: media_type={media_type}, original_base64_chars={}. Rely on adjacent tool text, or take a new screenshot if visual details are needed.]",
                data.len()
            );
            *block = ContentBlock::Text {
                text,
                cache_control: None,
            };
            stripped += 1;
        }
    }
    stripped
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
mod tests;

#[cfg(test)]
#[path = "image_rejection_tests.rs"]
mod image_rejection_tests;
