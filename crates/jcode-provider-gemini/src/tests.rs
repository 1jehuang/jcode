use super::*;
use serde_json::json;

#[test]
fn response_usage_counts_tool_use_prompts_without_inventing_input() {
    for (metadata, expected) in [
        (json!({}), None),
        (json!({"totalTokenCount": 99}), None),
        (
            json!({"promptTokenCount": null, "toolUsePromptTokenCount": null}),
            None,
        ),
        (json!({"promptTokenCount": 0}), Some(0)),
        (json!({"toolUsePromptTokenCount": 0}), Some(0)),
        (json!({"promptTokenCount": 10}), Some(10)),
        (json!({"toolUsePromptTokenCount": 90}), Some(90)),
        (
            json!({"promptTokenCount": 10, "toolUsePromptTokenCount": 90, "cachedContentTokenCount": 4}),
            Some(100),
        ),
        (
            json!({"promptTokenCount": u64::MAX, "toolUsePromptTokenCount": 1}),
            Some(u64::MAX),
        ),
    ] {
        let response: VertexGenerateContentResponse = serde_json::from_value(json!({
            "usageMetadata": metadata
        }))
        .unwrap();
        let usage = response.usage_metadata.unwrap();
        assert_eq!(usage.input_token_count(), expected, "{metadata}");
        assert_eq!(
            usage.cached_content_token_count,
            metadata["cachedContentTokenCount"].as_u64()
        );
    }
}

#[test]
fn response_usage_counts_thinking_without_inventing_output() {
    for (metadata, expected) in [
        (json!({"thoughtsTokenCount": 13}), Some(13)),
        (
            json!({"candidatesTokenCount": 7, "thoughtsTokenCount": 13}),
            Some(20),
        ),
        (json!({"candidatesTokenCount": 7}), Some(7)),
        (json!({}), None),
        (
            json!({"candidatesTokenCount": null, "thoughtsTokenCount": null}),
            None,
        ),
        (json!({"thoughtsTokenCount": 0}), Some(0)),
        (json!({"candidatesTokenCount": 0}), Some(0)),
        (
            json!({"candidatesTokenCount": 0, "thoughtsTokenCount": 0}),
            Some(0),
        ),
        (
            json!({"candidatesTokenCount": u64::MAX, "thoughtsTokenCount": 1}),
            Some(u64::MAX),
        ),
        (
            json!({"promptTokenCount": 10, "totalTokenCount": 100, "toolUsePromptTokenCount": 90}),
            None,
        ),
        (
            json!({"candidatesTokenCount": 7, "thoughtsTokenCount": 13, "totalTokenCount": 120, "toolUsePromptTokenCount": 90}),
            Some(20),
        ),
    ] {
        let response: VertexGenerateContentResponse = serde_json::from_value(json!({
            "usageMetadata": metadata
        }))
        .unwrap();
        assert_eq!(
            response.usage_metadata.unwrap().output_token_count(),
            expected,
            "{metadata}"
        );
    }
    let response: VertexGenerateContentResponse = serde_json::from_value(json!({})).unwrap();
    assert!(response.usage_metadata.is_none());
}

#[test]
fn compatible_schema_strips_nested_property_names() {
    let schema = json!({
        "type": "object",
        "properties": {
            "data": {
                "type": "object",
                "propertyNames": { "type": "string" },
                "additionalProperties": { "type": "string" }
            }
        }
    });
    let out = gemini_compatible_schema(&schema);
    assert!(out["properties"]["data"].get("propertyNames").is_none());
    // Existing Gemini normalization still drops additionalProperties.
    assert!(
        out["properties"]["data"]
            .get("additionalProperties")
            .is_none()
    );
}

#[test]
fn fallback_models_skip_current_model() {
    assert_eq!(
        gemini_fallback_models("gemini-2.5-flash"),
        vec![
            "gemini-pro-latest",
            "gemini-3.1-pro-preview",
            "gemini-3-pro-preview",
            "gemini-2.5-pro",
            "gemini-3-flash-preview",
            "gemini-2.0-flash",
        ]
    );
}

#[test]
fn extract_gemini_model_ids_discovers_nested_models() {
    let response = json!({
        "routing": {
            "manual": {
                "models": [
                    {"id": "gemini-3-pro-preview"},
                    {"name": "gemini-3.1-pro-preview"}
                ]
            },
            "auto": ["gemini-3-flash-preview", "not-a-model"]
        }
    });

    assert_eq!(
        extract_gemini_model_ids(&response),
        vec![
            "gemini-3.1-pro-preview".to_string(),
            "gemini-3-pro-preview".to_string(),
            "gemini-3-flash-preview".to_string(),
        ]
    );
}

#[test]
fn candidate_content_decodes_without_role() {
    // Antigravity/Cloud Code Gemini-3 responses occasionally omit `role` on
    // a candidate's `content` (and sometimes `parts` entirely). The whole
    // generateContent decode used to fail with "missing field `role`",
    // which aborted the turn; assert the response now decodes and the
    // function call survives.
    let raw = json!({
        "response": {
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {"name": "read", "args": {"file_path": "/tmp/x"}},
                        "thoughtSignature": "SIG_XYZ"
                    }]
                },
                "finishReason": "STOP"
            }]
        }
    })
    .to_string();

    let decoded: CodeAssistGenerateResponse =
        serde_json::from_str(&raw).expect("decode response with role-less content");
    let candidates = decoded.response.unwrap().candidates.unwrap();
    let part = &candidates[0].content.as_ref().unwrap().parts[0];
    assert_eq!(part.function_call.as_ref().unwrap().name, "read");
    assert_eq!(part.thought_signature.as_deref(), Some("SIG_XYZ"));
}

#[test]
fn candidate_content_decodes_without_parts() {
    // A bare `content: {}` (no `role`, no `parts`) must not abort the decode.
    let raw = json!({
        "response": {
            "candidates": [{ "content": {}, "finishReason": "STOP" }]
        }
    })
    .to_string();

    let decoded: CodeAssistGenerateResponse =
        serde_json::from_str(&raw).expect("decode response with empty content");
    let content = decoded.response.unwrap().candidates.unwrap()[0]
        .content
        .clone()
        .unwrap();
    assert!(content.role.is_empty());
    assert!(content.parts.is_empty());
}

#[test]
fn openai_compaction_marker_keeps_user_turn_between_model_turns() {
    let messages = vec![
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "Compacted summary.".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::OpenAICompaction {
                encrypted_content: "enc_blob".to_string(),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "ls" }),
                thought_signature: Some("SIG_1".to_string()),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "ok".to_string(),
                is_error: Some(false),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let contents = build_contents(&messages);
    assert_eq!(contents.len(), 4);
    assert_eq!(contents[0].role, "model");
    assert_eq!(contents[1].role, "user");
    assert_eq!(
        contents[1].parts[0].text.as_deref(),
        Some("[Prior conversation context compacted]")
    );
    assert_eq!(contents[2].role, "model");
    assert!(contents[2].parts[0].function_call.is_some());
    assert_eq!(contents[3].role, "user");
    assert!(contents[3].parts[0].function_response.is_some());
}

#[test]
fn unpaired_in_flight_call_and_orphaned_result_downgrade_without_losing_signatures() {
    let messages = vec![
        // Orphaned ToolResult left at the head of history after compaction.
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "evicted_call".to_string(),
                content: "old output".to_string(),
                is_error: Some(false),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        // Multi-call turn where `call_cancelled` was interrupted (yet carried
        // the turn's real signature) while `call_done` completed.
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: "call_cancelled".to_string(),
                    name: "bash".to_string(),
                    input: json!({ "command": "sleep 60" }),
                    thought_signature: Some("SIG_REAL".to_string()),
                },
                ContentBlock::ToolUse {
                    id: "call_done".to_string(),
                    name: "read".to_string(),
                    input: json!({ "path": "Cargo.toml" }),
                    thought_signature: None,
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_done".to_string(),
                content: "[package]".to_string(),
                is_error: Some(false),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let contents = build_contents(&messages);
    let call_count = contents
        .iter()
        .flat_map(|c| &c.parts)
        .filter(|p| p.function_call.is_some())
        .count();
    let resp_count = contents
        .iter()
        .flat_map(|c| &c.parts)
        .filter(|p| p.function_response.is_some())
        .count();
    assert_eq!(call_count, 1);
    assert_eq!(resp_count, 1);
    assert!(
        contents[0].parts[0]
            .text
            .as_deref()
            .unwrap_or_default()
            .contains("old output")
    );
    assert!(
        contents[1].parts[0]
            .text
            .as_deref()
            .unwrap_or_default()
            .contains("sleep 60")
    );
    assert_eq!(
        contents[1].parts[1].function_call.as_ref().unwrap().name,
        "read"
    );
    assert_eq!(
        contents[1].parts[1].thought_signature.as_deref(),
        Some("SIG_REAL"),
        "signature from a downgraded sibling must still carry forward"
    );
}

#[test]
fn parallel_tool_results_across_separate_user_messages_coalesce_and_match_call_order() {
    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Inspect the workspace".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: "call_read".to_string(),
                    name: "read".to_string(),
                    input: json!({ "path": "Cargo.toml" }),
                    thought_signature: Some("SIG_PARALLEL".to_string()),
                },
                ContentBlock::ToolUse {
                    id: "call_grep".to_string(),
                    name: "grep".to_string(),
                    input: json!({ "pattern": "name" }),
                    thought_signature: None,
                },
                ContentBlock::ToolUse {
                    id: "call_bash".to_string(),
                    name: "bash".to_string(),
                    input: json!({ "command": "pwd" }),
                    thought_signature: None,
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
        // Separate User messages appended in completion order, with trailing
        // image/text attachments mixed into individual tool result messages.
        Message {
            role: Role::User,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_bash".to_string(),
                    content: "/workspace".to_string(),
                    is_error: Some(false),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "AAAA".to_string(),
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![
                ContentBlock::Text {
                    text: "[Attached note]".to_string(),
                    cache_control: None,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_read".to_string(),
                    content: "[package]".to_string(),
                    is_error: Some(false),
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_grep".to_string(),
                content: "Cargo.toml:2".to_string(),
                is_error: Some(false),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let contents = build_contents(&messages);
    assert_eq!(
        contents.len(),
        3,
        "consecutive User tool-result messages must coalesce into one user turn: {contents:?}"
    );
    assert_eq!(contents[0].role, "user");
    assert_eq!(contents[1].role, "model");
    assert_eq!(contents[2].role, "user");

    let call_ids: Vec<&str> = contents[1]
        .parts
        .iter()
        .map(|p| p.function_call.as_ref().unwrap().id.as_deref().unwrap())
        .collect();
    assert_eq!(call_ids, vec!["call_read", "call_grep", "call_bash"]);

    // All three functionResponse parts must appear first in the coalesced user
    // turn and match the preceding model turn's functionCall order, before any
    // inlineData or text parts.
    assert_eq!(contents[2].parts.len(), 5);
    let resp_ids: Vec<&str> = contents[2].parts[..3]
        .iter()
        .map(|p| {
            p.function_response
                .as_ref()
                .expect("functionResponse must precede text/inlineData")
                .id
                .as_deref()
                .unwrap()
        })
        .collect();
    assert_eq!(resp_ids, vec!["call_read", "call_grep", "call_bash"]);
    assert!(contents[2].parts[3].inline_data.is_some());
    assert_eq!(
        contents[2].parts[4].text.as_deref(),
        Some("[Attached note]")
    );
}

#[test]
fn intervening_user_message_without_tool_result_stops_pairing() {
    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Run build".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_interrupted".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "cargo build" }),
                thought_signature: Some("SIG_INT".to_string()),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Stop and check status first".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_interrupted".to_string(),
                content: "cancelled".to_string(),
                is_error: Some(true),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let contents = build_contents(&messages);
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[0].role, "user");
    assert_eq!(contents[1].role, "model");
    assert_eq!(contents[2].role, "user");
    assert!(
        contents[1].parts.iter().all(|p| p.function_call.is_none()),
        "interrupted ToolUse must not pair across a plain user message: {contents:?}"
    );
    assert!(
        contents[2]
            .parts
            .iter()
            .all(|p| p.function_response.is_none()),
        "late ToolResult after user interruption must downgrade to text: {contents:?}"
    );
    assert_eq!(
        contents[2].parts[0].text.as_deref(),
        Some("Stop and check status first")
    );
    assert!(
        contents[2].parts[1]
            .text
            .as_deref()
            .unwrap_or_default()
            .contains("cancelled")
    );
}

#[test]
fn unanswered_tool_calls_before_assistant_or_at_tail_downgrade_and_coalesce_model_turns() {
    // 1. Unanswered ToolUse before another Assistant turn + empty-filtered User
    //    message before a valid functionCall turn: must coalesce consecutive
    //    model turns so the functionCall turn immediately follows `user`.
    let mid_history = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Start task".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_aborted".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "sleep 10" }),
                thought_signature: Some("SIG_CARRIED".to_string()),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "Retrying with read.".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ProviderNative {
                provider: "openai".to_string(),
                item: json!({}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_valid".to_string(),
                name: "read".to_string(),
                input: json!({ "path": "README.md" }),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_valid".to_string(),
                content: "# Readme".to_string(),
                is_error: Some(false),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let contents = build_contents(&mid_history);
    assert_eq!(
        contents.len(),
        3,
        "consecutive model turns from downgrading/empty-filtering must coalesce: {contents:?}"
    );
    assert_eq!(contents[0].role, "user");
    assert_eq!(contents[1].role, "model");
    assert_eq!(contents[2].role, "user");
    assert!(
        contents[1].parts[0]
            .text
            .as_deref()
            .unwrap_or_default()
            .contains("sleep 10")
    );
    assert_eq!(
        contents[1].parts[1].text.as_deref(),
        Some("Retrying with read.")
    );
    assert_eq!(
        contents[1].parts[2].function_call.as_ref().unwrap().name,
        "read"
    );
    assert_eq!(
        contents[1].parts[2].thought_signature.as_deref(),
        Some("SIG_CARRIED")
    );
    assert_eq!(
        contents[2].parts[0]
            .function_response
            .as_ref()
            .unwrap()
            .id
            .as_deref(),
        Some("call_valid")
    );

    // 2. Unanswered ToolUse at the very end of `messages` must also downgrade.
    let tail_unanswered = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Check files".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_dangling_tail".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "ls" }),
                thought_signature: Some("SIG_TAIL".to_string()),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let tail_contents = build_contents(&tail_unanswered);
    assert_eq!(tail_contents.len(), 2);
    assert_eq!(tail_contents[0].role, "user");
    assert_eq!(tail_contents[1].role, "model");
    assert!(
        tail_contents[1]
            .parts
            .iter()
            .all(|p| p.function_call.is_none()),
        "conversation must not end with an unanswered functionCall: {tail_contents:?}"
    );
    assert!(
        tail_contents[1].parts[0]
            .text
            .as_deref()
            .unwrap_or_default()
            .contains("bash")
    );
}

#[test]
fn recognizes_function_call_turn_order_and_count_rejections_for_downgrade_retry() {
    assert!(is_missing_thought_signature_error(
        "Antigravity generateContent failed (HTTP 400 Bad Request): Please ensure that function call turn comes immediately after a user turn or after a function response turn."
    ));
    assert!(is_missing_thought_signature_error(
        "Gemini generateContent failed (HTTP 400 Bad Request): Please ensure that the number of function response parts is equal to the number of function call parts."
    ));
}
