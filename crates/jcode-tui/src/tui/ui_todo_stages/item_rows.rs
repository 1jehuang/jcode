use super::*;

/// One todo row for a list without stages, with an optional agent chip.
///
/// Width priority when space runs out: the activity note shrinks first, then
/// the task text; the status glyph and agent emoji always stay visible.
pub(crate) fn render_todo_item_line(
    todo: &crate::todo::TodoItem,
    agents: &[TodoCardAgent],
    base_indent: &str,
    inner_width: usize,
) -> Line<'static> {
    if crate::todo::todo_completely_blocked(todo)
        || todo.status != "in_progress"
        || todo
            .group
            .as_deref()
            .is_some_and(|group| !group.trim().is_empty())
    {
        return render_todo_card_item_line(todo, base_indent, inner_width);
    }
    let Some(assigned) = todo
        .assigned_to
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return render_todo_card_item_line(todo, base_indent, inner_width);
    };
    // Agent rows drop the confidence suffix: the agent state is the more
    // useful signal in a width-constrained row.
    let mut spans = todo_card_item_spans(todo, false);
    let agent = agents.iter().find(|agent| agent.matches(assigned));
    let owner = if assigned == "@self" {
        Span::styled("you", Style::default().fg(rgb(255, 200, 100)))
    } else {
        Span::raw(
            agent
                .map(TodoCardAgent::icon)
                .unwrap_or_else(|| fallback_icon(assigned)),
        )
    };
    let mut chip = vec![Span::raw("  "), owner];
    let mut note = String::new();
    if let Some(agent) = agent {
        if let Some((mark, color)) = agent_state_mark(&agent.status) {
            chip.push(Span::styled(mark, Style::default().fg(color)));
        }
        if todo.status != "completed" && agent.live_status.is_none() {
            note = agent_note(agent, &[todo]);
        }
    }
    let chip_width = width_of(&chip);
    let head_width = width_of(&spans);
    if head_width + chip_width > inner_width {
        // Shrink the task text (last span) so the emoji chip still fits.
        let fixed: usize = width_of(&spans[..spans.len().saturating_sub(1)]);
        let keep = inner_width.saturating_sub(fixed + chip_width + 1);
        if let Some(content) = spans.last_mut() {
            let truncated = truncate_to_width(&content.content, keep);
            content.content = format!("{truncated}…").into();
        }
    } else if !note.is_empty() {
        let room = inner_width.saturating_sub(head_width + chip_width + 1);
        if room >= 4 {
            let shown = if note.width() > room {
                format!("{}…", truncate_to_width(&note, room - 1))
            } else {
                note
            };
            chip.push(Span::styled(
                format!(" {shown}"),
                Style::default().fg(todo_meta_color()),
            ));
        }
    }
    spans.extend(chip);
    todo_card_line(spans, base_indent, inner_width)
}

pub(super) fn truncate_to_width(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > max {
            break;
        }
        width += w;
        out.push(ch);
    }
    out
}

/// `Waiting for <task> · 18m` for a live worker blocked on this todo.
pub(crate) fn todo_wait_note(
    todo: &crate::todo::TodoItem,
    agents: &[TodoCardAgent],
) -> Option<String> {
    if crate::todo::todo_completely_blocked(todo) || is_done(todo) {
        return None;
    }
    let agent = todo
        .assigned_to
        .as_deref()
        .and_then(|owner| agents.iter().find(|agent| agent.matches(owner)))?;
    if !is_live_status(&agent.status) && agent.status != "waiting" {
        return None;
    }
    let reason = agent.wait_reason.as_deref()?.lines().next()?.trim();
    if reason.is_empty() {
        return None;
    }
    let mut note = reason.to_string();
    if let Some(age) = agent.wait_age_secs.filter(|age| *age >= 2) {
        note.push_str(" · ");
        note.push_str(&jcode_tui_render::swarm_gallery::humanize_age(age));
    }
    Some(note)
}

pub(crate) fn render_todo_item_line_with_background(
    todo: &crate::todo::TodoItem,
    agents: &[TodoCardAgent],
    tasks: &[TodoCardBackgroundTask],
    base_indent: &str,
    inner_width: usize,
) -> Line<'static> {
    let dependencies = todo_running_dependencies(todo, tasks);
    let wait_note = todo_wait_note(todo, agents);
    if dependencies.is_empty() || crate::todo::todo_completely_blocked(todo) {
        let mut line = render_todo_item_line(todo, agents, base_indent, inner_width);
        if let Some(note) = wait_note {
            line.spans.push(Span::styled(
                format!("  ◷ {note}"),
                Style::default().fg(todo_warning_color()),
            ));
            line = super::truncate_line_with_ellipsis_to_width(
                &line,
                inner_width.saturating_add(base_indent.width()),
            );
        }
        return line;
    }
    let mut spans = todo_card_item_spans(todo, false);
    spans[1] = Span::styled("◷ ", Style::default().fg(todo_warning_color()));
    if let Some(note) = wait_note {
        spans.push(Span::styled(
            format!("  {note}"),
            Style::default().fg(todo_warning_color()),
        ));
    }
    let title_width = spans.iter().map(|span| span.content.width()).sum::<usize>();
    let progress = todo_dependency_progress(
        &dependencies,
        inner_width
            .saturating_sub(title_width + 2)
            .max(inner_width.saturating_sub(12).min(inner_width / 2)),
    );
    let progress_width = progress.width();
    if progress_width > 0 {
        let left_width = inner_width.saturating_sub(progress_width + 2);
        let mut left = super::truncate_line_with_ellipsis_to_width(&Line::from(spans), left_width);
        let pad = inner_width.saturating_sub(left.width() + progress_width);
        left.spans.push(Span::raw(" ".repeat(pad)));
        left.spans.extend(progress.spans);
        spans = left.spans;
    }
    todo_card_line(spans, base_indent, inner_width)
}
