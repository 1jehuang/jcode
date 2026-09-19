//! First-class desktop panel lifecycle, backed by the legacy shared panel state.
use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

pub struct PanelTool;
impl PanelTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PanelInput {
    action: Option<String>,
    panel_id: Option<String>,
    title: Option<String>,
    content: Option<String>,
    file_path: Option<String>,
    focus: Option<bool>,
    #[serde(rename = "intent")]
    _intent: Option<String>,
    // ToolRegistry consumes this shared field after execution, but forwards it
    // in provider-shaped inputs, including null from strict-schema providers.
    #[serde(rename = "accept_large_output")]
    _accept_large_output: Option<bool>,
}

#[async_trait]
impl Tool for PanelTool {
    fn name(&self) -> &str {
        "panel"
    }
    fn description(&self) -> &str {
        "Open and manage desktop panels. Spawn creates a new panel each time from Markdown content or a linked Markdown/PDF file (PDF max 20 MiB, session PDF total max 32 MiB). Update, focus or close an existing same-session panel using its returned panel_id. TUI displays Markdown fallback for PDFs."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {"type":"string", "enum":["spawn","update","focus","close","list"], "default":"spawn", "description":"Defaults to spawn. Spawn/update require exactly one of content or file_path. Focus/close require panel_id. List needs no other arguments."},
                "panel_id": {"type":"string", "description":"Existing same-session ID for update/focus/close. Do not supply for spawn, which always generates a new ID."},
                "title": {"type":"string", "description":"Panel title for spawn/update."},
                "content": {"type":"string", "description":"Markdown for spawn/update, mutually exclusive with file_path."},
                "file_path": {"type":"string", "description":"Linked Markdown or PDF path for spawn/update, relative to working directory. Mutually exclusive with content."},
                "focus": {"type":"boolean", "description":"Focus after spawn/update. Defaults true for spawn and false for update."}
            }
        })
    }
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let mut params: PanelInput = serde_json::from_value(input)?;
        let action = params.action.as_deref().unwrap_or("spawn");
        let mut legacy = json!({});
        let panel_id =
            match action {
                "spawn" | "update" => {
                    ensure!(
                        params.content.is_some() != params.file_path.is_some(),
                        "spawn/update requires exactly one of content or file_path"
                    );
                    let id = if action == "spawn" {
                        ensure!(
                            params.panel_id.is_none(),
                            "spawn generates panel_id; do not supply one"
                        );
                        format!("panel-{}", uuid::Uuid::new_v4())
                    } else {
                        let id = params
                            .panel_id
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("panel_id is required for update"))?;
                        let snapshot = crate::side_panel::snapshot_for_session(&ctx.session_id)?;
                        let existing =
                            snapshot.pages.iter().find(|p| p.id == id).ok_or_else(|| {
                                anyhow::anyhow!("Panel not found in this session: {id}")
                            })?;
                        if params.title.is_none() {
                            params.title = Some(existing.title.clone());
                        }
                        id.to_owned()
                    };
                    legacy["action"] = json!(if params.content.is_some() {
                        "write"
                    } else {
                        "load"
                    });
                    legacy["content"] = json!(params.content);
                    legacy["file_path"] = json!(params.file_path);
                    legacy["title"] = json!(params.title);
                    legacy["focus"] = json!(params.focus.unwrap_or(action == "spawn"));
                    Some(id)
                }
                "focus" | "close" | "list" => {
                    ensure!(
                        params.content.is_none()
                            && params.file_path.is_none()
                            && params.title.is_none()
                            && params.focus.is_none(),
                        "content, file_path, title and focus are only valid for spawn/update"
                    );
                    legacy["action"] = json!(match action {
                        "focus" => "focus",
                        "close" => "delete",
                        _ => "status",
                    });
                    if action == "list" {
                        ensure!(params.panel_id.is_none(), "list does not accept panel_id");
                        None
                    } else {
                        Some(params.panel_id.ok_or_else(|| {
                            anyhow::anyhow!("panel_id is required for focus/close")
                        })?)
                    }
                }
                other => anyhow::bail!("unknown panel action: {other}"),
            };
        legacy["page_id"] = json!(panel_id);
        let session_id = ctx.session_id.clone();
        let mut output = super::side_panel::SidePanelTool::new()
            .execute(legacy, ctx)
            .await?;
        if let Some(id) = panel_id {
            output.output = format!(
                "panel_id: {id}\nidentity: side-panel://{session_id}/{id}\n{}",
                output.output
            );
        }
        Ok(output.with_title("panel"))
    }
}

#[cfg(test)]
#[path = "panel_tests.rs"]
mod tests;
