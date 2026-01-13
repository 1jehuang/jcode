use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::path::Path;

pub struct WriteTool;

impl WriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct WriteInput {
    file_path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, \
         or overwrites if it does. Creates parent directories as needed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["file_path", "content"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path to the file to write (absolute or relative)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: WriteInput = serde_json::from_value(input)?;

        let path = Path::new(&params.file_path);

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        // Check if file existed before and read old content for diff
        let existed = path.exists();
        let old_content = if existed {
            tokio::fs::read_to_string(path).await.ok()
        } else {
            None
        };

        // Write the file
        tokio::fs::write(path, &params.content).await?;

        let new_len = params.content.len();
        let line_count = params.content.lines().count();

        if existed {
            let diff = if let Some(ref old) = old_content {
                generate_diff_summary(old, &params.content)
            } else {
                String::new()
            };
            Ok(ToolOutput::new(format!(
                "Updated {} ({} lines){}\n{}",
                params.file_path,
                line_count,
                if diff.is_empty() { "" } else { ":" },
                diff
            )).with_title(format!("{}", params.file_path)))
        } else {
            Ok(ToolOutput::new(format!(
                "Created {} ({} bytes, {} lines)",
                params.file_path, new_len, line_count
            )).with_title(format!("{}", params.file_path)))
        }
    }
}

/// Generate a compact diff summary with line numbers (max 20 lines shown)
fn generate_diff_summary(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();
    let mut lines_shown = 0;
    const MAX_LINES: usize = 20;

    let mut old_line = 1usize;
    let mut new_line = 1usize;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                old_line += 1;
                new_line += 1;
                continue;
            }
            ChangeTag::Delete => {
                if lines_shown >= MAX_LINES {
                    output.push_str("...(truncated)\n");
                    break;
                }
                output.push_str(&format!("{:>4} - {}\n", old_line, change.value().trim_end()));
                old_line += 1;
                lines_shown += 1;
            }
            ChangeTag::Insert => {
                if lines_shown >= MAX_LINES {
                    output.push_str("...(truncated)\n");
                    break;
                }
                output.push_str(&format!("{:>4} + {}\n", new_line, change.value().trim_end()));
                new_line += 1;
                lines_shown += 1;
            }
        }
    }

    output.trim_end().to_string()
}
