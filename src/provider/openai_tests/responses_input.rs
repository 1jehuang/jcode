fn assistant_tool_use(id: &str, name: &str, input: serde_json::Value) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

fn user_text(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

fn response_item_type(item: &serde_json::Value) -> Option<&str> {
    item.get("type").and_then(|v| v.as_str())
}

fn response_item_call_id(item: &serde_json::Value) -> Option<&str> {
    item.get("call_id").and_then(|v| v.as_str())
}

fn function_call_pos(items: &[serde_json::Value], call_id: &str) -> Option<usize> {
    items.iter().position(|item| {
        response_item_type(item) == Some("function_call")
            && response_item_call_id(item) == Some(call_id)
    })
}

fn function_call_output_pos(items: &[serde_json::Value], call_id: &str) -> Option<usize> {
    items.iter().position(|item| {
        response_item_type(item) == Some("function_call_output")
            && response_item_call_id(item) == Some(call_id)
    })
}

fn function_call_outputs(items: &[serde_json::Value], call_id: &str) -> Vec<String> {
    items
        .iter()
        .filter(|item| {
            response_item_type(item) == Some("function_call_output")
                && response_item_call_id(item) == Some(call_id)
        })
        .filter_map(|item| item.get("output").and_then(|v| v.as_str()))
        .map(str::to_string)
        .collect()
}

#[expect(
    clippy::too_many_arguments,
    reason = "test helper mirrors the request builder to keep call sites explicit"
)]
fn build_test_response_request(
    model_id: &str,
    is_chatgpt_mode: bool,
    allow_optional_params: bool,
    max_output_tokens: Option<u32>,
    reasoning_effort: Option<&str>,
    service_tier: Option<&str>,
    prompt_cache_key: Option<&str>,
    prompt_cache_retention: Option<&str>,
    native_compaction_threshold: Option<usize>,
) -> serde_json::Value {
    OpenAIProvider::build_response_request(
        model_id,
        "system".to_string(),
        &[],
        &[],
        is_chatgpt_mode,
        allow_optional_params,
        max_output_tokens,
        reasoning_effort,
        service_tier,
        prompt_cache_key,
        prompt_cache_retention,
        native_compaction_threshold,
    )
}

#[test]
fn test_build_responses_input_injects_missing_tool_output() {
    let expected_missing = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);
    let messages = vec![
        user_text("hi"),
        assistant_tool_use("call_1", "bash", serde_json::json!({"command": "ls"})),
    ];

    let items = build_responses_input(&messages);
    assert!(function_call_pos(&items, "call_1").is_some());
    assert_eq!(
        function_call_outputs(&items, "call_1"),
        vec![expected_missing]
    );
}

#[test]
fn test_build_responses_input_preserves_tool_output() {
    let messages = vec![
        assistant_tool_use("call_1", "bash", serde_json::json!({"command": "ls"})),
        ChatMessage::tool_result("call_1", "ok", false),
    ];

    let items = build_responses_input(&messages);
    assert_eq!(function_call_outputs(&items, "call_1"), vec!["ok"]);
}

#[test]
fn test_build_responses_input_reorders_early_tool_output() {
    let messages = vec![
        ChatMessage::tool_result("call_1", "ok", false),
        assistant_tool_use("call_1", "bash", serde_json::json!({"command": "ls"})),
    ];

    let items = build_responses_input(&messages);
    let call_pos = function_call_pos(&items, "call_1");
    let output_pos = function_call_output_pos(&items, "call_1");

    assert!(call_pos.is_some());
    assert!(output_pos.is_some());
    assert!(output_pos.unwrap() > call_pos.unwrap());
    assert_eq!(function_call_outputs(&items, "call_1"), vec!["ok"]);
}

#[test]
fn test_build_responses_input_keeps_image_context_after_tool_output() {
    let messages = vec![
        assistant_tool_use(
            "call_1",
            "read",
            serde_json::json!({"file_path": "screenshot.png"}),
        ),
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
    let output_pos = function_call_output_pos(&items, "call_1");
    let mut image_msg_pos = None;

    for (idx, item) in items.iter().enumerate() {
        match response_item_type(item) {
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

    assert_eq!(
        function_call_outputs(&items, "call_1"),
        vec!["Image: screenshot.png\nImage sent to model for vision analysis."]
    );
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
        assistant_tool_use("call_a", "bash", serde_json::json!({"command": "pwd"})),
        assistant_tool_use("call_b", "bash", serde_json::json!({"command": "whoami"})),
        ChatMessage::tool_result("call_b", "done", false),
    ];

    let items = build_responses_input(&messages);

    assert_eq!(
        function_call_outputs(&items, "call_a"),
        vec![expected_missing]
    );
    assert_eq!(function_call_outputs(&items, "call_b"), vec!["done"]);
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
    let request = build_test_response_request(
        "gpt-5.4",
        true,
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
    assert_eq!(
        request["reasoning"],
        serde_json::json!({ "effort": "xhigh" })
    );
    assert_eq!(request["service_tier"], serde_json::json!("unused"));
    assert!(
        request["tools"]
            .as_array()
            .expect("tools should be an array")
            .contains(&serde_json::json!({ "type": "image_generation" }))
    );
}

#[test]
fn test_build_response_request_omits_long_context_for_plain_gpt_5_4() {
    let request = build_test_response_request(
        "gpt-5.4",
        true,
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

#[test]
fn test_build_response_request_defaults_extended_cache_retention_for_gpt_5_5() {
    let request = build_test_response_request(
        "gpt-5.5",
        false,
        true,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        None,
        None,
    );

    assert_eq!(request["prompt_cache_retention"], serde_json::json!("24h"));
    assert_eq!(
        request["max_output_tokens"],
        serde_json::json!(DEFAULT_MAX_OUTPUT_TOKENS)
    );
}

#[test]
fn test_build_response_request_respects_configured_cache_retention() {
    let request = build_test_response_request(
        "gpt-5.5",
        false,
        true,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        Some("in_memory"),
        None,
    );

    assert_eq!(
        request["prompt_cache_retention"],
        serde_json::json!("in_memory")
    );
}

#[test]
fn test_openai_cache_ttl_is_model_aware() {
    assert_eq!(
        crate::tui::cache_ttl_for_provider_model("openai", Some("gpt-5.5")),
        Some(24 * 60 * 60)
    );
    assert_eq!(
        crate::tui::cache_ttl_for_provider_model("openai", Some("gpt-4o")),
        Some(300)
    );
}
