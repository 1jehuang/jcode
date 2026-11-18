use super::{ContentBlock, StoredMessage};
use serde_json::json;

pub(super) fn message(blocks: Vec<ContentBlock>) -> StoredMessage {
    StoredMessage {
        id: "projection-fixture".into(),
        role: jcode_message_types::Role::User,
        content: blocks.into(),
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }
}
pub(super) fn text(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.into(),
        cache_control: None,
    }
}
pub(super) fn cases() -> Vec<(&'static str, StoredMessage, bool)> {
    let call = |name: &str, input| ContentBlock::ToolUse {
        id: "call".into(),
        name: name.into(),
        input,
        thought_signature: None,
    };
    vec![
        ("empty", message(vec![]), false),
        ("small", message(vec![text("hello 東京")]), false),
        (
            "large-text",
            message(vec![text(&"abc東京\n".repeat(150_000))]),
            false,
        ),
        (
            "large-result",
            message(vec![ContentBlock::ToolResult {
                tool_use_id: "call".into(),
                content: "output東京\n".repeat(150_000),
                is_error: None,
            }]),
            true,
        ),
        (
            "many-text",
            message((0..100).map(|_| text("alpha beta 東京\n")).collect()),
            false,
        ),
        (
            "null-call",
            message(vec![call("bash", serde_json::Value::Null)]),
            true,
        ),
        (
            "full-call",
            message(vec![call(
                "bash",
                json!({"command":"echo 東京", "intent":"inspect"}),
            )]),
            true,
        ),
        (
            "intent",
            message(vec![call(
                "bash",
                json!({"intent":"inspect", "command":"x".repeat(1000)}),
            )]),
            false,
        ),
        (
            "todo",
            message(vec![call(
                "todo",
                json!({"intent":"plan", "add":[{"name":"a","description":"b"}], "update":[{"rename_to":"renamed"}], "phases":[{"lanes":[{"items":[{"name":"nested","description":"東京"}]}]}]}),
            )]),
            false,
        ),
        (
            "empty-separators",
            message(vec![text(""), text(""), text("x"), text("")]),
            false,
        ),
        (
            "ignored",
            message(vec![ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc".into(),
            }]),
            false,
        ),
    ]
}
