use super::*;

#[test]
fn reusable_cache_boundary_excludes_only_split_uncached_tail() {
    for provider in ["Claude", "anthropic", "unknown"] {
        assert_eq!(
            reusable_cache_tokens_from_usage(provider, 2_482, Some(615_629), Some(1_375)),
            617_004
        );
    }
    for provider in ["openai", "openai-api", "codex"] {
        assert_eq!(
            reusable_cache_tokens_from_usage(provider, 619_486, Some(615_629), Some(1_375)),
            619_486
        );
    }
    assert_eq!(
        reusable_cache_tokens_from_usage("Claude", 2_482, Some(0), Some(0)),
        0
    );
    assert_eq!(
        reusable_cache_tokens_from_usage("Claude", 0, Some(615_629), Some(1_375)),
        617_004
    );
}

#[test]
fn compaction_text_keeps_native_search_query_and_urls() {
    let messages = vec![Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::ProviderNative {
                provider: "anthropic".to_string(),
                item: serde_json::json!({"type": "server_tool_use", "id": "srvtoolu_1",
                    "name": "web_search", "input": {"query": "rust news"}}),
            },
            ContentBlock::ProviderNative {
                provider: "anthropic".to_string(),
                item: serde_json::json!({"type": "web_search_tool_result",
                    "tool_use_id": "srvtoolu_1", "content": [{"type": "web_search_result",
                    "url": "https://blog.rust-lang.org/", "title": "Rust Blog",
                    "encrypted_content": "ENC"}]}),
            },
        ],
        timestamp: None,
        tool_duration_ms: None,
    }];
    let text = build_compaction_conversation_text(&messages, None);
    assert!(text.contains("rust news"), "{text}");
    assert!(text.contains("https://blog.rust-lang.org/"), "{text}");
    assert!(!text.contains("ENC"), "{text}");
}

#[test]
fn effective_context_split_accounting_adds_cache_counters() {
    // Anthropic-style: input is the uncached remainder.
    assert_eq!(
        effective_context_tokens_from_usage("anthropic", 10_000, Some(300_000), Some(5_000)),
        315_000
    );
    assert_eq!(
        effective_context_tokens_from_usage("Claude", 10_000, Some(300_000), None),
        310_000
    );
}

#[test]
fn effective_context_subset_accounting_does_not_double_count() {
    // OpenAI-style: prompt_tokens already includes cached tokens.
    assert_eq!(
        effective_context_tokens_from_usage("openai", 400_000, Some(390_000), None),
        400_000
    );
    assert_eq!(
        effective_context_tokens_from_usage("openai-api", 10_000, Some(6_000), Some(2_000)),
        10_000
    );
    // No cache info at all: pass through.
    assert_eq!(
        effective_context_tokens_from_usage("opencode-go", 396_000, None, None),
        396_000
    );
}

#[test]
fn effective_context_infers_split_accounting_from_counter_shape() {
    // cache_read > input implies input can't already contain it.
    assert_eq!(
        effective_context_tokens_from_usage("unknown", 10_000, Some(500_000), None),
        510_000
    );
    // Any cache_creation implies split accounting.
    assert_eq!(
        effective_context_tokens_from_usage("unknown", 10_000, Some(2_000), Some(1_000)),
        13_000
    );
}

#[test]
fn effective_context_zero_uncached_input_preserves_cached_prompt() {
    assert_eq!(
        effective_context_tokens_from_usage("anthropic", 0, Some(300_000), Some(5_000)),
        305_000
    );
}

#[test]
fn builds_compaction_prompt_with_summary_and_truncated_tool_result() {
    let summary = Summary {
        text: "prior work".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 1,
        original_turn_count: 1,
    };
    let message = Message::user("hello");
    let prompt = build_compaction_prompt(&[message], Some(&summary), 10_000);
    assert!(prompt.contains("## Previous Summary"));
    assert!(prompt.contains("prior work"));
    assert!(prompt.contains("**User:**"));
    assert!(prompt.contains(SUMMARY_PROMPT));
}

#[test]
fn truncates_on_utf8_boundary() {
    assert_eq!(truncate_str_boundary("éabc", 1), "");
    assert_eq!(truncate_str_boundary("éabc", 2), "é");
}

#[test]
fn mean_embedding_is_normalized() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    let mean = mean_embedding(&[&a, &b], 2);
    let norm = (mean[0] * mean[0] + mean[1] * mean[1]).sqrt();
    assert!((norm - 1.0).abs() < 0.0001);
}

#[test]
fn safe_cutoff_keeps_tool_use_with_tool_result() {
    let tool_use = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "call_1".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"file":"src/lib.rs"}),
            thought_signature: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    };
    let tool_result = Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call_1".to_string(),
            content: "ok".to_string(),
            is_error: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    };
    let messages = vec![
        Message::user("old"),
        tool_use,
        tool_result,
        Message::user("new"),
    ];

    assert_eq!(safe_compaction_cutoff(&messages, 2), 1);
}

#[test]
fn estimates_tokens_with_large_budget_overhead() {
    let summary = Summary {
        text: "abcd".repeat(100),
        openai_encrypted_content: None,
        covers_up_to_turn: 1,
        original_turn_count: 1,
    };

    assert_eq!(estimate_compaction_tokens(Some(&summary), 0, 1000), 100);
    assert_eq!(
        estimate_compaction_tokens(Some(&summary), 0, DEFAULT_TOKEN_BUDGET),
        100 + SYSTEM_OVERHEAD_TOKENS
    );
}

#[test]
fn image_token_cost_is_bounded_not_base64_length() {
    // Regression: a large base64 image payload must not be counted as ~len/4
    // tokens. Doing so inflated the estimate ~100x and caused repeated
    // back-to-back compactions.
    let huge_base64 = "A".repeat(1_400_000);
    let mut image_msg = Message::user("");
    image_msg.content = vec![ContentBlock::Image {
        media_type: "image/png".to_string(),
        data: huge_base64.clone(),
    }];

    let chars = message_char_count(&image_msg);
    // Flat per-image cost in char-equivalents, far below the raw payload.
    assert_eq!(chars, IMAGE_TOKEN_COST * CHARS_PER_TOKEN);
    assert!(
        chars < huge_base64.len() / 10,
        "image should not be charged anywhere near its base64 length"
    );

    // The token estimate for four such images stays small.
    let tokens = estimate_compaction_tokens_from_chars(chars * 4, DEFAULT_TOKEN_BUDGET);
    assert!(
        tokens < SYSTEM_OVERHEAD_TOKENS + 4 * IMAGE_TOKEN_COST + 10,
        "four images should cost ~{} tokens, got {}",
        SYSTEM_OVERHEAD_TOKENS + 4 * IMAGE_TOKEN_COST,
        tokens
    );
}

#[test]
fn openai_native_compaction_cost_is_bounded_not_ciphertext_length() {
    // Regression: a 1.1M-char encrypted payload was estimated at ~285k
    // tokens, so compaction reported 294k -> 303k while the provider saw
    // 219k -> 34k.
    let summary = Summary {
        text: String::new(),
        openai_encrypted_content: Some("x".repeat(1_141_900)),
        covers_up_to_turn: 229,
        original_turn_count: 229,
    };
    assert_eq!(
        estimate_compaction_tokens(Some(&summary), 0, DEFAULT_TOKEN_BUDGET),
        OPENAI_COMPACTION_TOKEN_COST + SYSTEM_OVERHEAD_TOKENS
    );

    let mut msg = Message::user("");
    msg.content = vec![ContentBlock::OpenAICompaction {
        encrypted_content: "x".repeat(1_141_900),
    }];
    assert_eq!(
        message_char_count(&msg),
        OPENAI_COMPACTION_TOKEN_COST * CHARS_PER_TOKEN
    );
}

#[test]
fn builds_semantic_text_from_relevant_content() {
    let message = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Text {
                text: "hello world".to_string(),
                cache_control: None,
            },
            ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "tool output".to_string(),
                is_error: None,
            },
        ],
        timestamp: None,
        tool_duration_ms: None,
    };

    assert_eq!(semantic_message_text(&message), "hello world");
    assert_eq!(semantic_goal_text(&[message]), "hello world tool output");
    assert_eq!(semantic_cache_key("stable"), semantic_cache_key("stable"));
}

#[test]
fn builds_emergency_summary_with_tools_and_files() {
    let messages = vec![
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file":"src/lib.rs"}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message::user("Edited src/compaction.rs and Cargo.toml, ignored https://example.com"),
    ];

    let summary = build_emergency_summary_text(Some("previous"), 2, 201_000, 200_000, &messages);
    assert!(summary.contains("previous"));
    assert!(summary.contains("2 messages were dropped"));
    assert!(summary.contains("Tools used: read"));
    assert!(summary.contains("Files referenced: Cargo.toml, src/compaction.rs"));
    assert!(!summary.contains("https://example.com"));
}

#[test]
fn emergency_truncation_is_utf8_safe() {
    let original = format!("{}middle{}", "é".repeat(20), "尾".repeat(20));
    let truncated = emergency_truncated_tool_result(&original, 25);
    assert!(truncated.contains("chars truncated for context recovery"));
    assert!(truncated.is_char_boundary(truncated.len()));
}

#[test]
fn emergency_truncation_replaces_large_images_with_text_marker() {
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "a".repeat(2048),
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let truncated = emergency_truncate_large_payloads(&mut messages, 4000, 1024);
    assert_eq!(truncated, 1);
    match &messages[0].content[0] {
        ContentBlock::Text { text, .. } => {
            assert!(text.contains("Image omitted during emergency context recovery"));
            assert!(text.contains("original_base64_chars=2048"));
        }
        other => panic!("expected image to be replaced with text marker, got {other:?}"),
    }
}

#[test]
fn detects_request_payload_too_large_errors() {
    // Image rejections are a different class.
    assert!(is_image_rejection_error(
        "Anthropic API error (400 Bad Request): {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"messages.720.content.0.tool_result.content.1.image.source.base64.media_type: Input should be 'image/jpeg', 'image/png', 'image/gif' or 'image/webp'\"}}"
    ));
    assert!(is_image_rejection_error(
        "400 invalid_request_error: messages.3.content.1.image.source.base64: Could not process image"
    ));
    assert!(!is_image_rejection_error(
        "Anthropic API error (413 Payload Too Large): image too big request_too_large"
    ));
    assert!(!is_image_rejection_error(
        "400 invalid_request_error: messages.5: `tool_use` ids were found without `tool_result` blocks"
    ));
    assert!(!is_image_rejection_error("rate limit exceeded"));

    assert!(is_request_payload_too_large_error(
        "Anthropic API error (413 Payload Too Large): {\"error\":{\"type\":\"request_too_large\",\"message\":\"Request exceeds the maximum size\"}}"
    ));
    assert!(is_request_payload_too_large_error(
        "413 Request Entity Too Large"
    ));
    assert!(is_request_payload_too_large_error("request too large"));
    // Not a payload error — should not match.
    assert!(!is_request_payload_too_large_error(
        "rate limit exceeded, retry after 20s"
    ));
    // Embedded digits must not trip the standalone 413 check.
    assert!(!is_request_payload_too_large_error(
        "model version 4130 is unavailable"
    ));
}

fn image_msg(data_len: usize) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "a".repeat(data_len),
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

#[test]
fn strip_large_images_drops_oldest_until_under_budget() {
    // Four 1000-char images = 4000 total; budget 2500 should drop the two
    // oldest (leaving 2000 <= 2500), keeping the two most recent.
    let mut messages = vec![
        image_msg(1000),
        image_msg(1000),
        image_msg(1000),
        image_msg(1000),
    ];

    let stripped = emergency_strip_large_images(&mut messages, 2500);
    assert_eq!(stripped, 2);
    // Oldest two replaced with text markers.
    assert!(matches!(messages[0].content[0], ContentBlock::Text { .. }));
    assert!(matches!(messages[1].content[0], ContentBlock::Text { .. }));
    // Most recent two preserved as images.
    assert!(matches!(messages[2].content[0], ContentBlock::Image { .. }));
    assert!(matches!(messages[3].content[0], ContentBlock::Image { .. }));
    if let ContentBlock::Text { text, .. } = &messages[0].content[0] {
        assert!(text.contains("Image omitted during request-size recovery"));
        assert!(text.contains("original_base64_chars=1000"));
    }
}

fn tool_image_msg(data_len: usize) -> Message {
    Message {
        role: Role::User,
        content: vec![
            ContentBlock::ToolResult {
                tool_use_id: "call".to_string(),
                content: "Captured main display".to_string(),
                is_error: None,
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "a".repeat(data_len),
            },
        ],
        timestamp: None,
        tool_duration_ms: None,
    }
}

fn decay(messages: &mut [Message]) -> usize {
    let mut contents: Vec<&mut Vec<ContentBlock>> =
        messages.iter_mut().map(|m| &mut m.content).collect();
    decay_old_tool_images_in_contents(&mut contents)
}

fn is_image(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::Image { .. }))
}

#[test]
fn tool_image_decay_waits_for_trigger_then_keeps_newest() {
    let mut messages: Vec<Message> = (0..TOOL_IMAGE_DECAY_TRIGGER_COUNT)
        .map(|_| tool_image_msg(10))
        .collect();
    assert_eq!(
        decay(&mut messages),
        0,
        "at the trigger count nothing decays"
    );

    messages.push(tool_image_msg(10));
    let total = messages.len();
    assert_eq!(decay(&mut messages), total - TOOL_IMAGE_DECAY_KEEP_COUNT);
    for (index, message) in messages.iter().enumerate() {
        assert_eq!(
            is_image(message),
            index >= total - TOOL_IMAGE_DECAY_KEEP_COUNT
        );
        // The tool_result itself is preserved for tool_use pairing.
        assert!(matches!(
            message.content[0],
            ContentBlock::ToolResult { .. }
        ));
    }
    let ContentBlock::Text { text, .. } = &messages[0].content[1] else {
        panic!("decayed image must become a text marker");
    };
    assert!(text.contains("Older tool screenshot omitted"));
    assert!(text.contains("original_base64_chars=10"));
    // Stable after decay: nothing more to do until the trigger is hit again.
    assert_eq!(decay(&mut messages), 0);
}

#[test]
fn tool_image_decay_bounds_bytes_but_keeps_newest_huge_image() {
    let huge = TOOL_IMAGE_DECAY_KEEP_CHARS + 1;
    let mut messages = vec![
        tool_image_msg(huge),
        tool_image_msg(huge),
        tool_image_msg(huge),
    ];
    assert_eq!(decay(&mut messages), 2);
    assert!(!is_image(&messages[0]));
    assert!(!is_image(&messages[1]));
    assert!(
        is_image(&messages[2]),
        "newest screenshot must still be sent"
    );
}

#[test]
fn tool_image_decay_ignores_user_pasted_images() {
    let mut messages = vec![image_msg(10)];
    messages.extend((0..TOOL_IMAGE_DECAY_TRIGGER_COUNT + 5).map(|_| tool_image_msg(10)));
    decay(&mut messages);
    assert!(
        is_image(&messages[0]),
        "user images are not tool screenshots"
    );
}

#[test]
fn strip_large_images_noop_when_under_budget() {
    let mut messages = vec![image_msg(500), image_msg(500)];
    let stripped = emergency_strip_large_images(&mut messages, 4000);
    assert_eq!(stripped, 0);
    assert!(matches!(messages[0].content[0], ContentBlock::Image { .. }));
    assert!(matches!(messages[1].content[0], ContentBlock::Image { .. }));
}

#[test]
fn strip_large_images_strips_all_when_single_image_exceeds_budget() {
    // Even a lone over-budget image is stripped (better than re-sending an
    // oversized request that the provider will reject again).
    let mut messages = vec![image_msg(8000)];
    let stripped = emergency_strip_large_images(&mut messages, 2000);
    assert_eq!(stripped, 1);
    assert!(matches!(messages[0].content[0], ContentBlock::Text { .. }));
}
