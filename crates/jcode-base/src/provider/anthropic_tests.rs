use super::*;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_if_missing(key: &'static str, value: &str) -> Option<Self> {
        if std::env::var_os(key).is_some() {
            return None;
        }
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Some(Self { key, previous })
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

async fn collect_live_smoke_stream(
    mut stream: EventStream,
    timeout: std::time::Duration,
) -> Result<(usize, usize, bool)> {
    tokio::time::timeout(timeout, async move {
        let mut text_bytes = 0usize;
        let mut thinking_bytes = 0usize;
        let mut saw_message_end = false;
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::TextDelta(text) => {
                    text_bytes += text.len();
                }
                StreamEvent::ThinkingDelta(text) => {
                    thinking_bytes += text.len();
                }
                StreamEvent::MessageEnd { .. } => {
                    saw_message_end = true;
                    break;
                }
                StreamEvent::Error { message, .. } => anyhow::bail!(message),
                _ => {}
            }
        }
        Ok((text_bytes, thinking_bytes, saw_message_end))
    })
    .await
    .context("live provider smoke timed out")?
}

#[test]
fn test_parse_sse_event() {
    let mut buffer = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n".to_string();
    let event = parse_sse_event(&mut buffer).unwrap();
    assert_eq!(event.event_type, "message_start");
    assert!(buffer.is_empty());
}

#[tokio::test]
async fn test_available_models() {
    let provider = AnthropicProvider::new();
    let models = provider.available_models();
    assert!(models.contains(&"claude-opus-4-8"));
    // Opus 4.8 is native-1M, so there is no redundant `[1m]` alias.
    assert!(!models.contains(&"claude-opus-4-8[1m]"));
    assert!(models.contains(&"claude-opus-4-6"));
    assert!(models.contains(&"claude-opus-4-6[1m]"));
    assert!(models.contains(&"claude-sonnet-4-6"));
    assert!(models.contains(&"claude-sonnet-4-6[1m]"));
    assert!(models.contains(&"claude-haiku-4-5"));
}

#[test]
fn test_effectively_1m_requires_explicit_suffix() {
    assert!(!effectively_1m("claude-opus-4-6"));
    assert!(!effectively_1m("claude-sonnet-4-6"));
    assert!(effectively_1m("claude-opus-4-6[1m]"));
    assert!(effectively_1m("claude-sonnet-4-6[1m]"));
}

#[test]
fn test_oauth_beta_headers_require_explicit_1m_suffix() {
    assert_eq!(oauth_beta_headers("claude-opus-4-6"), OAUTH_BETA_HEADERS);
    assert_eq!(
        oauth_beta_headers("claude-opus-4-6[1m]"),
        OAUTH_BETA_HEADERS_1M
    );
}

#[test]
fn test_anthropic_reasoning_effort_request_parts() {
    let provider = AnthropicProvider::new();
    provider.set_model("claude-sonnet-4-6").unwrap();
    provider.set_reasoning_effort("none").unwrap();

    assert_eq!(
        provider.available_efforts(),
        vec!["none", "low", "medium", "high"]
    );
    assert_eq!(provider.reasoning_effort().as_deref(), Some("none"));

    provider.set_reasoning_effort("max").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("high"));

    provider.set_reasoning_effort("medium").unwrap();
    let (thinking, output_config, temperature) =
        provider.build_reasoning_request_parts("claude-sonnet-4-6", true);

    match thinking.expect("adaptive thinking should be enabled") {
        ApiThinking::Adaptive { display } => assert_eq!(display, Some("summarized")),
        ApiThinking::Enabled { .. } => panic!("Claude 4.6 should use adaptive thinking"),
    }
    assert_eq!(
        output_config.expect("output_config should be set").effort,
        "medium"
    );
    assert_eq!(
        temperature, None,
        "thinking requests must omit OAuth temperature"
    );
}

#[test]
fn test_anthropic_show_thinking_enables_adaptive_thinking_without_effort() {
    // With no explicit reasoning effort, an adaptive-thinking model should still
    // request summarized thinking when the user has opted into the display.
    // Crucially, `output_config` must stay None so we do not force a stronger
    // (more expensive) reasoning level than the model's default.
    //
    // `build_reasoning_request_parts_inner` takes the model directly, so we do
    // not depend on `set_model` accepting a particular catalog entry. With no
    // effort configured, `self.reasoning_effort()` resolves to None regardless
    // of the default model.
    let provider = AnthropicProvider::new();
    // Make the test independent of the ambient config's anthropic_reasoning_effort
    // by clearing the field directly; we only exercise the show_thinking path.
    *provider.reasoning_effort.write().unwrap() = None;

    // show_thinking = false: nothing requested.
    let (thinking, output_config, _temp) =
        provider.build_reasoning_request_parts_inner("claude-opus-4-8", true, false);
    assert!(
        thinking.is_none(),
        "no thinking should be requested when both effort and show_thinking are off"
    );
    assert!(output_config.is_none());

    // show_thinking = true: adaptive thinking requested, no output_config.
    let (thinking, output_config, temperature) =
        provider.build_reasoning_request_parts_inner("claude-opus-4-8", true, true);
    match thinking.expect("show_thinking should enable adaptive thinking") {
        ApiThinking::Adaptive { display } => assert_eq!(display, Some("summarized")),
        ApiThinking::Enabled { .. } => panic!("Opus 4.8 should use adaptive thinking"),
    }
    assert!(
        output_config.is_none(),
        "show_thinking alone must not force an output reasoning effort"
    );
    assert_eq!(
        temperature, None,
        "thinking requests must omit OAuth temperature"
    );
}

#[test]
fn test_anthropic_show_thinking_enables_manual_thinking_without_effort() {
    // Manual-thinking models (e.g. Opus 4.5) need a concrete budget; with only
    // the display toggle on we fall back to the minimal budget. The model is
    // passed directly so this does not depend on `set_model` validation.
    let provider = AnthropicProvider::new();
    // Independent of ambient config: clear any configured effort.
    *provider.reasoning_effort.write().unwrap() = None;

    let (thinking, _output_config, _temp) =
        provider.build_reasoning_request_parts_inner("claude-opus-4-5", false, false);
    assert!(thinking.is_none());

    let (thinking, _output_config, _temperature) =
        provider.build_reasoning_request_parts_inner("claude-opus-4-5", false, true);
    match thinking.expect("show_thinking should enable manual thinking") {
        ApiThinking::Enabled { budget_tokens } => assert_eq!(budget_tokens, 1_024),
        ApiThinking::Adaptive { .. } => panic!("Opus 4.5 should use manual thinking"),
    }
}

#[test]
fn test_anthropic_max_alias_uses_strongest_real_effort() {
    assert_eq!(
        AnthropicProvider::actual_effort_for_model("claude-sonnet-4-6", "max"),
        "high"
    );
    assert_eq!(
        AnthropicProvider::actual_effort_for_model("claude-opus-4-7", "max"),
        "xhigh"
    );
    assert_eq!(
        AnthropicProvider::actual_effort_for_model("claude-opus-4-8", "max"),
        "xhigh"
    );
}

#[test]
fn test_anthropic_opus_48_fast_mode_service_tier_serializes_priority() {
    let provider = AnthropicProvider::new();
    provider.set_model("claude-opus-4-8").unwrap();

    assert_eq!(provider.available_service_tiers(), vec!["off", "priority"]);
    assert_eq!(provider.service_tier(), None);

    provider.set_service_tier("priority").unwrap();
    assert_eq!(provider.service_tier().as_deref(), Some("priority"));

    let request = ApiRequest {
        model: strip_1m_suffix(&provider.model()).to_string(),
        max_tokens: 1024,
        system: None,
        messages: vec![],
        tools: None,
        metadata: None,
        thinking: None,
        output_config: None,
        temperature: None,
        service_tier: provider.current_service_tier_for_model(&provider.model()),
        stream: true,
    };
    let value = serde_json::to_value(&request).unwrap();

    assert_eq!(value["model"], "claude-opus-4-8");
    assert_eq!(value["service_tier"], "auto");
}

#[test]
fn test_anthropic_fast_mode_is_limited_to_opus_48() {
    let provider = AnthropicProvider::new();
    provider.set_model("claude-opus-4-6").unwrap();

    assert!(provider.available_service_tiers().is_empty());
    assert!(provider.set_service_tier("priority").is_err());
    assert_eq!(provider.service_tier(), None);

    // A stale `[1m]` alias for a native-1M model is migrated to canonical form.
    provider.set_model("claude-opus-4-8[1m]").unwrap();
    assert_eq!(provider.model(), "claude-opus-4-8");
    provider.set_service_tier("priority").unwrap();
    assert_eq!(provider.service_tier().as_deref(), Some("priority"));

    provider.set_service_tier("off").unwrap();
    assert_eq!(provider.service_tier(), None);
}

#[test]
fn test_anthropic_manual_thinking_budget_for_opus_45() {
    let provider = AnthropicProvider::new();
    // Keep this request-builder test independent of the live/persisted Anthropic
    // model catalog, which may legitimately omit older Opus 4.5 models.
    *provider
        .model
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = "claude-opus-4-5".to_string();
    provider.set_reasoning_effort("high").unwrap();

    let (thinking, output_config, temperature) =
        provider.build_reasoning_request_parts("claude-opus-4-5", false);

    match thinking.expect("manual thinking should be enabled") {
        ApiThinking::Enabled { budget_tokens } => assert_eq!(budget_tokens, 8_192),
        ApiThinking::Adaptive { .. } => panic!("Claude Opus 4.5 should use manual thinking"),
    }
    assert_eq!(output_config.unwrap().effort, "high");
    assert_eq!(temperature, None);
}

#[test]
fn test_anthropic_thinking_sse_events() {
    let mut current_tool_use = None;
    let mut current_thinking_block = false;
    let mut input_tokens = None;
    let mut output_tokens = None;
    let mut cache_read_input_tokens = None;
    let mut cache_creation_input_tokens = None;

    let start = SseEvent {
        event_type: "content_block_start".to_string(),
        data: serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "thinking", "thinking": "", "signature": "sig"}
        })
        .to_string(),
    };
    let events = process_sse_event(
        &start,
        &mut current_tool_use,
        &mut current_thinking_block,
        &mut input_tokens,
        &mut output_tokens,
        &mut cache_read_input_tokens,
        &mut cache_creation_input_tokens,
        false,
    );
    assert!(matches!(events.as_slice(), [StreamEvent::ThinkingStart]));
    assert!(current_thinking_block);

    let delta = SseEvent {
        event_type: "content_block_delta".to_string(),
        data: serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "reasoning text"}
        })
        .to_string(),
    };
    let events = process_sse_event(
        &delta,
        &mut current_tool_use,
        &mut current_thinking_block,
        &mut input_tokens,
        &mut output_tokens,
        &mut cache_read_input_tokens,
        &mut cache_creation_input_tokens,
        false,
    );
    assert!(
        matches!(events.as_slice(), [StreamEvent::ThinkingDelta(text)] if text == "reasoning text")
    );

    let signature = SseEvent {
        event_type: "content_block_delta".to_string(),
        data: serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "signature_delta", "signature": "signed"}
        })
        .to_string(),
    };
    let events = process_sse_event(
        &signature,
        &mut current_tool_use,
        &mut current_thinking_block,
        &mut input_tokens,
        &mut output_tokens,
        &mut cache_read_input_tokens,
        &mut cache_creation_input_tokens,
        false,
    );
    assert!(
        matches!(events.as_slice(), [StreamEvent::ThinkingSignatureDelta(sig)] if sig == "signed")
    );

    let stop = SseEvent {
        event_type: "content_block_stop".to_string(),
        data: serde_json::json!({"type": "content_block_stop", "index": 0}).to_string(),
    };
    let events = process_sse_event(
        &stop,
        &mut current_tool_use,
        &mut current_thinking_block,
        &mut input_tokens,
        &mut output_tokens,
        &mut cache_read_input_tokens,
        &mut cache_creation_input_tokens,
        false,
    );
    assert!(matches!(events.as_slice(), [StreamEvent::ThinkingEnd]));
    assert!(!current_thinking_block);
}

#[test]
fn test_anthropic_signed_thinking_replayed_in_request_blocks() {
    let provider = AnthropicProvider::new();
    let blocks = provider.format_content_blocks(
        &[ContentBlock::AnthropicThinking {
            thinking: "reasoning text".to_string(),
            signature: "signed".to_string(),
        }],
        false,
    );

    let value = serde_json::to_value(&blocks).expect("serialize content blocks");
    assert_eq!(
        value,
        serde_json::json!([
            {
                "type": "thinking",
                "thinking": "reasoning text",
                "signature": "signed"
            }
        ])
    );
}

#[tokio::test]
#[ignore = "live smoke: requires ANTHROPIC_API_KEY, or set JCODE_LIVE_ANTHROPIC_ALLOW_OAUTH=1 to use Claude OAuth credentials"]
async fn live_anthropic_reasoning_smoke() -> Result<()> {
    let _env_lock = crate::storage::lock_test_env();
    let using_api_key = std::env::var_os("ANTHROPIC_API_KEY").is_some();
    let allow_oauth = std::env::var_os("JCODE_LIVE_ANTHROPIC_ALLOW_OAUTH").is_some();
    if !using_api_key && !allow_oauth {
        eprintln!(
            "skipping live Anthropic smoke: set ANTHROPIC_API_KEY or JCODE_LIVE_ANTHROPIC_ALLOW_OAUTH=1"
        );
        return Ok(());
    }

    let _max_tokens = EnvVarGuard::set_if_missing("JCODE_ANTHROPIC_MAX_TOKENS", "2048");
    let model = std::env::var("JCODE_LIVE_ANTHROPIC_MODEL")
        .or_else(|_| std::env::var("JCODE_ANTHROPIC_MODEL"))
        .unwrap_or_else(|_| "claude-sonnet-4-6".to_string());
    let effort = std::env::var("JCODE_LIVE_ANTHROPIC_REASONING_EFFORT")
        .unwrap_or_else(|_| "low".to_string());
    let prompt = std::env::var("JCODE_LIVE_ANTHROPIC_PROMPT")
        .unwrap_or_else(|_| "Live smoke test: answer exactly OK.".to_string());
    let system = std::env::var("JCODE_LIVE_ANTHROPIC_SYSTEM").unwrap_or_else(|_| {
        "You are a live provider smoke test. Keep the answer tiny.".to_string()
    });
    let require_thinking = std::env::var_os("JCODE_LIVE_ANTHROPIC_REQUIRE_THINKING").is_some();

    let provider = AnthropicProvider::new();
    provider.set_model(&model)?;
    provider.set_reasoning_effort(&effort)?;

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: prompt,
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let stream = provider.complete(&messages, &[], &system, None).await?;
    let (text_bytes, thinking_bytes, saw_message_end) =
        collect_live_smoke_stream(stream, std::time::Duration::from_secs(90)).await?;

    eprintln!(
        "live Anthropic reasoning smoke passed: model={model}, effort={effort}, text_bytes={text_bytes}, thinking_bytes={thinking_bytes}, message_end={saw_message_end}"
    );
    assert!(
        text_bytes > 0 || thinking_bytes > 0,
        "live Anthropic response contained neither text nor thinking deltas"
    );
    if require_thinking {
        assert!(
            thinking_bytes > 0,
            "live Anthropic response did not include thinking deltas despite JCODE_LIVE_ANTHROPIC_REQUIRE_THINKING"
        );
    }
    Ok(())
}

#[tokio::test]
async fn test_dangling_tool_use_repair() {
    let provider = AnthropicProvider::new();

    // Create messages with a dangling tool_use (no corresponding tool_result)
    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Let me check".to_string(),
                    cache_control: None,
                },
                ContentBlock::ToolUse {
                    id: "tool_123".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                    thought_signature: None,
                },
                ContentBlock::ToolUse {
                    id: "tool_456".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "/tmp/test"}),
                    thought_signature: None,
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
        // Missing tool_results for tool_123 and tool_456!
    ];

    let formatted = provider.format_messages(&messages, false);

    // Should have 3 messages:
    // 1. User: "Hello"
    // 2. Assistant: text + tool_uses
    // 3. User: synthetic tool_results for the dangling tool_uses
    assert_eq!(formatted.len(), 3);

    // Check the synthetic tool_result message
    let synthetic_msg = &formatted[2];
    assert_eq!(synthetic_msg.role, "user");
    assert_eq!(synthetic_msg.content.len(), 2);

    // Verify both tool_results are present
    let mut found_ids = std::collections::HashSet::new();
    for block in &synthetic_msg.content {
        if let ApiContentBlock::ToolResult {
            tool_use_id,
            is_error,
            content,
        } = block
        {
            found_ids.insert(tool_use_id.clone());
            assert!(is_error);
            match content {
                ToolResultContent::Text(t) => assert!(t.contains("interrupted")),
                ToolResultContent::Blocks(_) => panic!("Expected text content"),
            }
        } else {
            panic!("Expected ToolResult block");
        }
    }
    assert!(found_ids.contains("tool_123"));
    assert!(found_ids.contains("tool_456"));
}

#[tokio::test]
async fn test_no_repair_when_tool_results_present() {
    let provider = AnthropicProvider::new();

    // Create messages where tool_use has a corresponding tool_result
    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tool_123".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_123".to_string(),
                content: "file1.txt\nfile2.txt".to_string(),
                is_error: Some(false),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let formatted = provider.format_messages(&messages, false);

    // Should have exactly 3 messages (no synthetic ones added)
    assert_eq!(formatted.len(), 3);

    // The last message should be the actual tool_result, not synthetic
    let last_msg = &formatted[2];
    if let ApiContentBlock::ToolResult { content, .. } = &last_msg.content[0] {
        match content {
            ToolResultContent::Text(t) => assert!(t.contains("file1.txt")),
            ToolResultContent::Blocks(_) => panic!("Expected text content"),
        }
    } else {
        panic!("Expected ToolResult block");
    }
}

#[tokio::test]
async fn test_parallel_image_tool_results_stay_contiguous() {
    // Regression for Anthropic 400: "`tool_use` ids were found without `tool_result`
    // blocks immediately after". When the assistant issues several parallel `read`
    // calls that return images, each tool result is stored as its own user message in
    // the form [tool_result, image, "[Attached image ...]" text]. After merging the
    // consecutive user messages, the sibling label text blocks were wedged between the
    // tool_results, which Anthropic rejects. The label must be folded into the
    // tool_result content so every tool_result stays contiguous.
    let provider = AnthropicProvider::new();

    let make_image_result = |id: &str, label: &str| Message {
        role: Role::User,
        content: vec![
            ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: format!("Image: {label}"),
                is_error: None,
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            },
            ContentBlock::Text {
                text: format!(
                    "[Attached image associated with the preceding tool result: {label}]"
                ),
                cache_control: None,
            },
        ],
        timestamp: None,
        tool_duration_ms: None,
    };

    let messages = vec![
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: "tool_a".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "a.png"}),
                    thought_signature: None,
                },
                ContentBlock::ToolUse {
                    id: "tool_b".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "b.png"}),
                    thought_signature: None,
                },
                ContentBlock::ToolUse {
                    id: "tool_c".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "c.png"}),
                    thought_signature: None,
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
        make_image_result("tool_a", "a.png"),
        make_image_result("tool_b", "b.png"),
        make_image_result("tool_c", "c.png"),
    ];

    let formatted = provider.format_messages(&messages, false);

    // assistant message + merged user tool_result message
    assert_eq!(formatted.len(), 2);
    let user_msg = &formatted[1];
    assert_eq!(user_msg.role, "user");

    // Every block in the user message must be a tool_result (no sibling text blocks
    // wedged between them), and all three tool_use ids must be present.
    let mut seen = std::collections::HashSet::new();
    for block in &user_msg.content {
        match block {
            ApiContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                seen.insert(tool_use_id.clone());
                // Each image tool_result should carry its image and folded label text.
                match content {
                    ToolResultContent::Blocks(blocks) => {
                        assert!(
                            blocks
                                .iter()
                                .any(|b| matches!(b, ToolResultContentBlock::Image { .. })),
                            "image tool_result should contain an image block"
                        );
                        assert!(
                            blocks.iter().any(|b| matches!(
                                b,
                                ToolResultContentBlock::Text { text }
                                    if text.contains("[Attached image associated")
                            )),
                            "label text should be folded into the tool_result content"
                        );
                    }
                    ToolResultContent::Text(_) => {
                        panic!("image tool_result should use block content")
                    }
                }
            }
            _ => panic!("expected only tool_result blocks in the user message"),
        }
    }
    assert_eq!(
        seen,
        ["tool_a", "tool_b", "tool_c"]
            .iter()
            .map(|s| s.to_string())
            .collect::<std::collections::HashSet<_>>()
    );
}

#[test]
fn test_cache_breakpoint_no_messages() {
    let mut messages: Vec<ApiMessage> = vec![];
    add_message_cache_breakpoint(&mut messages);
    // Should not panic, just return early
    assert!(messages.is_empty());
}

#[test]
fn test_cache_breakpoint_too_few_messages() {
    let mut messages = vec![
        ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "World".to_string(),
                cache_control: None,
            }],
        },
    ];
    add_message_cache_breakpoint(&mut messages);
    // With only 2 messages, should not add cache control
    for msg in &messages {
        for block in &msg.content {
            if let ApiContentBlock::Text { cache_control, .. } = block {
                assert!(cache_control.is_none());
            }
        }
    }
}

#[test]
fn test_cache_breakpoint_adds_to_assistant_message() {
    let mut messages = vec![
        ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "Identity".to_string(),
                cache_control: None,
            }],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
        },
        ApiMessage {
            role: "assistant".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "Hi there!".to_string(),
                cache_control: None,
            }],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "How are you?".to_string(),
                cache_control: None,
            }],
        },
    ];

    add_message_cache_breakpoint(&mut messages);

    // Assistant message (index 2) should have cache_control
    if let ApiContentBlock::Text { cache_control, .. } = &messages[2].content[0] {
        assert!(cache_control.is_some());
    } else {
        panic!("Expected Text block");
    }

    // Other messages should NOT have cache_control
    for (i, msg) in messages.iter().enumerate() {
        if i == 2 {
            continue; // Skip the assistant message we just checked
        }
        for block in &msg.content {
            if let ApiContentBlock::Text { cache_control, .. } = block {
                assert!(
                    cache_control.is_none(),
                    "Message {} should not have cache_control",
                    i
                );
            }
        }
    }
}

#[test]
fn test_cache_breakpoint_finds_text_in_mixed_content() {
    // Assistant message with tool_use followed by text
    let mut messages = vec![
        ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "Identity".to_string(),
                cache_control: None,
            }],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "Run a command".to_string(),
                cache_control: None,
            }],
        },
        ApiMessage {
            role: "assistant".to_string(),
            content: vec![
                ApiContentBlock::Text {
                    text: "Running command...".to_string(),
                    cache_control: None,
                },
                ApiContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                    cache_control: None,
                },
            ],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "Thanks".to_string(),
                cache_control: None,
            }],
        },
    ];

    add_message_cache_breakpoint(&mut messages);

    // The last block (ToolUse) in the assistant message should have cache_control
    // (we prefer the last block for maximum cache coverage)
    let assistant_msg = &messages[2];
    let has_cached_block = assistant_msg.content.iter().any(|block| {
        matches!(
            block,
            ApiContentBlock::ToolUse {
                cache_control: Some(_),
                ..
            }
        )
    });
    assert!(
        has_cached_block,
        "Should have added cache_control to last block (ToolUse) in assistant message"
    );
}

#[test]
fn test_system_param_split_oauth() {
    let static_content = "This is static content";
    let dynamic_content = "This is dynamic content";

    let result = build_system_param_split(static_content, dynamic_content, true);

    if let Some(ApiSystem::Blocks(blocks)) = result {
        // Should have 4 blocks: identity, notice, static (cached), dynamic (not cached)
        assert_eq!(blocks.len(), 4);

        // Block 0: identity (no cache)
        assert!(blocks[0].cache_control.is_none());

        // Block 1: notice (no cache)
        assert!(blocks[1].cache_control.is_none());

        // Block 2: static (cached)
        assert!(blocks[2].cache_control.is_some());
        assert!(blocks[2].text.contains("static"));

        // Block 3: dynamic (not cached)
        assert!(blocks[3].cache_control.is_none());
        assert!(blocks[3].text.contains("dynamic"));
    } else {
        panic!("Expected Blocks variant");
    }
}

#[test]
fn test_system_param_split_non_oauth() {
    let static_content = "This is static content";
    let dynamic_content = "This is dynamic content";

    let result = build_system_param_split(static_content, dynamic_content, false);

    if let Some(ApiSystem::Blocks(blocks)) = result {
        // Should have 2 blocks: static (cached), dynamic (not cached)
        assert_eq!(blocks.len(), 2);

        // Block 0: static (cached)
        assert!(blocks[0].cache_control.is_some());

        // Block 1: dynamic (not cached)
        assert!(blocks[1].cache_control.is_none());
    } else {
        panic!("Expected Blocks variant");
    }
}

// --- Cross-turn cache correctness tests ---
// These tests verify the two-marker sliding-window strategy that allows each turn
// to READ from the previous turn's conversation cache.

fn count_message_cache_breakpoints(messages: &[ApiMessage]) -> usize {
    messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| {
            matches!(
                b,
                ApiContentBlock::Text {
                    cache_control: Some(_),
                    ..
                } | ApiContentBlock::ToolUse {
                    cache_control: Some(_),
                    ..
                }
            )
        })
        .count()
}

fn cached_message_indices(messages: &[ApiMessage]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.content.iter().any(|b| {
                matches!(
                    b,
                    ApiContentBlock::Text {
                        cache_control: Some(_),
                        ..
                    } | ApiContentBlock::ToolUse {
                        cache_control: Some(_),
                        ..
                    }
                )
            })
        })
        .map(|(i, _)| i)
        .collect()
}

/// Helper to build a minimal conversation with N exchanges (user→assistant pairs).
/// Returns messages suitable for add_message_cache_breakpoint (includes a trailing user msg).
fn build_conversation(exchanges: usize) -> Vec<ApiMessage> {
    let mut messages = vec![ApiMessage {
        role: "user".to_string(),
        content: vec![ApiContentBlock::Text {
            text: "identity".to_string(),
            cache_control: None,
        }],
    }];
    for i in 0..exchanges {
        messages.push(ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: format!("Question {}", i + 1),
                cache_control: None,
            }],
        });
        messages.push(ApiMessage {
            role: "assistant".to_string(),
            content: vec![ApiContentBlock::Text {
                text: format!("Answer {}", i + 1),
                cache_control: None,
            }],
        });
    }
    // Trailing user message (the current turn's input)
    messages.push(ApiMessage {
        role: "user".to_string(),
        content: vec![ApiContentBlock::Text {
            text: format!("Question {}", exchanges + 1),
            cache_control: None,
        }],
    });
    messages
}

#[test]
fn test_cache_one_exchange_single_marker() {
    // Turn 2: only one assistant reply exists → one marker (WRITE only)
    let mut messages = build_conversation(1);
    add_message_cache_breakpoint(&mut messages);

    let indices = cached_message_indices(&messages);
    assert_eq!(indices.len(), 1, "One assistant message → one cache marker");
    // The assistant message is at index 2 (identity=0, user=1, assistant=2, user=3)
    assert_eq!(indices[0], 2);
}

#[test]
fn test_cache_two_exchanges_two_markers() {
    // Turn 3: two assistant replies → two markers (READ prev + WRITE new)
    let mut messages = build_conversation(2);
    // identity=0, user=1, assistant=2, user=3, assistant=4, user=5
    add_message_cache_breakpoint(&mut messages);

    let indices = cached_message_indices(&messages);
    assert_eq!(
        indices.len(),
        2,
        "Two assistant messages → two cache markers"
    );
    assert!(
        indices.contains(&2),
        "Second-to-last assistant (READ marker) at index 2"
    );
    assert!(
        indices.contains(&4),
        "Last assistant (WRITE marker) at index 4"
    );
}

#[test]
fn test_cache_many_exchanges_still_two_markers() {
    // 10 exchanges → still only 2 markers (within the 4-breakpoint API limit)
    let mut messages = build_conversation(10);
    add_message_cache_breakpoint(&mut messages);

    let count = count_message_cache_breakpoints(&messages);
    assert_eq!(
        count, 2,
        "Should always place exactly 2 markers regardless of conversation length"
    );
}

#[test]
fn test_cache_cross_turn_read_marker_preserved() {
    // THE KEY REGRESSION TEST: simulates turn N → turn N+1 and verifies that the
    // assistant message from turn N still has cache_control in the turn N+1 request.
    // Without this, the turn N cache snapshot is written but never read.

    // Turn 2: one assistant reply
    let mut turn2 = build_conversation(1);
    // identity=0, user=1, assistant=2, user=3
    add_message_cache_breakpoint(&mut turn2);
    let turn2_cached = cached_message_indices(&turn2);
    assert_eq!(
        turn2_cached,
        vec![2],
        "Turn 2: cache marker at assistant index 2"
    );

    // The content of the assistant message from turn 2 (what gets written to cache)
    let cached_text = match &turn2[2].content[0] {
        ApiContentBlock::Text { text, .. } => text.clone(),
        _ => panic!("Expected text block"),
    };

    // Turn 3: same conversation + one more exchange (assistant[2] is now second-to-last)
    let mut turn3 = build_conversation(2);
    // identity=0, user=1, assistant=2(same as before), user=3, assistant=4(new), user=5
    add_message_cache_breakpoint(&mut turn3);
    let turn3_cached = cached_message_indices(&turn3);

    // CRITICAL: assistant at index 2 MUST still have cache_control in turn 3,
    // so Anthropic can serve a cache READ hit for the turn-2 snapshot.
    assert!(
        turn3_cached.contains(&2),
        "Turn 3 MUST keep cache_control on the turn-2 assistant message (index 2) \
             so Anthropic can serve a cache_read hit. Without this, turn-2's cache is \
             written but never read, wasting cache_creation tokens every turn."
    );
    assert!(
        turn3_cached.contains(&4),
        "Turn 3 must add cache_control on the new assistant message (index 4) to \
             write a fresh cache snapshot for turn 4 to read"
    );

    // Verify it's actually the same content (same assistant message, not a different one)
    match &turn3[2].content[0] {
        ApiContentBlock::Text {
            text,
            cache_control,
        } => {
            assert_eq!(text, &cached_text);
            assert!(cache_control.is_some(), "Must have cache_control set");
        }
        _ => panic!("Expected text block"),
    }
}

#[test]
fn test_cache_non_oauth_path_gets_breakpoints() {
    // Non-OAuth path should now also get conversation cache breakpoints
    // (previously it returned early without calling add_message_cache_breakpoint)
    let messages = vec![
        ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
        },
        ApiMessage {
            role: "assistant".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "Hi there!".to_string(),
                cache_control: None,
            }],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: "Follow-up".to_string(),
                cache_control: None,
            }],
        },
    ];

    let result = format_messages_with_identity(messages, false);
    let indices = cached_message_indices(&result);
    assert_eq!(
        indices,
        vec![1],
        "Non-OAuth path should add cache breakpoint to assistant message"
    );
}

#[test]
fn test_cache_total_breakpoints_within_api_limit() {
    // Anthropic allows at most 4 cache_control parameters per request total
    // (system blocks + tool definitions + message blocks).
    // System: 1 (static block) + Tools: 1 (last tool) + Messages: up to 2 = 4 max.
    // This test verifies messages never exceed 2 breakpoints.
    for exchanges in 1..=20 {
        let mut messages = build_conversation(exchanges);
        add_message_cache_breakpoint(&mut messages);
        let count = count_message_cache_breakpoints(&messages);
        assert!(
            count <= 2,
            "Conversation with {} exchanges produced {} message breakpoints, exceeding \
                 the 2-message budget (system+tools use the other 2 of Anthropic's 4-limit)",
            exchanges,
            count
        );
    }
}

#[tokio::test]
async fn test_sanitize_tool_ids_with_dots() {
    let provider = AnthropicProvider::new();

    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "chatcmpl-BF2xX.tool_call.0".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "chatcmpl-BF2xX.tool_call.0".to_string(),
                content: "file1.txt".to_string(),
                is_error: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let formatted = provider.format_messages(&messages, false);

    let sanitized_id = "chatcmpl-BF2xX_tool_call_0";
    for msg in &formatted {
        for block in &msg.content {
            match block {
                ApiContentBlock::ToolUse { id, .. } => {
                    assert_eq!(id, sanitized_id);
                }
                ApiContentBlock::ToolResult { tool_use_id, .. } => {
                    assert_eq!(tool_use_id, sanitized_id);
                }
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn test_sanitize_dangling_tool_ids_with_dots() {
    let provider = AnthropicProvider::new();

    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call.with.dots".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "crash"}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let formatted = provider.format_messages(&messages, false);

    let sanitized_id = "call_with_dots";
    for msg in &formatted {
        for block in &msg.content {
            match block {
                ApiContentBlock::ToolUse { id, .. } => {
                    assert_eq!(id, sanitized_id);
                }
                ApiContentBlock::ToolResult { tool_use_id, .. } => {
                    assert_eq!(tool_use_id, sanitized_id);
                }
                _ => {}
            }
        }
    }
}

/// The runtime-provider identity that `set_credential_mode` writes must decode
/// back to the exact same credential mode. This guards the model picker / header
/// widget from reporting OAuth when an API key is in use (or vice versa): the
/// env key is the single source of truth those surfaces read, so an asymmetric
/// mapping here would surface an inaccurate auth method to the user.
#[test]
fn credential_mode_runtime_provider_identity_round_trips() {
    let _guard = crate::storage::lock_test_env();
    let previous = std::env::var_os("JCODE_RUNTIME_PROVIDER");

    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "claude");
    assert_eq!(
        AnthropicCredentialMode::from_runtime_env(),
        AnthropicCredentialMode::OAuth,
        "OAuth selection must surface as the OAuth runtime identity"
    );

    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "claude-api");
    assert_eq!(
        AnthropicCredentialMode::from_runtime_env(),
        AnthropicCredentialMode::ApiKey,
        "API-key selection must surface as the API-key runtime identity"
    );

    match previous {
        Some(value) => crate::env::set_var("JCODE_RUNTIME_PROVIDER", value),
        None => crate::env::remove_var("JCODE_RUNTIME_PROVIDER"),
    }
}
