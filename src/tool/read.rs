use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

const DEFAULT_LIMIT: usize = 2000;
const MAX_LINE_LEN: usize = 2000;

pub struct ReadTool;

impl ReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct ReadInput {
    file_path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns lines with line numbers. \
         Supports reading specific ranges with offset and limit parameters. \
         Can read text files, and will indicate if a file is binary."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["file_path"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path to the file to read (absolute or relative)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (default 2000)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: ReadInput = serde_json::from_value(input)?;

        let path = Path::new(&params.file_path);

        // Check if file exists
        if !path.exists() {
            // Try to find similar files
            let suggestions = find_similar_files(&params.file_path);
            if suggestions.is_empty() {
                return Err(anyhow::anyhow!("File not found: {}", params.file_path));
            } else {
                return Err(anyhow::anyhow!(
                    "File not found: {}\nDid you mean: {}",
                    params.file_path,
                    suggestions.join(", ")
                ));
            }
        }

        // Check for binary files
        if is_binary_file(path) {
            return Ok(ToolOutput::new(format!(
                "Binary file detected: {}\nUse appropriate tools to handle binary files.",
                params.file_path
            )));
        }

        // Read file
        let content = tokio::fs::read_to_string(path).await?;
        let lines: Vec<&str> = content.lines().collect();

        let offset = params.offset.unwrap_or(0);
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT);

        let total_lines = lines.len();
        let end = (offset + limit).min(total_lines);

        let mut output = String::new();

        for (i, line) in lines.iter().enumerate().skip(offset).take(limit) {
            let line_num = i + 1; // 1-based line numbers
            let truncated = if line.len() > MAX_LINE_LEN {
                format!("{}...", &line[..MAX_LINE_LEN])
            } else {
                line.to_string()
            };
            output.push_str(&format!("{:>5}\t{}\n", line_num, truncated));
        }

        // Add metadata
        if end < total_lines {
            output.push_str(&format!(
                "\n... {} more lines (use offset={} to continue)\n",
                total_lines - end,
                end
            ));
        }

        if output.is_empty() {
            Ok(ToolOutput::new("(empty file)"))
        } else {
            Ok(ToolOutput::new(output))
        }
    }
}

fn is_binary_file(path: &Path) -> bool {
    // Check by extension first
    if let Some(ext) = path.extension() {
        let ext = ext.to_string_lossy().to_lowercase();
        let binary_exts = [
            "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "pdf", "zip", "tar", "gz", "bz2",
            "xz", "7z", "rar", "exe", "dll", "so", "dylib", "o", "a", "class", "pyc", "wasm",
            "mp3", "mp4", "avi", "mov", "mkv", "flac", "ogg", "wav",
        ];
        if binary_exts.contains(&ext.as_str()) {
            return true;
        }
    }

    // Check first bytes for binary content
    if let Ok(bytes) = std::fs::read(path) {
        let check_len = bytes.len().min(8192);
        let null_count = bytes[..check_len].iter().filter(|&&b| b == 0).count();
        return null_count > check_len / 10; // More than 10% null bytes = binary
    }

    false
}

fn find_similar_files(path: &str) -> Vec<String> {
    let path = Path::new(path);
    let parent = path.parent().unwrap_or(Path::new("."));
    let filename = path.file_name().map(|s| s.to_string_lossy().to_lowercase());

    let mut suggestions = Vec::new();

    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if let Some(ref target) = filename {
                // Simple similarity check
                let target_str: &str = target.as_ref();
                if name.contains(target_str) || target_str.contains(&name as &str) {
                    suggestions.push(entry.path().display().to_string());
                    if suggestions.len() >= 3 {
                        break;
                    }
                }
            }
        }
    }

    suggestions
}
