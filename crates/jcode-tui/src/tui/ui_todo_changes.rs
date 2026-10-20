//! Render a compact "what changed" delta below a `todo` tool call in the chat
//! transcript.
//!
//! The transcript renderer is stateless per message, so the previous todo list
//! is recovered from the most recent earlier `todo` tool message rather than
//! threaded through tool metadata. This keeps the feature reload-safe and does
//! not touch the model-facing tool output (no token cost).
//!
//! The display automatically chooses between two forms based on how much
//! changed:
//! - **Form A (one line):** trivial changes (a single status flip, a lone
//!   content edit) collapse to one summary line.
//! - **Form B (delta block):** substantive changes (items added/removed, the
//!   first plan, or more than one status change) show a header plus one line
//!   per changed item.

use super::*;
use crate::message::ToolCall;
use crate::todo::TodoItem;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Maximum number of per-item change lines shown in the delta block before
/// collapsing the remainder into a "+N more" footer.
const MAX_CHANGE_LINES: usize = 6;

/// Parse the todo list a `todo` tool call wrote. Returns `None` for read calls
/// (no `todos` argument) or when the payload cannot be parsed for display.
pub(super) fn todos_from_tool_input(tc: &ToolCall) -> Option<Vec<TodoItem>> {
    let todos = tc.input.get("todos")?;
    if todos.is_null() {
        return None;
    }
    serde_json::from_value::<Vec<TodoItem>>(todos.clone()).ok()
}

/// Find the todo list written by the most recent `todo` tool message before
/// `current_abs_idx`. Skips read-only todo calls so a write's delta is computed
/// against the previous *write*.
pub(super) fn previous_todos(
    messages: &[DisplayMessage],
    current_abs_idx: usize,
) -> Option<Vec<TodoItem>> {
    let end = current_abs_idx.min(messages.len());
    messages[..end]
        .iter()
        .rev()
        .filter(|msg| msg.effective_role() == "tool")
        .filter_map(|msg| msg.tool_data.as_ref())
        .filter(|tc| tools_ui::canonical_tool_name(&tc.name) == "todo")
        .find_map(todos_from_tool_input)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TodoChangeKind {
    Added,
    Removed,
    StatusChanged { to: String, blocked: bool },
    ContentEdited { status: String, blocked: bool },
}

#[derive(Debug, Clone)]
struct TodoChange {
    kind: TodoChangeKind,
    content: String,
}

#[derive(Debug, Default)]
struct TodoDiff {
    changes: Vec<TodoChange>,
    is_first: bool,
    added: usize,
    removed: usize,
    status_changes: usize,
    to_completed: usize,
    to_in_progress: usize,
    to_cancelled: usize,
    reopened: usize,
    edited: usize,
    completed: usize,
    total: usize,
}

impl TodoDiff {
    /// Form B (multi-line block) vs Form A (one line). Substantive changes get
    /// the block; minor tweaks collapse to a single summary line.
    fn is_substantive(&self) -> bool {
        self.is_first || self.added > 0 || self.removed > 0 || self.status_changes > 1
    }
}

fn blocked_of(todo: &TodoItem) -> bool {
    !todo.blocked_by.is_empty()
}

fn compute_diff(prev: Option<&[TodoItem]>, next: &[TodoItem]) -> TodoDiff {
    let mut diff = TodoDiff {
        is_first: prev.is_none(),
        total: next.len(),
        completed: next.iter().filter(|t| t.status == "completed").count(),
        ..Default::default()
    };

    let prev_items = prev.unwrap_or(&[]);
    let find_prev = |id: &str| prev_items.iter().find(|p| p.id == id);

    for item in next {
        match find_prev(&item.id) {
            None => {
                diff.added += 1;
                diff.changes.push(TodoChange {
                    kind: TodoChangeKind::Added,
                    content: item.content.clone(),
                });
            }
            Some(prev_item) => {
                if prev_item.status != item.status {
                    diff.status_changes += 1;
                    match item.status.as_str() {
                        "completed" => diff.to_completed += 1,
                        "in_progress" => diff.to_in_progress += 1,
                        "cancelled" => diff.to_cancelled += 1,
                        _ => diff.reopened += 1,
                    }
                    diff.changes.push(TodoChange {
                        kind: TodoChangeKind::StatusChanged {
                            to: item.status.clone(),
                            blocked: blocked_of(item),
                        },
                        content: item.content.clone(),
                    });
                } else if prev_item.content != item.content {
                    diff.edited += 1;
                    diff.changes.push(TodoChange {
                        kind: TodoChangeKind::ContentEdited {
                            status: item.status.clone(),
                            blocked: blocked_of(item),
                        },
                        content: item.content.clone(),
                    });
                }
            }
        }
    }

    for prev_item in prev_items {
        if !next.iter().any(|n| n.id == prev_item.id) {
            diff.removed += 1;
            diff.changes.push(TodoChange {
                kind: TodoChangeKind::Removed,
                content: prev_item.content.clone(),
            });
        }
    }

    diff
}

fn status_icon(status: &str, blocked: bool) -> (&'static str, Color) {
    if blocked && status != "completed" {
        return ("⊳", rgb(180, 140, 100));
    }
    match status {
        "completed" => ("✓", rgb(100, 180, 100)),
        "in_progress" => ("▶", rgb(255, 200, 100)),
        "cancelled" => ("✗", rgb(150, 90, 90)),
        _ => ("○", rgb(120, 120, 130)),
    }
}

/// Leading marker (glyph + color) and the text color used for a change line.
fn change_marker(kind: &TodoChangeKind) -> (&'static str, Color, Color) {
    match kind {
        TodoChangeKind::Added => ("+", rgb(100, 180, 100), rgb(190, 200, 195)),
        TodoChangeKind::Removed => ("-", rgb(200, 110, 100), rgb(120, 120, 125)),
        TodoChangeKind::StatusChanged { to, blocked } => {
            let (icon, color) = status_icon(to, *blocked);
            let text = if to == "completed" || to == "cancelled" {
                rgb(120, 120, 130)
            } else {
                rgb(190, 195, 205)
            };
            (icon, color, text)
        }
        TodoChangeKind::ContentEdited { status, blocked } => {
            let (icon, _) = status_icon(status, *blocked);
            (icon, rgb(150, 150, 160), rgb(180, 185, 195))
        }
    }
}

fn summary_text(diff: &TodoDiff) -> String {
    let mut parts: Vec<String> = Vec::new();
    if diff.to_completed > 0 {
        parts.push(format!("{} done", diff.to_completed));
    }
    if diff.to_in_progress > 0 {
        parts.push(format!("{} started", diff.to_in_progress));
    }
    if diff.reopened > 0 {
        parts.push(format!("{} reopened", diff.reopened));
    }
    if diff.to_cancelled > 0 {
        parts.push(format!("{} cancelled", diff.to_cancelled));
    }
    if diff.added > 0 {
        parts.push(format!("{} added", diff.added));
    }
    if diff.removed > 0 {
        parts.push(format!("{} removed", diff.removed));
    }
    if diff.edited > 0 {
        parts.push(format!("{} edited", diff.edited));
    }
    if parts.is_empty() {
        "updated".to_string()
    } else {
        parts.join(" · ")
    }
}

fn progress_span(diff: &TodoDiff) -> Span<'static> {
    Span::styled(
        format!("  ({}/{})", diff.completed, diff.total),
        Style::default().fg(rgb(120, 120, 135)),
    )
}

fn header_line(diff: &TodoDiff, width: u16) -> Line<'static> {
    let spans = vec![
        Span::styled("  ↳ ", Style::default().fg(dim_color())),
        Span::styled(summary_text(diff), Style::default().fg(rgb(150, 155, 170))),
        progress_span(diff),
    ];
    super::truncate_line_with_ellipsis_to_width(&Line::from(spans), width as usize)
}

fn change_line(change: &TodoChange, indent: &str, width: u16) -> Line<'static> {
    let (glyph, glyph_color, text_color) = change_marker(&change.kind);
    let spans = vec![
        Span::raw(indent.to_string()),
        Span::styled(format!("{} ", glyph), Style::default().fg(glyph_color)),
        Span::styled(change.content.clone(), Style::default().fg(text_color)),
    ];
    super::truncate_line_with_ellipsis_to_width(&Line::from(spans), width as usize)
}

/// Build the delta lines to inject below a `todo` tool message. Returns an empty
/// vec when nothing meaningful changed (reads, no-op writes).
pub(super) fn render_todo_change_lines(
    prev: Option<&[TodoItem]>,
    next: &[TodoItem],
    width: u16,
) -> Vec<Line<'static>> {
    let diff = compute_diff(prev, next);
    if diff.changes.is_empty() {
        return Vec::new();
    }

    // Form A: collapse to a single line.
    if !diff.is_substantive() {
        if diff.changes.len() == 1 {
            let change = &diff.changes[0];
            let (glyph, glyph_color, text_color) = change_marker(&change.kind);
            let spans = vec![
                Span::styled("  ↳ ", Style::default().fg(dim_color())),
                Span::styled(format!("{} ", glyph), Style::default().fg(glyph_color)),
                Span::styled(change.content.clone(), Style::default().fg(text_color)),
                progress_span(&diff),
            ];
            return vec![super::truncate_line_with_ellipsis_to_width(
                &Line::from(spans),
                width as usize,
            )];
        }
        return vec![header_line(&diff, width)];
    }

    // Form B: header plus one line per changed item (capped).
    let mut lines = vec![header_line(&diff, width)];
    let indent = "    ";
    let shown = diff.changes.len().min(MAX_CHANGE_LINES);
    for change in diff.changes.iter().take(shown) {
        lines.push(change_line(change, indent, width));
    }
    if diff.changes.len() > shown {
        let remaining = diff.changes.len() - shown;
        lines.push(super::truncate_line_with_ellipsis_to_width(
            &Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(
                    format!("+{} more", remaining),
                    Style::default().fg(rgb(110, 110, 120)),
                ),
            ]),
            width as usize,
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(id: &str, content: &str, status: &str) -> TodoItem {
        TodoItem {
            id: id.to_string(),
            content: content.to_string(),
            status: status.to_string(),
            priority: "medium".to_string(),
            group: None,
            confidence: None,
            completion_confidence: None,
            confidence_history: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
            wave: None,
        }
    }

    #[test]
    fn first_write_is_substantive_and_counts_all_added() {
        let next = vec![todo("1", "A", "pending"), todo("2", "B", "in_progress")];
        let diff = compute_diff(None, &next);
        assert!(diff.is_first);
        assert!(diff.is_substantive());
        assert_eq!(diff.added, 2);
        assert_eq!(diff.changes.len(), 2);
    }

    #[test]
    fn single_status_flip_is_form_a() {
        let prev = vec![todo("1", "A", "pending"), todo("2", "B", "pending")];
        let next = vec![todo("1", "A", "in_progress"), todo("2", "B", "pending")];
        let diff = compute_diff(Some(&prev), &next);
        assert!(!diff.is_substantive());
        assert_eq!(diff.status_changes, 1);
        assert_eq!(diff.to_in_progress, 1);
        let lines = render_todo_change_lines(Some(&prev), &next, 80);
        assert_eq!(lines.len(), 1, "form A should be a single line");
    }

    #[test]
    fn two_status_changes_is_form_b() {
        let prev = vec![todo("1", "A", "pending"), todo("2", "B", "in_progress")];
        let next = vec![todo("1", "A", "in_progress"), todo("2", "B", "completed")];
        let diff = compute_diff(Some(&prev), &next);
        assert!(diff.is_substantive());
        let lines = render_todo_change_lines(Some(&prev), &next, 80);
        // header + 2 item lines
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn add_and_remove_detected() {
        let prev = vec![todo("1", "A", "pending"), todo("2", "B", "pending")];
        let next = vec![todo("1", "A", "pending"), todo("3", "C", "pending")];
        let diff = compute_diff(Some(&prev), &next);
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
        assert!(diff.is_substantive());
    }

    #[test]
    fn no_change_yields_no_lines() {
        let prev = vec![todo("1", "A", "pending")];
        let next = vec![todo("1", "A", "pending")];
        let lines = render_todo_change_lines(Some(&prev), &next, 80);
        assert!(lines.is_empty());
    }

    #[test]
    fn content_edit_is_form_a() {
        let prev = vec![todo("1", "A", "pending")];
        let next = vec![todo("1", "A edited", "pending")];
        let diff = compute_diff(Some(&prev), &next);
        assert_eq!(diff.edited, 1);
        assert!(!diff.is_substantive());
        let lines = render_todo_change_lines(Some(&prev), &next, 80);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn change_block_caps_at_max_lines() {
        let next: Vec<TodoItem> = (0..10)
            .map(|i| todo(&i.to_string(), &format!("item {i}"), "pending"))
            .collect();
        let lines = render_todo_change_lines(None, &next, 80);
        // header + MAX_CHANGE_LINES items + "+N more"
        assert_eq!(lines.len(), 1 + MAX_CHANGE_LINES + 1);
    }
}
