use super::*;

/// Parse a streamed tool-call argument blob into a JSON object (empty object for
/// a blank payload), shared by the native tool smoke probes.
pub(super) fn parse_tool_arguments(input_json: &str) -> serde_json::Value {
    jcode_base::message::ToolCall::parse_streamed_input_to_object(if input_json.trim().is_empty() {
        "{}"
    } else {
        input_json.trim()
    })
}

/// Build the assistant `tool_use` replay block for a captured native tool call,
/// preserving any provider-emitted `thought_signature` so backends that require
/// it (Gemini-3 via the Cloud Code/Antigravity runtime) accept the follow-up.
pub(super) fn assistant_tool_use(
    call: &NativeClaudeToolCall,
    arguments: &serde_json::Value,
) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            input: arguments.clone(),
            thought_signature: call.thought_signature.clone(),
        }]
        .into(),
        timestamp: None,
        tool_duration_ms: None,
    }
}

/// Build a user turn carrying a synthetic `tool_result` for a captured native
/// tool call, used to answer each step of the multi-call replay loop.
pub(super) fn tool_result_then_text(tool_use_id: &str, result: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: result.to_string(),
            is_error: Some(false),
        }]
        .into(),
        timestamp: None,
        tool_duration_ms: None,
    }
}

/// Build a single assistant message that replays *every* captured tool call as a
/// parallel batch (multiple `ToolUse` blocks in one message), each preserving
/// its own `thought_signature`. This is the shape the parallel-tool-call phase
/// asserts the backend accepts as one assistant turn carrying N `functionCall`
/// parts.
pub(super) fn assistant_parallel_tool_uses(calls: &[NativeClaudeToolCall]) -> Message {
    let content = calls
        .iter()
        .map(|call| ContentBlock::ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            input: parse_tool_arguments(&call.input_json),
            thought_signature: call.thought_signature.clone(),
        })
        .collect();
    Message {
        role: Role::Assistant,
        content,
        timestamp: None,
        tool_duration_ms: None,
    }
}

/// Build a single user message answering *every* parallel tool call with a
/// synthetic `tool_result`, so a parallel assistant turn is fully resolved in
/// one follow-up message.
pub(super) fn parallel_tool_results(calls: &[NativeClaudeToolCall]) -> Message {
    let content = calls
        .iter()
        .enumerate()
        .map(|(index, call)| ContentBlock::ToolResult {
            tool_use_id: call.id.clone(),
            content: format!("Contents of file {}: token_{index}.", index + 1),
            is_error: Some(false),
        })
        .collect();
    Message {
        role: Role::User,
        content,
        timestamp: None,
        tool_duration_ms: None,
    }
}
