use jcode_message_types::{
    ContentBlock, Message, Role, TOOL_OUTPUT_MISSING_TEXT, sanitize_tool_id,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

/// Normalize a tool `parameters` JSON schema for whichever upstream OpenRouter
/// routes the model to.
///
/// OpenRouter forwards to Anthropic, Vertex, Bedrock, LM Studio and others, so
/// it must satisfy the strictest of them: object schemas need a `properties` key
/// (LM Studio, #446) and top-level combiners are rejected by the
/// Anthropic-family backends (#495).
///
/// The subset, the recursion, and the structural rewrites live in
/// `jcode-schema-dialect` so every provider shares one implementation and one
/// set of regression tests. Delegating here also strips constructs the previous
/// hand-written version forwarded (`propertyNames`, `uniqueItems`), which the
/// same upstreams reject when they reach them.
pub fn sanitize_tool_parameters_schema(schema: &Value) -> Value {
    jcode_schema_dialect::normalize(schema, &jcode_schema_dialect::registry::OPENROUTER)
}

fn orphan_tool_output_to_user_message(
    tool_use_id: &str,
    output: &str,
    missing_output: &str,
) -> Option<Value> {
    let output = output.trim();
    if output.is_empty() || output == missing_output {
        return None;
    }

    Some(serde_json::json!({
        "role": "user",
        "content": format!(
            "[Recovered orphaned tool output: {}]\n{}",
            sanitize_tool_id(tool_use_id),
            output
        )
    }))
}

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

    let mut rewritten_pending_orphans = 0usize;
    if !pending_tool_results.is_empty() {
        let mut pending_entries: Vec<(String, String)> = std::mem::take(&mut pending_tool_results)
            .into_iter()
            .collect();
        pending_entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (tool_use_id, output) in pending_entries {
            if let Some(message) =
                orphan_tool_output_to_user_message(&tool_use_id, &output, &missing_output)
            {
                api_messages.push(message);
                rewritten_pending_orphans += 1;
            } else {
                skipped_results += 1;
            }
        }
    }

    if injected_missing > 0 {
        jcode_logging::info(&format!(
            "[openrouter] Injected {} synthetic tool output(s) to prevent API error",
            injected_missing
        ));
    }
    if rewritten_pending_orphans > 0 {
        jcode_logging::info(&format!(
            "[openrouter] Rewrote {} pending orphaned tool output(s) as user messages",
            rewritten_pending_orphans
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
    let mut tool_output_map: HashMap<String, VecDeque<Value>> = HashMap::new();
    let mut missing_tool_outputs: HashMap<String, Value> = HashMap::new();
    for msg in &api_messages {
        if msg.get("role").and_then(|v| v.as_str()) == Some("tool")
            && let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str())
        {
            let is_missing = msg
                .get("content")
                .and_then(|v| v.as_str())
                .map(|v| v == missing_output)
                .unwrap_or(false);
            if is_missing {
                missing_tool_outputs
                    .entry(id.to_string())
                    .or_insert_with(|| msg.clone());
            } else {
                tool_output_map
                    .entry(id.to_string())
                    .or_default()
                    .push_back(msg.clone());
            }
        }
    }

    let mut reordered: Vec<Value> = Vec::with_capacity(api_messages.len());
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
                        if let Some(tool_msg) = tool_output_map
                            .get_mut(id)
                            .and_then(VecDeque::pop_front)
                            .or_else(|| missing_tool_outputs.get(id).cloned())
                        {
                            reordered.push(tool_msg);
                        } else {
                            injected_ordered += 1;
                            reordered.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": id,
                                "content": missing_output.clone()
                            }));
                        }
                    }
                }
                continue;
            }
        }

        if role == "tool" {
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

#[cfg(test)]
mod request_tests {
    use super::build_chat_messages;
    use jcode_message_types::{ContentBlock, Message, Role};
    use serde_json::json;

    fn tool_call(id: &str, output: &str) -> [Message; 2] {
        [
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: "read".to_string(),
                    input: json!({"path": output}),
                    thought_signature: None,
                }],
                timestamp: None,
                tool_duration_ms: None,
            },
            Message::tool_result(id, output, false),
        ]
    }

    #[test]
    fn repeated_tool_call_ids_get_their_own_outputs_in_order() {
        let messages = tool_call("read:0", "first output")
            .into_iter()
            .chain(tool_call("read_0", "second output"))
            .collect::<Vec<_>>();

        let api_messages = build_chat_messages(&messages, "", false, false, false);
        let outputs = api_messages
            .iter()
            .filter(|message| message["role"] == "tool")
            .map(|message| message["content"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(outputs, ["first output", "second output"]);
    }

    #[test]
    fn orphaned_tool_output_is_recovered_as_a_user_message() {
        let messages = vec![Message::tool_result("call_orphan", "orphan result", false)];

        let api_messages = build_chat_messages(&messages, "", false, false, false);

        assert_eq!(
            api_messages,
            vec![json!({
                "role": "user",
                "content": "[Recovered orphaned tool output: call_orphan]\norphan result"
            })]
        );
    }
}

#[cfg(test)]
mod sanitize_schema_tests {
    use super::sanitize_tool_parameters_schema;
    use serde_json::json;

    #[test]
    fn top_level_any_of_is_flattened_for_anthropic_family_upstreams() {
        // The swarm-tool shape from issue #495: top-level anyOf action
        // branches make Anthropic/Vertex/Bedrock upstreams reject the whole
        // request with HTTP 400, bricking fresh OpenRouter logins.
        let schema = json!({
            "type": "object",
            "properties": {
                "action": {"type": "string"},
            },
            "required": ["action"],
            "anyOf": [
                {
                    "type": "object",
                    "required": ["action", "label"],
                    "properties": {
                        "action": {"type": "string", "enum": ["spawn"]},
                        "label": {"type": "string"}
                    }
                },
                {
                    "type": "object",
                    "required": ["action"],
                    "properties": {
                        "action": {"type": "string", "enum": ["list"]}
                    }
                }
            ]
        });

        let sanitized = sanitize_tool_parameters_schema(&schema);

        for keyword in ["oneOf", "anyOf", "allOf"] {
            assert!(
                sanitized.get(keyword).is_none(),
                "top-level {keyword} must be flattened away: {sanitized}"
            );
        }
        // Branch-only properties merge in as optional fields.
        assert!(sanitized["properties"]["label"].is_object());
        // Pre-existing top-level shape is preserved.
        assert_eq!(sanitized["type"], "object");
        assert_eq!(sanitized["required"], json!(["action"]));
        // anyOf branch `required` must NOT be promoted (branches are
        // alternatives, not conjunctions).
        assert!(
            !sanitized["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "label"),
            "anyOf branch required must not become unconditional: {sanitized}"
        );
    }

    #[test]
    fn top_level_all_of_promotes_required_fields() {
        let schema = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}},
            "allOf": [
                {
                    "type": "object",
                    "required": ["b"],
                    "properties": {"b": {"type": "string"}}
                }
            ]
        });

        let sanitized = sanitize_tool_parameters_schema(&schema);

        assert!(sanitized.get("allOf").is_none());
        assert!(sanitized["properties"]["b"].is_object());
        assert!(
            sanitized["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "b"),
            "allOf required applies unconditionally and must be promoted: {sanitized}"
        );
    }

    #[test]
    fn bare_object_schema_gains_empty_properties() {
        // The no-argument MCP tool shape from issue #446.
        let sanitized = sanitize_tool_parameters_schema(&json!({"type": "object"}));
        assert_eq!(sanitized, json!({"type": "object", "properties": {}}));
    }

    #[test]
    fn empty_and_non_object_schemas_become_empty_object_schema() {
        let expected = json!({"type": "object", "properties": {}});
        assert_eq!(sanitize_tool_parameters_schema(&json!({})), expected);
        assert_eq!(sanitize_tool_parameters_schema(&json!(null)), expected);
    }

    #[test]
    fn existing_properties_and_unrelated_fields_are_preserved() {
        let schema = json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "a path"}},
            "required": ["path"],
            "additionalProperties": false
        });
        assert_eq!(sanitize_tool_parameters_schema(&schema), schema);
    }

    #[test]
    fn nested_object_schemas_are_sanitized_recursively() {
        let schema = json!({
            "type": "object",
            "properties": {
                "config": {"type": "object"},
                "items": {"type": "array", "items": {"type": "object"}},
                "choice": {"anyOf": [{"type": "object"}, {"type": "string"}]}
            }
        });
        let sanitized = sanitize_tool_parameters_schema(&schema);
        assert_eq!(
            sanitized["properties"]["config"],
            json!({"type": "object", "properties": {}})
        );
        assert_eq!(
            sanitized["properties"]["items"]["items"],
            json!({"type": "object", "properties": {}})
        );
        assert_eq!(
            sanitized["properties"]["choice"]["anyOf"][0],
            json!({"type": "object", "properties": {}})
        );
        assert_eq!(
            sanitized["properties"]["choice"]["anyOf"][1],
            json!({"type": "string"})
        );
    }

    #[test]
    fn type_arrays_including_object_are_sanitized() {
        let sanitized = sanitize_tool_parameters_schema(&json!({"type": ["object", "null"]}));
        assert_eq!(sanitized["properties"], json!({}));
    }
}
