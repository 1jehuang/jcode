use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::path::Path;

pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct EditInput {
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing text. Finds old_string in the file and replaces it with new_string. \
         The old_string must be unique in the file unless replace_all is true. \
         Preserves exact indentation and whitespace."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["file_path", "old_string", "new_string"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to find and replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to replace it with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: EditInput = serde_json::from_value(input)?;

        if params.old_string == params.new_string {
            return Err(anyhow::anyhow!(
                "old_string and new_string must be different"
            ));
        }

        let path = Path::new(&params.file_path);

        if !path.exists() {
            return Err(anyhow::anyhow!("File not found: {}", params.file_path));
        }

        let content = tokio::fs::read_to_string(path).await?;

        // Count occurrences
        let occurrences = content.matches(&params.old_string).count();

        if occurrences == 0 {
            // Try flexible matching
            return try_flexible_match(&content, &params.old_string, &params.file_path);
        }

        if occurrences > 1 && !params.replace_all {
            return Err(anyhow::anyhow!(
                "old_string found {} times in the file. Either:\n\
                 1. Provide more context to make it unique, or\n\
                 2. Set replace_all: true to replace all occurrences",
                occurrences
            ));
        }

        // Perform replacement
        let new_content = if params.replace_all {
            content.replace(&params.old_string, &params.new_string)
        } else {
            content.replacen(&params.old_string, &params.new_string, 1)
        };

        // Find line number where edit starts
        let start_line = find_line_number(&content, &params.old_string);

        // Write back
        tokio::fs::write(path, &new_content).await?;

        // Generate a diff with line numbers
        let diff = generate_diff(&params.old_string, &params.new_string, start_line);

        Ok(ToolOutput::new(format!(
            "Edited {}: replaced {} occurrence(s)\n{}",
            params.file_path, occurrences, diff
        )).with_title(format!("{}", params.file_path)))
    }
}

/// Find the 1-based line number where a substring starts
fn find_line_number(content: &str, substring: &str) -> usize {
    if let Some(pos) = content.find(substring) {
        content[..pos].lines().count() + 1
    } else {
        1
    }
}

/// Generate a diff with line numbers, left-aligned
fn generate_diff(old: &str, new: &str, start_line: usize) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();

    // Track line numbers for old and new content
    let mut old_line = start_line;
    let mut new_line = start_line;

    for change in diff.iter_all_changes() {
        let content = change.value().trim();
        let (prefix, line_num) = match change.tag() {
            ChangeTag::Delete => {
                let num = old_line;
                old_line += 1;
                // Skip whitespace-only changes
                if content.is_empty() {
                    continue;
                }
                ("-", num)
            }
            ChangeTag::Insert => {
                let num = new_line;
                new_line += 1;
                // Skip whitespace-only changes
                if content.is_empty() {
                    continue;
                }
                ("+", num)
            }
            ChangeTag::Equal => {
                old_line += 1;
                new_line += 1;
                continue; // Skip equal lines in output
            }
        };

        output.push_str(&format!("{:>4} ", line_num));
        output.push_str(prefix);
        output.push(' ');
        output.push_str(content);
        output.push('\n');
    }

    if output.is_empty() {
        "(no visible changes)".to_string()
    } else {
        output.trim_end().to_string()
    }
}

fn try_flexible_match(content: &str, old_string: &str, file_path: &str) -> Result<ToolOutput> {
    // Try trimmed matching
    let trimmed = old_string.trim();
    if content.contains(trimmed) && trimmed != old_string {
        return Err(anyhow::anyhow!(
            "old_string not found exactly, but found after trimming whitespace.\n\
             Try using the exact string from the file, including leading/trailing whitespace."
        ));
    }

    // Try line-by-line matching with normalized whitespace
    let old_lines: Vec<&str> = old_string.lines().collect();
    let content_lines: Vec<&str> = content.lines().collect();

    for (i, window) in content_lines.windows(old_lines.len()).enumerate() {
        let matches = window
            .iter()
            .zip(old_lines.iter())
            .all(|(a, b)| a.trim() == b.trim());

        if matches {
            return Err(anyhow::anyhow!(
                "old_string not found exactly, but found with different indentation around line {}.\n\
                 Make sure to preserve the exact whitespace from the file.",
                i + 1
            ));
        }
    }

    Err(anyhow::anyhow!(
        "old_string not found in {}.\n\
         Use the read tool to see the current file contents.",
        file_path
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_diff_single_line_change() {
        let old = "hello world";
        let new = "hello rust";
        let diff = generate_diff(old, new, 10);

        assert!(diff.contains("10 "), "Should have line number 10");
        assert!(diff.contains("- hello world"), "Should show deleted line");
        assert!(diff.contains("+ hello rust"), "Should show added line");
    }

    #[test]
    fn test_generate_diff_multi_line() {
        let old = "line one\nline two\nline three";
        let new = "line one\nmodified two\nline three";
        let diff = generate_diff(old, new, 5);

        // Line 6 should be the changed line (5 + 1 for "line two")
        assert!(diff.contains("6 "), "Should have line number 6");
        assert!(diff.contains("- line two"), "Should show deleted line");
        assert!(diff.contains("+ modified two"), "Should show added line");
        // Equal lines should not appear
        assert!(!diff.contains("line one"), "Should not show unchanged lines");
        assert!(!diff.contains("line three"), "Should not show unchanged lines");
    }

    #[test]
    fn test_generate_diff_addition_only() {
        let old = "first\nthird";
        let new = "first\nsecond\nthird";
        let diff = generate_diff(old, new, 1);

        assert!(diff.contains("+ second"), "Should show added line");
    }

    #[test]
    fn test_generate_diff_deletion_only() {
        let old = "first\nsecond\nthird";
        let new = "first\nthird";
        let diff = generate_diff(old, new, 1);

        assert!(diff.contains("- second"), "Should show deleted line");
    }

    #[test]
    fn test_generate_diff_no_changes() {
        let old = "same content";
        let new = "same content";
        let diff = generate_diff(old, new, 1);

        assert_eq!(diff, "(no visible changes)");
    }

    #[test]
    fn test_generate_diff_line_number_format() {
        let old = "old";
        let new = "new";
        let diff = generate_diff(old, new, 42);

        // Line numbers should be right-aligned in 4 chars
        assert!(diff.contains("  42 -"), "Line number should be right-aligned");
        assert!(diff.contains("  42 +"), "Line number should be right-aligned");
    }

    #[test]
    fn test_find_line_number() {
        let content = "line 1\nline 2\nline 3\nline 4";

        assert_eq!(find_line_number(content, "line 1"), 1);
        assert_eq!(find_line_number(content, "line 2"), 2);
        assert_eq!(find_line_number(content, "line 3"), 3);
        assert_eq!(find_line_number(content, "line 4"), 4);
        assert_eq!(find_line_number(content, "not found"), 1);
    }
}
