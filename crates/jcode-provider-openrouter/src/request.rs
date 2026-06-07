use jcode_message_types::{
    ContentBlock, Message, Role, TOOL_OUTPUT_MISSING_TEXT, sanitize_tool_id,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Build OpenAI-compatible chat `messages` for OpenRouter/direct compatible providers.
///
/// This stays in the OpenRouter leaf crate so provider-specific message normalization,
/// tool-call repair, and reasoning-content compatibility do not type-check inside
/// `jcode-base` on every provider edit.
pub fn build_chat_messages(
    messages: &[Message],
    system: &str,
    allow_reasoning: bool,
    include_reasoning_content: bool,
    allow_image_input: bool,
) -> Vec<Value> {
    // Build messages in OpenAI format
    let mut api_messages = Vec::new();

    // Add system message if provided
    if !system.is_empty() {
        api_messages.push(serde_json::json!({
            "role": "system",
            "content": system
        }));
    }

    let content_from_parts = |parts: Vec<Value>| -> Option<Value> {
        if parts.is_empty() {
            return None;
        }
        if parts.len() == 1 {
            let part = &parts[0];
            let has_cache = part.get("cache_control").is_some();
            if !has_cache && let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                return Some(serde_json::json!(text));
            }
        }
        Some(Value::Array(parts))
    };

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

    let missing_output = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);
    let mut injected_missing = 0usize;
    let mut delayed_results = 0usize;
    let mut skipped_results = 0usize;
    let mut tool_calls_seen: HashSet<String> = HashSet::new();
    let mut pending_tool_results: HashMap<String, String> = HashMap::new();
    let mut used_tool_results: HashSet<String> = HashSet::new();

    // Convert messages
    for (idx, msg) in messages.iter().enumerate() {
        match msg.role {
            Role::User => {
                let mut pending_user_parts: Vec<Value> = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text {
                            text,
                            cache_control,
                        } => {
                            let mut part = serde_json::json!({
                                "type": "text",
                                "text": text
                            });
                            if let Some(cache_control) = cache_control {
                                part["cache_control"] =
                                    serde_json::to_value(cache_control).unwrap_or(Value::Null);
                            }
                            pending_user_parts.push(part);
                        }
                        ContentBlock::Image { media_type, data } => {
                            if allow_image_input {
                                pending_user_parts.push(serde_json::json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!("data:{};base64,{}", media_type, data)
                                    }
                                }));
                            } else {
                                pending_user_parts.push(serde_json::json!({
                                    "type": "text",
                                    "text": format!(
                                        "[Image omitted: this provider/model does not support image input; media_type={}]",
                                        media_type
                                    )
                                }));
                            }
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            if let Some(content) =
                                content_from_parts(std::mem::take(&mut pending_user_parts))
                            {
                                api_messages.push(serde_json::json!({
                                    "role": "user",
                                    "content": content
                                }));
                            }

                            if used_tool_results.contains(tool_use_id) {
                                skipped_results += 1;
                                continue;
                            }
                            let output = if is_error == &Some(true) {
                                format!("[Error] {}", content)
                            } else {
                                content.clone()
                            };
                            if tool_calls_seen.contains(tool_use_id) {
                                api_messages.push(serde_json::json!({
                                    "role": "tool",
                                    "tool_call_id": sanitize_tool_id(tool_use_id),
                                    "content": output
                                }));
                                used_tool_results.insert(tool_use_id.clone());
                            } else if pending_tool_results.contains_key(tool_use_id) {
                                skipped_results += 1;
                            } else {
                                pending_tool_results.insert(tool_use_id.clone(), output);
                                delayed_results += 1;
                            }
                        }
                        _ => {}
                    }
                }

                if let Some(content) = content_from_parts(std::mem::take(&mut pending_user_parts)) {
                    api_messages.push(serde_json::json!({
                        "role": "user",
                        "content": content
                    }));
                }
            }
            Role::Assistant => {
                let mut text_content = String::new();
                let mut reasoning_content = String::new();
                let mut tool_calls = Vec::new();
                let mut post_tool_outputs: Vec<(String, String)> = Vec::new();
                let mut missing_tool_outputs: Vec<String> = Vec::new();

                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            text_content.push_str(text);
                        }
                        ContentBlock::Reasoning { text } => {
                            reasoning_content.push_str(text);
                        }
                        ContentBlock::ToolUse {
                            id, name, input, ..
                        } => {
                            let args = if input.is_object() {
                                serde_json::to_string(input).unwrap_or_default()
                            } else {
                                "{}".to_string()
                            };
                            tool_calls.push(serde_json::json!({
                                "id": sanitize_tool_id(id),
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": args
                                }
                            }));
                            tool_calls_seen.insert(id.clone());
                            if let Some(output) = pending_tool_results.remove(id) {
                                post_tool_outputs.push((id.clone(), output));
                                used_tool_results.insert(id.clone());
                            } else {
                                let has_future_output = tool_result_last_pos
                                    .get(id)
                                    .map(|pos| *pos > idx)
                                    .unwrap_or(false);
                                if !has_future_output {
                                    missing_tool_outputs.push(id.clone());
                                    used_tool_results.insert(id.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }

                let mut assistant_msg = serde_json::json!({
                    "role": "assistant",
                });

                if !text_content.is_empty() {
                    assistant_msg["content"] = serde_json::json!(text_content);
                }

                if !tool_calls.is_empty() {
                    assistant_msg["tool_calls"] = serde_json::json!(tool_calls);
                }

                let has_reasoning_content = !reasoning_content.is_empty();
                if allow_reasoning
                    && (include_reasoning_content || has_reasoning_content)
                    && (has_reasoning_content || !tool_calls.is_empty())
                {
                    let reasoning_payload = if has_reasoning_content {
                        reasoning_content.clone()
                    } else {
                        " ".to_string()
                    };
                    assistant_msg["reasoning_content"] = serde_json::json!(reasoning_payload);
                }

                let has_text_content = !text_content.is_empty();
                let has_tool_calls = !tool_calls.is_empty();

                // OpenAI-compatible providers require every assistant
                // message to carry `content` or `tool_calls`. An
                // interrupted turn can persist only a reasoning block; if
                // the provider does not accept a standalone
                // `reasoning_content` field (so it was not set above), this
                // would serialize to a bare `{"role":"assistant"}` and make
                // providers like DeepSeek reject the entire request with
                // 400 "Invalid assistant message: content or tool_calls
                // must be set", permanently wedging the session (issue
                // #321). Guarantee validity: when there is no text/tool
                // payload, only keep the turn if a provider-accepted
                // `reasoning_content` field is present, and in that case add
                // an explicit empty `content` so strict validators still
                // accept it. Otherwise drop the empty interrupted-thinking
                // artifact entirely (no tool outputs are possible without
                // tool calls).
                let keep_assistant_message = if has_text_content || has_tool_calls {
                    true
                } else if assistant_msg.get("reasoning_content").is_some() {
                    assistant_msg["content"] = serde_json::json!("");
                    true
                } else {
                    false
                };

                if keep_assistant_message {
                    api_messages.push(assistant_msg);

                    for (tool_call_id, output) in post_tool_outputs {
                        api_messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": sanitize_tool_id(&tool_call_id),
                            "content": output
                        }));
                    }

                    if !missing_tool_outputs.is_empty() {
                        injected_missing += missing_tool_outputs.len();
                        for missing_id in missing_tool_outputs {
                            api_messages.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": sanitize_tool_id(&missing_id),
                                "content": missing_output.clone()
                            }));
                        }
                    }
                }
            }
        }
    }

    if delayed_results > 0 {
        jcode_logging::info(&format!(
            "[openrouter] Delayed {} tool output(s) to preserve call ordering",
            delayed_results
        ));
    }

    if !pending_tool_results.is_empty() {
        skipped_results += pending_tool_results.len();
    }

    if injected_missing > 0 {
        jcode_logging::info(&format!(
            "[openrouter] Injected {} synthetic tool output(s) to prevent API error",
            injected_missing
        ));
    }
    if skipped_results > 0 {
        jcode_logging::info(&format!(
            "[openrouter] Filtered {} orphaned tool result(s) to prevent API error",
            skipped_results
        ));
    }

    // Safety pass: ensure tool-call messages include reasoning_content (when allowed)
    // and that every tool call has a matching tool output after it.
    let mut outputs_after: HashSet<String> = HashSet::new();
    let mut missing_by_index: Vec<Vec<String>> = vec![Vec::new(); api_messages.len()];

    for (idx, msg) in api_messages.iter().enumerate().rev() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role == "tool" {
            if let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str()) {
                outputs_after.insert(id.to_string());
            }
            continue;
        }

        if role == "assistant"
            && let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array())
        {
            for call in tool_calls {
                if let Some(id) = call.get("id").and_then(|v| v.as_str())
                    && !outputs_after.contains(id)
                {
                    outputs_after.insert(id.to_string());
                    missing_by_index[idx].push(id.to_string());
                }
            }
        }
    }

    let mut normalized = Vec::with_capacity(api_messages.len());
    let mut extra_outputs = 0usize;
    let mut missing_reasoning = 0usize;

    for (idx, mut msg) in api_messages.into_iter().enumerate() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role == "assistant"
            && allow_reasoning
            && msg.get("tool_calls").and_then(|v| v.as_array()).is_some()
        {
            let needs_reasoning = match msg.get("reasoning_content") {
                Some(value) => value.as_str().map(|s| s.trim().is_empty()).unwrap_or(true),
                None => true,
            };
            if needs_reasoning {
                msg["reasoning_content"] = serde_json::json!(" ");
                missing_reasoning += 1;
            }
        }

        normalized.push(msg);

        if let Some(missing) = missing_by_index.get(idx) {
            for id in missing {
                extra_outputs += 1;
                normalized.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": missing_output.clone()
                }));
            }
        }
    }

    api_messages = normalized;

    if missing_reasoning > 0 {
        jcode_logging::info(&format!(
            "[openrouter] Filled reasoning_content on {} tool-call message(s)",
            missing_reasoning
        ));
    }
    if extra_outputs > 0 {
        jcode_logging::info(&format!(
            "[openrouter] Safety-injected {} missing tool output(s) at request build",
            extra_outputs
        ));
    }

    // Final safety pass: ensure every tool_call_id has at least one tool response after it.
    let mut tool_output_positions: HashMap<String, usize> = HashMap::new();
    for (idx, msg) in api_messages.iter().enumerate() {
        if msg.get("role").and_then(|v| v.as_str()) == Some("tool")
            && let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str())
        {
            tool_output_positions.entry(id.to_string()).or_insert(idx);
        }
    }

    let mut missing_after: HashSet<String> = HashSet::new();
    for (idx, msg) in api_messages.iter().enumerate() {
        if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
            for call in tool_calls {
                if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                    let has_after = tool_output_positions
                        .get(id)
                        .map(|pos| *pos > idx)
                        .unwrap_or(false);
                    if !has_after {
                        missing_after.insert(id.to_string());
                    }
                }
            }
        }
    }

    if !missing_after.is_empty() {
        for id in missing_after.iter() {
            api_messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": id,
                "content": missing_output.clone()
            }));
        }
        jcode_logging::info(&format!(
            "[openrouter] Appended {} tool output(s) to satisfy call ordering",
            missing_after.len()
        ));
    }

    // Final pass: ensure tool outputs immediately follow assistant tool calls.
    let mut tool_output_map: HashMap<String, Value> = HashMap::new();
    for msg in &api_messages {
        if msg.get("role").and_then(|v| v.as_str()) == Some("tool")
            && let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str())
        {
            let is_missing = msg
                .get("content")
                .and_then(|v| v.as_str())
                .map(|v| v == missing_output)
                .unwrap_or(false);
            match tool_output_map.get(id) {
                Some(existing) => {
                    let existing_missing = existing
                        .get("content")
                        .and_then(|v| v.as_str())
                        .map(|v| v == missing_output)
                        .unwrap_or(false);
                    if existing_missing && !is_missing {
                        tool_output_map.insert(id.to_string(), msg.clone());
                    }
                }
                None => {
                    tool_output_map.insert(id.to_string(), msg.clone());
                }
            }
        }
    }

    let mut reordered: Vec<Value> = Vec::with_capacity(api_messages.len());
    let mut used_outputs: HashSet<String> = HashSet::new();
    let mut injected_ordered = 0usize;
    let mut dropped_orphans = 0usize;

    for msg in api_messages.into_iter() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role == "assistant" {
            let tool_calls = msg.get("tool_calls").and_then(|v| v.as_array()).cloned();
            if let Some(tool_calls) = tool_calls {
                if tool_calls.is_empty() {
                    reordered.push(msg);
                    continue;
                }
                reordered.push(msg);
                for call in tool_calls {
                    if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                        if let Some(tool_msg) = tool_output_map.get(id) {
                            reordered.push(tool_msg.clone());
                            used_outputs.insert(id.to_string());
                        } else {
                            injected_ordered += 1;
                            reordered.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": id,
                                "content": missing_output.clone()
                            }));
                            used_outputs.insert(id.to_string());
                        }
                    }
                }
                continue;
            }
        }

        if role == "tool" {
            if let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str())
                && used_outputs.contains(id)
            {
                dropped_orphans += 1;
                continue;
            }
            dropped_orphans += 1;
            continue;
        }

        reordered.push(msg);
    }

    api_messages = reordered;

    if injected_ordered > 0 {
        jcode_logging::info(&format!(
            "[openrouter] Inserted {} tool output(s) to enforce call ordering",
            injected_ordered
        ));
    }
    if dropped_orphans > 0 {
        jcode_logging::info(&format!(
            "[openrouter] Dropped {} orphaned tool output(s) during re-ordering",
            dropped_orphans
        ));
    }

    api_messages
}
