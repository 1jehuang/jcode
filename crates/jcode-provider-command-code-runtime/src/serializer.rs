use jcode_message_types::{ContentBlock, Message, Role};
use serde_json::{Value, json};

fn output(value: &str, error: bool) -> Value {
    json!({"type": if error { "error-text" } else { "text" }, "value": value})
}

/// Serialize the native wire, repairing orphan, out-of-order, and cancelled calls.
pub fn serialize_messages(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut pending: Vec<(String, String)> = Vec::new();
    let close = |out: &mut Vec<Value>, pending: &mut Vec<(String, String)>| {
        for (id, name) in pending.drain(..) {
            out.push(json!({"role":"tool","content":[{"type":"tool-result","toolCallId":id,"toolName":name,"output":output("cancelled: [ocx] no tool result was recorded for this tool call; execution status unknown.", true)}]}));
        }
    };
    for message in messages {
        match message.role {
            Role::Assistant => {
                close(&mut out, &mut pending);
                let mut content = Vec::new();
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            content.push(json!({"type":"text","text":text}))
                        }
                        ContentBlock::Reasoning { text }
                        | ContentBlock::ReasoningTrace { text } => {
                            content.push(json!({"type":"reasoning","text":text}))
                        }
                        ContentBlock::ToolUse {
                            id, name, input, ..
                        } => {
                            pending.push((id.clone(), name.clone()));
                            content.push(json!({"type":"tool-call","toolCallId":id,"toolName":name,"input":input}));
                        }
                        _ => {}
                    }
                }
                out.push(json!({"role":"assistant","content":content}));
            }
            Role::User => {
                let mut carrier = Vec::new();
                for block in &message.content {
                    match block {
                        ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                            if let Some(pos) = pending.iter().position(|(id, _)| id == tool_use_id) {
                                let (_, name) = pending.remove(pos);
                                out.push(json!({"role":"tool","content":[{"type":"tool-result","toolCallId":tool_use_id,"toolName":name,"output":output(content, is_error.unwrap_or(false))}]}));
                            } else {
                                close(&mut out, &mut pending);
                                carrier.push(json!({"type":"text","text":format!("[tool result without adjacent tool call: {}]\\n{}", tool_use_id, content)}));
                            }
                        }
                        ContentBlock::Text { text, .. } => carrier.push(json!({"type":"text","text":text})),
                        ContentBlock::Image { media_type, data } => carrier.push(json!({"type":"image","image":format!("data:{};base64,{}", media_type, data),"mediaType":media_type})),
                        _ => {}
                    }
                }
                if !carrier.is_empty() {
                    close(&mut out, &mut pending);
                    out.push(json!({"role":"user","content":carrier}));
                }
            }
        }
    }
    close(&mut out, &mut pending);
    out
}
