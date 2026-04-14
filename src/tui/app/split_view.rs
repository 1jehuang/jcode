use super::App;
use crate::side_panel::{
    SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub(super) const SPLIT_VIEW_PAGE_ID: &str = "split_view";
const SPLIT_VIEW_TITLE: &str = "Split View";

impl App {
    pub(super) fn split_view_enabled(&self) -> bool {
        self.split_view_enabled
    }

    pub(super) fn set_split_view_enabled(&mut self, enabled: bool, focus: bool) {
        self.split_view_enabled = enabled;
        self.refresh_split_view_cache(true);

        let mut snapshot = self.snapshot_without_split_view();
        if enabled {
            snapshot = self.decorate_side_panel_with_split_view(snapshot, focus);
        } else if snapshot.focused_page_id.is_none() {
            snapshot.focused_page_id = self
                .last_side_panel_focus_id
                .clone()
                .filter(|id| snapshot.pages.iter().any(|page| page.id == *id))
                .or_else(|| snapshot.pages.first().map(|page| page.id.clone()));
        }
        self.apply_side_panel_snapshot(snapshot);
    }

    pub(super) fn decorate_side_panel_with_split_view(
        &self,
        mut snapshot: SidePanelSnapshot,
        focus_split_view: bool,
    ) -> SidePanelSnapshot {
        if !self.split_view_enabled {
            return snapshot;
        }

        snapshot.pages.retain(|page| page.id != SPLIT_VIEW_PAGE_ID);
        snapshot.pages.push(self.split_view_page());
        snapshot.pages.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        if focus_split_view || snapshot.focused_page_id.is_none() {
            snapshot.focused_page_id = Some(SPLIT_VIEW_PAGE_ID.to_string());
        }
        snapshot
    }

    pub(super) fn snapshot_without_split_view(&self) -> SidePanelSnapshot {
        let mut snapshot = self.side_panel.clone();
        snapshot.pages.retain(|page| page.id != SPLIT_VIEW_PAGE_ID);
        if snapshot.focused_page_id.as_deref() == Some(SPLIT_VIEW_PAGE_ID) {
            snapshot.focused_page_id = None;
        }
        snapshot
    }

    pub(super) fn refresh_split_view_if_needed(&mut self) {
        let changed = self.refresh_split_view_cache(false);
        if !self.split_view_enabled || !changed {
            return;
        }
        self.refresh_split_view_page();
    }

    fn refresh_split_view_page(&mut self) {
        if !self.split_view_enabled {
            return;
        }

        let focus_split_view =
            self.side_panel.focused_page_id.as_deref() == Some(SPLIT_VIEW_PAGE_ID);
        let snapshot = self.decorate_side_panel_with_split_view(
            self.snapshot_without_split_view(),
            focus_split_view,
        );
        self.apply_side_panel_snapshot(snapshot);
    }

    fn refresh_split_view_cache(&mut self, force: bool) -> bool {
        let streaming_hash = hash_str(&self.streaming_text);
        if !force
            && self.split_view_rendered_display_version == self.display_messages_version
            && self.split_view_rendered_streaming_hash == streaming_hash
        {
            return false;
        }

        self.split_view_markdown = build_split_view_markdown(self);
        self.split_view_updated_at_ms = now_ms();
        self.split_view_rendered_display_version = self.display_messages_version;
        self.split_view_rendered_streaming_hash = streaming_hash;
        true
    }

    fn split_view_page(&self) -> SidePanelPage {
        SidePanelPage {
            id: SPLIT_VIEW_PAGE_ID.to_string(),
            title: SPLIT_VIEW_TITLE.to_string(),
            file_path: "split://chat-mirror".to_string(),
            format: SidePanelPageFormat::Markdown,
            source: SidePanelPageSource::Ephemeral,
            content: if self.split_view_markdown.trim().is_empty() {
                split_view_placeholder_markdown()
            } else {
                self.split_view_markdown.clone()
            },
            updated_at_ms: self.split_view_updated_at_ms.max(1),
        }
    }
}

pub(super) fn split_view_status_message(app: &App) -> String {
    format!(
        "Split view: **{}**\n\nWhen enabled, the side panel mirrors the current chat so you can scroll older context there while keeping the main composer and live output in view. It is transient and not persisted to session side-panel storage.",
        if app.split_view_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    )
}

pub(super) fn handle_split_view_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/splitview") && !trimmed.starts_with("/split-view") {
        return false;
    }

    let arg = trimmed
        .strip_prefix("/splitview")
        .or_else(|| trimmed.strip_prefix("/split-view"))
        .unwrap_or_default()
        .trim();

    match arg {
        "" => {
            let enabled = !app.split_view_enabled();
            app.set_split_view_enabled(enabled, true);
            if enabled {
                app.set_status_notice("Split view: ON");
                app.push_display_message(crate::tui::DisplayMessage::system(
                    "Split view enabled. The side panel now mirrors this chat with its own scroll position."
                        .to_string(),
                ));
            } else {
                app.set_status_notice("Split view: OFF");
                app.push_display_message(crate::tui::DisplayMessage::system(
                    "Split view disabled.".to_string(),
                ));
            }
        }
        "on" => {
            app.set_split_view_enabled(true, true);
            app.set_status_notice("Split view: ON");
            app.push_display_message(crate::tui::DisplayMessage::system(
                "Split view enabled. The side panel now mirrors this chat with its own scroll position."
                    .to_string(),
            ));
        }
        "off" => {
            app.set_split_view_enabled(false, false);
            app.set_status_notice("Split view: OFF");
            app.push_display_message(crate::tui::DisplayMessage::system(
                "Split view disabled.".to_string(),
            ));
        }
        "status" => {
            app.push_display_message(crate::tui::DisplayMessage::system(
                split_view_status_message(app),
            ));
        }
        _ => {
            app.push_display_message(crate::tui::DisplayMessage::error(
                "Usage: `/splitview [on|off|status]`".to_string(),
            ));
        }
    }

    true
}

fn build_split_view_markdown(app: &App) -> String {
    if app.display_messages().is_empty() && app.streaming_text().trim().is_empty() {
        return split_view_placeholder_markdown();
    }

    let mut markdown = String::from(
        "# Split View\n\nMirror of the current chat. Scroll here independently while keeping the composer active in the main pane.\n\nTips:\n- `Ctrl+L` focuses the side pane\n- `Ctrl+H` returns focus to the main chat\n- `j/k`, `PageUp/PageDown`, and `g/G` scroll the side pane\n",
    );

    let mut prompt_number = 0usize;
    let mut assistant_after_prompt = 0usize;

    for message in app.display_messages() {
        markdown.push_str("\n---\n\n");
        match message.role.as_str() {
            "user" => {
                prompt_number += 1;
                assistant_after_prompt = 0;
                markdown.push_str(&format!("## Prompt {}\n\n", prompt_number));
                push_markdown_body(&mut markdown, &message.content);
            }
            "assistant" => {
                assistant_after_prompt += 1;
                if prompt_number > 0 {
                    if assistant_after_prompt == 1 {
                        markdown.push_str(&format!("## Response {}\n\n", prompt_number));
                    } else {
                        markdown.push_str(&format!(
                            "## Response {}.{}\n\n",
                            prompt_number, assistant_after_prompt
                        ));
                    }
                } else {
                    markdown.push_str("## Assistant\n\n");
                }
                push_markdown_body(&mut markdown, &message.content);
            }
            "tool" => {
                let title = message
                    .title
                    .as_deref()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or("Tool");
                markdown.push_str(&format!("## {}\n\n", title));
                if let Some(tool) = message.tool_data.as_ref() {
                    markdown.push_str(&format!("- Tool: `{}`\n\n", tool.name));
                }
                markdown.push_str(&fenced_block(
                    "text",
                    if message.content.trim().is_empty() {
                        "(empty)"
                    } else {
                        message.content.as_str()
                    },
                ));
                markdown.push('\n');
            }
            "system" => {
                let title = message
                    .title
                    .as_deref()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or("System");
                markdown.push_str(&format!("## {}\n\n", title));
                markdown.push_str(&fenced_block(
                    "text",
                    if message.content.trim().is_empty() {
                        "(empty)"
                    } else {
                        message.content.as_str()
                    },
                ));
                markdown.push('\n');
            }
            other => {
                markdown.push_str(&format!("## {}\n\n", capitalize_role(other)));
                push_markdown_body(&mut markdown, &message.content);
            }
        }
    }

    let streaming_text = app.streaming_text().trim();
    if !streaming_text.is_empty() {
        markdown.push_str("\n---\n\n## Live response\n\n");
        push_markdown_body(&mut markdown, streaming_text);
    }

    markdown
}

fn push_markdown_body(markdown: &mut String, body: &str) {
    let body = body.trim();
    if body.is_empty() {
        markdown.push_str("_empty_\n");
        return;
    }
    markdown.push_str(body);
    if !body.ends_with('\n') {
        markdown.push('\n');
    }
}

fn split_view_placeholder_markdown() -> String {
    "# Split View\n\nMirror of the current chat. Open it while you scroll old context in the side pane and keep typing in the main composer.\n\nOnce the conversation has content, the full transcript will appear here with its own scroll position.\n".to_string()
}

fn fenced_block(language: &str, text: &str) -> String {
    let max_run = text
        .split('\n')
        .flat_map(|line| line.split(|ch| ch != '`'))
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(max_run.max(3) + 1);
    if language.trim().is_empty() {
        format!("{fence}\n{text}\n{fence}")
    } else {
        format!("{fence}{language}\n{text}\n{fence}")
    }
}

fn capitalize_role(role: &str) -> String {
    let mut chars = role.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Message".to_string(),
    }
}

fn hash_str(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|dur| dur.as_millis() as u64)
        .unwrap_or(0)
}
