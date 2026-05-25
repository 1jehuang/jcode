use super::App;
use crate::side_panel::{
    SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
};
use crate::todo::TodoItem;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub(super) const TODOS_VIEW_PAGE_ID: &str = "session_todos";
const TODOS_VIEW_TITLE: &str = "Todos";

impl App {
    pub(super) fn todos_view_enabled(&self) -> bool {
        self.todos_view_enabled
    }

    pub(super) fn set_todos_view_enabled(&mut self, enabled: bool, focus: bool) {
        self.todos_view_enabled = enabled;
        if enabled {
            self.refresh_todos_view_cache(true);
        } else {
            self.clear_todos_view_cache();
        }

        let mut snapshot = self.snapshot_without_todos_view();
        if enabled {
            snapshot = self.decorate_side_panel_with_todos_view(snapshot, focus);
        } else if snapshot.focused_page_id.is_none() {
            snapshot.focused_page_id = self
                .last_side_panel_focus_id
                .clone()
                .filter(|id| snapshot.pages.iter().any(|page| page.id == *id))
                .or_else(|| snapshot.pages.first().map(|page| page.id.clone()));
        }
        self.apply_side_panel_snapshot(snapshot);
    }

    pub(super) fn decorate_side_panel_with_todos_view(
        &self,
        mut snapshot: SidePanelSnapshot,
        focus_todos: bool,
    ) -> SidePanelSnapshot {
        if !self.todos_view_enabled {
            return snapshot;
        }

        snapshot.pages.retain(|page| page.id != TODOS_VIEW_PAGE_ID);
        snapshot.pages.push(self.todos_view_page());
        snapshot.pages.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        if focus_todos || snapshot.focused_page_id.is_none() {
            snapshot.focused_page_id = Some(TODOS_VIEW_PAGE_ID.to_string());
        }
        snapshot
    }

    pub(super) fn snapshot_without_todos_view(&self) -> SidePanelSnapshot {
        let mut snapshot = self.side_panel.clone();
        snapshot.pages.retain(|page| page.id != TODOS_VIEW_PAGE_ID);
        if snapshot.focused_page_id.as_deref() == Some(TODOS_VIEW_PAGE_ID) {
            snapshot.focused_page_id = None;
        }
        snapshot
    }

    pub(super) fn refresh_todos_view_if_needed(&mut self) -> bool {
        if !self.todos_view_enabled {
            return false;
        }
        let changed = self.refresh_todos_view_cache(false);
        if !changed {
            return false;
        }
        self.refresh_todos_view_page();
        true
    }

    pub(super) fn refresh_todos_view_now(&mut self) -> bool {
        if !self.todos_view_enabled {
            return false;
        }
        let changed = self.refresh_todos_view_cache(true);
        self.refresh_todos_view_page();
        changed
    }

    fn clear_todos_view_cache(&mut self) {
        self.todos_view_markdown.clear();
        self.todos_view_markdown.shrink_to_fit();
        self.todos_view_updated_at_ms = now_ms();
        self.todos_view_rendered_hash = 0;
    }

    fn refresh_todos_view_page(&mut self) {
        if !self.todos_view_enabled {
            return;
        }

        let focus_todos = self.side_panel.focused_page_id.as_deref() == Some(TODOS_VIEW_PAGE_ID);
        let snapshot = self
            .decorate_side_panel_with_todos_view(self.snapshot_without_todos_view(), focus_todos);
        self.apply_side_panel_snapshot(snapshot);
    }

    fn refresh_todos_view_cache(&mut self, force: bool) -> bool {
        let session_id = self.active_client_session_id();
        let todos = load_current_session_todos(session_id);
        let next_hash = hash_todos_payload(session_id, &todos);
        if !force && self.todos_view_rendered_hash == next_hash {
            return false;
        }

        self.todos_view_markdown = build_todos_view_markdown(session_id, &todos);
        self.todos_view_updated_at_ms = now_ms();
        self.todos_view_rendered_hash = next_hash;
        true
    }

    fn todos_view_page(&self) -> SidePanelPage {
        SidePanelPage {
            id: TODOS_VIEW_PAGE_ID.to_string(),
            title: TODOS_VIEW_TITLE.to_string(),
            file_path: "todos://current-session".to_string(),
            format: SidePanelPageFormat::Markdown,
            source: SidePanelPageSource::Ephemeral,
            content: if self.todos_view_markdown.trim().is_empty() {
                todos_view_placeholder_markdown()
            } else {
                self.todos_view_markdown.clone()
            },
            updated_at_ms: self.todos_view_updated_at_ms.max(1),
        }
    }
}

pub(super) fn todos_view_status_message(app: &App) -> String {
    format!(
        "Todo screen: **{}**\n\nWhen enabled, the side panel shows a transient `Todos` page dedicated to the current session's todo list and refreshes as the list changes. It is not persisted to session side-panel storage.",
        if app.todos_view_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    )
}

pub(super) fn handle_todos_view_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/todos") {
        return false;
    }

    let arg = trimmed.strip_prefix("/todos").unwrap_or_default().trim();
    match arg {
        "" => {
            let enabled = !app.todos_view_enabled();
            app.set_todos_view_enabled(enabled, true);
            if enabled {
                app.set_status_notice("Todos: ON");
                app.push_display_message(crate::tui::DisplayMessage::system(
                    "Todo screen enabled. The side panel now shows only this session's todo list."
                        .to_string(),
                ));
            } else {
                app.set_status_notice("Todos: OFF");
                app.push_display_message(crate::tui::DisplayMessage::system(
                    "Todo screen disabled.".to_string(),
                ));
            }
        }
        "on" => {
            app.set_todos_view_enabled(true, true);
            app.set_status_notice("Todos: ON");
            app.push_display_message(crate::tui::DisplayMessage::system(
                "Todo screen enabled. The side panel now shows only this session's todo list."
                    .to_string(),
            ));
        }
        "off" => {
            app.set_todos_view_enabled(false, false);
            app.set_status_notice("Todos: OFF");
            app.push_display_message(crate::tui::DisplayMessage::system(
                "Todo screen disabled.".to_string(),
            ));
        }
        "status" => {
            app.push_display_message(crate::tui::DisplayMessage::system(
                todos_view_status_message(app),
            ));
        }
        _ => {
            app.push_display_message(crate::tui::DisplayMessage::error(
                "Usage: `/todos [on|off|status]`".to_string(),
            ));
        }
    }

    true
}

fn load_current_session_todos(session_id: Option<&str>) -> Vec<TodoItem> {
    let Some(session_id) = session_id else {
        return Vec::new();
    };
    crate::todo::load_todos(session_id).unwrap_or_default()
}

fn build_todos_view_markdown(session_id: Option<&str>, todos: &[TodoItem]) -> String {
    let session_label = session_id
        .and_then(crate::id::extract_session_name)
        .map(|name| format!("`{}`", name))
        .unwrap_or_else(|| "this session".to_string());
    let session_id_line = session_id.map(|id| format!("- Session ID: `{}`\n", id));

    if todos.is_empty() {
        return format!(
            "# Todos\n\nDedicated todo view for {}.\n\n{}\nNo todos saved yet for this session. The model can populate them with the `todo` tool.\n",
            session_label,
            session_id_line.unwrap_or_default()
        );
    }

    let total = todos.len();
    let completed = todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .count();
    let in_progress = todos
        .iter()
        .filter(|todo| todo.status == "in_progress")
        .count();
    let pending = todos.iter().filter(|todo| todo.status == "pending").count();
    let cancelled = todos
        .iter()
        .filter(|todo| todo.status == "cancelled")
        .count();
    let blocked = todos
        .iter()
        .filter(|todo| todo.status != "completed" && !todo.blocked_by.is_empty())
        .count();
    let percent = ((completed as f64 / total as f64) * 100.0).round() as u64;

    let mut markdown = format!(
        "# Todos\n\nDedicated todo view for {}.\n\n{}- Progress: **{}/{} completed** ({}%)\n- In progress: {}\n- Pending: {}\n- Blocked: {}\n- Cancelled: {}\n",
        session_label,
        session_id_line.unwrap_or_default(),
        completed,
        total,
        percent,
        in_progress,
        pending,
        blocked,
        cancelled,
    );

    let sections = [
        ("in_progress", "In progress"),
        ("pending", "Pending"),
        ("completed", "Completed"),
        ("cancelled", "Cancelled"),
    ];

    for (status, heading) in sections {
        let items = sorted_todos_for_status(todos, status);
        if items.is_empty() {
            continue;
        }
        markdown.push_str(&format!("\n## {}\n\n", heading));
        for todo in items {
            markdown.push_str(&format_todo_markdown(todo));
        }
    }

    markdown
}

fn sorted_todos_for_status<'a>(todos: &'a [TodoItem], status: &str) -> Vec<&'a TodoItem> {
    let mut items: Vec<&TodoItem> = todos.iter().filter(|todo| todo.status == status).collect();
    items.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then_with(|| a.content.cmp(&b.content))
            .then_with(|| a.id.cmp(&b.id))
    });
    items
}

fn format_todo_markdown(todo: &TodoItem) -> String {
    let mut line = format!(
        "- {} `[{}]` {}\n",
        status_badge(&todo.status, !todo.blocked_by.is_empty()),
        todo.priority,
        todo.content
    );
    line.push_str(&format!("  - id: `{}`\n", todo.id));
    if let Some(assigned_to) = todo
        .assigned_to
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        line.push_str(&format!("  - assigned to: `{}`\n", assigned_to));
    }
    if !todo.blocked_by.is_empty() {
        let deps = todo
            .blocked_by
            .iter()
            .map(|id| format!("`{}`", id))
            .collect::<Vec<_>>()
            .join(", ");
        line.push_str(&format!("  - blocked by: {}\n", deps));
    }
    line
}

fn status_badge(status: &str, blocked: bool) -> &'static str {
    if blocked && status != "completed" {
        return "[blocked]";
    }
    match status {
        "completed" => "[done]",
        "in_progress" => "[doing]",
        "cancelled" => "[x]",
        _ => "[todo]",
    }
}

fn priority_rank(priority: &str) -> u8 {
    match priority {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    }
}

fn hash_todos_payload(session_id: Option<&str>, todos: &[TodoItem]) -> u64 {
    let mut hasher = DefaultHasher::new();
    session_id.hash(&mut hasher);
    for todo in todos {
        todo.id.hash(&mut hasher);
        todo.content.hash(&mut hasher);
        todo.status.hash(&mut hasher);
        todo.priority.hash(&mut hasher);
        todo.blocked_by.hash(&mut hasher);
        todo.assigned_to.hash(&mut hasher);
    }
    hasher.finish()
}

fn todos_view_placeholder_markdown() -> String {
    "# Todos\n\nWaiting for a session todo list.\n".to_string()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|dur| dur.as_millis() as u64)
        .unwrap_or(0)
}
