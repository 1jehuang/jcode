//! Bridge between jcode's internal message format and DCP's canonical IR.
//!
//! This module provides converters:
//! - `jcode_to_dcp`: Convert jcode messages to DCP format
//! - `dcp_to_jcode`: Convert DCP messages back to jcode format
//!
//! Wiring into Agent::messages_for_provider is deferred to a later PR
//! (see PLAN.md §9.1).

use crate::message::{ContentBlock, Message as JMsg, Role as JRole};
use dynamic_context_pruning::{Message as DcpMessage, Part, Role as DcpRole, ToolStatus};

/// Convert jcode messages to DCP canonical IR.
pub fn jcode_to_dcp(msgs: &[JMsg]) -> Vec<DcpMessage> {
    msgs.iter().map(jmsg_to_dcp).collect()
}

/// Convert a single jcode message to DCP format.
fn jmsg_to_dcp(m: &JMsg) -> DcpMessage {
    let role = match m.role {
        JRole::User => DcpRole::User,
        JRole::Assistant => DcpRole::Assistant,
    };

    let time = m.timestamp.map(|t| t.timestamp_millis()).unwrap_or(0);

    // Generate a stable ID from the message content
    let hash = crate::message::stable_message_hash(m);
    let id = format!("{:x}", hash);

    let parts: Vec<Part> = m.content.iter().filter_map(content_to_part).collect();

    DcpMessage {
        id,
        role,
        parts,
        time,
        ignored: false,
    }
}

/// Map jcode ContentBlock to DCP Part.
fn content_to_part(b: &ContentBlock) -> Option<Part> {
    Some(match b {
        ContentBlock::Text { text, .. } => Part::Text(text.clone()),
        ContentBlock::Reasoning { text } => Part::Reasoning(text.clone()),
        ContentBlock::ToolUse { id, name, input } => Part::ToolCall {
            call_id: id.clone(),
            tool: name.clone(),
            input: input.clone(),
        },
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => Part::ToolResult {
            call_id: tool_use_id.clone(),
            status: if is_error.unwrap_or(false) {
                ToolStatus::Error
            } else {
                ToolStatus::Completed
            },
            output: Some(content.clone()),
            error: None,
        },
        ContentBlock::Image { media_type, data } => Part::Image {
            media_type: media_type.clone(),
            data: data.clone(),
        },
        // OpenAICompaction has no DCP equivalent — skip
        ContentBlock::AnthropicThinking { .. }
        | ContentBlock::OpenAIReasoning { .. }
        | ContentBlock::OpenAICompaction { .. } => return None,
    })
}

/// Convert DCP messages back to jcode format.
pub fn dcp_to_jcode(msgs: Vec<DcpMessage>) -> Vec<JMsg> {
    msgs.into_iter().map(dcp_msg_to_jcode).collect()
}

/// Convert a single DCP message to jcode format.
fn dcp_msg_to_jcode(m: DcpMessage) -> JMsg {
    let role = match m.role {
        DcpRole::User => JRole::User,
        DcpRole::Assistant => JRole::Assistant,
        DcpRole::System => JRole::User, // shouldn't happen in practice
        _ => JRole::User,               // exhaustive fallback for non-exhaustive enum
    };

    let timestamp = if m.time != 0 {
        chrono::DateTime::from_timestamp_millis(m.time)
    } else {
        None
    };

    let content: Vec<ContentBlock> = m.parts.into_iter().filter_map(part_to_content).collect();

    JMsg {
        role,
        content,
        timestamp,
        tool_duration_ms: None,
    }
}

/// Map DCP Part back to jcode ContentBlock.
fn part_to_content(p: Part) -> Option<ContentBlock> {
    Some(match p {
        Part::Text(text) => ContentBlock::Text {
            text,
            cache_control: None,
        },
        Part::Reasoning(text) => ContentBlock::Reasoning { text },
        Part::ToolCall {
            call_id,
            tool,
            input,
        } => ContentBlock::ToolUse {
            id: call_id,
            name: tool,
            input,
        },
        Part::ToolResult {
            call_id,
            status,
            output,
            error,
        } => ContentBlock::ToolResult {
            tool_use_id: call_id,
            content: output.or(error).unwrap_or_default(),
            is_error: matches!(status, ToolStatus::Error).then_some(true),
        },
        Part::Image { media_type, data } => ContentBlock::Image { media_type, data },
        _ => return None, // exhaustive fallback for non-exhaustive enum
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ContentBlock;

    #[test]
    fn test_jcode_to_dcp_roundtrip() {
        let jmsg = JMsg {
            role: JRole::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
            timestamp: Some(chrono::Utc::now()),
            tool_duration_ms: None,
        };

        let dcp = jmsg_to_dcp(&jmsg);
        assert!(!dcp.id.is_empty());
        assert_eq!(dcp.role, DcpRole::User);
        assert_eq!(dcp.parts.len(), 1);

        let back = dcp_msg_to_jcode(dcp);
        assert_eq!(back.role, JRole::User);
    }

    #[test]
    fn test_tool_call_roundtrip() {
        use dynamic_context_pruning::Part;

        let dcp_msg = DcpMessage {
            id: "test123".to_string(),
            role: DcpRole::Assistant,
            parts: vec![
                Part::Text("I will read the file".to_string()),
                Part::ToolCall {
                    call_id: "tool_1".to_string(),
                    tool: "read".to_string(),
                    input: serde_json::json!({"path": "foo.rs"}),
                },
            ],
            time: 1234567890,
            ignored: false,
        };

        let jmsg = dcp_msg_to_jcode(dcp_msg);
        assert_eq!(jmsg.role, JRole::Assistant);
        assert!(
            jmsg.content
                .iter()
                .any(|c| matches!(c, ContentBlock::ToolUse { .. }))
        );
    }
}
