use super::*;
use crate::message::ContentBlock;
use serde_json::json;

fn estimate(messages: &[Message], tools: &[ToolDefinition]) -> (usize, usize) {
    MultiProvider::estimate_request_input(messages, tools, CompletionMode::Unified { system: "" })
}

#[test]
fn request_estimate_charges_images_by_context_not_base64_bytes() {
    let small = Message::user_with_images("look", vec![("image/png".into(), "a".repeat(100))]);
    let large =
        Message::user_with_images("look", vec![("image/png".into(), "a".repeat(4_000_000))]);
    assert_eq!(estimate(&[small], &[]), estimate(&[large], &[]));
    assert_eq!(
        estimate(
            &[Message::user_with_images(
                "",
                vec![("image/png".into(), "a".into())]
            )],
            &[]
        )
        .1,
        jcode_compaction_core::IMAGE_TOKEN_COST
    );
}

#[test]
fn request_estimate_bounds_native_compaction_payload() {
    let mut message = Message::user("");
    message.content = vec![ContentBlock::OpenAICompaction {
        encrypted_content: "x".repeat(1_100_000),
    }]
    .into();
    assert_eq!(
        estimate(&[message], &[]).1,
        jcode_compaction_core::OPENAI_COMPACTION_TOKEN_COST
    );
}

#[test]
fn request_estimate_only_charges_loaded_deferred_definitions_once() {
    let eager = ToolDefinition::new("eager", "ordinary", json!({"type":"object"}));
    let loaded = ToolDefinition::new("loaded", "x".repeat(80_000), json!({})).deferred();
    let unloaded = ToolDefinition::new("unloaded", "x".repeat(800_000), json!({})).deferred();
    let tools = [eager.clone(), loaded.clone(), unloaded];
    assert_eq!(estimate(&[], &tools).0, eager.prompt_chars());
    let mut message = Message::user("");
    message.content = vec![
        ContentBlock::ToolReference {
            tool_use_id: "call".into(),
            tool_name: "loaded".into(),
        };
        2
    ]
    .into();
    let message_chars = jcode_compaction_core::message_char_count(&message);
    assert_eq!(
        estimate(&[message], &tools).0,
        message_chars + eager.prompt_chars() + loaded.prompt_chars()
    );
}

#[test]
fn request_estimate_counts_split_and_unified_system_text_equally() {
    let messages = [Message::user("hello world")];
    let unified = MultiProvider::estimate_request_input(
        &messages,
        &[],
        CompletionMode::Unified {
            system: "staticdynamic",
        },
    );
    let split = MultiProvider::estimate_request_input(
        &messages,
        &[],
        CompletionMode::Split {
            system_static: "static",
            system_dynamic: "dynamic",
        },
    );
    assert_eq!(unified, split);
    assert_eq!(unified, (24, 6));
    assert_eq!(estimate(&[], &[]), (0, 0));
}
