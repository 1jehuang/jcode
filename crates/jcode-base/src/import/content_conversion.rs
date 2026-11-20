use super::*;

/// Convert Claude Code content blocks to jcode ContentBlocks
pub(super) fn convert_content_blocks(content: &ClaudeCodeContent) -> Vec<ContentBlock> {
    match content {
        ClaudeCodeContent::Empty => vec![],
        ClaudeCodeContent::Text(text) => {
            if text.is_empty() {
                vec![]
            } else {
                vec![ContentBlock::Text {
                    text: text.clone(),
                    cache_control: None,
                }]
            }
        }
        ClaudeCodeContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                ClaudeCodeContentBlock::Text { text } => Some(ContentBlock::Text {
                    text: text.clone(),
                    cache_control: None,
                }),
                ClaudeCodeContentBlock::Thinking { thinking, .. } => {
                    Some(ContentBlock::Reasoning {
                        text: thinking.clone(),
                    })
                }
                ClaudeCodeContentBlock::ToolUse { id, name, input } => {
                    Some(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        thought_signature: None,
                    })
                }
                ClaudeCodeContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => Some(ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                }),
                ClaudeCodeContentBlock::Unknown => None,
            })
            .collect(),
    }
}
