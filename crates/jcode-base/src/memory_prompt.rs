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
                Some(format!(
                    "[Tool error: {}]",
                    truncate_chars(content.trim(), MEMORY_CONTEXT_MAX_BLOCK_CHARS / 4)
                ))
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
