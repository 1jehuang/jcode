use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

pub struct ApplyPatchTool;

impl ApplyPatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct ApplyPatchInput {
    patch_text: String,
}

struct UpdatePatch {
    path: String,
    move_to: Option<String>,
    lines: Vec<String>,
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a patch in the Codex apply_patch format (*** Begin Patch). Supports add, update, and delete operations."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["patch_text"],
            "properties": {
                "patch_text": {
                    "type": "string",
                    "description": "Patch text in apply_patch format"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: ApplyPatchInput = serde_json::from_value(input)?;
        let parsed = parse_apply_patch(&params.patch_text)?;

        let mut unified = String::new();
        for patch in &parsed.updates {
            unified.push_str(&format!("--- a/{}\n", patch.path));
            unified.push_str(&format!("+++ b/{}\n", patch.path));
            for line in &patch.lines {
                unified.push_str(line);
                unified.push('\n');
            }
        }

        for (path, lines) in &parsed.adds {
            let added_lines: Vec<String> = lines
                .iter()
                .map(|line| {
                    if let Some(stripped) = line.strip_prefix('+') {
                        format!("+{}", stripped)
                    } else if line.starts_with("+++") || line.starts_with("---") {
                        format!("+{}", line)
                    } else {
                        format!("+{}", line)
                    }
                })
                .collect();

            let count = added_lines.len();
            unified.push_str("--- /dev/null\n");
            unified.push_str(&format!("+++ b/{}\n", path));
            unified.push_str(&format!("@@ -0,0 +1,{} @@\n", count));
            for line in &added_lines {
                unified.push_str(line);
                unified.push('\n');
            }
        }

        let mut results = Vec::new();
        if !unified.trim().is_empty() {
            let patch_tool = super::patch::PatchTool::new();
            let patch_result = patch_tool
                .execute(
                    json!({"patch_text": unified}),
                    ctx.for_subcall("patch".to_string()),
                )
                .await?;
            results.push(patch_result.output);
        }

        for path in &parsed.deletes {
            let display = path.clone();
            let resolved = ctx.resolve_path(Path::new(path));
            if std::fs::remove_file(&resolved).is_ok() {
                results.push(format!("✓ {}: deleted", display));
            } else {
                results.push(format!("✗ {}: failed to delete", display));
            }
        }

        for patch in &parsed.updates {
            if let Some(move_to) = &patch.move_to {
                let from = ctx.resolve_path(Path::new(&patch.path));
                let to = ctx.resolve_path(Path::new(move_to));
                if let Err(err) = std::fs::rename(&from, &to) {
                    results.push(format!("✗ {}: move failed ({})", patch.path, err));
                } else {
                    results.push(format!("✓ {}: moved to {}", patch.path, move_to));
                }
            }
        }

        if results.is_empty() {
            Ok(ToolOutput::new("No changes applied"))
        } else {
            Ok(ToolOutput::new(results.join("\n")))
        }
    }
}

struct ParsedApplyPatch {
    updates: Vec<UpdatePatch>,
    adds: Vec<(String, Vec<String>)>,
    deletes: Vec<String>,
}

fn parse_apply_patch(input: &str) -> Result<ParsedApplyPatch> {
    let lines: Vec<&str> = input.lines().collect();
    if lines.is_empty() || lines[0].trim() != "*** Begin Patch" {
        anyhow::bail!("Patch must start with *** Begin Patch");
    }

    let mut updates = Vec::new();
    let mut adds = Vec::new();
    let mut deletes = Vec::new();

    let mut i = 1;
    while i < lines.len() {
        let line = lines[i].trim_end();
        if line.trim() == "*** End Patch" {
            break;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let path = path.trim().to_string();
            i += 1;

            let mut move_to = None;
            if i < lines.len() {
                if let Some(target) = lines[i].trim_end().strip_prefix("*** Move to: ") {
                    move_to = Some(target.trim().to_string());
                    i += 1;
                }
            }

            let mut content = Vec::new();
            while i < lines.len() {
                let current = lines[i].trim_end();
                if current.starts_with("*** ") {
                    break;
                }
                content.push(current.to_string());
                i += 1;
            }

            updates.push(UpdatePatch {
                path,
                move_to,
                lines: content,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let path = path.trim().to_string();
            i += 1;
            let mut content = Vec::new();
            while i < lines.len() {
                let current = lines[i].trim_end();
                if current.starts_with("*** ") {
                    break;
                }
                content.push(current.to_string());
                i += 1;
            }
            adds.push((path, content));
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            deletes.push(path.trim().to_string());
            i += 1;
            continue;
        }

        i += 1;
    }

    if updates.is_empty() && adds.is_empty() && deletes.is_empty() {
        anyhow::bail!("No valid patch directives found");
    }

    Ok(ParsedApplyPatch {
        updates,
        adds,
        deletes,
    })
}
