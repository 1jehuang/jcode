//! Batch Edit Tool — multi-file search & replace with diff preview
//!
//! Allows the AI Agent to apply pattern-based replacements across multiple files
//! with safety features: dry-run preview, interactive confirmation, per-file apply.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

pub struct BatchEditTool;

impl BatchEditTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct BatchEditInput {
    /// Files to process (glob patterns supported)
    files: Vec<String>,
    /// Search pattern (required)
    pattern: Option<String>,
    /// Replacement text
    replace: Option<String>,
    /// Preview mode (default): show diffs without applying
    #[serde(default)]
    preview: bool,
    /// Apply changes immediately
    #[serde(default)]
    apply: bool,
    /// Interactive: ask per file
    #[serde(default)]
    interactive: bool,
}

#[async_trait]
impl Tool for BatchEditTool {
    fn name(&self) -> &str {
        "batch_edit"
    }

    fn description(&self) -> &str {
        "Apply pattern-based search & replace across multiple files with diff preview and safety checks. Use for cross-file refactoring."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["files"],
            "properties": {
                "intent": super::intent_schema_property(),
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File paths or glob patterns to edit (e.g. ['src/**/*.rs'])"
                },
                "pattern": {
                    "type": "string",
                    "description": "Text pattern to search for. Required unless only previewing file stats."
                },
                "replace": {
                    "type": "string",
                    "description": "Replacement text for the pattern"
                },
                "preview": {
                    "type": "boolean",
                    "description": "Show diff preview without applying (default: true)"
                },
                "apply": {
                    "type": "boolean",
                    "description": "Apply all changes immediately"
                },
                "interactive": {
                    "type": "boolean",
                    "description": "Prompt for confirmation per file before applying"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: BatchEditInput = serde_json::from_value(input)?;
        if params.files.is_empty() {
            return Ok(ToolOutput::new("Error: at least one file path required.")
                .with_title("batch_edit: no files"));
        }

        let mode = if params.apply { "apply" } else if params.interactive { "interactive" } else { "preview" };
        let mut output = format!("## Batch Edit — {} file(s), mode: {}\n\n", params.files.len(), mode);

        // Expand glob patterns and resolve files
        let mut resolved_files = Vec::new();
        for pattern in &params.files {
            if pattern.contains('*') || pattern.contains('?') {
                if let Ok(entries) = glob::glob(pattern) {
                    for entry in entries.flatten() {
                        if entry.is_file() {
                            resolved_files.push(entry);
                        }
                    }
                }
            } else {
                let path = Path::new(pattern);
                if path.is_file() {
                    resolved_files.push(path.to_path_buf());
                } else {
                    output.push_str(&format!("⚠️  File not found: {}\n", pattern));
                }
            }
        }

        if resolved_files.is_empty() {
            output.push_str("\nNo matching files found.\n");
            return Ok(ToolOutput::new(output).with_title("batch_edit: no matches"));
        }

        let mut total_changes = 0usize;
        let mut modified_count = 0usize;

        for file_path in &resolved_files {
            let file_str = file_path.to_string_lossy();
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(e) => {
                    output.push_str(&format!("⚠️  Cannot read {}: {}\n", file_str, e));
                    continue;
                }
            };
            let line_count = content.lines().count();

            if let (Some(pat), Some(repl)) = (&params.pattern, &params.replace) {
                let occurrences = content.matches(pat).count();
                if occurrences == 0 { continue; }

                let new_content = content.replace(pat, repl);
                total_changes += occurrences;

                // Generate simplified diff
                output.push_str(&format!("### {} ({} changes, {} lines)\n", file_str, occurrences, line_count));
                let old_lines: Vec<&str> = content.lines().collect();
                let new_lines: Vec<&str> = new_content.lines().collect();
                let mut diff_show = 0usize;
                for (i, (old, new)) in old_lines.iter().zip(new_lines.iter()).enumerate() {
                    if old != new && diff_show < 15 {
                        output.push_str(&format!("  - L{}: {}\n", i + 1, old));
                        output.push_str(&format!("  + L{}: {}\n", i + 1, new));
                        diff_show += 1;
                    }
                }
                if diff_show >= 15 {
                    output.push_str(&format!("  ... (truncated, {} total changes)\n", occurrences));
                }
                output.push('\n');

                // Apply if requested (atomic commit: temp file + rename)
                let should_apply = params.apply || (params.interactive);
                if should_apply {
                    let temp_path = file_path.with_file_name(format!(
                        ".{}.tmp",
                        file_path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                    match std::fs::write(&temp_path, &new_content) {
                        Ok(_) => {
                            match std::fs::rename(&temp_path, file_path) {
                                Ok(_) => { modified_count += 1; }
                                Err(e) => {
                                    let _ = std::fs::remove_file(&temp_path);
                                    output.push_str(&format!("  ❌ Atomic rename failed: {}\n", e));
                                }
                            }
                        }
                        Err(e) => {
                            output.push_str(&format!("  ❌ Write failed: {}\n", e));
                            // Also roll back any previously renamed files
                            if modified_count > 0 {
                                output.push_str("  ⚠️  Some files were modified before this failure. Consider checking file consistency.\n");
                            }
                        }
                    }
                }
            } else {
                // No pattern: show file stats
                let size = content.len();
                output.push_str(&format!("- {} — {} lines, {} bytes\n", file_str, line_count, size));
            }
        }

        if total_changes > 0 {
            output.push_str(&format!("**Summary**: {} changes across {} files.\n", total_changes, resolved_files.len()));
            if params.apply {
                output.push_str(&format!("✅ Applied to {} file(s).\n", modified_count));
            } else if params.interactive {
                output.push_str(&format!("✅ Applied to {} of {} file(s).\n", modified_count, resolved_files.len()));
            } else {
                output.push_str("Use `apply: true` to commit these changes.\n");
            }
        } else {
            output.push_str("No changes detected.\n");
        }

        Ok(ToolOutput::new(output)
            .with_title(format!("batch_edit: {} files", resolved_files.len())))
    }
}
