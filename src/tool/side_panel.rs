#![cfg_attr(test, allow(clippy::await_holding_lock))]

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
                "intent": super::intent_schema_property(),
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
        let action_label = params.action.clone();
        let page_label = params
            .page_id
            .clone()
            .unwrap_or_else(|| "<none>".to_string());
        let file_label = params
            .file_path
            .clone()
            .unwrap_or_else(|| "<none>".to_string());
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
                let resolved = ctx.resolve_path_checked(Path::new(file_path))?;
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
        .map_err(|err| {
            crate::logging::warn(&format!(
                "[tool:side_panel] action failed action={} page_id={} file_path={} session_id={} error={}",
                action_label, page_label, file_label, ctx.session_id, err
            ));
            err
        })
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
#[path = "side_panel_tests.rs"]
mod side_panel_tests;
