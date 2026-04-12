use crate::message::{
    ContentBlock, Message as ChatMessage, Role, TOOL_OUTPUT_MISSING_TEXT, ToolDefinition,
};
use jcode_provider_core::openai_schema::{
    openai_compatible_schema, schema_supports_strict, strict_normalize_schema,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

static REWRITTEN_ORPHAN_TOOL_OUTPUTS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn build_tools(tools: &[ToolDefinition]) -> Vec<Value> {
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

#[cfg(test)]
mod tests {
    use super::build_tools;
    use crate::message::{Message, ToolDefinition};
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use jcode_provider_core::openai_schema::{
        make_schema_nullable, openai_compatible_schema, schema_supports_strict,
        strict_normalize_schema,
    };
    use serde_json::json;
    use std::sync::Arc;

    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            anyhow::bail!("Mock provider should not be called in schema tests")
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self)
        }

        fn available_models_display(&self) -> Vec<String> {
            Vec::new()
        }

        async fn prefetch_models(&self) -> Result<()> {
            Ok(())
        }
    }

    fn schema_contains_keyword(schema: &serde_json::Value, keyword: &str) -> bool {
        match schema {
            serde_json::Value::Object(map) => {
                map.contains_key(keyword)
                    || map
                        .values()
                        .any(|value| schema_contains_keyword(value, keyword))
            }
            serde_json::Value::Array(items) => items
                .iter()
                .any(|value| schema_contains_keyword(value, keyword)),
            _ => false,
        }
    }

    #[test]
    fn strict_normalize_schema_marks_optional_properties_nullable_and_required() {
        let schema = json!({
            "type": "object",
            "properties": {
                "required_name": { "type": "string" },
                "optional_age": { "type": "integer" }
            },
            "required": ["required_name"]
        });

        let normalized = strict_normalize_schema(&schema);

        assert_eq!(
            normalized,
            json!({
                "type": "object",
                "properties": {
                    "required_name": { "type": "string" },
                    "optional_age": { "type": ["integer", "null"] }
                },
                "required": ["optional_age", "required_name"],
                "additionalProperties": false
            })
        );
    }

    #[test]
    fn strict_normalize_schema_preserves_existing_nullability() {
        let schema = json!({
            "anyOf": [
                { "type": "string" },
                { "type": "null" }
            ]
        });

        assert_eq!(
            make_schema_nullable(schema.clone()),
            json!({
                "anyOf": [
                    { "type": "string" },
                    { "type": "null" }
                ]
            })
        );
    }

    #[test]
    fn strict_normalize_schema_recurses_through_nested_object_keywords() {
        let schema = json!({
            "type": "object",
            "properties": {
                "child": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    }
                }
            }
        });

        let normalized = strict_normalize_schema(&schema);

        assert_eq!(
            normalized,
            json!({
                "type": "object",
                "properties": {
                    "child": {
                        "type": ["object", "null"],
                        "properties": {
                            "name": { "type": ["string", "null"] }
                        },
                        "required": ["name"],
                        "additionalProperties": false
                    }
                },
                "required": ["child"],
                "additionalProperties": false
            })
        );
    }

    #[test]
    fn schema_supports_strict_rejects_open_or_empty_objects() {
        assert!(!schema_supports_strict(&json!({ "type": "object" })));
        assert!(!schema_supports_strict(&json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "additionalProperties": true
        })));
        assert!(schema_supports_strict(&json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "additionalProperties": false
        })));
    }

    #[test]
    fn openai_compatible_schema_flattens_allof_object_branches() {
        let schema = json!({
            "description": "Read params",
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
        });

        let normalized = openai_compatible_schema(&schema);

        assert!(normalized.get("allOf").is_none());
        assert_eq!(normalized["type"], json!("object"));
        assert_eq!(normalized["description"], json!("Read params"));
        assert_eq!(
            normalized["properties"]["file_path"]["type"],
            json!("string")
        );
        assert_eq!(
            normalized["properties"]["start_line"]["type"],
            json!("integer")
        );
        assert_eq!(normalized["required"], json!(["file_path"]));
    }

    #[tokio::test]
    async fn build_tools_never_sends_allof_to_openai() {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let registry = Registry::new(provider).await;
        let defs = registry.definitions(None).await;
        let api_tools = build_tools(&defs);

        let offending: Vec<String> = api_tools
            .iter()
            .filter_map(|tool| {
                if schema_contains_keyword(&tool["parameters"], "allOf") {
                    tool["name"].as_str().map(ToString::to_string)
                } else {
                    None
                }
            })
            .collect();

        assert!(
            offending.is_empty(),
            "OpenAI tool schemas must not contain allOf; offending tools: {offending:?}"
        );
    }
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

pub(crate) fn build_responses_input(messages: &[ChatMessage]) -> Vec<Value> {
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
                            items.push(serde_json::json!({
                                "type": "compaction",
                                "encrypted_content": encrypted_content,
                            }));
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
                                    "call_id": crate::message::sanitize_tool_id(tool_use_id),
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
                                "call_id": crate::message::sanitize_tool_id(id)
                            }));

                            if let Some(output) = pending_outputs.remove(id.as_str()) {
                                items.push(serde_json::json!({
                                    "type": "function_call_output",
                                    "call_id": crate::message::sanitize_tool_id(id),
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
                                        "call_id": crate::message::sanitize_tool_id(id),
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
                "call_id": crate::message::sanitize_tool_id(&call_id),
                "output": output
            }));
        } else {
            injected_missing += 1;
            items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": crate::message::sanitize_tool_id(&call_id),
                "output": missing_output.clone()
            }));
        }
    }

    if delayed_results > 0 {
        crate::logging::info(&format!(
            "[openai] Delayed {} tool output(s) to preserve call ordering",
            delayed_results
        ));
    }

    let mut rewritten_pending_orphans = 0usize;
    if !pending_outputs.is_empty() {
        let mut pending_entries: Vec<(String, String)> =
            std::mem::take(&mut pending_outputs).into_iter().collect();
        pending_entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (call_id, output) in pending_entries {
            let orphan_item = serde_json::json!({
                "type": "function_call_output",
                "call_id": crate::message::sanitize_tool_id(&call_id),
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
        crate::logging::info(&format!(
            "[openai] Injected {} synthetic tool output(s) to prevent API error",
            injected_missing
        ));
    }
    if rewritten_pending_orphans > 0 {
        let total = REWRITTEN_ORPHAN_TOOL_OUTPUTS
            .fetch_add(rewritten_pending_orphans as u64, Ordering::Relaxed)
            + rewritten_pending_orphans as u64;
        crate::logging::info(&format!(
            "[openai] Rewrote {} pending orphaned tool output(s) as user messages (total={})",
            rewritten_pending_orphans, total
        ));
    }
    if skipped_results > 0 {
        crate::logging::info(&format!(
            "[openai] Filtered {} orphaned tool result(s) to prevent API error",
            skipped_results
        ));
    }

    let mut output_ids: HashSet<String> = HashSet::new();
    for item in &items {
        if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output") {
            if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                output_ids.insert(call_id.to_string());
            }
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

        if is_call {
            if let Some(call_id) = call_id {
                if !output_ids.contains(&call_id) {
                    extra_injected += 1;
                    output_ids.insert(call_id.clone());
                    normalized.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": missing_output.clone()
                    }));
                }
            }
        }
    }

    if extra_injected > 0 {
        crate::logging::info(&format!(
            "[openai] Safety-injected {} missing tool output(s) at request build",
            extra_injected
        ));
    }

    let mut output_map: HashMap<String, Value> = HashMap::new();
    for item in &normalized {
        if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output") {
            if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
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
            if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                if used_outputs.contains(call_id) {
                    dropped_duplicate_outputs += 1;
                    continue;
                }
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
        crate::logging::info(&format!(
            "[openai] Inserted {} tool output(s) to enforce call ordering",
            injected_ordered
        ));
    }
    if dropped_duplicate_outputs > 0 {
        crate::logging::info(&format!(
            "[openai] Dropped {} duplicate tool output(s) during re-ordering",
            dropped_duplicate_outputs
        ));
    }
    if rewritten_orphans > 0 {
        let total = REWRITTEN_ORPHAN_TOOL_OUTPUTS
            .fetch_add(rewritten_orphans as u64, Ordering::Relaxed)
            + rewritten_orphans as u64;
        crate::logging::info(&format!(
            "[openai] Rewrote {} orphaned tool output(s) as user messages (total={})",
            rewritten_orphans, total
        ));
    }
    if skipped_empty_orphans > 0 {
        crate::logging::info(&format!(
            "[openai] Skipped {} empty orphaned tool output(s) during re-ordering",
            skipped_empty_orphans
        ));
    }

    ordered
}
