use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, SidePanelUpdated};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

pub struct SidePanelTool;

impl SidePanelTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct SidePanelInput {
    action: String,
    #[serde(default)]
    page_id: Option<String>,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    focus: Option<bool>,
}

#[async_trait]
impl Tool for SidePanelTool {
    fn name(&self) -> &str {
        "side_panel"
    }

    fn description(&self) -> &str {
        "Manage side panel pages."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["status", "write", "append", "load", "focus", "delete"],
                    "description": "Action."
                },
                "page_id": {
                    "type": "string",
                    "description": "Page ID."
                },
                "file_path": {
                    "type": "string",
                    "description": "File path."
                },
                "title": {
                    "type": "string",
                    "description": "Page title."
                },
                "content": {
                    "type": "string",
                    "description": "Page content."
                },
                "focus": {
                    "type": "boolean",
                    "description": "Focus the page."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: SidePanelInput = serde_json::from_value(input)?;
        let focus = params.focus.unwrap_or(true);

        let snapshot = match params.action.as_str() {
            "status" => crate::side_panel::snapshot_for_session(&ctx.session_id)?,
            "write" => crate::side_panel::write_markdown_page(
                &ctx.session_id,
                params
                    .page_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("page_id is required for write"))?,
                params.title.as_deref(),
                params
                    .content
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("content is required for write"))?,
                focus,
            )?,
            "append" => crate::side_panel::append_markdown_page(
                &ctx.session_id,
                params
                    .page_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("page_id is required for append"))?,
                params.title.as_deref(),
                params
                    .content
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("content is required for append"))?,
                focus,
            )?,
            "load" => {
                let file_path = params
                    .file_path
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("file_path is required for load"))?;
                let resolved = ctx.resolve_path(Path::new(file_path));
                let page_id = params
                    .page_id
                    .clone()
                    .unwrap_or_else(|| derive_page_id(&resolved));
                let title = params.title.clone().or_else(|| {
                    resolved
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                });
                crate::side_panel::load_markdown_file(
                    &ctx.session_id,
                    &page_id,
                    title.as_deref(),
                    &resolved,
                    focus,
                )?
            }
            "focus" => crate::side_panel::focus_page(
                &ctx.session_id,
                params
                    .page_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("page_id is required for focus"))?,
            )?,
            "delete" => crate::side_panel::delete_page(
                &ctx.session_id,
                params
                    .page_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("page_id is required for delete"))?,
            )?,
            other => anyhow::bail!("unknown side_panel action: {}", other),
        };

        if params.action != "status" {
            Bus::global().publish(BusEvent::SidePanelUpdated(SidePanelUpdated {
                session_id: ctx.session_id.clone(),
                snapshot: snapshot.clone(),
            }));
        }

        Ok(ToolOutput::new(crate::side_panel::status_output(&snapshot))
            .with_title("side_panel")
            .with_metadata(serde_json::to_value(&snapshot)?))
    }
}

fn derive_page_id(path: &Path) -> String {
    let raw = path
        .file_stem()
        .or_else(|| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "page".to_string());

    let mut page_id = String::new();
    let mut prev_dash = false;
    for ch in raw.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || matches!(lower, '_' | '.') {
            page_id.push(lower);
            prev_dash = false;
        } else if !prev_dash {
            page_id.push('-');
            prev_dash = true;
        }
    }

    let page_id = page_id.trim_matches('-').trim_matches('.').to_string();
    if page_id.is_empty() {
        "page".to_string()
    } else {
        page_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn side_panel_tool_writes_page() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let tool = SidePanelTool::new();
        let output = tool
            .execute(
                json!({
                    "action": "write",
                    "page_id": "notes",
                    "title": "Notes",
                    "content": "# Notes"
                }),
                ToolContext {
                    session_id: "ses_side_panel_tool".to_string(),
                    message_id: "msg1".to_string(),
                    tool_call_id: "tool1".to_string(),
                    working_dir: None,
                    stdin_request_tx: None,
                    graceful_shutdown_signal: None,
                    execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
                },
            )
            .await
            .expect("tool execute");

        assert!(output.output.contains("notes"));

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[tokio::test]
    async fn side_panel_tool_loads_file_with_derived_page_id() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        let doc_path = temp.path().join("Project Plan.md");
        std::fs::write(&doc_path, "# Plan\n\nInitial").expect("write source file");

        let tool = SidePanelTool::new();
        let output = tool
            .execute(
                json!({
                    "action": "load",
                    "file_path": "Project Plan.md"
                }),
                ToolContext {
                    session_id: "ses_side_panel_tool_load".to_string(),
                    message_id: "msg1".to_string(),
                    tool_call_id: "tool1".to_string(),
                    working_dir: Some(temp.path().to_path_buf()),
                    stdin_request_tx: None,
                    graceful_shutdown_signal: None,
                    execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
                },
            )
            .await
            .expect("tool execute");

        assert!(output.output.contains("project-plan"));
        let snapshot: crate::side_panel::SidePanelSnapshot =
            serde_json::from_value(output.metadata.expect("snapshot metadata"))
                .expect("parse side panel metadata");
        let page = snapshot
            .pages
            .iter()
            .find(|page| page.id == "project-plan")
            .expect("loaded page");
        assert_eq!(page.title, "Project Plan.md");
        assert_eq!(page.content, "# Plan\n\nInitial");

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
