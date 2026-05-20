#[test]
fn test_parse_openai_response_completed_captures_incomplete_stop_reason() {
    let data = r#"{"type":"response.completed","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}"#;
    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();

    let event = parse_openai_response_event(
        data,
        &mut saw_text_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    )
    .expect("expected message end");
    match event {
        StreamEvent::MessageEnd { stop_reason } => {
            assert_eq!(stop_reason.as_deref(), Some("max_output_tokens"));
        }
        other => panic!("expected MessageEnd, got {:?}", other),
    }
}

#[test]
fn test_parse_openai_response_completed_without_stop_reason() {
    let data = r#"{"type":"response.completed","response":{"status":"completed"}}"#;
    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();

    let event = parse_openai_response_event(
        data,
        &mut saw_text_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    )
    .expect("expected message end");
    match event {
        StreamEvent::MessageEnd { stop_reason } => {
            assert!(stop_reason.is_none());
        }
        other => panic!("expected MessageEnd, got {:?}", other),
    }
}

#[test]
fn test_parse_openai_response_completed_commentary_phase_sets_stop_reason() {
    let data = r#"{"type":"response.completed","response":{"status":"completed","output":[{"type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"Still working"}]}]}}"#;
    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();

    let event = parse_openai_response_event(
        data,
        &mut saw_text_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    )
    .expect("expected message end");
    match event {
        StreamEvent::MessageEnd { stop_reason } => {
            assert_eq!(stop_reason.as_deref(), Some("commentary"));
        }
        other => panic!("expected MessageEnd, got {:?}", other),
    }
}

#[test]
fn test_parse_openai_response_incomplete_emits_message_end_with_reason() {
    let data = r#"{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"content_filter"}}}"#;
    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();

    let event = parse_openai_response_event(
        data,
        &mut saw_text_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    )
    .expect("expected message end");
    match event {
        StreamEvent::MessageEnd { stop_reason } => {
            assert_eq!(stop_reason.as_deref(), Some("content_filter"));
        }
        other => panic!("expected MessageEnd, got {:?}", other),
    }
}

#[test]
fn test_parse_openai_response_function_call_arguments_streaming() {
    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();

    let added = r#"{"type":"response.output_item.added","item":{"id":"fc_123","type":"function_call","call_id":"call_123","name":"batch","arguments":""}}"#;
    assert!(
        parse_openai_response_event(
            added,
            &mut saw_text_delta,
            &mut streaming_tool_calls,
            &mut completed_tool_items,
            &mut pending,
        )
        .is_none(),
        "output_item.added should just seed tool state"
    );

    let delta = r#"{"type":"response.function_call_arguments.delta","item_id":"fc_123","delta":"{\"tool_calls\":[{\"tool\":\"read\"}]"}"#;
    assert!(
        parse_openai_response_event(
            delta,
            &mut saw_text_delta,
            &mut streaming_tool_calls,
            &mut completed_tool_items,
            &mut pending,
        )
        .is_none(),
        "argument delta should accumulate state only"
    );

    let done = r#"{"type":"response.function_call_arguments.done","item_id":"fc_123","arguments":"{\"tool_calls\":[{\"tool\":\"read\"}]}"}"#;
    let first = parse_openai_response_event(
        done,
        &mut saw_text_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    )
    .expect("expected tool start");

    match first {
        StreamEvent::ToolUseStart { id, name } => {
            assert_eq!(id, "call_123");
            assert_eq!(name, "batch");
        }
        other => panic!("expected ToolUseStart, got {:?}", other),
    }

    match pending.pop_front() {
        Some(StreamEvent::ToolInputDelta(delta)) => {
            let parsed: Value = serde_json::from_str(&delta).expect("valid args json");
            let tool_calls = parsed
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .expect("tool_calls array");
            assert_eq!(tool_calls.len(), 1);
        }
        other => panic!("expected ToolInputDelta, got {:?}", other),
    }

    assert!(matches!(pending.pop_front(), Some(StreamEvent::ToolUseEnd)));
    assert!(streaming_tool_calls.is_empty());
    assert!(completed_tool_items.contains("fc_123"));
}

#[test]
fn test_parse_openai_response_output_item_done_skips_duplicate_after_arguments_done() {
    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::from(["fc_123".to_string()]);
    let mut pending = VecDeque::new();

    let duplicate_done = r#"{"type":"response.output_item.done","item":{"id":"fc_123","type":"function_call","call_id":"call_123","name":"batch","arguments":"{\"tool_calls\":[]}"}}"#;
    let event = parse_openai_response_event(
        duplicate_done,
        &mut saw_text_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    );

    assert!(event.is_none(), "duplicate function call should be skipped");
    assert!(pending.is_empty());
    assert!(!completed_tool_items.contains("fc_123"));
}

#[test]
fn test_parse_openai_response_output_item_done_emits_native_compaction() {
    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();

    let compaction_done = r#"{"type":"response.output_item.done","item":{"id":"cmp_123","type":"compaction","encrypted_content":"enc_abc"}}"#;
    let event = parse_openai_response_event(
        compaction_done,
        &mut saw_text_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    )
    .expect("expected compaction event");

    match event {
        StreamEvent::Compaction {
            trigger,
            pre_tokens,
            openai_encrypted_content,
        } => {
            assert_eq!(trigger, "openai_native_auto");
            assert_eq!(pre_tokens, None);
            assert_eq!(openai_encrypted_content.as_deref(), Some("enc_abc"));
        }
        other => panic!("expected Compaction, got {:?}", other),
    }
    assert!(pending.is_empty());
}

#[test]
fn test_parse_openai_response_image_generation_saves_metadata_and_emits_event() {
    let _lock = ENV_LOCK.lock().unwrap();
    let original_dir = std::env::current_dir().expect("current dir");
    let temp = tempfile::Builder::new()
        .prefix("jcode-openai-image-test-")
        .tempdir()
        .expect("tempdir");
    std::env::set_current_dir(temp.path()).expect("set temp cwd");

    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();
    let data = r#"{
        "type":"response.output_item.done",
        "item":{
            "id":"ig_test_123",
            "type":"image_generation_call",
            "status":"completed",
            "output_format":"png",
            "revised_prompt":"A polished robot painter prompt",
            "result":"AQID"
        }
    }"#;

    let event = parse_openai_response_event(
        data,
        &mut saw_text_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    )
    .expect("expected generated image event");

    let (image_path, metadata_path) = match event {
        StreamEvent::GeneratedImage {
            id,
            path,
            metadata_path,
            output_format,
            revised_prompt,
        } => {
            assert_eq!(id, "ig_test_123");
            assert_eq!(output_format, "png");
            assert_eq!(revised_prompt.as_deref(), Some("A polished robot painter prompt"));
            (path, metadata_path.expect("metadata path"))
        }
        other => panic!("expected GeneratedImage, got {:?}", other),
    };

    assert!(std::path::Path::new(&image_path).exists());
    assert!(std::path::Path::new(&metadata_path).exists());
    match pending.pop_front() {
        Some(StreamEvent::TextDelta(markdown)) => {
            assert!(markdown.contains("![Generated image]"));
            assert!(markdown.contains("Metadata saved"));
        }
        other => panic!("expected generated image markdown TextDelta, got {:?}", other),
    }

    let metadata: Value = serde_json::from_slice(
        &std::fs::read(&metadata_path).expect("read generated image metadata"),
    )
    .expect("metadata json");
    assert_eq!(metadata["schema_version"], serde_json::json!(1));
    assert_eq!(metadata["provider"], serde_json::json!("openai"));
    assert_eq!(metadata["native_tool"], serde_json::json!("image_generation"));
    assert_eq!(metadata["revised_prompt"], serde_json::json!("A polished robot painter prompt"));
    assert!(metadata["response_item"].get("result").is_none());

    std::env::set_current_dir(original_dir).expect("restore cwd");
}

#[test]
fn test_build_tools_sets_strict_true() {
    let defs = vec![ToolDefinition {
        name: "bash".to_string(),
        description: "run shell".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": { "command": { "type": "string" } }
        }),
    }];
    let api_tools = build_tools(&defs);
    assert_eq!(api_tools.len(), 1);
    assert_eq!(api_tools[0]["strict"], serde_json::json!(true));
}

#[test]
fn test_build_tools_disables_strict_for_free_form_object_nodes() {
    let defs = vec![ToolDefinition {
        name: "batch".to_string(),
        description: "batch calls".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["tool_calls"],
            "properties": {
                "tool_calls": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["tool", "parameters"],
                        "properties": {
                            "tool": { "type": "string" },
                            "parameters": { "type": "object" }
                        }
                    }
                }
            }
        }),
    }];
    let api_tools = build_tools(&defs);
    assert_eq!(api_tools.len(), 1);
    assert_eq!(api_tools[0]["strict"], serde_json::json!(false));
    assert_eq!(
        api_tools[0]["parameters"]["properties"]["tool_calls"]["items"]["properties"]["parameters"]
            ["type"],
        serde_json::json!("object")
    );
}

#[test]
fn test_build_tools_normalizes_object_schema_additional_properties() {
    let defs = vec![ToolDefinition {
        name: "edit".to_string(),
        description: "apply edit".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "options": {
                    "type": "object",
                    "properties": {
                        "force": { "type": "boolean" }
                    }
                },
                "description": {
                    "type": "string"
                }
            },
            "required": ["path"]
        }),
    }];
    let api_tools = build_tools(&defs);
    assert_eq!(
        api_tools[0]["parameters"]["additionalProperties"],
        serde_json::json!(false)
    );
    assert_eq!(
        api_tools[0]["parameters"]["properties"]["options"]["additionalProperties"],
        serde_json::json!(false)
    );
    assert_eq!(
        api_tools[0]["parameters"]["required"],
        serde_json::json!(["description", "options", "path"])
    );
    assert_eq!(
        api_tools[0]["parameters"]["properties"]["description"]["type"],
        serde_json::json!(["string", "null"])
    );
}

#[test]
fn test_build_tools_rewrites_oneof_to_anyof_for_openai() {
    let defs = vec![ToolDefinition {
        name: "batch".to_string(),
        description: "batch calls".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["tool_calls"],
            "properties": {
                "tool_calls": {
                    "type": "array",
                    "items": {
                        "oneOf": [
                            {
                                "type": "object",
                                "required": ["tool"],
                                "properties": {
                                    "tool": { "type": "string" }
                                }
                            }
                        ]
                    }
                }
            }
        }),
    }];
    let api_tools = build_tools(&defs);
    assert!(api_tools[0]["parameters"]["properties"]["tool_calls"]["items"]["oneOf"].is_null());
    assert_eq!(
        api_tools[0]["parameters"]["properties"]["tool_calls"]["items"]["anyOf"][0]["type"],
        serde_json::json!("object")
    );
}

#[test]
fn test_build_tools_keeps_strict_for_anyof_object_branches_with_properties() {
    let defs = vec![ToolDefinition {
        name: "schedule".to_string(),
        description: "schedule work".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["task"],
            "anyOf": [
                {
                    "type": "object",
                    "required": ["wake_in_minutes"],
                    "properties": {
                        "wake_in_minutes": { "type": "integer" }
                    },
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "required": ["wake_at"],
                    "properties": {
                        "wake_at": { "type": "string" }
                    },
                    "additionalProperties": false
                }
            ],
            "properties": {
                "task": { "type": "string" },
                "wake_in_minutes": { "type": "integer" },
                "wake_at": { "type": "string" }
            }
        }),
    }];
    let api_tools = build_tools(&defs);
    assert_eq!(api_tools[0]["strict"], serde_json::json!(true));
    assert_eq!(
        api_tools[0]["parameters"]["anyOf"][0]["additionalProperties"],
        serde_json::json!(false)
    );
    assert_eq!(
        api_tools[0]["parameters"]["anyOf"][1]["additionalProperties"],
        serde_json::json!(false)
    );
}

#[test]
fn test_parse_text_wrapped_tool_call_prefers_trailing_json_object() {
    let text = "Status update\nassistant to=functions.batch commentary {}json\n{\"tool_calls\":[{\"tool\":\"read\",\"file_path\":\"src/main.rs\"}]}";
    let parsed = parse_text_wrapped_tool_call(text).expect("should parse wrapped tool call");
    assert_eq!(parsed.1, "batch");
    assert!(parsed.0.contains("Status update"));
    let args: Value = serde_json::from_str(&parsed.2).expect("valid args json");
    assert!(args.get("tool_calls").is_some());
}

#[test]
fn test_handle_openai_output_item_normalizes_null_arguments() {
    let item = serde_json::json!({
        "type": "function_call",
        "call_id": "call_1",
        "name": "bash",
        "arguments": "null",
    });
    let mut saw_text_delta = false;
    let mut pending = VecDeque::new();
    let first = handle_openai_output_item(item, &mut saw_text_delta, &mut pending)
        .expect("expected tool event");

    match first {
        StreamEvent::ToolUseStart { id, name } => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "bash");
        }
        _ => panic!("expected ToolUseStart"),
    }
    match pending.pop_front() {
        Some(StreamEvent::ToolInputDelta(delta)) => assert_eq!(delta, "{}"),
        _ => panic!("expected ToolInputDelta"),
    }
    assert!(matches!(pending.pop_front(), Some(StreamEvent::ToolUseEnd)));
}

#[test]
fn test_handle_openai_output_item_recovers_bright_pearl_fixture() {
    let item = serde_json::json!({
        "type": "message",
        "content": [{
            "type": "output_text",
            "text": BRIGHT_PEARL_WRAPPED_TOOL_CALL_FIXTURE,
        }],
    });

    let mut saw_text_delta = false;
    let mut pending = VecDeque::new();
    let mut events = Vec::new();

    if let Some(first) = handle_openai_output_item(item, &mut saw_text_delta, &mut pending) {
        events.push(first);
    }
    while let Some(ev) = pending.pop_front() {
        events.push(ev);
    }

    let mut saw_prefix = false;
    let mut saw_tool = false;
    let mut saw_input = false;

    for event in events {
        match event {
            StreamEvent::TextDelta(text)
                if text.contains("Status: I detected pre-existing local edits") =>
            {
                saw_prefix = true;
            }
            StreamEvent::ToolUseStart { name, .. } if name == "batch" => {
                saw_tool = true;
            }
            StreamEvent::ToolInputDelta(delta) => {
                let args: Value = serde_json::from_str(&delta).expect("valid tool args");
                let calls = args
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .expect("tool_calls array");
                assert_eq!(calls.len(), 3);
                saw_input = true;
            }
            _ => {}
        }
    }

    assert!(saw_prefix);
    assert!(saw_tool);
    assert!(saw_input);
}

#[test]
fn test_build_responses_input_rewrites_orphan_tool_output_as_user_message() {
    let messages = vec![ChatMessage::tool_result(
        "call_orphan",
        "orphan result",
        false,
    )];

    let items = build_responses_input(&messages);
    let mut saw_rewritten_message = false;

    for item in &items {
        assert_ne!(
            item.get("type").and_then(|v| v.as_str()),
            Some("function_call_output")
        );
        if item.get("type").and_then(|v| v.as_str()) == Some("message")
            && item.get("role").and_then(|v| v.as_str()) == Some("user")
            && let Some(content) = item.get("content").and_then(|v| v.as_array())
        {
            for part in content {
                if part.get("type").and_then(|v| v.as_str()) == Some("input_text") {
                    let text = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    if text.contains("[Recovered orphaned tool output: call_orphan]")
                        && text.contains("orphan result")
                    {
                        saw_rewritten_message = true;
                    }
                }
            }
        }
    }

    assert!(saw_rewritten_message);
}
