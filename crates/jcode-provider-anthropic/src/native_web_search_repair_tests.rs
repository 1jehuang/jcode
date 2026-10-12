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
