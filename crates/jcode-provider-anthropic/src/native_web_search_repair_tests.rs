use super::*;
use jcode_message_types::{ContentBlock, Message, Role};
use serde_json::json;

fn assistant_with_native(item: serde_json::Value) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ProviderNative {
            provider: jcode_message_types::provider_native::PROVIDER_NATIVE_ANTHROPIC.to_string(),
            item,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

#[test]
fn orphaned_server_tool_use_is_dropped_from_completed_history() {
    let messages = vec![
        assistant_with_native(json!({
            "type": "server_tool_use",
            "id": "srvtoolu_orphan",
            "name": "web_search",
            "input": {"query": "rust news"}
        })),
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "continue".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let formatted = format_messages_with_native(&messages, false, &[], true);
    assert_eq!(formatted.len(), 1);
    assert_eq!(formatted[0].role, "user");
    let only = serde_json::to_value(&formatted[0]).unwrap();
    assert_eq!(only["content"][0]["type"], "text");
    assert_eq!(only["content"][0]["text"], "continue");
}

#[test]
fn unmatched_server_tool_result_is_removed() {
    let messages = vec![
        assistant_with_native(json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_missing",
            "content": []
        })),
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "continue".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let formatted = format_messages_with_native(&messages, false, &[], true);
    assert_eq!(formatted.len(), 1);
    assert_eq!(formatted[0].role, "user");
}

#[test]
fn final_unmatched_server_tool_use_is_preserved_for_pause_turn_resume() {
    let messages = vec![assistant_with_native(json!({
        "type": "server_tool_use",
        "id": "srvtoolu_paused",
        "name": "web_search",
        "input": {"query": "rust news"}
    }))];

    let formatted = format_messages_with_native(&messages, false, &[], true);
    assert_eq!(formatted.len(), 1);

    let serialized = serde_json::to_value(&formatted[0]).unwrap();
    assert_eq!(serialized["role"], "assistant");
    assert_eq!(serialized["content"][0]["type"], "server_tool_use");
    assert_eq!(serialized["content"][0]["id"], "srvtoolu_paused");
    assert!(
        serialized["content"].get(1).is_none(),
        "pause_turn resumes must not get a synthetic result"
    );
}

#[test]
fn matched_server_tool_pair_is_replayed_unchanged() {
    let messages = vec![
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ProviderNative {
                    provider: jcode_message_types::provider_native::PROVIDER_NATIVE_ANTHROPIC
                        .to_string(),
                    item: json!({
                        "type": "server_tool_use",
                        "id": "srvtoolu_valid",
                        "name": "web_search",
                        "input": {"query": "rust news"}
                    }),
                },
                ContentBlock::ProviderNative {
                    provider: jcode_message_types::provider_native::PROVIDER_NATIVE_ANTHROPIC
                        .to_string(),
                    item: json!({
                        "type": "web_search_tool_result",
                        "tool_use_id": "srvtoolu_valid",
                        "content": []
                    }),
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "continue".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let formatted = format_messages_with_native(&messages, false, &[], true);
    let first = serde_json::to_value(&formatted[0]).unwrap();
    assert_eq!(first["content"][0]["id"], "srvtoolu_valid");
    assert_eq!(first["content"][1]["tool_use_id"], "srvtoolu_valid");
    assert_eq!(first["content"][1]["content"], json!([]));
}

#[test]
fn invalid_encrypted_content_error_downgrades_replayed_search_to_text() {
    let err = r#"Anthropic API error (400 Bad Request): {"type":"error","error":{"type":"invalid_request_error","message":"messages.1.content.0: Invalid `encrypted_content` in `search_result` block"}}"#;
    assert!(is_invalid_native_replay_error(err));
    assert!(!is_invalid_native_replay_error(
        "400 Bad Request: prompt is too long"
    ));

    let mut messages = vec![ApiMessage {
        role: "assistant".to_string(),
        content: vec![
            ApiContentBlock::Raw(json!({
                "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
                "input": {"query": "desks"}
            })),
            ApiContentBlock::Raw(json!({
                "type": "web_search_tool_result", "tool_use_id": "srvtoolu_1",
                "content": [{"type": "web_search_result", "url": "https://a.example",
                             "title": "A", "encrypted_content": "bad"}]
            })),
            ApiContentBlock::Text {
                text: "answer".to_string(),
                cache_control: None,
            },
        ],
    }];
    assert!(downgrade_native_replay(&mut messages));
    let json = serde_json::to_string(&messages).unwrap();
    assert!(!json.contains("encrypted_content"), "{json}");
    assert!(
        json.contains("https://a.example") && json.contains("desks"),
        "{json}"
    );
    assert!(!downgrade_native_replay(&mut messages));
}
