use super::App;
use crate::side_panel::{
    SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
};
use crate::todo::TodoItem;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

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
        let plan = load_current_session_plan(session_id.as_deref());
        let content = todo_card_payload_json(&todos, &plan, &goals);
        self.todo_card_rendered_hash =
            hash_todos_payload(session_id.as_deref(), &todos, &plan, &goals);

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
        let plan = load_current_session_plan(session_id.as_deref());
        let next_hash = hash_todos_payload(session_id.as_deref(), &todos, &plan, &goals);
        if next_hash == self.todo_card_rendered_hash {
            return false;
        }
        self.todo_card_rendered_hash = next_hash;
        let content = todo_card_payload_json(&todos, &plan, &goals);
        self.replace_display_message_content(idx, content)
    }

    /// Live-refresh the payload behind the pinned todo band
    /// (`display.pin_todos`). Returns true when the payload changed and the
    /// viewport should redraw. Disk reads are throttled to once per second.
    pub(super) fn refresh_pinned_todos_if_needed(&mut self) -> bool {
        if !crate::config::config().display.pin_todos {
            if self.pinned_todos_payload.is_some() {
                self.pinned_todos_payload = None;
                self.pinned_todos_rendered_hash = 0;
                return true;
            }
            return false;
        }
        const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
        if let Some(checked_at) = self.pinned_todos_checked_at
            && checked_at.elapsed() < REFRESH_INTERVAL
        {
            return false;
        }
        self.pinned_todos_checked_at = Some(Instant::now());
        let session_id = self.active_client_session_id().map(str::to_string);
        let todos = load_current_session_todos(session_id.as_deref());
        if todos.is_empty() {
            if self.pinned_todos_payload.is_some() {
                self.pinned_todos_payload = None;
                self.pinned_todos_rendered_hash = 0;
                return true;
            }
            return false;
        }
        let goals = load_current_session_goals(session_id.as_deref());
        let plan = load_current_session_plan(session_id.as_deref());
        let next_hash = hash_todos_payload(session_id.as_deref(), &todos, &plan, &goals);
        if next_hash == self.pinned_todos_rendered_hash && self.pinned_todos_payload.is_some() {
            return false;
        }
        self.pinned_todos_rendered_hash = next_hash;
        self.pinned_todos_payload = Some(todo_card_payload_json(&todos, &plan, &goals));
        true
    }

    /// The pinned-band renderer that reads this is landing separately, so the
    /// accessor is allowed to be unused (outside tests) until it does.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn pinned_todos_payload_ref(&self) -> Option<&str> {
        self.pinned_todos_payload.as_deref()
    }

    /// Force the pinned todo band to re-read state on the next tick, bypassing
    /// the 1s throttle. Used right after the user toggles `/todos pin`.
    pub(super) fn refresh_pinned_todos_now(&mut self) {
        self.pinned_todos_checked_at = None;
        self.refresh_pinned_todos_if_needed();
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
        let plan = load_current_session_plan(session_id);
        let next_hash = hash_todos_payload(session_id, &todos, &plan, &goals);
        if !force && self.todos_view_rendered_hash == next_hash {
            return false;
        }

        self.todos_view_markdown = build_todos_view_markdown(session_id, &todos, &plan, &goals);
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
        "Todo card: shown inline in the chat with /todos or {}.\n\nTodo side-panel screen: {}\n\nPinned todo band: {}\n\nWhen the panel screen is enabled (/todos panel), the side panel shows a transient Todos page dedicated to the current session's todo list and refreshes as the list changes. It is not persisted to session side-panel storage.\n\nWhen the pinned band is enabled (/todos pin), the full todo list stays pinned to the top of the chat transcript while it scrolls, like the previous-prompt preview.",
        crate::tui::keybind::todo_card_key_label(),
        if app.todos_view_enabled() {
            "enabled"
        } else {
            "disabled"
        },
        if crate::config::config().display.pin_todos {
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
        // Pin the full todo list to the top of the chat transcript.
        "pin" | "pin on" | "pin off" => {
            let enabled = match arg {
                "pin on" => true,
                "pin off" => false,
                _ => !crate::config::config().display.pin_todos,
            };
            app.set_status_notice(if enabled {
                "Pinned todos: ON"
            } else {
                "Pinned todos: OFF"
            });
            match crate::config::Config::set_pin_todos(enabled) {
                Ok(()) => app.push_display_message(crate::tui::DisplayMessage::system(
                    if enabled {
                        "Pinned todo band enabled. The todo list stays pinned to the top of the transcript while it scrolls."
                    } else {
                        "Pinned todo band disabled."
                    }
                    .to_string(),
                )),
                Err(error) => app.push_display_message(crate::tui::DisplayMessage::error(
                    format!("Failed to save display.pin_todos: {}", error),
                )),
            }
            app.refresh_pinned_todos_now();
        }
        "status" => {
            app.push_display_message(crate::tui::DisplayMessage::system(
                todos_view_status_message(app),
            ));
        }
        _ => {
            app.push_display_message(crate::tui::DisplayMessage::error(
                "Usage: /todos [card|panel|pin|on|off|status]".to_string(),
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

fn load_current_session_plan(session_id: Option<&str>) -> crate::todo::TodoPlan {
    let Some(session_id) = session_id else {
        return crate::todo::TodoPlan::default();
    };
    crate::todo::load_plan(session_id).unwrap_or_default()
}

fn todo_card_payload_json(
    todos: &[TodoItem],
    plan: &crate::todo::TodoPlan,
    goals: &[crate::todo::TodoGoal],
) -> String {
    serde_json::to_string(&serde_json::json!({
        "todos": todos,
        "plan": plan,
        "goals": goals,
    }))
    .unwrap_or_else(|_| r#"{"todos":[],"goals":[]}"#.to_string())
}

fn build_todos_view_markdown(
    session_id: Option<&str>,
    todos: &[TodoItem],
    plan: &crate::todo::TodoPlan,
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

    markdown.push_str(&format_plan_markdown(plan));

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
    if let Some(state) = goal.closed_feedback_loop {
        line.push('\n');
        line.push_str(&format!("- Closed feedback loop: **{}**\n", state.as_str()));
    }
    if let Some(feedback_loop) = goal
        .feedback_loop
        .as_deref()
        .filter(|feedback_loop| !feedback_loop.trim().is_empty())
    {
        if line.is_empty() {
            line.push('\n');
        }
        line.push_str(&format!("- Feedback loop: {}\n", feedback_loop.trim()));
    }
    if let Some(state) = goal.feedback_loop_relevance {
        line.push_str(&format!(
            "- Feedback-loop relevance: **{}**\n",
            state.as_str()
        ));
    }
    if let Some(state) = goal.feedback_loop_coverage {
        line.push_str(&format!(
            "- Feedback-loop coverage: **{}**\n",
            state.as_str()
        ));
    }
    if let Some(state) = goal.feedback_loop_traceability {
        line.push_str(&format!(
            "- Feedback-loop traceability: **{}**\n",
            state.as_str()
        ));
    }
    if let Some(state) = goal.delivery_state {
        line.push_str(&format!("- Delivery state: **{}**\n", state.as_str()));
    }
    if let Some(difficulty) = goal.difficulty {
        line.push_str(&format!("- Difficulty: {}\n", difficulty.as_str()));
    }
    if let Some(autonomy) = goal.autonomy {
        line.push_str(&format!("- Autonomy: {}\n", autonomy.as_str()));
    }
    line
}

/// Plan-level intent lines, shown once for the whole todo list.
fn format_plan_markdown(plan: &crate::todo::TodoPlan) -> String {
    let mut markdown = String::new();
    if let Some(intention) = plan
        .user_intention
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        markdown.push_str(&format!("- User intention: {}\n", intention));
    }
    if let Some(state) = plan.understands_user_intent {
        markdown.push_str(&format!(
            "- Understands user intent: **{}**\n",
            state.as_str()
        ));
    }
    markdown
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

fn todo_effective_confidence(todo: &TodoItem) -> Option<crate::todo::ConfidenceState> {
    if todo.status == "completed" {
        todo.completion_confidence.or(todo.confidence)
    } else {
        todo.confidence
    }
}

/// Weighted-typical confidence across the list, reported as the semantic
/// state nearest the weighted mean of representative scores.
fn weighted_todo_confidence(todos: &[TodoItem]) -> Option<crate::todo::ConfidenceState> {
    let mut weighted_sum = 0u32;
    let mut total_weight = 0u32;
    for todo in todos.iter().filter(|todo| todo.status != "cancelled") {
        let Some(state) = todo_effective_confidence(todo) else {
            continue;
        };
        let weight = todo_confidence_weight(&todo.priority);
        weighted_sum += u32::from(state.legacy_score()) * weight;
        total_weight += weight;
    }
    if total_weight == 0 {
        None
    } else {
        Some(crate::todo::ConfidenceState::from_legacy_score(
            ((weighted_sum + total_weight / 2) / total_weight) as u8,
        ))
    }
}

fn format_confidence_value(state: Option<crate::todo::ConfidenceState>) -> String {
    state
        .map(|state| state.as_str().to_string())
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
    plan: &crate::todo::TodoPlan,
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
    plan.user_intention.hash(&mut hasher);
    plan.understands_user_intent.hash(&mut hasher);
    for goal in goals {
        goal.group.hash(&mut hasher);
        goal.closed_feedback_loop.hash(&mut hasher);
        goal.feedback_loop.hash(&mut hasher);
        goal.feedback_loop_relevance.hash(&mut hasher);
        goal.feedback_loop_coverage.hash(&mut hasher);
        goal.feedback_loop_traceability.hash(&mut hasher);
        goal.delivery_state.hash(&mut hasher);
        goal.difficulty.hash(&mut hasher);
        goal.autonomy.hash(&mut hasher);
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

    /// Plan-level intent used by the view tests.
    fn plan() -> crate::todo::TodoPlan {
        crate::todo::TodoPlan {
            user_intention: Some("make navigation feel immediate".to_string()),
            understands_user_intent: Some(crate::todo::IntentUnderstanding::from_legacy_score(96)),
            ..Default::default()
        }
    }

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
            confidence: confidence.map(crate::todo::ConfidenceState::from_legacy_score),
            completion_confidence: completion_confidence
                .map(crate::todo::ConfidenceState::from_legacy_score),
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

        let markdown = build_todos_view_markdown(Some("session_test"), &todos, &plan(), &[]);

        assert!(markdown.contains("- Weighted confidence: **plausible**"));
        assert!(markdown.contains("- Lowest completed confidence: **plausible**"));
        assert!(markdown.contains("- Missing completion confidence: 0"));
        assert!(markdown.contains("  - confidence: `plausible`"));
        assert!(markdown.contains("  - confidence: `plausible`"));
        assert!(markdown.contains("  - completion confidence: `plausible`"));
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
        let before = hash_todos_payload(Some("session_test"), &todos, &plan(), &[]);
        todos[0].confidence = Some(crate::todo::ConfidenceState::Validated);
        let after = hash_todos_payload(Some("session_test"), &todos, &plan(), &[]);

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
            &plan(),
            &[crate::todo::TodoGoal {
                group: Some("optimize rendering".to_string()),
                closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(90)),
                feedback_loop: Some(
                    "run the frame benchmark and compare p95 frame time".to_string(),
                ),
                feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
                feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
                delivery_state: Some(crate::todo::DeliveryState::from_legacy_score(85)),
                ..Default::default()
            }],
        );

        assert!(
            markdown.contains("## optimize rendering (1/2)"),
            "{markdown}"
        );
        // Plan-level intent renders once for the whole list.
        assert!(
            markdown.contains("- User intention: make navigation feel immediate"),
            "{markdown}"
        );
        assert!(
            markdown.contains("- Understands user intent: **clear**"),
            "{markdown}"
        );
        assert!(
            markdown.contains("- Closed feedback loop: **strong**"),
            "{markdown}"
        );
        assert!(
            markdown
                .contains("- Feedback loop: run the frame benchmark and compare p95 frame time"),
            "{markdown}"
        );
        assert!(
            markdown.contains("- Feedback-loop relevance: **representative**"),
            "{markdown}"
        );
        assert!(
            markdown.contains("- Feedback-loop coverage: **main_paths**"),
            "{markdown}"
        );
        assert!(
            markdown.contains("- Delivery state: **workflow_validated**"),
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
        let before = hash_todos_payload(Some("session_test"), &todos, &plan(), &[]);
        todos[0].group = Some("rendering".to_string());
        let after = hash_todos_payload(Some("session_test"), &todos, &plan(), &[]);
        assert_ne!(before, after);
    }

    #[test]
    fn todos_view_hash_changes_when_goals_change() {
        let todos = vec![todo("g", "Goal hash", "pending", "high", Some(80), None)];
        let before = hash_todos_payload(Some("session_test"), &todos, &plan(), &[]);
        let goals = vec![crate::todo::TodoGoal {
            closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(30)),
            ..Default::default()
        }];
        let after = hash_todos_payload(Some("session_test"), &todos, &plan(), &goals);
        assert_ne!(before, after);
    }

    #[test]
    fn todos_view_hash_changes_when_feedback_loop_changes() {
        let todos = vec![todo("g", "Goal hash", "pending", "high", Some(80), None)];
        let mut goals = vec![crate::todo::TodoGoal {
            feedback_loop: Some("run test A".to_string()),
            ..Default::default()
        }];
        let before = hash_todos_payload(Some("session_test"), &todos, &plan(), &goals);
        goals[0].feedback_loop = Some("run test B".to_string());
        let after = hash_todos_payload(Some("session_test"), &todos, &plan(), &goals);
        assert_ne!(before, after);
    }

    #[test]
    fn todos_view_hash_changes_when_user_intention_changes() {
        let todos = vec![todo("g", "Goal hash", "pending", "high", Some(80), None)];
        let mut current = plan();
        let before = hash_todos_payload(Some("session_test"), &todos, &current, &[]);
        current.user_intention = Some("increase clarity".to_string());
        let after = hash_todos_payload(Some("session_test"), &todos, &current, &[]);
        assert_ne!(before, after);
    }

    #[test]
    fn todos_view_hash_changes_when_intent_understanding_changes() {
        let todos = vec![todo("g", "Goal hash", "pending", "high", Some(80), None)];
        let mut current = plan();
        let before = hash_todos_payload(Some("session_test"), &todos, &current, &[]);
        current.understands_user_intent = Some(crate::todo::IntentUnderstanding::Complete);
        let after = hash_todos_payload(Some("session_test"), &todos, &current, &[]);
        assert_ne!(before, after);
    }
}
