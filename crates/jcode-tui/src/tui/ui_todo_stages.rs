//! Stage rows and swarm agent chips for todo cards.
//!
//! Layout comes from [`crate::todo::stages::stage_layout`], the same function
//! `run_plan` uses, so the card always matches execution order. The header row
//! labels each phase with its pips (`phase 1 ●●●  phase 2 ○○`). Below it each
//! lane (todo group) is one row: its status strip, its name, then its titles
//! left to right. Lanes run in parallel within a phase, titles along a row in
//! sequence. No divider lines or rails. A list with no stages renders exactly
//! like a plain checklist.
//!
//! Agents are swarm workers linked through `TodoItem::assigned_to`. They are
//! shown as their session emoji (🦊, 🐑, ...) with a short live activity note.

use super::*;
use unicode_width::UnicodeWidthStr;

/// Display-only snapshot of a swarm worker attached to the todo card payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TodoCardAgent {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub status: String,
    /// Short live activity note (current tool intent or status detail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_secs: Option<u64>,
}

impl TodoCardAgent {
    pub(crate) fn from_member(member: &crate::protocol::SwarmMemberStatus) -> Self {
        let activity = member
            .todo_items
            .iter()
            .find(|item| item.status == "in_progress")
            .and_then(|item| {
                item.tool_intents
                    .iter()
                    .rev()
                    .find(|intent| intent.status == "running")
                    .or_else(|| item.tool_intents.last())
                    .map(|intent| {
                        let intent_text = intent.intent.trim();
                        if intent_text.is_empty() {
                            intent.tool_name.clone()
                        } else {
                            intent_text.to_string()
                        }
                    })
                    .or_else(|| Some(item.content.clone()))
            })
            .or_else(|| member.detail.clone())
            .map(|text| text.lines().next().unwrap_or_default().trim().to_string())
            .filter(|text| !text.is_empty());
        Self {
            session_id: member.session_id.clone(),
            name: member.friendly_name.clone(),
            status: member.status.clone(),
            activity,
            age_secs: member.status_age_secs,
        }
    }

    fn matches(&self, key: &str) -> bool {
        let key = key.trim();
        !key.is_empty()
            && (self.session_id == key
                || self.name.as_deref() == Some(key)
                || crate::id::extract_session_name(&self.session_id) == Some(key))
    }

    fn icon(&self) -> String {
        let name = self
            .name
            .as_deref()
            .or_else(|| crate::id::extract_session_name(&self.session_id));
        match name.map(crate::id::session_icon) {
            Some(icon) if icon != "💫" => icon.to_string(),
            _ => name
                .and_then(|name| name.chars().next())
                .map(|c| c.to_string())
                .unwrap_or_else(|| "◆".to_string()),
        }
    }
}

/// Emoji for an `assigned_to` value with no live member record.
fn fallback_icon(assigned_to: &str) -> String {
    let name = crate::id::extract_session_name(assigned_to).unwrap_or(assigned_to);
    let icon = crate::id::session_icon(name);
    if icon == "💫" {
        name.chars()
            .next()
            .map(|c| c.to_string())
            .unwrap_or_default()
    } else {
        icon.to_string()
    }
}

/// Roster of agent emoji for the card header, e.g. `🦊🐑🐢`.
pub(crate) fn agent_roster_spans(
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
) -> Vec<Span<'static>> {
    let mut seen: Vec<String> = Vec::new();
    let mut spans = Vec::new();
    for todo in todos {
        let Some(assigned) = todo.assigned_to.as_deref() else {
            continue;
        };
        let agent = agents.iter().find(|agent| agent.matches(assigned));
        let key = agent
            .map(|agent| agent.session_id.clone())
            .unwrap_or_else(|| assigned.to_string());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let icon = agent
            .map(TodoCardAgent::icon)
            .unwrap_or_else(|| fallback_icon(assigned));
        let failed =
            agent.is_some_and(|agent| matches!(agent.status.as_str(), "failed" | "crashed"));
        spans.push(Span::raw(icon));
        if failed {
            spans.push(Span::styled("✗", Style::default().fg(todo_failure_color())));
        }
    }
    if !spans.is_empty() {
        spans.insert(0, Span::raw("  "));
    }
    spans
}

fn agent_state_mark(status: &str) -> Option<(&'static str, Color)> {
    match status {
        "failed" | "crashed" => Some(("✗", todo_failure_color())),
        "blocked" | "waiting_network" | "running_stale" => Some(("⏸", rgb(225, 165, 90))),
        "stopped" => Some(("◼", todo_meta_color())),
        _ => None,
    }
}

fn stage_color() -> Color {
    rgb(150, 130, 210)
}

fn lane_header_color() -> Color {
    todo_group_color()
}

fn is_done(todo: &crate::todo::TodoItem) -> bool {
    matches!(todo.status.as_str(), "completed" | "cancelled")
}

fn width_of(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

/// Lay out todos with stage lanes ordered consistently: a group keeps the same
/// relative column order in every stage it appears in.
fn ordered_layout(todos: &[&crate::todo::TodoItem]) -> Vec<crate::todo::stages::TodoStage> {
    let mut stages = crate::todo::stages::stage_layout(todos);
    let mut order: Vec<Option<String>> = Vec::new();
    for stage in &stages {
        for lane in &stage.lanes {
            if lane.group.is_some() && !order.contains(&lane.group) {
                order.push(lane.group.clone());
            }
        }
    }
    for stage in &mut stages {
        stage.lanes.sort_by_key(|lane| {
            order
                .iter()
                .position(|group| lane.group.is_some() && *group == lane.group)
                .unwrap_or(usize::MAX)
        });
    }
    stages
}

/// Index of the first stage that still has open work.
fn current_stage(
    stages: &[crate::todo::stages::TodoStage],
    todos: &[&crate::todo::TodoItem],
) -> Option<usize> {
    stages
        .iter()
        .position(|stage| stage.item_indices().any(|i| !is_done(todos[i])))
}

fn pip_span(todo: &crate::todo::TodoItem) -> Span<'static> {
    let (glyph, color) = match todo.status.as_str() {
        "completed" => ("●", rgb(100, 180, 100)),
        "in_progress" => ("●", asap_color()),
        _ => ("○", rgb(90, 90, 105)),
    };
    Span::styled(glyph, Style::default().fg(color))
}

/// Header pips labelled per phase: `phase 1 ●●●  phase 2 ●○  phase 3 ○○`.
///
/// Narrower widths fall back to `1 ●●●  2 ●○`, then per-phase counts
/// (`1 ✓  2 1/2`), then plain proportional pips. A list without stages
/// produces the same pips as before.
pub(crate) fn push_stage_pips(
    spans: &mut Vec<Span<'static>>,
    todos: &[&crate::todo::TodoItem],
    max_width: usize,
) {
    if !crate::todo::stages::has_stages(todos.iter().copied()) {
        push_todo_status_pips(spans, todos.iter().copied(), max_width);
        return;
    }
    let stages = ordered_layout(todos);
    let current = current_stage(&stages, todos);
    let label_style = |index: usize| match current {
        Some(current) if index == current => Style::default().fg(stage_color()).bold(),
        Some(current) if index > current => Style::default().fg(rgb(105, 100, 135)),
        _ => Style::default().fg(stage_color()),
    };
    let variant = |prefix: &str, counts: bool| -> Vec<Span<'static>> {
        let mut out = Vec::new();
        for (index, stage) in stages.iter().enumerate() {
            if index > 0 {
                out.push(Span::raw("  "));
            }
            let label = match stage.number {
                Some(n) => format!("{prefix}{n} "),
                None => String::new(),
            };
            if !label.is_empty() {
                out.push(Span::styled(label, label_style(index)));
            }
            if counts {
                let total = stage.len();
                let done = stage.item_indices().filter(|&i| is_done(todos[i])).count();
                if done == total {
                    out.push(Span::styled("✓", Style::default().fg(rgb(100, 180, 100))));
                } else {
                    let color = if Some(index) == current {
                        asap_color()
                    } else {
                        rgb(120, 120, 135)
                    };
                    out.push(Span::styled(
                        format!("{done}/{total}"),
                        Style::default().fg(color),
                    ));
                }
            } else {
                out.extend(stage.item_indices().map(|i| pip_span(todos[i])));
            }
        }
        out
    };
    for candidate in [
        variant("phase ", false),
        variant("", false),
        variant("", true),
    ] {
        if width_of(&candidate) <= max_width {
            spans.extend(candidate);
            return;
        }
    }
    push_todo_status_pips(spans, todos.iter().copied(), max_width);
}

/// Agent emoji (with failure marks) and the live activity of the lane's
/// running worker.
fn lane_agent_chip(
    lane_todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
) -> (Vec<Span<'static>>, String) {
    let mut spans = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut note = String::new();
    for todo in lane_todos {
        let Some(assigned) = todo
            .assigned_to
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let agent = agents.iter().find(|agent| agent.matches(assigned));
        let key = agent
            .map(|agent| agent.session_id.clone())
            .unwrap_or_else(|| assigned.to_string());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        spans.push(Span::raw(
            agent
                .map(TodoCardAgent::icon)
                .unwrap_or_else(|| fallback_icon(assigned)),
        ));
        if let Some(agent) = agent {
            if let Some((mark, color)) = agent_state_mark(&agent.status) {
                spans.push(Span::styled(mark, Style::default().fg(color)));
            }
            if note.is_empty() && todo.status == "in_progress" {
                note = agent_note(agent);
            }
        }
    }
    (spans, note)
}

fn agent_note(agent: &TodoCardAgent) -> String {
    let mut note = agent.activity.clone().unwrap_or_default();
    if let Some(age) = agent.age_secs.filter(|age| *age >= 2) {
        if !note.is_empty() {
            note.push_str(" · ");
        }
        note.push_str(&jcode_tui_render::swarm_gallery::humanize_age(age));
    }
    note
}

/// Bold group name followed by the lane's worker emoji.
fn lane_name_spans(
    group: Option<&str>,
    lane_todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
) -> Vec<Span<'static>> {
    let mut name = Vec::new();
    if let Some(group) = group {
        name.push(Span::styled(
            group.to_string(),
            Style::default().fg(lane_header_color()).bold(),
        ));
    }
    let (chip, _) = lane_agent_chip(lane_todos, agents);
    if !chip.is_empty() {
        if !name.is_empty() {
            name.push(Span::raw(" "));
        }
        name.extend(chip);
    }
    name
}

/// A lane's status strip (`✓●○`), padded to `strip_width` so the names after
/// it line up across lanes.
fn lane_strip_spans(
    lane_todos: &[&crate::todo::TodoItem],
    strip_width: usize,
) -> Vec<Span<'static>> {
    let mut strip: Vec<Span<'static>> = lane_todos.iter().map(|todo| pip_span(todo)).collect();
    strip.push(Span::raw(
        " ".repeat(strip_width.saturating_sub(lane_todos.len()) + 2),
    ));
    strip
}

/// Titles never shrink below this many cells before earlier finished titles
/// are dropped to make room.
const MIN_TITLE_CELLS: usize = 8;

/// A lane's titles in order (`a, b, c`), coloured by status, each capped to a
/// fair share of the row so every task stays visible.
fn lane_titles_spans(lane_todos: &[&crate::todo::TodoItem], width: usize) -> Vec<Span<'static>> {
    let comma = || Span::styled(", ", Style::default().fg(rgb(105, 100, 135)));
    let title = |todo: &crate::todo::TodoItem| -> Span<'static> {
        todo_card_item_spans(todo, false)
            .pop()
            .unwrap_or_else(|| Span::raw(todo.content.clone()))
    };
    // Every shown title gets a fair share of the row, so a long first title
    // cannot push the rest off screen. Short titles give their spare cells to
    // longer ones. Leading finished titles drop first when even the minimum
    // share does not fit (the dot strip still counts them).
    let fit = |items: &[&crate::todo::TodoItem]| -> Option<Vec<Span<'static>>> {
        let n = items.len();
        let budget = width.checked_sub(2 * n.saturating_sub(1))?;
        let natural: Vec<usize> = items.iter().map(|t| t.content.width()).collect();
        let mut caps = natural.clone();
        if natural.iter().sum::<usize>() > budget {
            let mut sorted = natural.clone();
            sorted.sort_unstable();
            let mut remaining = budget;
            let mut cap = 0;
            for (i, w) in sorted.iter().enumerate() {
                let share = remaining / (n - i);
                if *w <= share {
                    remaining -= w;
                } else {
                    cap = share;
                    break;
                }
            }
            if cap < MIN_TITLE_CELLS.min(*natural.iter().max().unwrap_or(&0)) {
                return None;
            }
            caps = natural.iter().map(|w| (*w).min(cap)).collect();
        }
        let mut spans = Vec::new();
        for (i, (todo, cap)) in items.iter().zip(caps).enumerate() {
            if i > 0 {
                spans.push(comma());
            }
            let mut span = title(todo);
            if span.content.width() > cap {
                span.content = format!(
                    "{}…",
                    truncate_to_width(&span.content, cap.saturating_sub(1))
                )
                .into();
            }
            spans.push(span);
        }
        Some(spans)
    };
    let done_prefix = lane_todos.iter().take_while(|todo| is_done(todo)).count();
    let max_skip = done_prefix.min(lane_todos.len().saturating_sub(1));
    for skip in 0..=max_skip {
        if let Some(spans) = fit(&lane_todos[skip..]) {
            return spans;
        }
    }
    // Still too narrow: show the active-most tail and cut it.
    let spans = fit(&lane_todos[max_skip..max_skip + 1])
        .unwrap_or_else(|| vec![title(lane_todos[max_skip])]);
    super::truncate_line_with_ellipsis_to_width(&Line::from(spans), width).spans
}

/// Render todos that declare stages, one row per lane:
/// `●●○  parser 🦊  Extract lexer, Update callers, Add tests`.
/// The status strip leads, so the left edge reads as a block of progress;
/// then the group name; then the titles. Phases are labelled on the header
/// row, so the body has no heading rows. Finished phases are left to the
/// header unless everything is done.
pub(crate) fn push_stage_lines(
    lines: &mut Vec<Line<'static>>,
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
    base_indent: &str,
    inner_width: usize,
) {
    let stages = ordered_layout(todos);
    let current = current_stage(&stages, todos);
    let shown = |index: usize| stages.len() < 2 || !current.is_some_and(|current| index < current);
    let lanes: Vec<(bool, &crate::todo::stages::TodoLane)> = stages
        .iter()
        .enumerate()
        .filter(|(index, _)| shown(*index))
        .flat_map(|(index, stage)| {
            // Lanes in phases after the active one are dimmed, matching the
            // dimmed phase labels on the header row.
            let later = current.is_some_and(|current| index > current);
            stage.lanes.iter().map(move |lane| (later, lane))
        })
        .collect();
    let items_of = |lane: &crate::todo::stages::TodoLane| -> Vec<&crate::todo::TodoItem> {
        lane.items.iter().map(|&i| todos[i]).collect()
    };
    let strip_width = lanes
        .iter()
        .map(|(_, lane)| lane.items.len())
        .max()
        .unwrap_or(0);
    let name_width = lanes
        .iter()
        .map(|(_, lane)| {
            width_of(&lane_name_spans(
                lane.group.as_deref(),
                &items_of(lane),
                agents,
            ))
        })
        .max()
        .unwrap_or(0)
        .min(inner_width / 3);

    for (later, lane) in lanes {
        let items = items_of(lane);
        let mut row = lane_strip_spans(&items, strip_width);
        if name_width > 0 {
            let name = lane_name_spans(lane.group.as_deref(), &items, agents);
            let name =
                super::truncate_line_with_ellipsis_to_width(&Line::from(name), name_width).spans;
            let pad = name_width.saturating_sub(width_of(&name)) + 2;
            row.extend(name);
            row.push(Span::raw(" ".repeat(pad)));
        }
        let room = inner_width.saturating_sub(width_of(&row));
        let titles = lane_titles_spans(&items, room);
        let titles_width = width_of(&titles);
        row.extend(titles);
        let (_, note) = lane_agent_chip(&items, agents);
        let note_room = room.saturating_sub(titles_width + 2);
        if !note.is_empty() && note_room >= 6 {
            let shown = if note.width() > note_room {
                format!("{}…", truncate_to_width(&note, note_room - 1))
            } else {
                note
            };
            row.push(Span::styled(
                format!("  {shown}"),
                Style::default().fg(todo_meta_color()),
            ));
        }
        if later {
            row = row.into_iter().map(dim_span).collect();
        }
        lines.push(todo_card_line(row, base_indent, inner_width));
    }
}

/// Grey out a span for a phase that has not started yet, keeping emoji intact.
fn dim_span(span: Span<'static>) -> Span<'static> {
    let style = span.style.fg(rgb(105, 100, 135)).not_bold();
    Span::styled(span.content, style)
}

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
    let icon = agent
        .map(TodoCardAgent::icon)
        .unwrap_or_else(|| fallback_icon(assigned));
    let mut chip = vec![Span::raw("  "), Span::raw(icon)];
    let mut note = String::new();
    if let Some(agent) = agent {
        if let Some((mark, color)) = agent_state_mark(&agent.status) {
            chip.push(Span::styled(mark, Style::default().fg(color)));
        }
        if todo.status != "completed" {
            note = agent_note(agent);
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

fn truncate_to_width(text: &str, max: usize) -> String {
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

#[cfg(test)]
#[path = "ui_todo_stages_tests.rs"]
mod tests;
