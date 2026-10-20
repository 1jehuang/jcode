//! Stage grid and swarm agent chips for todo cards.
//!
//! Stages run down, lanes run across. Layout comes from
//! [`crate::todo::stages::stage_layout`], the same function `run_plan` uses,
//! so the card always matches execution order. Each stage starts with its
//! number in the left gutter, matching the numbered header pips
//! (`1 ●●●  2 ○○`). Lanes (todo groups inside a stage) sit side by side as
//! columns under their group name when the terminal is wide enough, and stack
//! under the same headings otherwise. No divider lines or rails: spacing,
//! bold lane headers and the gutter number carry the structure. A list with
//! no stages renders exactly like a plain checklist.
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

/// Columns narrower than this make lanes stack instead.
const MIN_LANE_COLUMN: usize = 22;
/// Blank cells between lane columns.
const LANE_GAP: usize = 3;
/// Rows a lane may use in column mode before the rest fold into `+N more`.
const MAX_LANE_ROWS: usize = 6;

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

fn stage_label_style(index: usize, current: Option<usize>) -> Style {
    match current {
        Some(current) if index == current => Style::default().fg(stage_color()).bold(),
        Some(current) if index > current => Style::default().fg(rgb(105, 100, 135)),
        _ => Style::default().fg(stage_color()),
    }
}

fn pip_span(todo: &crate::todo::TodoItem) -> Span<'static> {
    let (glyph, color) = match todo.status.as_str() {
        "completed" => ("●", rgb(100, 180, 100)),
        "in_progress" => ("●", asap_color()),
        _ => ("○", rgb(90, 90, 105)),
    };
    Span::styled(glyph, Style::default().fg(color))
}

/// Header pips with each stage labelled by its number, e.g. `1 ●●●  2 ○○`.
///
/// Falls back to per-stage counts (`1 ✓  2 1/3`) when the pips do not fit, and
/// to plain proportional pips when even that is too wide. A list without
/// stages produces the same pips as before.
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
    let label = |index: usize, stage: &crate::todo::stages::TodoStage| -> Option<Span<'static>> {
        stage
            .number
            .map(|n| Span::styled(format!("{n} "), stage_label_style(index, current)))
    };

    let mut full: Vec<Span<'static>> = Vec::new();
    for (index, stage) in stages.iter().enumerate() {
        if index > 0 {
            full.push(Span::raw(if stage.number.is_some() { "  " } else { " " }));
        }
        full.extend(label(index, stage));
        full.extend(stage.item_indices().map(|i| pip_span(todos[i])));
    }
    if width_of(&full) <= max_width {
        spans.extend(full);
        return;
    }

    let mut compact: Vec<Span<'static>> = Vec::new();
    for (index, stage) in stages.iter().enumerate() {
        if index > 0 {
            compact.push(Span::raw("  "));
        }
        compact.extend(label(index, stage));
        let total = stage.len();
        let done = stage.item_indices().filter(|&i| is_done(todos[i])).count();
        if done == total {
            compact.push(Span::styled("✓", Style::default().fg(rgb(100, 180, 100))));
        } else {
            let color = if Some(index) == current {
                asap_color()
            } else {
                rgb(120, 120, 135)
            };
            compact.push(Span::styled(
                format!("{done}/{total}"),
                Style::default().fg(color),
            ));
        }
    }
    if width_of(&compact) <= max_width {
        spans.extend(compact);
    } else {
        push_todo_status_pips(spans, todos.iter().copied(), max_width);
    }
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

/// Fit spans into exactly `width` cells: truncate with an ellipsis or pad.
fn fit_cell(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let line = super::truncate_line_with_ellipsis_to_width(&Line::from(spans), width);
    let mut spans = line.spans;
    let used = width_of(&spans);
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    spans
}

/// Lane header: bold group name, worker emoji, then the live activity note.
/// The note shrinks first so the name and emoji stay visible.
fn lane_header_spans(
    group: Option<&str>,
    lane_todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
    width: usize,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(group) = group {
        spans.push(Span::styled(
            group.to_string(),
            Style::default().fg(lane_header_color()).bold(),
        ));
    }
    let (chip, note) = lane_agent_chip(lane_todos, agents);
    if !chip.is_empty() {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.extend(chip);
    }
    let room = width.saturating_sub(width_of(&spans) + 1);
    if !note.is_empty() && room >= 4 {
        let shown = if note.width() > room {
            format!("{}…", truncate_to_width(&note, room - 1))
        } else {
            note
        };
        spans.push(Span::styled(
            format!(" {shown}"),
            Style::default().fg(todo_meta_color()),
        ));
    }
    spans
}

/// Status glyph plus task text, no gutter or confidence suffix.
fn item_cell_spans(todo: &crate::todo::TodoItem) -> Vec<Span<'static>> {
    let mut spans = todo_card_item_spans(todo, false);
    if !spans.is_empty() {
        spans.remove(0);
    }
    spans
}

/// Which rows of a lane to show in column mode: everything when it fits,
/// otherwise a window that keeps the first open item visible, plus the count
/// of hidden rows.
fn lane_window(lane_todos: &[&crate::todo::TodoItem]) -> (std::ops::Range<usize>, usize) {
    let len = lane_todos.len();
    if len <= MAX_LANE_ROWS {
        return (0..len, 0);
    }
    let visible = MAX_LANE_ROWS - 1;
    let focus = lane_todos
        .iter()
        .position(|todo| !is_done(todo))
        .unwrap_or(len - 1);
    let start = focus.saturating_sub(1).min(len - visible);
    (start..start + visible, len - visible)
}

/// Render todos that declare stages as a stage grid: stage number in the left
/// gutter, lanes as columns (or stacked when narrow), no divider lines.
pub(crate) fn push_stage_lines(
    lines: &mut Vec<Line<'static>>,
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
    base_indent: &str,
    inner_width: usize,
) {
    let stages = ordered_layout(todos);
    let current = current_stage(&stages, todos);
    let digits = stages
        .iter()
        .filter_map(|stage| stage.number)
        .map(|n| n.to_string().len())
        .max()
        .unwrap_or(1);
    let gutter_width = digits + 1;
    let body_width = inner_width.saturating_sub(gutter_width);
    let mut previous_groups: Option<Vec<Option<String>>> = None;

    for (index, stage) in stages.iter().enumerate() {
        let mut gutter_label = stage
            .number
            .map(|n| Span::styled(format!("{n:>digits$} "), stage_label_style(index, current)));
        let mut gutter = || -> Span<'static> {
            gutter_label
                .take()
                .unwrap_or_else(|| Span::raw(" ".repeat(gutter_width)))
        };
        let lanes: Vec<(Option<&str>, Vec<&crate::todo::TodoItem>)> = stage
            .lanes
            .iter()
            .map(|lane| {
                (
                    lane.group.as_deref(),
                    lane.items.iter().map(|&i| todos[i]).collect(),
                )
            })
            .collect();
        let groups: Vec<Option<String>> = stage.lanes.iter().map(|l| l.group.clone()).collect();
        let any_agents = lanes
            .iter()
            .flat_map(|(_, items)| items.iter())
            .any(|todo| todo.assigned_to.is_some());
        let named = groups.iter().any(Option::is_some);
        // Repeat headers only when the lanes changed or a worker is attached.
        let show_headers = (named && previous_groups.as_ref() != Some(&groups)) || any_agents;
        previous_groups = Some(groups);

        if index > 0 && stages.len() > 1 && stage.number.is_some() {
            lines.push(Line::from(""));
        }

        let k = lanes.len();
        let column_mode = k >= 2 && body_width >= k * MIN_LANE_COLUMN + (k - 1) * LANE_GAP;
        if column_mode {
            let col = (body_width - (k - 1) * LANE_GAP) / k;
            let gap = || Span::raw(" ".repeat(LANE_GAP));
            if show_headers {
                let mut row = vec![gutter()];
                for (lane_index, (group, items)) in lanes.iter().enumerate() {
                    if lane_index > 0 {
                        row.push(gap());
                    }
                    row.extend(fit_cell(lane_header_spans(*group, items, agents, col), col));
                }
                lines.push(todo_card_line(row, base_indent, inner_width));
            }
            let windows: Vec<_> = lanes.iter().map(|(_, items)| lane_window(items)).collect();
            let height = windows
                .iter()
                .map(|(range, hidden)| range.len() + usize::from(*hidden > 0))
                .max()
                .unwrap_or(0);
            for row_index in 0..height {
                let mut row = vec![gutter()];
                for (lane_index, ((_, items), (range, hidden))) in
                    lanes.iter().zip(&windows).enumerate()
                {
                    if lane_index > 0 {
                        row.push(gap());
                    }
                    let cell = if row_index < range.len() {
                        item_cell_spans(items[range.start + row_index])
                    } else if row_index == range.len() && *hidden > 0 {
                        vec![Span::styled(
                            format!("  +{hidden} more"),
                            Style::default().fg(todo_meta_color()),
                        )]
                    } else {
                        Vec::new()
                    };
                    row.extend(fit_cell(cell, col));
                }
                // Drop trailing padding so rows do not carry dead cells.
                while row
                    .last()
                    .is_some_and(|span| span.content.trim().is_empty())
                {
                    row.pop();
                }
                lines.push(todo_card_line(row, base_indent, inner_width));
            }
            continue;
        }

        for (group, items) in &lanes {
            if show_headers && (group.is_some() || items.iter().any(|t| t.assigned_to.is_some())) {
                let mut row = vec![gutter()];
                row.extend(lane_header_spans(*group, items, agents, body_width));
                lines.push(todo_card_line(row, base_indent, inner_width));
            }
            for todo in items {
                let mut row = vec![gutter()];
                row.extend(item_cell_spans(todo));
                lines.push(todo_card_line(row, base_indent, inner_width));
            }
        }
    }
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
