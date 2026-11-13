use super::*;
use jcode_message_types::{ContentBlock, Message, Role};
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
fn test_in_flight_tool_call_is_omitted() {
    let messages = vec![Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "tool_call_123".to_string(),
            name: "read".to_string(),
            input: json!({"file_path": "foo.txt"}),
            thought_signature: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let contents = build_contents(&messages);

    // The `build_contents` function should produce no content, because the only
    // block is a tool use with no corresponding tool result.
    assert!(
        contents.is_empty(),
        "Dangling tool call should be omitted, leaving an empty list"
    );
}

#[test]
fn test_history_ending_with_model_turn_gets_user_turn_appended() {
    // This tests BUG 2. A history ending in a `model` role (e.g. a simple text response)
    // must have a `user` turn appended.
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
            content: vec![ContentBlock::Text {
                text: "Hi there!".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let contents = build_contents(&messages);
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[0].role, "user");
    assert_eq!(contents[1].role, "model");
    assert_eq!(contents[2].role, "user", "A user turn should be appended");
    assert_eq!(contents[2].parts[0].text, Some("".to_string()));
}

#[test]
fn test_history_with_tool_result_image_is_handled_correctly() {
    // This tests BUG 2 specifically with an image tool result, which was the original report.
    // The result is that the last turn should be a user turn.
    let messages = vec![
        Message {
            role: Role::User,
            content: vec![],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "t".to_string(),
                name: "n".to_string(),
                input: json!({}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "garbage".to_string(),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let contents = build_contents(&messages);
    // The final list will have the user, model, user (with tool result).
    // The last turn is already 'user', so no extra turn should be appended.
    assert_eq!(
        contents.last().unwrap().role,
        "user",
        "The last turn should be a user turn."
    );
    // Count the number of empty user turns. There should not be one.
    let empty_user_turns = contents
        .iter()
        .filter(|c| {
            c.role == "user" && c.parts.len() == 1 && c.parts[0].text == Some("".to_string())
        })
        .count();
    assert_eq!(
        empty_user_turns, 0,
        "An extra empty user turn should not be added if the last turn is already user."
    );
}

/// A `read` result that carries an image must not leave the transcript ending in a model turn,
/// which Gemini rejects with "Requests ending with a model turn are not supported". The
/// functionResponse and its inlineData belong in the same user turn, and the final turn is
/// never "model".
#[test]
fn test_image_tool_result_keeps_user_turn_last() {
    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "look at this".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file_path": "/tmp/x.png"}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "image attached".to_string(),
                    is_error: None,
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgo=".to_string(),
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let contents = build_contents(&messages);
    let last = contents.last().expect("contents must not be empty");
    assert_eq!(
        last.role, "user",
        "the request must not end on a model turn"
    );
    let has_inline = last.parts.iter().any(|p| p.inline_data.is_some());
    assert!(
        has_inline,
        "the image inlineData must stay in the user turn"
    );
    let has_response = last.parts.iter().any(|p| p.function_response.is_some());
    assert!(
        has_response,
        "the functionResponse must stay with the image"
    );
}
