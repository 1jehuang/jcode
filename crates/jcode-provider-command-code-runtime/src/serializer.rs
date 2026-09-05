use jcode_message_types::{ContentBlock, Message, Role};
use serde_json::Value;
/// Serialize messages while repairing missing tool results for replay safety.
pub fn serialize_messages(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let content = message.content.iter().map(|block| match block {
            ContentBlock::ToolUse { id, name, input, .. } => serde_json::json!({"type":"tool-call","toolCallId":id,"toolName":name,"input":input}),
            ContentBlock::ToolResult { tool_use_id, content, is_error } => serde_json::json!({"type":"tool-result","toolCallId":tool_use_id,"content":content,"isError":is_error.unwrap_or(false)}),
            _ => serde_json::to_value(block).unwrap_or(Value::Null),
        }).collect::<Vec<_>>();
        out.push(serde_json::json!({"role": role, "content": content}));
        if message.role == Role::Assistant {
            for block in &message.content {
                if let ContentBlock::ToolUse { id, .. } = block {
                    let adjacent = messages.get(index + 1).map(|next| next.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id))).unwrap_or(false);
                    if !adjacent {
                        out.push(serde_json::json!({"role":"user","content":[{"type":"tool-result","toolCallId":id,"content":"cancelled","isError":true}]}));
                    }
                }
            }
        }
    }
    out
}
