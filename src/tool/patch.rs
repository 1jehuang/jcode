use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::path::Path;

pub struct PatchTool;

impl PatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct PatchInput {
    patch_text: String,
}

#[derive(Debug)]
struct FilePatch {
    path: String,
    hunks: Vec<Hunk>,
    is_new: bool,
    is_delete: bool,
}

#[derive(Debug)]
struct Hunk {
    old_start: usize,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to modify files. Supports adding, updating, and deleting files. \
         The patch should be in standard unified diff format (like output from git diff)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["patch_text"],
            "properties": {
                "patch_text": {
                    "type": "string",
                    "description": "The unified diff patch text to apply"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: PatchInput = serde_json::from_value(input)?;

        let patches = parse_patch(&params.patch_text)?;

        if patches.is_empty() {
            return Err(anyhow::anyhow!("No valid patches found in input"));
        }

        let mut results = Vec::new();

        for patch in patches {
            let resolved_path = ctx.resolve_path(&patch.path);
            let result = apply_patch_with_diff(&patch, &resolved_path).await;
            match result {
                Ok((msg, diff)) => {
                    if diff.is_empty() {
                        results.push(format!("✓ {}: {}", patch.path, msg));
                    } else {
                        results.push(format!("✓ {}: {}\n{}", patch.path, msg, diff));
                    }
                }
                Err(e) => results.push(format!("✗ {}: {}", patch.path, e)),
            }
        }

        Ok(ToolOutput::new(results.join("\n\n")))
    }
}

fn parse_patch(text: &str) -> Result<Vec<FilePatch>> {
    let mut patches = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        // Look for --- line
        if lines[i].starts_with("---") {
            let old_file = lines[i]
                .strip_prefix("--- ")
                .unwrap_or("")
                .split('\t')
                .next()
                .unwrap_or("");

            i += 1;
            if i >= lines.len() || !lines[i].starts_with("+++") {
                continue;
            }

            let new_file = lines[i]
                .strip_prefix("+++ ")
                .unwrap_or("")
                .split('\t')
                .next()
                .unwrap_or("");

            // Determine the actual file path
            let path = if new_file == "/dev/null" {
                old_file.strip_prefix("a/").unwrap_or(old_file).to_string()
            } else {
                new_file.strip_prefix("b/").unwrap_or(new_file).to_string()
            };

            let is_new = old_file == "/dev/null";
            let is_delete = new_file == "/dev/null";

            i += 1;

            // Parse hunks
            let mut hunks = Vec::new();
            while i < lines.len() && !lines[i].starts_with("---") {
                if lines[i].starts_with("@@") {
                    if let Some(hunk) = parse_hunk(&lines, &mut i) {
                        hunks.push(hunk);
                    }
                } else {
                    i += 1;
                }
            }

            if !hunks.is_empty() || is_new || is_delete {
                patches.push(FilePatch {
                    path,
                    hunks,
                    is_new,
                    is_delete,
                });
            }
        } else {
            i += 1;
        }
    }

    Ok(patches)
}

fn parse_hunk(lines: &[&str], i: &mut usize) -> Option<Hunk> {
    // Parse @@ -start,count +start,count @@
    let header = lines[*i];
    let parts: Vec<&str> = header.split_whitespace().collect();

    if parts.len() < 3 {
        *i += 1;
        return None;
    }

    let old_range = parts[1].strip_prefix('-').unwrap_or(parts[1]);
    let old_start: usize = old_range
        .split(',')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    *i += 1;

    let mut old_lines = Vec::new();
    let mut new_lines = Vec::new();

    while *i < lines.len() {
        let line = lines[*i];

        if line.starts_with("@@") || line.starts_with("---") {
            break;
        }

        if let Some(content) = line.strip_prefix('-') {
            old_lines.push(content.to_string());
        } else if let Some(content) = line.strip_prefix('+') {
            new_lines.push(content.to_string());
        } else if let Some(content) = line.strip_prefix(' ') {
            old_lines.push(content.to_string());
            new_lines.push(content.to_string());
        } else if line.is_empty() || line == "\\ No newline at end of file" {
            // Context line or special marker
        }

        *i += 1;
    }

    Some(Hunk {
        old_start,
        old_lines,
        new_lines,
    })
}

/// Apply a patch and return (status_message, diff_output)
async fn apply_patch_with_diff(patch: &FilePatch, path: &Path) -> Result<(String, String)> {
    // Handle deletion
    if patch.is_delete {
        if path.exists() {
            let old_content = tokio::fs::read_to_string(path).await.unwrap_or_default();
            tokio::fs::remove_file(path).await?;
            let diff = generate_diff(&old_content, "", 1);
            return Ok(("deleted".to_string(), diff));
        } else {
            return Err(anyhow::anyhow!("file does not exist"));
        }
    }

    // Handle new file
    if patch.is_new {
        if path.exists() {
            return Err(anyhow::anyhow!("file already exists"));
        }

        // Create parent directories
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Collect all new lines from hunks
        let content: String = patch
            .hunks
            .iter()
            .flat_map(|h| h.new_lines.iter())
            .map(|l| format!("{}\n", l))
            .collect();

        tokio::fs::write(path, &content).await?;
        let diff = generate_diff("", &content, 1);
        return Ok(("created".to_string(), diff));
    }

    // Handle modification
    if !path.exists() {
        return Err(anyhow::anyhow!("file does not exist"));
    }

    let old_content = tokio::fs::read_to_string(path).await?;
    let mut lines: Vec<String> = old_content.lines().map(|s| s.to_string()).collect();

    // Find the first affected line for diff context
    let first_line = patch.hunks.iter().map(|h| h.old_start).min().unwrap_or(1);

    // Apply hunks in reverse order to preserve line numbers
    for hunk in patch.hunks.iter().rev() {
        let start = hunk.old_start.saturating_sub(1);
        let end = (start + hunk.old_lines.len()).min(lines.len());

        // Remove old lines and insert new ones
        lines.splice(start..end, hunk.new_lines.iter().cloned());
    }

    let new_content = lines.join("\n") + "\n";
    tokio::fs::write(path, &new_content).await?;

    let diff = generate_diff(&old_content, &new_content, first_line);
    Ok((format!("modified ({} hunks)", patch.hunks.len()), diff))
}

/// Generate a compact diff with line numbers (max 30 lines)
fn generate_diff(old: &str, new: &str, start_line: usize) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();
    let mut line_count = 0;
    const MAX_LINES: usize = 30;

    let mut old_line = start_line;
    let mut new_line = start_line;

    for change in diff.iter_all_changes() {
        if line_count >= MAX_LINES {
            output.push_str("... (diff truncated)\n");
            break;
        }

        let content = change.value().trim_end_matches('\n');
        let (prefix, line_num) = match change.tag() {
            ChangeTag::Delete => {
                let num = old_line;
                old_line += 1;
                if content.trim().is_empty() {
                    continue;
                }
                ("-", num)
            }
            ChangeTag::Insert => {
                let num = new_line;
                new_line += 1;
                if content.trim().is_empty() {
                    continue;
                }
                ("+", num)
            }
            ChangeTag::Equal => {
                old_line += 1;
                new_line += 1;
                continue;
            }
        };

        output.push_str(&format!("{}{} {}\n", line_num, prefix, content));
        line_count += 1;
    }

    output.trim_end().to_string()
}
