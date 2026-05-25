use super::*;

/// Render todos widget content
pub(super) fn render_todos_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    if data.todos.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<Line> = Vec::new();
    let total = data.todos.len();
    let completed: usize = data
        .todos
        .iter()
        .filter(|t| t.status == "completed")
        .count();
    let _in_progress: usize = data
        .todos
        .iter()
        .filter(|t| t.status == "in_progress")
        .count();

    // Header with progress
    lines.push(Line::from(vec![
        Span::styled("Todos ", Style::default().fg(rgb(180, 180, 190)).bold()),
        Span::styled(
            format!("{}/{}", completed, total),
            Style::default().fg(rgb(140, 140, 150)),
        ),
    ]));

    // Mini progress bar
    let bar_width = inner.width.saturating_sub(2).min(20) as usize;
    if bar_width >= 4 && total > 0 {
        let filled = ((completed as f64 / total as f64) * bar_width as f64).round() as usize;
        let empty = bar_width.saturating_sub(filled);
        lines.push(Line::from(vec![
            Span::styled("[", Style::default().fg(rgb(90, 90, 100))),
            Span::styled("█".repeat(filled), Style::default().fg(rgb(100, 180, 100))),
            Span::styled("░".repeat(empty), Style::default().fg(rgb(50, 50, 60))),
            Span::styled("]", Style::default().fg(rgb(90, 90, 100))),
        ]));
    }

    // Sort todos: in_progress first, then pending, then completed
    let mut sorted_todos: Vec<&crate::todo::TodoItem> = data.todos.iter().collect();
    sorted_todos.sort_by(|a, b| {
        let order = |s: &str| match s {
            "in_progress" => 0,
            "pending" => 1,
            "completed" => 2,
            "cancelled" => 3,
            _ => 4,
        };
        order(&a.status).cmp(&order(&b.status))
    });

    // Render todos (limit based on available height)
    let available_lines = inner.height.saturating_sub(2) as usize; // Account for header + bar
    for todo in sorted_todos.iter().take(available_lines.min(5)) {
        let is_blocked = !todo.blocked_by.is_empty();
        let (icon, status_color) = if is_blocked && todo.status != "completed" {
            ("⊳", rgb(180, 140, 100))
        } else {
            match todo.status.as_str() {
                "completed" => ("✓", rgb(100, 180, 100)),
                "in_progress" => ("▶", rgb(255, 200, 100)),
                "cancelled" => ("✗", rgb(120, 80, 80)),
                _ => ("○", rgb(120, 120, 130)),
            }
        };

        let suffix = if is_blocked && todo.status != "completed" {
            " (blocked)"
        } else {
            ""
        };
        let max_len = inner.width.saturating_sub(3 + suffix.len() as u16) as usize;
        let content = truncate_smart(&todo.content, max_len);

        let text_color = if todo.status == "completed" {
            rgb(100, 100, 110)
        } else if is_blocked {
            rgb(120, 120, 130)
        } else if todo.status == "in_progress" {
            rgb(200, 200, 210)
        } else {
            rgb(160, 160, 170)
        };

        let mut spans = vec![
            Span::styled(format!("{} ", icon), Style::default().fg(status_color)),
            Span::styled(content, Style::default().fg(text_color)),
        ];
        if !suffix.is_empty() {
            spans.push(Span::styled(
                suffix.to_string(),
                Style::default().fg(rgb(100, 100, 110)),
            ));
        }
        lines.push(Line::from(spans));
    }

    // Show count of remaining items
    let shown = available_lines.min(5).min(sorted_todos.len());
    if data.todos.len() > shown {
        let remaining = data.todos.len() - shown;
        lines.push(Line::from(vec![Span::styled(
            format!("  +{} more", remaining),
            Style::default().fg(rgb(100, 100, 110)),
        )]));
    }

    lines
}

pub(super) fn render_todos_expanded(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    if data.todos.is_empty() {
        return lines;
    }

    // Calculate stats
    let total = data.todos.len();
    let completed: usize = data
        .todos
        .iter()
        .filter(|t| t.status == "completed")
        .count();
    let _in_progress: usize = data
        .todos
        .iter()
        .filter(|t| t.status == "in_progress")
        .count();

    // Header with progress
    lines.push(Line::from(vec![
        Span::styled("Todos ", Style::default().fg(rgb(180, 180, 190)).bold()),
        Span::styled(
            format!("{}/{}", completed, total),
            Style::default().fg(rgb(140, 140, 150)),
        ),
    ]));

    // Mini progress bar
    let bar_width = inner.width.saturating_sub(2).min(20) as usize;
    if bar_width >= 4 && total > 0 {
        let filled = ((completed as f64 / total as f64) * bar_width as f64).round() as usize;
        let empty = bar_width.saturating_sub(filled);
        lines.push(Line::from(vec![
            Span::styled("[", Style::default().fg(rgb(90, 90, 100))),
            Span::styled("█".repeat(filled), Style::default().fg(rgb(100, 180, 100))),
            Span::styled("░".repeat(empty), Style::default().fg(rgb(50, 50, 60))),
            Span::styled("]", Style::default().fg(rgb(90, 90, 100))),
        ]));
    }

    // Sort todos: in_progress first, then pending, then completed
    let mut sorted_todos: Vec<&crate::todo::TodoItem> = data.todos.iter().collect();
    sorted_todos.sort_by(|a, b| {
        let order = |s: &str| match s {
            "in_progress" => 0,
            "pending" => 1,
            "completed" => 2,
            "cancelled" => 3,
            _ => 4,
        };
        order(&a.status).cmp(&order(&b.status))
    });

    // Render todos with priority colors
    let available_lines = MAX_TODO_LINES.saturating_sub(2); // Account for header + bar
    for todo in sorted_todos.iter().take(available_lines) {
        let is_blocked = !todo.blocked_by.is_empty();
        let (icon, status_color) = if is_blocked && todo.status != "completed" {
            ("⊳", rgb(180, 140, 100))
        } else {
            match todo.status.as_str() {
                "completed" => ("✓", rgb(100, 180, 100)),
                "in_progress" => ("▶", rgb(255, 200, 100)),
                "cancelled" => ("✗", rgb(120, 80, 80)),
                _ => ("○", rgb(120, 120, 130)),
            }
        };

        // Priority indicator
        let priority_marker = match todo.priority.as_str() {
            "high" => ("!", rgb(255, 120, 100)),
            "medium" => ("", rgb(200, 180, 100)),
            _ => ("", rgb(120, 120, 130)),
        };

        let suffix = if is_blocked && todo.status != "completed" {
            " (blocked)"
        } else {
            ""
        };
        let max_len = inner.width.saturating_sub(4 + suffix.len() as u16) as usize;
        let content = truncate_smart(&todo.content, max_len);

        // Dim completed and blocked items
        let text_color = if todo.status == "completed" {
            rgb(100, 100, 110)
        } else if is_blocked {
            rgb(120, 120, 130)
        } else if todo.status == "in_progress" {
            rgb(200, 200, 210)
        } else {
            rgb(160, 160, 170)
        };

        let mut spans = vec![Span::styled(
            format!("{} ", icon),
            Style::default().fg(status_color),
        )];

        if !priority_marker.0.is_empty() {
            spans.push(Span::styled(
                priority_marker.0,
                Style::default().fg(priority_marker.1),
            ));
        }

        spans.push(Span::styled(content, Style::default().fg(text_color)));

        if !suffix.is_empty() {
            spans.push(Span::styled(
                suffix.to_string(),
                Style::default().fg(rgb(100, 100, 110)),
            ));
        }

        lines.push(Line::from(spans));
    }

    // Show count of remaining items
    let shown = available_lines.min(sorted_todos.len());
    if data.todos.len() > shown {
        let remaining = data.todos.len() - shown;
        let remaining_completed = sorted_todos
            .iter()
            .skip(shown)
            .filter(|t| t.status == "completed")
            .count();
        let desc = if remaining_completed == remaining {
            format!("  +{} done", remaining)
        } else if remaining_completed > 0 {
            format!("  +{} more ({} done)", remaining, remaining_completed)
        } else {
            format!("  +{} more", remaining)
        };
        lines.push(Line::from(vec![Span::styled(
            desc,
            Style::default().fg(rgb(100, 100, 110)),
        )]));
    }

    lines
}

pub(super) fn render_todos_compact(data: &InfoWidgetData, _inner: Rect) -> Vec<Line<'static>> {
    if data.todos.is_empty() {
        return Vec::new();
    }
    let total = data.todos.len();
    let mut completed = 0usize;
    let mut in_progress = 0usize;
    for todo in &data.todos {
        match todo.status.as_str() {
            "completed" => completed += 1,
            "in_progress" => in_progress += 1,
            _ => {}
        }
    }
    let pending = total.saturating_sub(completed);
    vec![
        Line::from(vec![Span::styled(
            "Todos",
            Style::default().fg(rgb(180, 180, 190)).bold(),
        )]),
        Line::from(vec![
            Span::styled(
                format!("{} total", total),
                Style::default().fg(rgb(160, 160, 170)),
            ),
            Span::styled(" · ", Style::default().fg(rgb(100, 100, 110))),
            Span::styled(
                format!("{} active", in_progress),
                Style::default().fg(rgb(255, 200, 100)),
            ),
            Span::styled(" · ", Style::default().fg(rgb(100, 100, 110))),
            Span::styled(
                format!("{} open", pending),
                Style::default().fg(rgb(140, 140, 150)),
            ),
        ]),
    ]
}
