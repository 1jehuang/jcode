use jcode_message_types::{ContentBlock, Message, Role};
use serde_json::Value;
/// Serialize messages while repairing missing tool results for replay safety.
pub fn serialize_messages(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for message in messages {
        out.push(serde_json::to_value(message).unwrap_or(Value::Null));
        if message.role == Role::Assistant { for block in &message.content { if let ContentBlock::ToolUse { id, .. } = block { let paired = messages.iter().any(|candidate| candidate.role == Role::User && candidate.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id))); if !paired { out.push(serde_json::json!({"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":"cancelled","is_error":true}]})); } } } }
    }
    out
}
