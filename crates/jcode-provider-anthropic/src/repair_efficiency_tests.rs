//! Frozen pre-optimization oracle and repair efficiency coverage.
use super::*;

#[cfg(target_os = "linux")]
#[path = "repair_cpu_probe.rs"]
mod cpu_probe;

fn native(item: Value) -> ContentBlock {
    ContentBlock::ProviderNative {
        provider: "anthropic".into(),
        item,
    }
}
fn message(role: Role, blocks: Vec<ContentBlock>) -> Message {
    Message {
        role,
        content: blocks.into(),
        timestamp: None,
        tool_duration_ms: Some(17),
    }
}
fn text() -> ContentBlock {
    ContentBlock::Text {
        text: "large history payload 🦀".repeat(2048),
        cache_control: None,
    }
}
fn parity(messages: &[Message]) {
    assert_eq!(
        serde_json::to_value(repair_dangling_anthropic_server_tools(messages)).unwrap(),
        serde_json::to_value(legacy_repair(messages)).unwrap()
    );
}

#[test]
fn frozen_oracle_exhaustive_native_shapes_and_final_pause() {
    let blocks = vec![
        text(),
        native(serde_json::json!({"type":"server_tool_use","id":"a"})),
        native(serde_json::json!({"type":"server_tool_use","id":"b"})),
        native(serde_json::json!({"type":"web_search_tool_result","tool_use_id":"a"})),
        native(serde_json::json!({"type":"web_search_tool_result","tool_use_id":"b"})),
        native(serde_json::json!({"type":"server_tool_use"})),
        native(serde_json::json!({"type":"server_tool_use","id":17})),
        native(serde_json::json!({"type":"web_search_tool_result","tool_use_id":null})),
        native(serde_json::json!({"type":"server_tool_use","id":""})),
        native(serde_json::json!({"type":"web_search_tool_result","tool_use_id":""})),
        native(serde_json::json!({"type":"future_native","payload":[1,2]})),
        ContentBlock::ProviderNative {
            provider: "other".into(),
            item: serde_json::json!({"type":"server_tool_use"}),
        },
    ];
    parity(&[]);
    for a in &blocks {
        for b in &blocks {
            for assistant in [false, true] {
                let mut history = vec![
                    message(Role::Assistant, vec![a.clone()]),
                    message(
                        if assistant {
                            Role::Assistant
                        } else {
                            Role::User
                        },
                        vec![b.clone()],
                    ),
                ];
                parity(&history);
                history.push(message(Role::Assistant, vec![]));
                parity(&history); // Empty final messages must not extend pause semantics.
                history[0].content = vec![text(), a.clone(), text(), b.clone()].into();
                parity(&history);
            }
        }
    }
}

#[test]
fn shares_only_unchanged_nonempty_messages() {
    let history = vec![
        message(Role::User, vec![text()]),
        message(
            Role::Assistant,
            vec![
                text(),
                native(serde_json::json!({"type":"server_tool_use","id":"drop"})),
                text(),
            ],
        ),
        message(Role::User, vec![]),
        message(
            Role::Assistant,
            vec![native(
                serde_json::json!({"type":"server_tool_use","id":"pause"}),
            )],
        ),
    ];
    let repaired = repair_dangling_anthropic_server_tools(&history);
    parity(&history);
    assert_eq!(repaired.len(), 3);
    assert!(repaired[0].content.shares_storage_with(&history[0].content));
    assert!(!repaired[1].content.shares_storage_with(&history[1].content));
    assert!(repaired[2].content.shares_storage_with(&history[3].content));
    assert_eq!(history[1].content.len(), 3);
    let valid = vec![message(
        Role::Assistant,
        vec![
            native(serde_json::json!({"type":"server_tool_use","id":"a"})),
            native(serde_json::json!({"type":"web_search_tool_result","tool_use_id":"a"})),
        ],
    )];
    assert!(
        repair_dangling_anthropic_server_tools(&valid)[0]
            .content
            .shares_storage_with(&valid[0].content)
    );
}

fn legacy_repair(messages: &[Message]) -> Vec<Message> {
    use std::collections::HashSet;

    let mut starts = HashSet::new();
    let mut results = HashSet::new();
    for msg in messages {
        for block in &msg.content {
            let ContentBlock::ProviderNative { provider, item } = block else {
                continue;
            };
            if provider != jcode_message_types::provider_native::PROVIDER_NATIVE_ANTHROPIC {
                continue;
            }
            match item.get("type").and_then(Value::as_str) {
                Some("server_tool_use") => {
                    if let Some(id) = item.get("id").and_then(Value::as_str) {
                        starts.insert(id.to_string());
                    }
                }
                Some("web_search_tool_result") => {
                    if let Some(id) = item.get("tool_use_id").and_then(Value::as_str) {
                        results.insert(id.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let final_paused_id = messages.last().and_then(|message| {
        if !matches!(message.role, Role::Assistant) {
            return None;
        }
        let Some(ContentBlock::ProviderNative { provider, item }) = message.content.last() else {
            return None;
        };
        if provider != jcode_message_types::provider_native::PROVIDER_NATIVE_ANTHROPIC {
            return None;
        }
        (item.get("type").and_then(Value::as_str) == Some("server_tool_use"))
            .then(|| item.get("id").and_then(Value::as_str))
            .flatten()
            .map(str::to_string)
    });

    let mut repaired = Vec::with_capacity(messages.len());
    for message in messages {
        let mut content = Vec::with_capacity(message.content.len());
        for block in &message.content {
            let ContentBlock::ProviderNative { provider, item } = block else {
                content.push(block.clone());
                continue;
            };

            if provider != jcode_message_types::provider_native::PROVIDER_NATIVE_ANTHROPIC {
                content.push(block.clone());
                continue;
            }

            match item.get("type").and_then(Value::as_str) {
                Some("server_tool_use") => {
                    let Some(id) = item.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    if results.contains(id) || final_paused_id.as_deref() == Some(id) {
                        content.push(block.clone());
                    }
                }
                Some("web_search_tool_result") => {
                    let Some(id) = item.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    if starts.contains(id) {
                        content.push(block.clone());
                    }
                }
                _ => content.push(block.clone()),
            }
        }

        if !content.is_empty() {
            let mut repaired_message = message.clone();
            repaired_message.content = content.into();
            repaired.push(repaired_message);
        }
    }
    repaired
}
