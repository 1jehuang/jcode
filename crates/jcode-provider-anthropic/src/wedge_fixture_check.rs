use super::*;
use jcode_message_types::Message;

#[test]
fn real_wedged_session_becomes_sendable() {
    let Ok(path) = std::env::var("JCODE_WEDGE_FIXTURE") else {
        return;
    };
    let raw = std::fs::read_to_string(path).expect("fixture");
    let messages: Vec<Message> = serde_json::from_str(&raw).expect("parse");
    let formatted = format_messages(&messages, false);

    let mut seen = std::collections::HashSet::new();
    for m in &formatted {
        for b in &m.content {
            if let ApiContentBlock::ToolResult { tool_use_id, .. } = b {
                assert!(seen.insert(tool_use_id.clone()), "dup {tool_use_id}");
            }
        }
    }
    // Every tool_result must be answered by a tool_use in the previous message.
    for (i, m) in formatted.iter().enumerate() {
        for b in &m.content {
            let ApiContentBlock::ToolResult { tool_use_id, .. } = b else {
                continue;
            };
            let prev = formatted.get(i.wrapping_sub(1)).expect("prev message");
            assert!(
                prev.content.iter().any(|pb| matches!(
                    pb,
                    ApiContentBlock::ToolUse { id, .. } if id == tool_use_id
                )),
                "messages.{i}: tool_result {tool_use_id} has no tool_use in the previous message"
            );
        }
    }
    println!(
        "formatted {} messages, all tool_results valid",
        formatted.len()
    );
}
