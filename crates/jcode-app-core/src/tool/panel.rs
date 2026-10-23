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
        "Open and manage desktop panels from Markdown content or a Markdown/PDF file."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {"type":"string", "enum":["spawn","update","focus","close","list","history","reopen","purge","refresh"], "default":"spawn", "description":"Default spawn. Close archives recoverably. Reopen restores an open or archived panel. Refresh retries an open panel's linked source while retaining its saved copy on failure. Purge permanently removes its saved copy, never the linked source. List/history return metadata without bodies."},
                "panel_id": {"type":"string", "description":"Same-session panel ID for update/focus/close/reopen/purge/refresh. Omit for spawn/list/history."},
                "title": {"type":"string", "description":"Panel title for spawn/update."},
                "content": {"type":"string", "description":"Markdown for spawn/update, mutually exclusive with file_path."},
                "file_path": {"type":"string", "description":"Markdown/PDF path for spawn/update, relative to cwd. Exclusive with content."},
                "focus": {"type":"boolean", "description":"Focus after spawn/update. Defaults true for spawn and false for update."}
            }
        })
    }
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let mut params: PanelInput = serde_json::from_value(input)?;
        let action = params.action.as_deref().unwrap_or("spawn");
        if matches!(
            action,
            "close" | "reopen" | "purge" | "list" | "history" | "refresh"
        ) {
            ensure!(
                params.content.is_none()
                    && params.file_path.is_none()
                    && params.title.is_none()
                    && params.focus.is_none(),
                "content, file_path, title and focus are only valid for spawn/update"
            );
            let snapshot = if matches!(action, "list" | "history") {
                ensure!(
                    params.panel_id.is_none(),
                    "list/history does not accept panel_id"
                );
                crate::side_panel::snapshot_for_session(&ctx.session_id)?
            } else {
                let id = params
                    .panel_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("panel_id is required for {action}"))?;
                match action {
                    "close" => crate::side_panel::archive_page(&ctx.session_id, id)?,
                    "reopen" => crate::side_panel::reopen_page(&ctx.session_id, id)?,
                    "refresh" => {
                        let mut snapshot =
                            crate::side_panel::snapshot_for_session(&ctx.session_id)?;
                        ensure!(
                            snapshot.pages.iter().any(|page| page.id == id),
                            "Panel not found in this session: {id}"
                        );
                        crate::side_panel::refresh_linked_page_content(&mut snapshot, Some(id));
                        snapshot
                    }
                    _ => crate::side_panel::purge_page(&ctx.session_id, id)?,
                }
            };
            if !matches!(action, "list" | "history") {
                crate::bus::Bus::global().publish(crate::bus::BusEvent::SidePanelUpdated(
                    crate::bus::SidePanelUpdated {
                        session_id: ctx.session_id.clone(),
                        snapshot: snapshot.clone(),
                    },
                ));
            }
            let mut status = crate::side_panel::status_output(&snapshot);
            for entry in &snapshot.archived {
                status.push_str(&format!("\narchived: {} | {} | {} | last_opened_at_ms={} | last_successful_update_ms={}",
                    entry.id, entry.title, entry.file_path, entry.status.last_opened_at_ms, entry.status.last_successful_update_ms));
            }
            let mut metadata = snapshot;
            for page in &mut metadata.pages {
                page.content.clear();
                page.pdf_data = None;
            }
            if let Some(id) = params.panel_id {
                status = format!(
                    "panel_id: {id}\nidentity: side-panel://{}/{id}\n{status}",
                    ctx.session_id
                );
            }
            return Ok(ToolOutput::new(status)
                .with_title("panel")
                .with_metadata(serde_json::to_value(metadata)?));
        }
        let mut legacy = json!({});
        let panel_id = match action {
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
            "focus" => {
                ensure!(
                    params.content.is_none()
                        && params.file_path.is_none()
                        && params.title.is_none()
                        && params.focus.is_none(),
                    "content, file_path, title and focus are only valid for spawn/update"
                );
                legacy["action"] = json!("focus");
                Some(
                    params
                        .panel_id
                        .ok_or_else(|| anyhow::anyhow!("panel_id is required for focus"))?,
                )
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
