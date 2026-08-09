const MEMORY_CONTEXT_MAX_CHARS: usize = 8_000;
const MEMORY_CONTEXT_MAX_MESSAGES: usize = 12;
const MEMORY_CONTEXT_MAX_BLOCK_CHARS: usize = 1_200;
const EXTRACTION_CONTEXT_MAX_MESSAGES: usize = 40;
const EXTRACTION_CONTEXT_MAX_CHARS: usize = 24_000;

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}

fn format_content_block_for_relevance(block: &crate::message::ContentBlock) -> Option<String> {
    match block {
        crate::message::ContentBlock::Text { text, .. } => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(truncate_chars(trimmed, MEMORY_CONTEXT_MAX_BLOCK_CHARS))
            }
        }
        crate::message::ContentBlock::ToolUse { name, .. } => Some(format!("[Tool: {}]", name)),
        crate::message::ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            if is_error.unwrap_or(false) {
                // Keep the serialized block on one physical line. The focused-query
                // filter drops tool blocks by their leading marker, so preserving
                // payload newlines would let every line after the first escape as
                // apparent conversation prose.
                let content = truncate_chars(content.trim(), MEMORY_CONTEXT_MAX_BLOCK_CHARS / 4);
                let content = content.split_whitespace().collect::<Vec<_>>().join(" ");
                Some(format!("[Tool error: {}]", content))
            } else {
                None
            }
        }
        crate::message::ContentBlock::Reasoning { .. }
        | crate::message::ContentBlock::ReasoningTrace { .. }
        | crate::message::ContentBlock::AnthropicThinking { .. }
        | crate::message::ContentBlock::OpenAIReasoning { .. } => None,
        crate::message::ContentBlock::Image { .. } => Some("[Image]".to_string()),
        crate::message::ContentBlock::OpenAICompaction { .. } => {
            Some("[OpenAI native compaction]".to_string())
        }
    }
}

fn format_content_block_for_extraction(block: &crate::message::ContentBlock) -> Option<String> {
    match block {
        crate::message::ContentBlock::Text { text, .. } => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(truncate_chars(trimmed, MEMORY_CONTEXT_MAX_BLOCK_CHARS))
            }
        }
        crate::message::ContentBlock::ToolUse { name, input, .. } => {
            let input_str =
                serde_json::to_string(input).unwrap_or_else(|_| "<invalid json>".into());
            let input_str = truncate_chars(&input_str, MEMORY_CONTEXT_MAX_BLOCK_CHARS / 2);
            Some(format!("[Tool: {} input: {}]", name, input_str))
        }
        crate::message::ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            let label = if is_error.unwrap_or(false) {
                "Tool error"
            } else {
                "Tool result"
            };
            let content = truncate_chars(content, MEMORY_CONTEXT_MAX_BLOCK_CHARS / 2);
            Some(format!("[{}: {}]", label, content))
        }
        crate::message::ContentBlock::Reasoning { .. }
        | crate::message::ContentBlock::ReasoningTrace { .. }
        | crate::message::ContentBlock::AnthropicThinking { .. }
        | crate::message::ContentBlock::OpenAIReasoning { .. } => None,
        crate::message::ContentBlock::Image { .. } => Some("[Image]".to_string()),
        crate::message::ContentBlock::OpenAICompaction { .. } => {
            Some("[OpenAI native compaction]".to_string())
        }
    }
}

fn format_message_context_with(
    message: &crate::message::Message,
    format_block: fn(&crate::message::ContentBlock) -> Option<String>,
) -> String {
    let role = match message.role {
        crate::message::Role::User => "User",
        crate::message::Role::Assistant => "Assistant",
    };

    let mut chunk = String::new();
    chunk.push_str(role);
    chunk.push_str(":\n");

    let mut has_content = false;
    for block in &message.content {
        if let Some(text) = format_block(block)
            && !text.is_empty()
        {
            has_content = true;
            chunk.push_str(&text);
            chunk.push('\n');
        }
    }

    if has_content { chunk } else { String::new() }
}

/// Format messages into a context string for relevance checking
pub fn format_context_for_relevance(messages: &[crate::message::Message]) -> String {
    let mut chunks: Vec<String> = Vec::new();
    let mut total_chars = 0usize;

    for message in messages.iter().rev().take(MEMORY_CONTEXT_MAX_MESSAGES) {
        let chunk = format_message_context_with(message, format_content_block_for_relevance);
        if chunk.is_empty() {
            continue;
        }
        let chunk_len = chunk.chars().count();
        if total_chars + chunk_len > MEMORY_CONTEXT_MAX_CHARS {
            if total_chars == 0 {
                chunks.push(truncate_chars(&chunk, MEMORY_CONTEXT_MAX_CHARS));
            }
            break;
        }
        total_chars += chunk_len;
        chunks.push(chunk);
    }

    chunks.reverse();
    chunks.join("\n").trim().to_string()
}

/// Build a focused retrieval query from recent messages.
///
/// Unlike [`format_context_for_relevance`], which concatenates up to a dozen
/// role-prefixed messages (including tool output) into one blob, this produces a
/// tighter query centered on the user's *current intent*. Memory retrieval and
/// (especially) cross-encoder reranking degrade badly on long, noisy inputs:
/// embeddings get diluted and rerankers trained on short search queries fail to
/// score relevance. This builder:
///   - drops `<system-reminder>...</system-reminder>` blocks (session boilerplate),
///   - drops tool-call / tool-result lines,
///   - keeps human/assistant prose,
///   - over-weights the most recent user message (the strongest intent signal)
///     by placing it first.
///
/// Falls back to the raw relevance context if stripping leaves nothing.
pub fn format_focused_query_for_relevance(messages: &[crate::message::Message]) -> String {
    let raw = format_context_for_relevance(messages);
    focus_query_text(&raw)
}

/// Pure text transform powering [`format_focused_query_for_relevance`]. Split out
/// so it can be unit-tested without constructing `Message` values.
pub fn focus_query_text(raw: &str) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut last_user: Option<String> = None;
    let mut in_reminder = false;
    let mut current_role_is_user = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        // Skip system-reminder blocks (session bootstrap / memory injections).
        if trimmed.starts_with("<system-reminder>") {
            in_reminder = true;
            // A single-line reminder both opens and closes.
            if trimmed.ends_with("</system-reminder>") {
                in_reminder = false;
            }
            continue;
        }
        if in_reminder {
            if trimmed.ends_with("</system-reminder>") {
                in_reminder = false;
            }
            continue;
        }

        // Role markers emitted by format_message_context_with ("User:" / "Assistant:").
        if trimmed == "User:" {
            current_role_is_user = true;
            continue;
        }
        if trimmed == "Assistant:" {
            current_role_is_user = false;
            continue;
        }

        // Drop tool noise; keep human/assistant prose.
        if trimmed.is_empty()
            || trimmed.starts_with("[Tool:")
            || trimmed.starts_with("[Tool error:")
            || trimmed.starts_with("[Result:")
            || trimmed.starts_with("[Image]")
            || trimmed.starts_with("[OpenAI native compaction]")
        {
            continue;
        }

        kept.push(trimmed.to_string());
        if current_role_is_user {
            last_user = Some(trimmed.to_string());
        }
    }

    let mut out = String::new();
    // Lead with the most recent user intent so the query is anchored on it.
    if let Some(user) = last_user {
        out.push_str(&user);
        out.push('\n');
    }
    out.push_str(&kept.join("\n"));

    let out = out.trim();
    if out.is_empty() {
        raw.to_string()
    } else {
        out.to_string()
    }
}

/// Format messages into a wider context string for extraction.
/// Uses a larger window than relevance checking since extraction needs to
/// capture learnings from a broader portion of the conversation.
pub(crate) fn format_context_for_extraction(messages: &[crate::message::Message]) -> String {
    let mut chunks: Vec<String> = Vec::new();
    let mut total_chars = 0usize;

    for message in messages.iter().rev().take(EXTRACTION_CONTEXT_MAX_MESSAGES) {
        let chunk = format_message_context_with(message, format_content_block_for_extraction);
        if chunk.is_empty() {
            continue;
        }
        let chunk_len = chunk.chars().count();
        if total_chars + chunk_len > EXTRACTION_CONTEXT_MAX_CHARS {
            if total_chars == 0 {
                chunks.push(truncate_chars(&chunk, EXTRACTION_CONTEXT_MAX_CHARS));
            }
            break;
        }
        total_chars += chunk_len;
        chunks.push(chunk);
    }

    chunks.reverse();
    chunks.join("\n").trim().to_string()
}
