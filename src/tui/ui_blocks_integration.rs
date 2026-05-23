use super::ui_blocks::*;
use ratatui::style::Style;

pub struct SimpleMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCallInfo>,
    pub duration_secs: Option<f64>,
    pub title: Option<String>,
}

pub struct ToolCallInfo {
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

pub fn message_to_command_blocks(message: &SimpleMessage) -> Vec<CommandBlock> {
    let mut blocks = Vec::new();

    let block_type = match message.role.as_str() {
        "user" => BlockType::UserInput,
        "assistant" => {
            if message.tool_calls.is_empty() {
                BlockType::Reasoning {
                    model_name: message.title.clone().unwrap_or_default(),
                }
            } else {
                BlockType::ToolCall {
                    tool_name: message.tool_calls[0].tool_name.clone(),
                }
            }
        }
        "system" => BlockType::SystemNotification,
        _ => BlockType::SystemNotification,
    };

    let mut block = CommandBlock::new(block_type, &message.title.clone().unwrap_or_default());

    let content = build_block_content(&message.content);
    block = block.with_content(content);

    if let Some(duration) = message.duration_secs {
        block.duration_ms = Some((duration * 1000.0) as u64);
    }

    let status = if message.role == "system" {
        BlockStatus::Success
    } else {
        BlockStatus::Success
    };
    block = block.with_status(status);

    add_default_actions(&mut block);

    blocks.push(block);

    if !message.tool_calls.is_empty() {
        for tool_call in &message.tool_calls {
            let tool_block = build_tool_result_block(tool_call);
            blocks.push(tool_block);
        }
    }

    blocks
}

fn build_block_content(content: &str) -> BlockContent {
    if content.trim().starts_with('{') && content.trim().ends_with('}') {
        if let Ok(json) = serde_json::from_str(content) {
            return BlockContent::JsonTree(json);
        }
    }

    if content.contains('\n') && content.lines().count() > 3 {
        let first_line = content.lines().next().unwrap_or("");
        if first_line.starts_with("diff --git") {
            return build_diff_content(content);
        }

        if let Some(lang) = detect_language(content) {
            return BlockContent::Code(CodeBlock {
                language: Some(lang),
                code: content.to_string(),
                line_numbers: true,
            });
        }

        if looks_like_table(content) {
            return build_table_content(content);
        }
    }

    BlockContent::PlainText(content.to_string())
}

fn build_diff_content(content: &str) -> BlockContent {
    let mut hunks = Vec::new();
    let mut current_hunk: Option<DiffHunk> = None;

    for line in content.lines() {
        if line.starts_with("@@ ") {
            if let Some(hunk) = current_hunk.take() {
                hunks.push(hunk);
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            let old_part = parts.get(1).unwrap_or(&"+0,0");
            let new_part = parts.get(2).unwrap_or(&"+0,0");
            let old_start = parse_hunk_start(old_part);
            let new_start = parse_hunk_start(new_part);
            current_hunk = Some(DiffHunk {
                old_start,
                new_start,
                lines: Vec::new(),
            });
        } else if let Some(hunk) = current_hunk.as_mut() {
            let diff_line = if line.starts_with('+') {
                DiffLine::Added(line[1..].to_string())
            } else if line.starts_with('-') {
                DiffLine::Removed(line[1..].to_string())
            } else {
                DiffLine::Context(line.to_string())
            };
            hunk.lines.push(diff_line);
        }
    }

    if let Some(hunk) = current_hunk {
        hunks.push(hunk);
    }

    BlockContent::Diff(DiffContent {
        old_text: content.to_string(),
        new_text: content.to_string(),
        hunks,
    })
}

fn parse_hunk_start(part: &str) -> usize {
    let num_str = part.trim_start_matches(|c| c == '-' || c == '+');
    num_str.split(',').next().unwrap_or("1").parse().unwrap_or(1)
}

fn detect_language(content: &str) -> Option<String> {
    let first_line = content.lines().next().unwrap_or("");
    if first_line.starts_with("//") || first_line.starts_with("fn ") || first_line.starts_with("pub ") {
        return Some("rust".to_string());
    }
    if first_line.starts_with("# ") || first_line.contains("def ") || first_line.contains("import ") {
        return Some("python".to_string());
    }
    if first_line.starts_with("// ") || first_line.contains("function ") || first_line.contains("const ") {
        return Some("javascript".to_string());
    }
    if first_line.starts_with("package ") || first_line.contains("class ") && !first_line.contains("def ") {
        return Some("java".to_string());
    }
    if first_line.starts_with("#include") || first_line.starts_with("int main") {
        return Some("cpp".to_string());
    }
    if content.contains("echo ") || content.contains("$(") || content.starts_with("#!") {
        return Some("bash".to_string());
    }
    None
}

fn looks_like_table(content: &str) -> bool {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < 2 {
        return false;
    }
    lines[1].chars().all(|c| c == '-' || c == '|' || c == ' ')
}

fn build_table_content(content: &str) -> BlockContent {
    let mut headers = Vec::new();
    let mut rows = Vec::new();

    for (i, line) in content.lines().enumerate() {
        let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
        let parts: Vec<&str> = parts.into_iter().filter(|s| !s.is_empty()).collect();

        if i == 0 {
            headers = parts.into_iter().map(|s| s.to_string()).collect();
        } else if i == 1 {
            continue;
        } else {
            let row: Vec<TableCell> = parts
                .into_iter()
                .map(|s| TableCell {
                    content: s.to_string(),
                    style: Style::default(),
                })
                .collect();
            rows.push(row);
        }
    }

    BlockContent::Table(TableData { headers, rows })
}

fn build_tool_result_block(tool_call: &ToolCallInfo) -> CommandBlock {
    let mut block = CommandBlock::new(
        BlockType::ToolResult {
            tool_name: tool_call.tool_name.clone(),
            success: true,
        },
        &tool_call.tool_name,
    );

    let content = BlockContent::JsonTree(tool_call.arguments.clone());

    block = block.with_content(content);
    block = block.with_status(BlockStatus::Success);

    block.with_action(BlockAction {
        icon: '↻',
        label: "Retry".to_string(),
        shortcut: Some(KeyBinding {
            key: 'r',
            modifiers: vec![KeyModifier::Ctrl],
        }),
        action_type: ActionType::Retry,
    })
}

fn add_default_actions(block: &mut CommandBlock) {
    block.actions.push(BlockAction {
        icon: '📋',
        label: "Copy".to_string(),
        shortcut: Some(KeyBinding {
            key: 'c',
            modifiers: vec![KeyModifier::Ctrl],
        }),
        action_type: ActionType::Copy,
    });

    if matches!(block.block_type, BlockType::ToolResult { .. }) {
        block.actions.push(BlockAction {
            icon: '↻',
            label: "Retry".to_string(),
            shortcut: Some(KeyBinding {
                key: 'r',
                modifiers: vec![KeyModifier::Ctrl],
            }),
            action_type: ActionType::Retry,
        });
    }

    if matches!(block.block_type, BlockType::Reasoning { .. }) {
        block.actions.push(BlockAction {
            icon: '🔍',
            label: "Search".to_string(),
            shortcut: Some(KeyBinding {
                key: 'f',
                modifiers: vec![KeyModifier::Ctrl],
            }),
            action_type: ActionType::Search,
        });
    }
}

pub fn render_messages_with_blocks(
    messages: &[SimpleMessage],
    area: ratatui::layout::Rect,
    _buf: &mut ratatui::buffer::Buffer,
) -> u16 {
    let mut y = area.y;
    let width = area.width;

    for message in messages {
        let blocks = message_to_command_blocks(message);
        
        for block in blocks {
            let height = block.estimate_height(width);
            if y + height > area.y + area.height {
                break;
            }
            
            y += height;
        }
    }

    y - area.y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_message_conversion() {
        let message = SimpleMessage {
            role: "user".to_string(),
            content: "Hello world".to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
        };

        let blocks = message_to_command_blocks(&message);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0].block_type, BlockType::UserInput));
    }

    #[test]
    fn test_assistant_message_with_code() {
        let message = SimpleMessage {
            role: "assistant".to_string(),
            content: "fn main() {\n    println!(\"Hello\");\n}".to_string(),
            tool_calls: Vec::new(),
            duration_secs: Some(2.5),
            title: Some("gpt-4".to_string()),
        };

        let blocks = message_to_command_blocks(&message);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0].content, BlockContent::Code(_)));
        assert_eq!(blocks[0].duration_ms, Some(2500));
    }

    #[test]
    fn test_tool_call_message() {
        let tool_call = ToolCallInfo {
            tool_name: "edit_file".to_string(),
            arguments: serde_json::json!({"path": "test.txt", "content": "hello"}),
        };

        let message = SimpleMessage {
            role: "assistant".to_string(),
            content: "".to_string(),
            tool_calls: vec![tool_call],
            duration_secs: None,
            title: None,
        };

        let blocks = message_to_command_blocks(&message);
        assert!(blocks.len() >= 1);
        assert!(matches!(blocks[0].block_type, BlockType::ToolCall { .. }));
    }

    #[test]
    fn test_diff_content_detection() {
        let diff_content = "diff --git a/test.txt b/test.txt\n--- a/test.txt\n+++ b/test.txt\n@@ -1,2 +1,2 @@\n-old line\n+new line";
        
        let message = SimpleMessage {
            role: "assistant".to_string(),
            content: diff_content.to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
        };

        let blocks = message_to_command_blocks(&message);
        assert!(matches!(blocks[0].content, BlockContent::Diff(_)));
    }

    #[test]
    fn test_json_content_detection() {
        let json_content = r#"{"name": "test", "value": 42}"#;
        
        let message = SimpleMessage {
            role: "assistant".to_string(),
            content: json_content.to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
        };

        let blocks = message_to_command_blocks(&message);
        assert!(matches!(blocks[0].content, BlockContent::JsonTree(_)));
    }
}