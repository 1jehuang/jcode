#[test]
fn test_build_responses_input_injects_missing_tool_output() {
    let expected_missing = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);
    let messages = vec![
        ChatMessage {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        ChatMessage {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let items = build_responses_input(&messages);
    let mut saw_call = false;
    let mut saw_output = false;

    for item in &items {
        let item_type = item.get("type").and_then(|v| v.as_str());
        match item_type {
            Some("function_call") => {
                if item.get("call_id").and_then(|v| v.as_str()) == Some("call_1") {
                    saw_call = true;
                }
            }
            Some("function_call_output") => {
                if item.get("call_id").and_then(|v| v.as_str()) == Some("call_1") {
                    let output = item.get("output").and_then(|v| v.as_str());
                    assert_eq!(output, Some(expected_missing.as_str()));
                    saw_output = true;
                }
            }
            _ => {}
        }
    }

    assert!(saw_call);
    assert!(saw_output);
}

#[test]
fn test_build_responses_input_preserves_tool_output() {
    let messages = vec![
        ChatMessage {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        ChatMessage::tool_result("call_1", "ok", false),
    ];

    let items = build_responses_input(&messages);
    let mut outputs = Vec::new();

    for item in &items {
        if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
            && item.get("call_id").and_then(|v| v.as_str()) == Some("call_1")
            && let Some(output) = item.get("output").and_then(|v| v.as_str())
        {
            outputs.push(output.to_string());
        }
    }

    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0], "ok");
}

#[test]
fn test_build_responses_input_reorders_early_tool_output() {
    let messages = vec![
        ChatMessage::tool_result("call_1", "ok", false),
        ChatMessage {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let items = build_responses_input(&messages);
    let mut call_pos = None;
    let mut output_pos = None;
    let mut outputs = Vec::new();

    for (idx, item) in items.iter().enumerate() {
        let item_type = item.get("type").and_then(|v| v.as_str());
        match item_type {
            Some("function_call") => {
                if item.get("call_id").and_then(|v| v.as_str()) == Some("call_1") {
                    call_pos = Some(idx);
                }
            }
            Some("function_call_output") => {
                if item.get("call_id").and_then(|v| v.as_str()) == Some("call_1") {
                    output_pos = Some(idx);
                    if let Some(output) = item.get("output").and_then(|v| v.as_str()) {
                        outputs.push(output.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    assert!(call_pos.is_some());
    assert!(output_pos.is_some());
    assert!(output_pos.unwrap() > call_pos.unwrap());
    assert_eq!(outputs, vec!["ok".to_string()]);
}

#[test]
fn test_build_responses_input_keeps_image_context_after_tool_output() {
    let messages = vec![
        ChatMessage {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file_path": "screenshot.png"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        ChatMessage {
            role: Role::User,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "Image: screenshot.png\nImage sent to model for vision analysis."
                        .to_string(),
                    is_error: None,
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "ZmFrZQ==".to_string(),
                },
                ContentBlock::Text {
                    text:
                        "[Attached image associated with the preceding tool result: screenshot.png]"
                            .to_string(),
                    cache_control: None,
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let items = build_responses_input(&messages);
    let mut output_pos = None;
    let mut image_msg_pos = None;

    for (idx, item) in items.iter().enumerate() {
        match item.get("type").and_then(|v| v.as_str()) {
            Some("function_call_output")
                if item.get("call_id").and_then(|v| v.as_str()) == Some("call_1") =>
            {
                output_pos = Some(idx);
                assert_eq!(
                    item.get("output").and_then(|v| v.as_str()),
                    Some("Image: screenshot.png\nImage sent to model for vision analysis.")
                );
            }
            Some("message") if item.get("role").and_then(|v| v.as_str()) == Some("user") => {
                let Some(content) = item.get("content").and_then(|v| v.as_array()) else {
                    continue;
                };
                let has_image = content
                    .iter()
                    .any(|part| part.get("type").and_then(|v| v.as_str()) == Some("input_image"));
                let has_label = content.iter().any(|part| {
                    part.get("type").and_then(|v| v.as_str()) == Some("input_text")
                        && part
                            .get("text")
                            .and_then(|v| v.as_str())
                            .map(|text| text.contains("screenshot.png"))
                            .unwrap_or(false)
                });
                if has_image && has_label {
                    image_msg_pos = Some(idx);
                }
            }
            _ => {}
        }
    }

    assert!(output_pos.is_some(), "expected function call output item");
    assert!(
        image_msg_pos.is_some(),
        "expected follow-up user image message"
    );
    assert!(
        image_msg_pos.unwrap() > output_pos.unwrap(),
        "image context should stay after the tool output"
    );
}

#[test]
fn test_build_responses_input_injects_only_missing_outputs() {
    let expected_missing = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);
    let messages = vec![
        ChatMessage {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_a".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "pwd"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        ChatMessage {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_b".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "whoami"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        ChatMessage::tool_result("call_b", "done", false),
    ];

    let items = build_responses_input(&messages);
    let mut output_a = None;
    let mut output_b = None;

    for item in &items {
        if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output") {
            match item.get("call_id").and_then(|v| v.as_str()) {
                Some("call_a") => {
                    output_a = item
                        .get("output")
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string());
                }
                Some("call_b") => {
                    output_b = item
                        .get("output")
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string());
                }
                _ => {}
            }
        }
    }

    assert_eq!(output_a.as_deref(), Some(expected_missing.as_str()));
    assert_eq!(output_b.as_deref(), Some("done"));
}

#[test]
fn test_openai_retryable_error_patterns() {
    assert!(is_retryable_error(
        "stream disconnected before completion: transport error"
    ));
    assert!(is_retryable_error(
        "falling back from websockets to https transport. stream disconnected before completion"
    ));
    assert!(is_retryable_error(
        "OpenAI HTTPS stream ended before message completion marker"
    ));
}

#[test]
fn test_parse_max_output_tokens_defaults_to_safe_value() {
    assert_eq!(
        OpenAIProvider::parse_max_output_tokens(None),
        Some(DEFAULT_MAX_OUTPUT_TOKENS)
    );
    assert_eq!(
        OpenAIProvider::parse_max_output_tokens(Some("")),
        Some(DEFAULT_MAX_OUTPUT_TOKENS)
    );
}

#[test]
fn test_parse_max_output_tokens_allows_disable_and_override() {
    assert_eq!(OpenAIProvider::parse_max_output_tokens(Some("0")), None);
    assert_eq!(
        OpenAIProvider::parse_max_output_tokens(Some("32768")),
        Some(32768)
    );
    assert_eq!(
        OpenAIProvider::parse_max_output_tokens(Some("not-a-number")),
        Some(DEFAULT_MAX_OUTPUT_TOKENS)
    );
}

#[test]
fn test_build_response_request_for_gpt_5_4_1m_uses_base_model_without_extra_flags() {
    let request = OpenAIProvider::build_response_request(
        "gpt-5.4",
        "system".to_string(),
        &[],
        &[],
        true,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        Some("xhigh"),
        Some("unused"),
        Some("unused"),
        None,
        None,
    );

    assert_eq!(request["model"], serde_json::json!("gpt-5.4"));
    assert!(request.get("model_context_window").is_none());
    assert!(request.get("max_output_tokens").is_none());
    assert!(request.get("prompt_cache_key").is_none());
    assert!(request.get("prompt_cache_retention").is_none());
}

#[test]
fn test_build_response_request_omits_long_context_for_plain_gpt_5_4() {
    let request = OpenAIProvider::build_response_request(
        "gpt-5.4",
        "system".to_string(),
        &[],
        &[],
        true,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        None,
        None,
    );

    assert!(request.get("model_context_window").is_none());
}
