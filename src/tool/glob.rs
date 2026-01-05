use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

const MAX_RESULTS: usize = 100;

pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct GlobInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Supports patterns like '**/*.rs', 'src/**/*.ts', etc. \
         Returns files sorted by modification time (newest first)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match (e.g., '**/*.rs', 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "Base directory to search in (default: current directory)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: GlobInput = serde_json::from_value(input)?;

        let base_path = params.path.as_deref().unwrap_or(".");
        let base = Path::new(base_path);

        if !base.exists() {
            return Err(anyhow::anyhow!("Directory not found: {}", base_path));
        }

        // Combine base path with pattern
        let full_pattern = if params.pattern.starts_with('/') {
            params.pattern.clone()
        } else {
            format!("{}/{}", base_path, params.pattern)
        };

        // Use ignore crate for gitignore-aware globbing
        let mut results: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();

        let walker = ignore::WalkBuilder::new(base)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        let glob_pattern = glob::Pattern::new(&params.pattern)?;

        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();

            // Skip directories
            if path.is_dir() {
                continue;
            }

            // Check if matches the pattern
            let relative = path.strip_prefix(base).unwrap_or(path);
            let path_str = relative.to_string_lossy();

            if glob_pattern.matches(&path_str) || glob_pattern.matches_path(relative) {
                if let Ok(meta) = path.metadata() {
                    if let Ok(mtime) = meta.modified() {
                        results.push((path.to_path_buf(), mtime));
                    }
                }
            }

            // Limit results
            if results.len() >= MAX_RESULTS * 2 {
                break;
            }
        }

        // Sort by modification time (newest first)
        results.sort_by(|a, b| b.1.cmp(&a.1));

        // Take top results
        let truncated = results.len() > MAX_RESULTS;
        results.truncate(MAX_RESULTS);

        // Format output
        let mut output = String::new();
        output.push_str(&format!(
            "Found {} files matching '{}' in {}\n\n",
            results.len(),
            params.pattern,
            base_path
        ));

        for (path, _) in &results {
            let display = path
                .strip_prefix(base)
                .unwrap_or(path)
                .display()
                .to_string();
            output.push_str(&display);
            output.push('\n');
        }

        if truncated {
            output.push_str(&format!(
                "\n... results truncated (showing {} of more)",
                MAX_RESULTS
            ));
        }

        Ok(ToolOutput::new(output))
    }
}
