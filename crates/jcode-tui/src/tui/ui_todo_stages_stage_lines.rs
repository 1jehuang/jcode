//! Todo stage line rendering.

use super::*;

/// Render staged todos with a bounded row per lane and optional prerequisite
/// progress. Later lanes retain phase dimming unless explicit prerequisites
/// replace that barrier and have been satisfied.
#[cfg(test)]
pub(crate) fn push_stage_lines(
    lines: &mut Vec<Line<'static>>,
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
    background_tasks: &[TodoCardBackgroundTask],
    base_indent: &str,
    inner_width: usize,
) {
    push_ordered_stage_lines(
        lines,
        todos,
        agents,
        background_tasks,
        base_indent,
        inner_width,
        false,
    );
}

pub(crate) fn push_ordered_stage_lines(
    lines: &mut Vec<Line<'static>>,
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
    background_tasks: &[TodoCardBackgroundTask],
    base_indent: &str,
    inner_width: usize,
    active_first: bool,
) {
    let stages = ordered_layout(todos);
    let current = current_stage(&stages, todos);
    let mut lanes: Vec<(usize, &crate::todo::stages::TodoLane)> = stages
        .iter()
        .enumerate()
        .flat_map(|(index, stage)| stage.lanes.iter().map(move |lane| (index, lane)))
        .collect();
    let items_of = |lane: &crate::todo::stages::TodoLane| -> Vec<&crate::todo::TodoItem> {
        lane.items.iter().map(|&i| todos[i]).collect()
    };
    // A name shared by every lane (typically a placeholder such as "main" on
    // a plan with no parallel work) distinguishes nothing, so it is not drawn.
    let uniform_name =
        lanes.len() > 1 && lanes.iter().all(|(_, lane)| lane.group == lanes[0].1.group);
    let shown_group = |lane: &crate::todo::stages::TodoLane| -> Option<String> {
        (!uniform_name).then(|| lane.group.clone()).flatten()
    };
    let strip_width = lanes
        .iter()
        .map(|(_, lane)| {
            lane_item_statuses(&items_of(lane), agents)
                .iter()
                .map(|(status, _)| status_pip_span(status).content.width())
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0)
        .min((inner_width / 3).max(1));
    let name_width = lanes
        .iter()
        .map(|(_, lane)| {
            width_of(&lane_name_spans(
                shown_group(lane).as_deref(),
                Style::default(),
                &items_of(lane),
                agents,
            ))
        })
        .max()
        .unwrap_or(0)
        .min(inner_width / 3);

    if active_first {
        lanes.sort_by_cached_key(|(_, lane)| work_rank(&items_of(lane), agents));
    }
    for (phase, lane) in lanes {
        // A lane wears its phase's colour: bold while its phase is active,
        // dimmed (with its titles and dots) while the phase has not started.
        let items = items_of(lane);
        let prerequisites = lane_prerequisite_progress(&items, todos);
        let later = prerequisites.map_or_else(
            || current.is_some_and(|current| phase > current),
            |(complete, total)| complete < total,
        );
        // Explicit prerequisites replace the phase barrier. Keep a ready
        // later-phase lane bright even while unrelated earlier work runs.
        let lane_style = if prerequisites.is_some() && !later && items.iter().any(|t| !is_done(t)) {
            Style::default()
                .fg(asap_color())
                .add_modifier(Modifier::BOLD)
        } else {
            phase_style(phase, current)
        };
        if let Some((complete, total)) = prerequisites {
            let state = if complete == total { "met" } else { "waiting" };
            let name = lane
                .group
                .as_deref()
                .unwrap_or_else(|| items[0].id.as_str());
            let note = Span::styled(
                format!("  {name}: after {complete}/{total} · {state}"),
                Style::default().fg(todo_meta_color()),
            );
            lines.push(todo_card_line(vec![note], base_indent, inner_width));
        }
        // Expand a waiting lane so its dependency progress belongs to the
        // corresponding todo rather than an ambiguous lane-wide status dot.
        if items.iter().any(|todo| {
            crate::todo::todo_requires_user_auth(todo)
                || !todo_running_dependencies(todo, background_tasks).is_empty()
        }) {
            for todo in &items {
                lines.push(render_todo_item_line_with_background(
                    todo,
                    agents,
                    background_tasks,
                    base_indent,
                    inner_width,
                ));
            }
            push_lane_status_lines_below_rows(lines, &items, agents, base_indent, inner_width);
            continue;
        }
        let statuses = lane_item_statuses(&items, agents);
        let mut telemetry = Vec::new();
        if items.iter().any(|todo| {
            todo.assigned_to.as_deref().is_some_and(|owner| {
                agents
                    .iter()
                    .any(|agent| agent.matches(owner) && agent.live_status.is_some())
            })
        }) {
            push_lane_status_lines(&mut telemetry, &items, agents, "", inner_width);
        }
        if !telemetry.is_empty() {
            // Column budgets depend only on terminal width, never changing
            // worker names, task counts, token totals, or live status text.
            let pip_width = 6.min((inner_width / 10).max(1));
            let worker_width = 24.min(inner_width / 4);
            // Progress pips stay static when the same row already has a live
            // activity indicator beside the worker's status.
            let static_statuses: Vec<_> = statuses
                .iter()
                .map(|(status, _)| (*status, false))
                .collect();
            let mut row = lane_strip_spans(&static_statuses, pip_width);
            if let Some(padding) = row.last_mut() {
                // Retain the unused pip cells, replacing only the separator.
                let pad = padding.content.width().saturating_sub(1);
                padding.content = " ".repeat(pad).into();
            }
            let name = lane_name_spans(shown_group(lane).as_deref(), lane_style, &items, agents);
            let name =
                super::super::truncate_line_with_ellipsis_to_width(&Line::from(name), worker_width);
            let name_padding = worker_width.saturating_sub(name.width());
            row.extend(name.spans);
            row.push(Span::raw(" ".repeat(name_padding)));
            row.push(Span::raw(" · "));
            let room = inner_width.saturating_sub(width_of(&row));
            let mut status = Vec::new();
            for (index, line) in telemetry.into_iter().enumerate() {
                if index > 0 {
                    status.push(Span::raw(" · "));
                }
                // Strip only the legacy continuation-row indent.
                status.extend(line.spans.into_iter().skip(2));
            }
            if let Some(live) =
                lane_live_agent(&items, agents).and_then(|agent| agent.live_status.as_ref())
                && live.phase != "streaming"
                && let Some(tps) = live.output_tps_milli
            {
                status.push(Span::styled(
                    format!(" · {:.1} tps", tps as f64 / 1000.0),
                    Style::default().fg(todo_meta_color()),
                ));
            }
            // Live activity owns the remaining width instead of repeating task titles.
            let status =
                super::super::truncate_line_with_ellipsis_to_width(&Line::from(status), room);
            row.extend(status.spans);
            if later {
                row = row.into_iter().map(dim_span).collect();
            }
            lines.push(todo_card_line(row, base_indent, inner_width));
            continue;
        }
        let mut row = lane_strip_spans(&statuses, strip_width);
        if name_width > 0 {
            let name = lane_name_spans(shown_group(lane).as_deref(), lane_style, &items, agents);
            let name =
                super::super::truncate_line_with_ellipsis_to_width(&Line::from(name), name_width)
                    .spans;
            let pad = name_width.saturating_sub(width_of(&name)) + 2;
            row.extend(name);
            row.push(Span::raw(" ".repeat(pad)));
        }
        let room = inner_width.saturating_sub(width_of(&row));
        // The live note (intent, liveness) gets up to a third of the row so it
        // is not always crowded out by titles; titles take the rest. On narrow
        // rows the note is dropped entirely rather than wrapping.
        let note = lane_note_spans(&items, agents);
        let note_width = width_of(&note);
        let note_budget = if note.is_empty() || room < 40 {
            0
        } else {
            (note_width + 2).min(room / 3).min(40)
        };
        let titles = lane_titles_spans(&items, room.saturating_sub(note_budget));
        let titles_width = width_of(&titles);
        row.extend(titles);
        if note_budget >= 8 {
            let space = room.saturating_sub(titles_width).min(note_budget);
            let mut note_line = vec![Span::raw("  ")];
            note_line.extend(note);
            row.extend(
                super::super::truncate_line_with_ellipsis_to_width(&Line::from(note_line), space)
                    .spans,
            );
        }
        if later {
            row = row.into_iter().map(dim_span).collect();
        }
        lines.push(todo_card_line(row, base_indent, inner_width));
    }
}
