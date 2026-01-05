use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

const MAX_RESULTS: usize = 100;
const MAX_LINE_LEN: usize = 2000;

pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct GrepInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    include: Option<String>,
}

struct GrepResult {
    file: String,
    line_num: usize,
    line: String,
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern in files. Returns matching lines with file paths and line numbers. \
         Respects .gitignore and skips binary files."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "include": {
                    "type": "string",
                    "description": "File pattern to include (e.g., '*.rs', '*.{ts,tsx}')"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: GrepInput = serde_json::from_value(input)?;

        let regex = Regex::new(&params.pattern)?;
        let base_path = params.path.as_deref().unwrap_or(".");
        let base = Path::new(base_path);

        if !base.exists() {
            return Err(anyhow::anyhow!("Directory not found: {}", base_path));
        }

        let include_pattern = params
            .include
            .as_ref()
            .map(|p| glob::Pattern::new(p))
            .transpose()?;

        let mut results: Vec<GrepResult> = Vec::new();

        let walker = ignore::WalkBuilder::new(base)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();

            // Skip directories
            if path.is_dir() {
                continue;
            }

            // Check include pattern
            if let Some(ref pattern) = include_pattern {
                let filename = path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default();
                if !pattern.matches(&filename) {
                    continue;
                }
            }

            // Skip binary files
            if is_binary_extension(path) {
                continue;
            }

            // Read and search file
            if let Ok(content) = std::fs::read_to_string(path) {
                for (line_num, line) in content.lines().enumerate() {
                    if regex.is_match(line) {
                        let relative = path
                            .strip_prefix(base)
                            .unwrap_or(path)
                            .display()
                            .to_string();

                        let truncated = if line.len() > MAX_LINE_LEN {
                            format!("{}...", &line[..MAX_LINE_LEN])
                        } else {
                            line.to_string()
                        };

                        results.push(GrepResult {
                            file: relative,
                            line_num: line_num + 1,
                            line: truncated,
                        });

                        if results.len() >= MAX_RESULTS {
                            break;
                        }
                    }
                }
            }

            if results.len() >= MAX_RESULTS {
                break;
            }
        }

        // Format output grouped by file
        let mut output = String::new();
        output.push_str(&format!(
            "Found {} matches for '{}'\n\n",
            results.len(),
            params.pattern
        ));

        let mut current_file = String::new();
        for result in &results {
            if result.file != current_file {
                if !current_file.is_empty() {
                    output.push('\n');
                }
                output.push_str(&format!("{}:\n", result.file));
                current_file = result.file.clone();
            }
            output.push_str(&format!("  {:>4}: {}\n", result.line_num, result.line));
        }

        if results.len() >= MAX_RESULTS {
            output.push_str(&format!(
                "\n... results truncated at {} matches",
                MAX_RESULTS
            ));
        }

        Ok(ToolOutput::new(output))
    }
}

fn is_binary_extension(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        let ext = ext.to_string_lossy().to_lowercase();
        let binary_exts = [
            "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "pdf", "zip", "tar", "gz", "bz2",
            "xz", "7z", "rar", "exe", "dll", "so", "dylib", "o", "a", "class", "pyc", "wasm",
            "mp3", "mp4", "avi", "mov", "mkv", "flac", "ogg", "wav", "ttf", "woff", "woff2",
        ];
        return binary_exts.contains(&ext.as_str());
    }
    false
}
