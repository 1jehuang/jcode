use jcode_message_types::{ContentBlock, Message, Role, ToolCall};
use serde_json::json;
use std::collections::HashSet;

use super::{GeminiContent, GeminiPart, tool_name_from_tool_result};

pub fn build_contents(messages: &[Message]) -> Vec<GeminiContent> {
    build_contents_with_signature_policy(messages, SignaturePolicy::ReplayCarriedForward)
}

/// How [`build_contents_with_signature_policy`] handles Gemini-3 function-call
/// `thoughtSignature` replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignaturePolicy {
    /// Normal path: replay each call's own signature, carrying the most recent
    /// real signature forward onto calls that lack one.
    ReplayCarriedForward,
    /// Recovery path for a `missing a thought_signature` 400. The backend has
    /// told us it will not accept this history's function-call parts, so
    /// downgrade every tool call and tool result to plain text. This loses the
    /// structured form but keeps the conversation's *content*, turning a hard
    /// dead-end into a turn that completes.
    DowngradeToolCallsToText,
}

/// Whether a provider error body is the Cloud Code / Antigravity backend
/// rejecting a turn because its function calls lack a usable thought signature.
pub fn is_missing_thought_signature_error(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("thought_signature") || lowered.contains("thoughtsignature")
}

pub fn build_contents_with_signature_policy(
    messages: &[Message],
    policy: SignaturePolicy,
) -> Vec<GeminiContent> {
    // Pre-pass: find tool calls that are still in-flight. Gemini rejects a
    // request where the number of `functionCall` parts is not equal to the number
    // of `functionResponse` parts. By filtering out `tool_use` blocks that have
    // no corresponding `tool_result`, we guarantee the counts match. This is
    // safer than inventing a placeholder response that Gemini might not
    // understand. See issue #1988.
    let tool_results_map: HashSet<String> = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect();

    // Symmetric check: a ToolResult whose tool_use_id has no matching ToolUse
    // must be dropped.
    let tool_uses_map: HashSet<String> = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();

    // Gemini-3 attaches an opaque `thoughtSignature` to function-call parts, and the Cloud
    // Code / Antigravity backend rejects an assistant turn whose function calls are ALL
    // unsigned with `Function call is missing a thought_signature in functionCall parts`
    // (HTTP 400, issue #339). This happens because a parallel multi-call turn only signs its
    // FIRST call, and synthesized/imported calls carry no signature.
    //
    // Live-verified backend rule: a turn is accepted as long as *at least one* of its
    // function calls carries a (valid) signature. We carry the most recent real signature
    // forward across the whole conversation. This keeps multi-call turns and synthesized
    // histories replayable.
    let mut last_signature: Option<String> = None;
    let mut contents: Vec<GeminiContent> = messages
        .iter()
        .filter_map(|message| {
            let role = match message.role {
                Role::User => "user",
                Role::Assistant => "model",
            };
            let mut parts = Vec::new();
            for block in &message.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        parts.push(GeminiPart {
                            text: Some(text.clone()),
                            ..Default::default()
                        });
                    }
                    ContentBlock::Reasoning { .. }
                    | ContentBlock::ReasoningTrace { .. }
                    | ContentBlock::AnthropicThinking { .. }
                    | ContentBlock::OpenAIReasoning { .. } => {}
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input,
                        thought_signature,
                    } => {
                        // A tool call still in flight has no result yet. Do not
                        // emit a `functionCall` for it, as Gemini requires the
                        // number of calls and responses to be equal.
                        if !tool_results_map.contains(id) {
                            continue;
                        }

                        if policy == SignaturePolicy::DowngradeToolCallsToText {
                            // The backend rejected this history's function-call
                            // parts. Preserve the call as readable text so the
                            // model keeps its context instead of 400ing forever.
                            parts.push(GeminiPart {
                                text: Some(format!(
                                    "[previous tool call] {name}({})",
                                    ToolCall::input_as_object(input)
                                )),
                                ..Default::default()
                            });
                            continue;
                        }
                        let own_signature = thought_signature
                            .as_ref()
                            .filter(|sig| !sig.is_empty())
                            .cloned();
                        if own_signature.is_some() {
                            last_signature = own_signature.clone();
                        }
                        let signature = own_signature.or_else(|| last_signature.clone());
                        parts.push(GeminiPart {
                            function_call: Some(super::GeminiFunctionCall {
                                name: name.clone(),
                                args: ToolCall::input_as_object(input),
                                id: Some(id.clone()),
                            }),
                            thought_signature: signature,
                            ..Default::default()
                        });
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        // A tool result for a tool use that is not in the history
                        // must be dropped.
                        if !tool_uses_map.contains(tool_use_id) {
                            continue;
                        }

                        if policy == SignaturePolicy::DowngradeToolCallsToText {
                            // A `functionResponse` with no matching signed
                            // `functionCall` is also rejected, so downgrade it too.
                            let label = if is_error.unwrap_or(false) {
                                "previous tool error"
                            } else {
                                "previous tool result"
                            };
                            parts.push(GeminiPart {
                                text: Some(format!(
                                    "[{label}] {}: {content}",
                                    tool_name_from_tool_result(tool_use_id, messages)
                                )),
                                ..Default::default()
                            });
                            continue;
                        }
                        parts.push(GeminiPart {
                            function_response: Some(super::GeminiFunctionResponse {
                                name: tool_name_from_tool_result(tool_use_id, messages),
                                response: if is_error.unwrap_or(false) {
                                    json!({ "error": content })
                                } else {
                                    json!({ "content": content })
                                },
                                id: Some(tool_use_id.clone()),
                            }),
                            ..Default::default()
                        });
                    }
                    ContentBlock::Image { media_type, data } => {
                        parts.push(GeminiPart {
                            inline_data: Some(super::InlineData {
                                mime_type: media_type.clone(),
                                data: data.clone(),
                            }),
                            ..Default::default()
                        });
                    }
                    ContentBlock::OpenAICompaction { .. }
                    | ContentBlock::ToolReference { .. }
                    | ContentBlock::ProviderNative { .. } => {}
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(GeminiContent {
                    role: role.to_string(),
                    parts,
                })
            }
        })
        .collect();

    // BUG 2: Ensure the conversation does not end with a "model" turn, which
    // Gemini rejects with "Requests ending with a model turn are not supported".
    // This can happen if the last action was a tool call that produced an image
    // or other content. Appending a minimal user turn fixes this. See issue #1988.
    if let Some(last_content) = contents.last()
        && last_content.role == "model"
    {
        contents.push(GeminiContent {
            role: "user".to_string(),
            parts: vec![GeminiPart {
                text: Some("".to_string()),
                ..Default::default()
            }],
        });
    }

    contents
}
