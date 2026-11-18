// Frozen pre-optimization oracle. Keep independent of optimized function bodies.
use super::*;

// Frozen fallback implementation, independent of the new shared renderer.
fn legacy_text_fallback(
    provider: &str,
    item: &Value,
    call_input: Option<&Value>,
) -> Option<String> {
    let display = jcode_message_types::provider_native::provider_native_display(provider, item)?;
    let output = display.output?;
    let query = display
        .input
        .as_ref()
        .or(call_input)
        .and_then(|input| input.get("query").and_then(Value::as_str))
        .filter(|query| !query.is_empty());
    const NOTE: &str =
        "(summary of an earlier provider-side search; page contents are no longer available)";
    Some(match query {
        Some(query) => format!("[{} for \"{query}\"] {NOTE}\n{output}", display.name),
        None => format!("[{} results] {NOTE}\n{output}", display.name),
    })
}

pub(super) fn legacy_downgrade_native_replay(messages: &mut [ApiMessage]) -> bool {
    use jcode_message_types::provider_native;
    let mut changed = false;
    for message in messages.iter_mut() {
        let mut call_inputs: std::collections::HashMap<String, Value> =
            std::collections::HashMap::new();
        let mut out = Vec::with_capacity(message.content.len());
        for block in message.content.drain(..) {
            let ApiContentBlock::Raw(item) = &block else {
                out.push(block);
                continue;
            };
            changed = true;
            let provider = provider_native::PROVIDER_NATIVE_ANTHROPIC;
            let display = provider_native::provider_native_display(provider, item);
            if let Some(display) = &display
                && display.output.is_none()
                && let Some(input) = &display.input
            {
                call_inputs.insert(display.id.clone(), input.clone());
            }
            if let Some(text) = legacy_text_fallback(
                provider,
                item,
                display.as_ref().and_then(|d| call_inputs.get(&d.id)),
            ) {
                out.push(ApiContentBlock::Text {
                    text,
                    cache_control: None,
                });
            }
        }
        if out.is_empty() {
            out.push(ApiContentBlock::Text {
                text: "[earlier provider-side search]".to_string(),
                cache_control: None,
            });
        }
        message.content = out;
    }
    changed
}

pub(super) fn legacy_format_content_blocks_with_native(
    blocks: &[ContentBlock],
    is_oauth: bool,
    native_replay: bool,
) -> Vec<ApiContentBlock> {
    let mut result: Vec<ApiContentBlock> = Vec::new();
    // Inputs of server tool calls seen so far, so a downgraded result can name
    // the query that produced it.
    let mut native_call_inputs: std::collections::HashMap<String, Value> =
        std::collections::HashMap::new();
    for block in blocks {
        match block {
            ContentBlock::ProviderNative { provider, item } => {
                use jcode_message_types::provider_native;
                let display = provider_native::provider_native_display(provider, item);
                if let Some(display) = &display
                    && display.output.is_none()
                    && let Some(input) = &display.input
                {
                    native_call_inputs.insert(display.id.clone(), input.clone());
                }
                let is_anthropic_block = provider == provider_native::PROVIDER_NATIVE_ANTHROPIC
                    && item
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(provider_native::is_anthropic_server_tool_block);
                if native_replay && is_anthropic_block {
                    result.push(ApiContentBlock::Raw(item.clone()));
                } else if let Some(text) = legacy_text_fallback(
                    provider,
                    item,
                    display
                        .as_ref()
                        .and_then(|display| native_call_inputs.get(&display.id)),
                ) {
                    result.push(ApiContentBlock::Text {
                        text,
                        cache_control: None,
                    });
                }
            }
            ContentBlock::Text { text, .. } => {
                // A text block that immediately follows an image-bearing tool_result is the
                // "[Attached image associated with the preceding tool result: ...]" label
                // emitted alongside image tool outputs. The Anthropic API requires every
                // tool_result for a parallel tool-call turn to be contiguous in the next user
                // message; a sibling text block wedged between tool_results makes the API
                // report later tool_use ids as missing their tool_result. Fold the label into
                // the tool_result's content blocks so the tool_results stay contiguous.
                if let Some(ApiContentBlock::ToolResult {
                    content: ToolResultContent::Blocks(blocks),
                    ..
                }) = result.last_mut()
                    && blocks
                        .iter()
                        .any(|b| matches!(b, ToolResultContentBlock::Image { .. }))
                {
                    blocks.push(ToolResultContentBlock::Text { text: text.clone() });
                } else {
                    result.push(ApiContentBlock::Text {
                        text: text.clone(),
                        cache_control: None,
                    });
                }
            }
            ContentBlock::AnthropicThinking {
                thinking,
                signature,
            } => {
                result.push(ApiContentBlock::Thinking {
                    thinking: thinking.clone(),
                    signature: signature.clone(),
                });
            }
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                result.push(ApiContentBlock::ToolUse {
                    id: sanitize_tool_id(id),
                    name: if is_oauth {
                        map_tool_name_for_oauth(name)
                    } else {
                        name.clone()
                    },
                    input: if input.is_object() {
                        input.clone()
                    } else {
                        serde_json::json!({})
                    },
                    cache_control: None,
                });
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                result.push(ApiContentBlock::ToolResult {
                    tool_use_id: sanitize_tool_id(tool_use_id),
                    content: ToolResultContent::Text(content.clone()),
                    is_error: is_error.unwrap_or(false),
                });
            }
            ContentBlock::Image { media_type, data } => {
                let img_block = ToolResultContentBlock::Image {
                    source: ApiImageSource {
                        kind: "base64".to_string(),
                        media_type: media_type.clone(),
                        data: data.clone(),
                    },
                };
                if let Some(ApiContentBlock::ToolResult { content, .. }) = result.last_mut() {
                    match content {
                        ToolResultContent::Text(text) => {
                            let text_block = ToolResultContentBlock::Text {
                                text: std::mem::take(text),
                            };
                            *content = ToolResultContent::Blocks(vec![text_block, img_block]);
                        }
                        ToolResultContent::Blocks(blocks) => {
                            blocks.push(img_block);
                        }
                    }
                } else {
                    result.push(ApiContentBlock::Image {
                        source: ApiImageSource {
                            kind: "base64".to_string(),
                            media_type: media_type.clone(),
                            data: data.clone(),
                        },
                    });
                }
            }
            _ => {}
        }
    }
    result
}
