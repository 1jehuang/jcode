// Frozen accepted 3de375bd4 formatter before borrowing replay call inputs.
use super::*;

pub(super) fn accepted_format_content_blocks_with_native(
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
                // Replay raw blocks without constructing discarded result summaries.
                // Call starts must still update the shared same-id map, including
                // collisions with later cross-provider items.
                let replay = native_replay
                    && provider == provider_native::PROVIDER_NATIVE_ANTHROPIC
                    && item
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(provider_native::is_anthropic_server_tool_block);
                if replay {
                    if item.get("type").and_then(Value::as_str) == Some("server_tool_use")
                        && let Some(id) = item.get("id").and_then(Value::as_str)
                    {
                        native_call_inputs.insert(
                            id.to_string(),
                            item.get("input").cloned().unwrap_or(Value::Null),
                        );
                    }
                    result.push(ApiContentBlock::Raw(item.clone()));
                    continue;
                }
                let mut display = provider_native::provider_native_display(provider, item);
                if let Some(display) = &mut display
                    && display.output.is_none()
                    && let Some(input) = display.input.take()
                {
                    native_call_inputs.insert(display.id.clone(), input);
                }
                if let Some(display) = &display
                    && let Some(text) = provider_native::provider_native_display_text_fallback(
                        display,
                        native_call_inputs.get(&display.id),
                    )
                {
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
