use jcode_message_types::{
    ContentBlock, Message as ChatMessage, Role, TOOL_OUTPUT_MISSING_TEXT, ToolDefinition,
    sanitize_tool_id,
};
use jcode_provider_core::openai_schema::{
    openai_compatible_schema, schema_supports_strict, strict_normalize_schema,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

static REWRITTEN_ORPHAN_TOOL_OUTPUTS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiRequestLogLevel {
    Info,
    Warn,
}

/// OpenAI rejects `input[*].encrypted_content` strings above this size.
pub const OPENAI_ENCRYPTED_CONTENT_PROVIDER_MAX_CHARS: usize = 10_485_760;

/// Stay below the provider hard limit so JSON escaping/near-boundary changes do
/// not brick a session on the next replay.
pub const OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS: usize = 9_500_000;

pub fn openai_encrypted_content_is_sendable(encrypted_content: &str) -> bool {
    encrypted_content.len() <= OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS
}

pub fn openai_encrypted_content_fallback_summary(encrypted_content_len: usize) -> String {
    format!(
        "OpenAI native compaction state was discarded because its encrypted payload was {} chars, above Jcode's safe replay limit of {} chars (provider hard limit: {} chars). Earlier compacted details may be unavailable; use the recent visible messages and session search/tools if exact prior details are needed.",
        encrypted_content_len,
        OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS,
        OPENAI_ENCRYPTED_CONTENT_PROVIDER_MAX_CHARS,
    )
}

pub fn is_openai_encrypted_content_too_large_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("encrypted_content")
        && (lower.contains("string_above_max_length")
            || lower.contains("string too long")
            || lower.contains("maximum length")
            || lower.contains("large_string_param")
            || lower.contains("largestringparam"))
}

pub fn build_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let compatible_schema = openai_compatible_schema(&t.input_schema);
            let supports_strict = schema_supports_strict(&compatible_schema);
            let parameters = if supports_strict {
                strict_normalize_schema(&compatible_schema)
            } else {
                compatible_schema
            };
            serde_json::json!({
                "type": "function",
                "name": t.name,
                // Prompt-visible. Approximate token cost for this field:
                // t.description_token_estimate().
                "description": t.description,
                "strict": supports_strict,
                "parameters": parameters,
            })
        })
        .collect()
}

fn orphan_tool_output_to_user_message(item: &Value, missing_output: &str) -> Option<Value> {
    let output_value = item.get("output")?;
    let output = if let Some(text) = output_value.as_str() {
        text.trim().to_string()
    } else {
        output_value.to_string()
    };
    if output.is_empty() || output == missing_output {
        return None;
    }

    let call_id = item
        .get("call_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown_call");

    Some(serde_json::json!({
        "type": "message",
        "role": "user",
        "content": [{
            "type": "input_text",
            "text": format!("[Recovered orphaned tool output: {}]\n{}", call_id, output)
        }]
    }))
}

pub fn build_responses_input(messages: &[ChatMessage]) -> Vec<Value> {
    build_responses_input_with_logger(messages, |_, _| {})
}

pub fn build_responses_input_with_logger(
    messages: &[ChatMessage],
    mut logger: impl FnMut(OpenAiRequestLogLevel, &str),
) -> Vec<Value> {
    let missing_output = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);

    let mut tool_result_last_pos: HashMap<String, usize> = HashMap::new();
    for (idx, msg) in messages.iter().enumerate() {
        if let Role::User = msg.role {
            for block in &msg.content {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    tool_result_last_pos.insert(tool_use_id.clone(), idx);
                }
            }
        }
    }

    let mut items = Vec::new();
    let mut open_calls: HashSet<String> = HashSet::new();
    let mut pending_outputs: HashMap<String, String> = HashMap::new();
    let mut used_outputs: HashSet<String> = HashSet::new();
    let mut skipped_results = 0usize;
    let mut delayed_results = 0usize;
    let mut injected_missing = 0usize;

    for (idx, msg) in messages.iter().enumerate() {
        match msg.role {
            Role::User => {
                let mut content_parts: Vec<serde_json::Value> = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Image { media_type, data } => {
                            content_parts.push(serde_json::json!({
                                "type": "input_image",
                                "image_url": format!("data:{};base64,{}", media_type, data)
                            }));
                        }
                        ContentBlock::Text { text, .. } => {
                            content_parts.push(serde_json::json!({
                                "type": "input_text",
                                "text": text
                            }));
                        }
                        ContentBlock::OpenAICompaction { encrypted_content } => {
                            if !content_parts.is_empty() {
                                items.push(serde_json::json!({
                                    "type": "message",
                                    "role": "user",
                                    "content": std::mem::take(&mut content_parts)
                                }));
                            }
                            if openai_encrypted_content_is_sendable(encrypted_content) {
                                items.push(serde_json::json!({
                                    "type": "compaction",
                                    "encrypted_content": encrypted_content,
                                }));
                            } else {
                                logger(
                                    OpenAiRequestLogLevel::Warn,
                                    &format!(
                                        "[openai] Dropping oversized native compaction payload before request build ({} chars > safe limit {} chars)",
                                        encrypted_content.len(),
                                        OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS,
                                    ),
                                );
                                items.push(serde_json::json!({
                                    "type": "message",
                                    "role": "user",
                                    "content": [{
                                        "type": "input_text",
                                        "text": openai_encrypted_content_fallback_summary(encrypted_content.len()),
                                    }]
                                }));
                            }
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            if !content_parts.is_empty() {
                                items.push(serde_json::json!({
                                    "type": "message",
                                    "role": "user",
                                    "content": std::mem::take(&mut content_parts)
                                }));
                            }
                            if used_outputs.contains(tool_use_id.as_str()) {
                                skipped_results += 1;
                                continue;
                            }
                            let output = if is_error == &Some(true) {
                                format!("[Error] {}", content)
                            } else {
                                content.clone()
                            };
                            if open_calls.contains(tool_use_id.as_str()) {
                                items.push(serde_json::json!({
                                    "type": "function_call_output",
                                    "call_id": sanitize_tool_id(tool_use_id),
                                    "output": output
                                }));
                                open_calls.remove(tool_use_id.as_str());
                                used_outputs.insert(tool_use_id.clone());
                            } else if pending_outputs.contains_key(tool_use_id.as_str()) {
                                skipped_results += 1;
                            } else {
                                pending_outputs.insert(tool_use_id.clone(), output);
                                delayed_results += 1;
                            }
                        }
                        _ => {}
                    }
                }
                if !content_parts.is_empty() {
                    items.push(serde_json::json!({
                        "type": "message",
                        "role": "user",
                        "content": content_parts
                    }));
                }
            }
            Role::Assistant => {
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            items.push(serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{ "type": "output_text", "text": text }]
                            }));
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            let arguments = if input.is_object() {
                                serde_json::to_string(&input).unwrap_or_default()
                            } else {
                                "{}".to_string()
                            };
                            items.push(serde_json::json!({
                                "type": "function_call",
                                "name": name,
                                "arguments": arguments,
                                "call_id": sanitize_tool_id(id)
                            }));

                            if let Some(output) = pending_outputs.remove(id.as_str()) {
                                items.push(serde_json::json!({
                                    "type": "function_call_output",
                                    "call_id": sanitize_tool_id(id),
                                    "output": output
                                }));
                                used_outputs.insert(id.clone());
                            } else {
                                let has_future_output = tool_result_last_pos
                                    .get(id)
                                    .map(|pos| *pos > idx)
                                    .unwrap_or(false);
                                if has_future_output {
                                    open_calls.insert(id.clone());
                                } else {
                                    injected_missing += 1;
                                    items.push(serde_json::json!({
                                        "type": "function_call_output",
                                        "call_id": sanitize_tool_id(id),
                                        "output": missing_output.clone()
                                    }));
                                    used_outputs.insert(id.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    for call_id in open_calls {
        if used_outputs.contains(&call_id) {
            continue;
        }
        if let Some(output) = pending_outputs.remove(&call_id) {
            items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": sanitize_tool_id(&call_id),
                "output": output
            }));
        } else {
            injected_missing += 1;
            items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": sanitize_tool_id(&call_id),
                "output": missing_output.clone()
            }));
        }
    }

    if delayed_results > 0 {
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Delayed {} tool output(s) to preserve call ordering",
                delayed_results
            ),
        );
    }

    let mut rewritten_pending_orphans = 0usize;
    if !pending_outputs.is_empty() {
        let mut pending_entries: Vec<(String, String)> =
            std::mem::take(&mut pending_outputs).into_iter().collect();
        pending_entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (call_id, output) in pending_entries {
            let orphan_item = serde_json::json!({
                "type": "function_call_output",
                "call_id": sanitize_tool_id(&call_id),
                "output": output,
            });
            if let Some(message_item) =
                orphan_tool_output_to_user_message(&orphan_item, &missing_output)
            {
                items.push(message_item);
                rewritten_pending_orphans += 1;
            } else {
                skipped_results += 1;
            }
        }
    }

    if injected_missing > 0 {
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Injected {} synthetic tool output(s) to prevent API error",
                injected_missing
            ),
        );
    }
    if rewritten_pending_orphans > 0 {
        let total = REWRITTEN_ORPHAN_TOOL_OUTPUTS
            .fetch_add(rewritten_pending_orphans as u64, Ordering::Relaxed)
            + rewritten_pending_orphans as u64;
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Rewrote {} pending orphaned tool output(s) as user messages (total={})",
                rewritten_pending_orphans, total
            ),
        );
    }
    if skipped_results > 0 {
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Filtered {} orphaned tool result(s) to prevent API error",
                skipped_results
            ),
        );
    }

    let mut output_ids: HashSet<String> = HashSet::new();
    for item in &items {
        if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
            && let Some(call_id) = item.get("call_id").and_then(|v| v.as_str())
        {
            output_ids.insert(call_id.to_string());
        }
    }

    let mut normalized: Vec<Value> = Vec::with_capacity(items.len());
    let mut extra_injected = 0;
    for item in items {
        let is_call = matches!(
            item.get("type").and_then(|v| v.as_str()),
            Some("function_call") | Some("custom_tool_call")
        );
        let call_id = item
            .get("call_id")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        normalized.push(item);

        if is_call
            && let Some(call_id) = call_id
            && !output_ids.contains(&call_id)
        {
            extra_injected += 1;
            output_ids.insert(call_id.clone());
            normalized.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": missing_output.clone()
            }));
        }
    }

    if extra_injected > 0 {
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Safety-injected {} missing tool output(s) at request build",
                extra_injected
            ),
        );
    }

    let mut output_map: HashMap<String, Value> = HashMap::new();
    for item in &normalized {
        if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
            && let Some(call_id) = item.get("call_id").and_then(|v| v.as_str())
        {
            let is_missing = item
                .get("output")
                .and_then(|v| v.as_str())
                .map(|v| v == missing_output)
                .unwrap_or(false);
            match output_map.get(call_id) {
                Some(existing) => {
                    let existing_missing = existing
                        .get("output")
                        .and_then(|v| v.as_str())
                        .map(|v| v == missing_output)
                        .unwrap_or(false);
                    if existing_missing && !is_missing {
                        output_map.insert(call_id.to_string(), item.clone());
                    }
                }
                None => {
                    output_map.insert(call_id.to_string(), item.clone());
                }
            }
        }
    }

    let mut ordered: Vec<Value> = Vec::with_capacity(normalized.len());
    let mut used_outputs: HashSet<String> = HashSet::new();
    let mut injected_ordered = 0usize;
    let mut dropped_duplicate_outputs = 0usize;
    let mut rewritten_orphans = 0usize;
    let mut skipped_empty_orphans = 0usize;

    for item in normalized {
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let is_call = matches!(kind, "function_call" | "custom_tool_call");
        if is_call {
            let call_id = item
                .get("call_id")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string());
            ordered.push(item);
            if let Some(call_id) = call_id {
                if let Some(output_item) = output_map.get(&call_id) {
                    ordered.push(output_item.clone());
                    used_outputs.insert(call_id);
                } else {
                    injected_ordered += 1;
                    ordered.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": missing_output.clone()
                    }));
                    used_outputs.insert(call_id);
                }
            }
            continue;
        }

        if kind == "function_call_output" {
            if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str())
                && used_outputs.contains(call_id)
            {
                dropped_duplicate_outputs += 1;
                continue;
            }
            if let Some(message_item) = orphan_tool_output_to_user_message(&item, &missing_output) {
                ordered.push(message_item);
                rewritten_orphans += 1;
            } else {
                skipped_empty_orphans += 1;
            }
            continue;
        }

        ordered.push(item);
    }

    if injected_ordered > 0 {
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Inserted {} tool output(s) to enforce call ordering",
                injected_ordered
            ),
        );
    }
    if dropped_duplicate_outputs > 0 {
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Dropped {} duplicate tool output(s) during re-ordering",
                dropped_duplicate_outputs
            ),
        );
    }
    if rewritten_orphans > 0 {
        let total = REWRITTEN_ORPHAN_TOOL_OUTPUTS
            .fetch_add(rewritten_orphans as u64, Ordering::Relaxed)
            + rewritten_orphans as u64;
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Rewrote {} orphaned tool output(s) as user messages (total={})",
                rewritten_orphans, total
            ),
        );
    }
    if skipped_empty_orphans > 0 {
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Skipped {} empty orphaned tool output(s) during re-ordering",
                skipped_empty_orphans
            ),
        );
    }

    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_message_types::ToolDefinition;
    use serde_json::json;

    #[test]
    fn build_tools_flattens_allof_schema_for_openai() {
        let defs = vec![ToolDefinition {
            name: "read".to_string(),
            description: "Read params".to_string(),
            input_schema: json!({
                "allOf": [
                    {
                        "type": "object",
                        "properties": {
                            "file_path": { "type": "string" }
                        },
                        "required": ["file_path"]
                    },
                    {
                        "type": "object",
                        "properties": {
                            "start_line": { "type": "integer" }
                        }
                    }
                ]
            }),
        }];

        let api_tools = build_tools(&defs);
        let parameters = &api_tools[0]["parameters"];

        assert!(parameters.get("allOf").is_none());
        assert_eq!(parameters["type"], json!("object"));
        assert_eq!(
            parameters["properties"]["file_path"]["type"],
            json!("string")
        );
        assert_eq!(
            parameters["properties"]["start_line"]["type"],
            json!(["integer", "null"])
        );
    }

    #[test]
    fn build_responses_input_logs_oversized_native_compaction() {
        let oversized = "x".repeat(OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS + 1);
        let messages = vec![ChatMessage {
            role: Role::User,
            content: vec![ContentBlock::OpenAICompaction {
                encrypted_content: oversized,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }];
        let mut logs = Vec::new();

        let items = build_responses_input_with_logger(&messages, |level, message| {
            logs.push((level, message.to_string()));
        });

        assert!(items.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("message")
                && item.get("role").and_then(|v| v.as_str()) == Some("user")
        }));
        assert!(logs.iter().any(|(level, message)| {
            *level == OpenAiRequestLogLevel::Warn
                && message.contains("Dropping oversized native compaction payload")
        }));
    }
}
