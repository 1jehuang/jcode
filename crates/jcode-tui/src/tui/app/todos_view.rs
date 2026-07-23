use super::App;
use crate::side_panel::{
    SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
};
use crate::todo::TodoItem;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub(super) const TODOS_VIEW_PAGE_ID: &str = "session_todos";
const TODOS_VIEW_TITLE: &str = "Todos";
/// Display-message role used by the inline chat todo card.
const TODO_CARD_ROLE: &str = "todos";

impl App {
    pub(super) fn todos_view_enabled(&self) -> bool {
        self.todos_view_enabled
    }

    fn latest_todo_card_index(&self) -> Option<usize> {
        self.display_messages
            .iter()
            .rposition(|message| message.role == TODO_CARD_ROLE)
    }

    /// Show the session todo list as an inline card in the chat transcript, or
    /// dismiss it when the card is already the trailing message.
    pub(super) fn toggle_todo_card(&mut self) {
        if let Some(idx) = self.latest_todo_card_index()
            && idx + 1 == self.display_messages.len()
        {
            self.remove_display_message(idx);
            self.todo_card_rendered_hash = 0;
            self.set_status_notice("Todos card dismissed");
            return;
        }
        self.show_todo_card();
    }

    /// Push (or move to the bottom) the inline todo card with fresh data. The
    /// transcript keeps at most one card so repeated toggles don't stack.
    pub(super) fn show_todo_card(&mut self) {
        let session_id = self.active_client_session_id().map(str::to_string);
        let todos = load_current_session_todos(session_id.as_deref());
        let goals = load_current_session_goals(session_id.as_deref());
        let content = todo_card_payload_json(&todos, &goals);
        self.todo_card_rendered_hash = hash_todos_payload(session_id.as_deref(), &todos, &goals);

        if let Some(idx) = self.latest_todo_card_index() {
            if idx + 1 == self.display_messages.len() {
                self.replace_display_message_content(idx, content);
                return;
            }
            self.remove_display_message(idx);
        }
        self.push_display_message(crate::tui::DisplayMessage::todos(content));
        self.set_status_notice("Todos card");
    }

    /// Live-refresh the inline todo card when the session todo list changed.
    /// Returns true when the transcript was updated.
    pub(super) fn refresh_todo_card_if_needed(&mut self) -> bool {
        let Some(idx) = self.latest_todo_card_index() else {
            return false;
        };
        let session_id = self.active_client_session_id().map(str::to_string);
        let todos = load_current_session_todos(session_id.as_deref());
        let goals = load_current_session_goals(session_id.as_deref());
        let next_hash = hash_todos_payload(session_id.as_deref(), &todos, &goals);
        if next_hash == self.todo_card_rendered_hash {
            return false;
        }
        self.todo_card_rendered_hash = next_hash;
        let content = todo_card_payload_json(&todos, &goals);
        self.replace_display_message_content(idx, content)
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
        let goals = load_current_session_goals(session_id);
        let next_hash = hash_todos_payload(session_id, &todos, &goals);
        if !force && self.todos_view_rendered_hash == next_hash {
            return false;
        }

        self.todos_view_markdown = build_todos_view_markdown(session_id, &todos, &goals);
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
        "Todo card: shown inline in the chat with /todos or {}.\n\nTodo side-panel screen: {}\n\nWhen the panel screen is enabled (/todos panel), the side panel shows a transient Todos page dedicated to the current session's todo list and refreshes as the list changes. It is not persisted to session side-panel storage.",
        crate::tui::keybind::todo_card_key_label(),
        if app.todos_view_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    )
}

pub(super) fn handle_todos_view_command(app: &mut App, trimmed: &str) -> bool {
    let arg = if trimmed == "/todo" {
        ""
    } else if let Some(rest) = trimmed.strip_prefix("/todos") {
        rest.trim()
    } else {
        return false;
    };

    match arg {
        // Default: show the todo list as an inline chat card (toggles off when
        // the card is already the latest message).
        "" | "card" => {
            app.toggle_todo_card();
        }
        // Legacy side-panel screen, now behind an explicit subcommand.
        "panel" => {
            let enabled = !app.todos_view_enabled();
            app.set_todos_view_enabled(enabled, true);
            if enabled {
                app.set_status_notice("Todos panel: ON");
                app.push_display_message(crate::tui::DisplayMessage::system(
                    "Todo screen enabled. The side panel now shows only this session's todo list."
                        .to_string(),
                ));
            } else {
                app.set_status_notice("Todos panel: OFF");
                app.push_display_message(crate::tui::DisplayMessage::system(
                    "Todo screen disabled.".to_string(),
                ));
            }
        }
        "on" | "panel on" => {
            app.set_todos_view_enabled(true, true);
            app.set_status_notice("Todos panel: ON");
            app.push_display_message(crate::tui::DisplayMessage::system(
                "Todo screen enabled. The side panel now shows only this session's todo list."
                    .to_string(),
            ));
        }
        "off" | "panel off" => {
            app.set_todos_view_enabled(false, false);
            app.set_status_notice("Todos panel: OFF");
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
                "Usage: /todos [card|panel|on|off|status]".to_string(),
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

fn load_current_session_goals(session_id: Option<&str>) -> Vec<crate::todo::TodoGoal> {
    let Some(session_id) = session_id else {
        return Vec::new();
    };
    crate::todo::load_goals(session_id).unwrap_or_default()
}

fn todo_card_payload_json(todos: &[TodoItem], goals: &[crate::todo::TodoGoal]) -> String {
    serde_json::to_string(&serde_json::json!({
        "todos": todos,
        "goals": goals,
    }))
    .unwrap_or_else(|_| r#"{"todos":[],"goals":[]}"#.to_string())
}

fn build_todos_view_markdown(
    session_id: Option<&str>,
    todos: &[TodoItem],
    goals: &[crate::todo::TodoGoal],
) -> String {
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
    let weighted_confidence = weighted_todo_confidence(todos);
    let lowest_completed_confidence = todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .filter_map(|todo| todo.completion_confidence)
        .min();
    let missing_completion_confidence = todos
        .iter()
        .filter(|todo| todo.status == "completed" && todo.completion_confidence.is_none())
        .count();

    let mut markdown = format!(
        "# Todos\n\nDedicated todo view for {}.\n\n{}- Progress: **{}/{} completed** ({}%)\n- In progress: {}\n- Pending: {}\n- Blocked: {}\n- Cancelled: {}\n- Weighted confidence: **{}**\n- Lowest completed confidence: **{}**\n- Missing completion confidence: {}\n",
        session_label,
        session_id_line.unwrap_or_default(),
        completed,
        total,
        percent,
        in_progress,
        pending,
        blocked,
        cancelled,
        format_confidence_value(weighted_confidence),
        format_confidence_value(lowest_completed_confidence),
        missing_completion_confidence,
    );

    let sections = [
        ("in_progress", "In progress"),
        ("pending", "Pending"),
        ("completed", "Completed"),
        ("cancelled", "Cancelled"),
    ];

    if let Some(groups) = grouped_todos_view(todos) {
        for (group, items) in groups {
            let group_name = group.as_deref().unwrap_or("Other");
            let group_total = items.len();
            let group_done = items.iter().filter(|t| t.status == "completed").count();
            markdown.push_str(&format!(
                "\n## {} ({}/{})\n",
                group_name, group_done, group_total
            ));
            markdown.push_str(&format_goal_markdown(goals, group.as_deref()));
            for (status, heading) in sections {
                let status_items = sorted_group_items_for_status(&items, status);
                if status_items.is_empty() {
                    continue;
                }
                markdown.push_str(&format!("\n### {}\n\n", heading));
                for todo in status_items {
                    markdown.push_str(&format_todo_markdown(todo));
                }
            }
        }
        return markdown;
    }

    markdown.push_str(&format_goal_markdown(goals, None));
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

/// Group key for the side-panel view, treating empty/whitespace as ungrouped.
fn todo_group_key(todo: &TodoItem) -> Option<String> {
    todo.group
        .as_deref()
        .map(str::trim)
        .filter(|group| !group.is_empty())
        .map(|group| group.to_string())
}

/// Goal assessment line(s) for a group header (or the ungrouped/flat list
/// when `group` is `None`). Empty when no goal is recorded for that key.
fn format_goal_markdown(goals: &[crate::todo::TodoGoal], group: Option<&str>) -> String {
    let key = group.map(str::trim).filter(|group| !group.is_empty());
    let Some(goal) = goals.iter().find(|goal| {
        goal.group
            .as_deref()
            .map(str::trim)
            .filter(|group| !group.is_empty())
            == key
    }) else {
        return String::new();
    };
    let mut line = String::new();
    if let Some(user_intention) = goal
        .user_intention
        .as_deref()
        .filter(|user_intention| !user_intention.trim().is_empty())
    {
        line.push_str(&format!("\n- User intention: {}\n", user_intention.trim()));
    }
    if let Some(score) = goal.alignment_score {
        if line.is_empty() {
            line.push('\n');
        }
        line.push_str(&format!("- Alignment score: **{}%**\n", score));
    }
    if let Some(score) = goal.hill_climbability {
        if line.is_empty() {
            line.push('\n');
        }
        line.push_str(&format!("- Hill climbability: **{}%**", score));
        line.push('\n');
    }
    if let Some(objective) = goal
        .objective
        .as_deref()
        .filter(|objective| !objective.trim().is_empty())
    {
        line.push_str(&format!("- Objective: {}\n", objective.trim()));
    }
    if let Some(feedback_loop) = goal
        .feedback_loop
        .as_deref()
        .filter(|feedback_loop| !feedback_loop.trim().is_empty())
    {
        line.push_str(&format!("- Feedback loop: {}\n", feedback_loop.trim()));
    }
    if let Some(score) = goal.end_to_end_ownership {
        line.push_str(&format!("- End-to-end ownership: **{}%**\n", score));
    }
    line
}

/// Partition todos into ordered groups (first-seen order, ungrouped last).
/// Returns `None` when no todo declares a group so callers keep the flat layout.
fn grouped_todos_view(todos: &[TodoItem]) -> Option<Vec<(Option<String>, Vec<&TodoItem>)>> {
    if !todos.iter().any(|todo| todo_group_key(todo).is_some()) {
        return None;
    }
    let mut groups: Vec<(Option<String>, Vec<&TodoItem>)> = Vec::new();
    for todo in todos {
        let key = todo_group_key(todo);
        if let Some(entry) = groups.iter_mut().find(|(existing, _)| *existing == key) {
            entry.1.push(todo);
        } else {
            groups.push((key, vec![todo]));
        }
    }
    groups.sort_by_key(|(key, _)| key.is_none());
    Some(groups)
}

fn sorted_group_items_for_status<'a>(items: &[&'a TodoItem], status: &str) -> Vec<&'a TodoItem> {
    let mut filtered: Vec<&TodoItem> = items
        .iter()
        .copied()
        .filter(|todo| todo.status == status)
        .collect();
    filtered.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then_with(|| a.content.cmp(&b.content))
            .then_with(|| a.id.cmp(&b.id))
    });
    filtered
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
    line.push_str(&format!(
        "  - confidence: `{}`\n",
        format_confidence_value(todo.confidence)
    ));
    if todo.status == "completed" || todo.completion_confidence.is_some() {
        line.push_str(&format!(
            "  - completion confidence: `{}`\n",
            format_confidence_value(todo.completion_confidence)
        ));
    }
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

fn todo_confidence_weight(priority: &str) -> u32 {
    match priority {
        "high" => 3,
        "medium" => 2,
        _ => 1,
    }
}

fn todo_effective_confidence(todo: &TodoItem) -> Option<u8> {
    if todo.status == "completed" {
        todo.completion_confidence.or(todo.confidence)
    } else {
        todo.confidence
    }
}

fn weighted_todo_confidence(todos: &[TodoItem]) -> Option<u8> {
    let mut weighted_sum = 0u32;
    let mut total_weight = 0u32;
    for todo in todos.iter().filter(|todo| todo.status != "cancelled") {
        let Some(score) = todo_effective_confidence(todo) else {
            continue;
        };
        let weight = todo_confidence_weight(&todo.priority);
        weighted_sum += u32::from(score) * weight;
        total_weight += weight;
    }
    if total_weight == 0 {
        None
    } else {
        Some(((weighted_sum + total_weight / 2) / total_weight) as u8)
    }
}

fn format_confidence_value(score: Option<u8>) -> String {
    score
        .map(|score| format!("{}%", score))
        .unwrap_or_else(|| "unknown".to_string())
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

fn hash_todos_payload(
    session_id: Option<&str>,
    todos: &[TodoItem],
    goals: &[crate::todo::TodoGoal],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    session_id.hash(&mut hasher);
    for todo in todos {
        todo.id.hash(&mut hasher);
        todo.content.hash(&mut hasher);
        todo.status.hash(&mut hasher);
        todo.priority.hash(&mut hasher);
        todo.group.hash(&mut hasher);
        todo.confidence.hash(&mut hasher);
        todo.completion_confidence.hash(&mut hasher);
        todo.blocked_by.hash(&mut hasher);
        todo.assigned_to.hash(&mut hasher);
    }
    for goal in goals {
        goal.group.hash(&mut hasher);
        goal.user_intention.hash(&mut hasher);
        goal.alignment_score.hash(&mut hasher);
        goal.hill_climbability.hash(&mut hasher);
        goal.objective.hash(&mut hasher);
        goal.feedback_loop.hash(&mut hasher);
        goal.end_to_end_ownership.hash(&mut hasher);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(
        id: &str,
        content: &str,
        status: &str,
        priority: &str,
        confidence: Option<u8>,
        completion_confidence: Option<u8>,
    ) -> TodoItem {
        TodoItem {
            id: id.to_string(),
            content: content.to_string(),
            status: status.to_string(),
            priority: priority.to_string(),
            group: None,
            confidence,
            completion_confidence,
            confidence_history: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
        }
    }

    #[test]
    fn todos_view_markdown_includes_confidence_summary_and_item_fields() {
        let todos = vec![
            todo(
                "todo-1",
                "Validate confidence side panel",
                "in_progress",
                "high",
                Some(80),
                None,
            ),
            todo(
                "todo-2",
                "Finish completed item",
                "completed",
                "medium",
                Some(70),
                Some(95),
            ),
        ];

        let markdown = build_todos_view_markdown(Some("session_test"), &todos, &[]);

        assert!(markdown.contains("- Weighted confidence: **86%**"));
        assert!(markdown.contains("- Lowest completed confidence: **95%**"));
        assert!(markdown.contains("- Missing completion confidence: 0"));
        assert!(markdown.contains("  - confidence: `80%`"));
        assert!(markdown.contains("  - confidence: `70%`"));
        assert!(markdown.contains("  - completion confidence: `95%`"));
    }

    #[test]
    fn todos_view_hash_changes_when_confidence_changes() {
        let mut todos = vec![todo(
            "todo-1",
            "Track confidence hash",
            "pending",
            "high",
            Some(80),
            None,
        )];
        let before = hash_todos_payload(Some("session_test"), &todos, &[]);
        todos[0].confidence = Some(81);
        let after = hash_todos_payload(Some("session_test"), &todos, &[]);

        assert_ne!(before, after);
    }

    #[test]
    fn todos_view_markdown_groups_items_under_group_headers() {
        let mut grouped_a = todo(
            "g1",
            "Cut frame allocs",
            "in_progress",
            "high",
            Some(80),
            None,
        );
        grouped_a.group = Some("optimize rendering".to_string());
        let mut grouped_b = todo(
            "g2",
            "Batch draw calls",
            "completed",
            "medium",
            Some(70),
            Some(90),
        );
        grouped_b.group = Some("optimize rendering".to_string());
        let mut other = todo("o1", "Fix scrollback", "pending", "low", Some(60), None);
        other.group = Some("scrollback".to_string());
        let ungrouped = todo("u1", "Misc cleanup", "pending", "low", Some(60), None);

        let markdown = build_todos_view_markdown(
            Some("session_test"),
            &[grouped_a, grouped_b, other, ungrouped],
            &[crate::todo::TodoGoal {
                group: Some("optimize rendering".to_string()),
                user_intention: Some("make navigation feel immediate".to_string()),
                alignment_score: Some(96),
                hill_climbability: Some(90),
                objective: Some("frame time under 8ms".to_string()),
                feedback_loop: Some(
                    "run the frame benchmark and compare p95 frame time".to_string(),
                ),
                end_to_end_ownership: Some(85),
            }],
        );

        assert!(
            markdown.contains("## optimize rendering (1/2)"),
            "{markdown}"
        );
        assert!(
            markdown.contains("- User intention: make navigation feel immediate"),
            "{markdown}"
        );
        assert!(
            markdown.contains("- Alignment score: **96%**"),
            "{markdown}"
        );
        assert!(
            markdown.contains("- Hill climbability: **90%**"),
            "{markdown}"
        );
        assert!(
            markdown.contains("- Objective: frame time under 8ms"),
            "{markdown}"
        );
        assert!(
            markdown
                .contains("- Feedback loop: run the frame benchmark and compare p95 frame time"),
            "{markdown}"
        );
        assert!(
            markdown.contains("- End-to-end ownership: **85%**"),
            "{markdown}"
        );
        assert!(markdown.contains("## scrollback (0/1)"), "{markdown}");
        assert!(markdown.contains("## Other (0/1)"), "{markdown}");
        // Status sub-headings nest under groups.
        assert!(markdown.contains("### In progress"), "{markdown}");
        // First-seen group order, ungrouped bucket last.
        let opt = markdown.find("## optimize rendering").unwrap();
        let scroll = markdown.find("## scrollback").unwrap();
        let other_idx = markdown.find("## Other").unwrap();
        assert!(opt < scroll && scroll < other_idx, "{markdown}");
    }

    #[test]
    fn todos_view_hash_changes_when_group_changes() {
        let mut todos = vec![todo("g", "Group hash", "pending", "high", Some(80), None)];
        let before = hash_todos_payload(Some("session_test"), &todos, &[]);
        todos[0].group = Some("rendering".to_string());
        let after = hash_todos_payload(Some("session_test"), &todos, &[]);
        assert_ne!(before, after);
    }

    #[test]
    fn todos_view_hash_changes_when_goals_change() {
        let todos = vec![todo("g", "Goal hash", "pending", "high", Some(80), None)];
        let before = hash_todos_payload(Some("session_test"), &todos, &[]);
        let goals = vec![crate::todo::TodoGoal {
            hill_climbability: Some(30),
            ..Default::default()
        }];
        let after = hash_todos_payload(Some("session_test"), &todos, &goals);
        assert_ne!(before, after);
    }

    #[test]
    fn todos_view_hash_changes_when_feedback_loop_changes() {
        let todos = vec![todo("g", "Goal hash", "pending", "high", Some(80), None)];
        let mut goals = vec![crate::todo::TodoGoal {
            feedback_loop: Some("run test A".to_string()),
            ..Default::default()
        }];
        let before = hash_todos_payload(Some("session_test"), &todos, &goals);
        goals[0].feedback_loop = Some("run test B".to_string());
        let after = hash_todos_payload(Some("session_test"), &todos, &goals);
        assert_ne!(before, after);
    }

    #[test]
    fn todos_view_hash_changes_when_user_intention_changes() {
        let todos = vec![todo("g", "Goal hash", "pending", "high", Some(80), None)];
        let mut goals = vec![crate::todo::TodoGoal {
            user_intention: Some("reduce friction".to_string()),
            ..Default::default()
        }];
        let before = hash_todos_payload(Some("session_test"), &todos, &goals);
        goals[0].user_intention = Some("increase clarity".to_string());
        let after = hash_todos_payload(Some("session_test"), &todos, &goals);
        assert_ne!(before, after);
    }

    #[test]
    fn todos_view_hash_changes_when_alignment_score_changes() {
        let todos = vec![todo("g", "Goal hash", "pending", "high", Some(80), None)];
        let mut goals = vec![crate::todo::TodoGoal {
            alignment_score: Some(70),
            ..Default::default()
        }];
        let before = hash_todos_payload(Some("session_test"), &todos, &goals);
        goals[0].alignment_score = Some(95);
        let after = hash_todos_payload(Some("session_test"), &todos, &goals);
        assert_ne!(before, after);
    }
}
