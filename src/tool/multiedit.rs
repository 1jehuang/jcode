use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::path::Path;

pub struct MultiEditTool;

impl MultiEditTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct MultiEditInput {
    file_path: String,
    edits: Vec<EditOperation>,
}

#[derive(Deserialize)]
struct EditOperation {
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for MultiEditTool {
    fn name(&self) -> &str {
        "multiedit"
    }

    fn description(&self) -> &str {
        "Apply multiple edits to a single file sequentially. Each edit replaces old_string with new_string. \
         Edits are applied in order, so later edits see the result of earlier ones."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["file_path", "edits"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path to the file to edit"
                },
                "edits": {
                    "type": "array",
                    "description": "Array of edit operations to apply sequentially",
                    "items": {
                        "type": "object",
                        "required": ["old_string", "new_string"],
                        "properties": {
                            "old_string": {
                                "type": "string",
                                "description": "The text to find and replace"
                            },
                            "new_string": {
                                "type": "string",
                                "description": "The replacement text"
                            },
                            "replace_all": {
                                "type": "boolean",
                                "description": "Replace all occurrences (default: false)"
                            }
                        }
                    },
                    "minItems": 1
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: MultiEditInput = serde_json::from_value(input)?;

        let path = Path::new(&params.file_path);

        if !path.exists() {
            return Err(anyhow::anyhow!("File not found: {}", params.file_path));
        }

        let original_content = tokio::fs::read_to_string(path).await?;
        let mut content = original_content.clone();
        let mut applied = Vec::new();
        let mut failed = Vec::new();

        for (i, edit) in params.edits.iter().enumerate() {
            if edit.old_string == edit.new_string {
                failed.push(format!("Edit {}: old_string equals new_string", i + 1));
                continue;
            }

            let occurrences = content.matches(&edit.old_string).count();

            if occurrences == 0 {
                failed.push(format!("Edit {}: old_string not found", i + 1));
                continue;
            }

            if occurrences > 1 && !edit.replace_all {
                failed.push(format!(
                    "Edit {}: found {} occurrences, use replace_all or be more specific",
                    i + 1,
                    occurrences
                ));
                continue;
            }

            // Apply the edit
            if edit.replace_all {
                content = content.replace(&edit.old_string, &edit.new_string);
                applied.push(format!("Edit {}: replaced {} occurrences", i + 1, occurrences));
            } else {
                content = content.replacen(&edit.old_string, &edit.new_string, 1);
                applied.push(format!("Edit {}: replaced 1 occurrence", i + 1));
            }
        }

        // Write the result
        tokio::fs::write(path, &content).await?;

        // Format output
        let mut output = format!("Edited {}\n\n", params.file_path);

        if !applied.is_empty() {
            output.push_str("Applied:\n");
            for msg in &applied {
                output.push_str(&format!("  ✓ {}\n", msg));
            }
        }

        if !failed.is_empty() {
            output.push_str("\nFailed:\n");
            for msg in &failed {
                output.push_str(&format!("  ✗ {}\n", msg));
            }
        }

        output.push_str(&format!(
            "\nTotal: {} applied, {} failed\n",
            applied.len(),
            failed.len()
        ));

        // Generate diff summary
        if !applied.is_empty() {
            output.push_str("\nDiff:\n");
            output.push_str(&generate_diff_summary(&original_content, &content));
        }

        Ok(ToolOutput::new(output).with_title(format!("{}", params.file_path)))
    }
}

/// Generate a compact diff summary with line numbers (max 30 lines shown)
fn generate_diff_summary(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();
    let mut lines_shown = 0;
    const MAX_LINES: usize = 30;

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

    output
}
