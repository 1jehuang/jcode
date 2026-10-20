//! Stage rows and swarm agent chips for todo cards.
//!
//! Layout comes from [`crate::todo::stages::stage_layout`], the same function
//! `run_plan` uses, so the card always matches execution order. Each stage
//! opens with a plain word (`now`, `next`, `later`, `done`), and the header
//! pips put an arrow between stages (`●●● → ○○`). Each lane (todo group) is
//! one row: its name, then its tasks left to right. Rows in a stage run in
//! parallel, tasks along a row run in sequence. No divider lines or rails. A
//! list with no stages renders exactly like a plain checklist.
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

/// Plain-word position of a stage relative to the active one.
fn stage_word(index: usize, current: Option<usize>) -> (&'static str, Style) {
    match current {
        Some(current) if index == current => ("now", Style::default().fg(asap_color()).bold()),
        Some(current) if index == current + 1 => ("next", Style::default().fg(stage_color())),
        Some(current) if index > current => ("later", Style::default().fg(rgb(105, 100, 135))),
        _ => ("done", Style::default().fg(rgb(100, 180, 100))),
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

/// Header pips with an arrow between stages, e.g. `●●● → ○○`.
///
/// Falls back to per-stage counts (`✓ → 1/3`) when the pips do not fit, and
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
    let arrow = || Span::styled(" → ", Style::default().fg(stage_color()));

    let mut full: Vec<Span<'static>> = Vec::new();
    for (index, stage) in stages.iter().enumerate() {
        if index > 0 {
            full.push(arrow());
        }
        full.extend(stage.item_indices().map(|i| pip_span(todos[i])));
    }
    if width_of(&full) <= max_width {
        spans.extend(full);
        return;
    }

    let mut compact: Vec<Span<'static>> = Vec::new();
    for (index, stage) in stages.iter().enumerate() {
        if index > 0 {
            compact.push(arrow());
        }
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

/// Header-row summary of finished phases, placed to the right of the pips:
/// `  ✓ phase 1, 2 done · schema, render, swarm`. Group names drop first, then
/// the whole summary, so it never wraps. Empty when no phase has finished or
/// when every phase has (the body then shows them all).
pub(crate) fn finished_phases_summary(
    todos: &[&crate::todo::TodoItem],
    width: usize,
) -> Vec<Span<'static>> {
    if !crate::todo::stages::has_stages(todos.iter().copied()) {
        return Vec::new();
    }
    let stages = ordered_layout(todos);
    let Some(current) = current_stage(&stages, todos) else {
        return Vec::new();
    };
    if current == 0 || stages.len() < 2 {
        return Vec::new();
    }
    let finished = &stages[..current];
    let numbers: Vec<String> = finished
        .iter()
        .filter_map(|stage| stage.number.map(|n| n.to_string()))
        .collect();
    let mut groups: Vec<String> = Vec::new();
    for stage in finished {
        for lane in &stage.lanes {
            if let Some(group) = &lane.group
                && !groups.contains(group)
            {
                groups.push(group.clone());
            }
        }
    }
    let green = Style::default().fg(rgb(100, 180, 100));
    let meta = Style::default().fg(todo_meta_color());
    let label = if numbers.is_empty() {
        format!("{} phases done", finished.len())
    } else if numbers.len() == 1 {
        format!("phase {} done", numbers[0])
    } else {
        format!("phase {} done", numbers.join(", "))
    };
    let mut spans = vec![Span::raw("   "), Span::styled(format!("✓ {label}"), green)];
    let base = width_of(&spans);
    if base > width {
        return Vec::new();
    }
    if !groups.is_empty() {
        let names = format!(" · {}", groups.join(", "));
        let room = width - base;
        if room >= 8 {
            let shown = if names.width() > room {
                format!("{}…", truncate_to_width(&names, room - 1))
            } else {
                names
            };
            spans.push(Span::styled(shown, meta));
        }
    }
    spans
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

/// Status glyph plus task text, no gutter or confidence suffix.
fn item_cell_spans(todo: &crate::todo::TodoItem) -> Vec<Span<'static>> {
    let mut spans = todo_card_item_spans(todo, false);
    if !spans.is_empty() {
        spans.remove(0);
    }
    spans
}

/// Task chain for one lane, left to right: `✓ a → ● b → ○ c`. When it does
/// not fit, leading finished tasks fold into a `✓N` count so the active task
/// stays visible, and the tail is cut with an ellipsis.
fn lane_chain_spans(lane_todos: &[&crate::todo::TodoItem], width: usize) -> Vec<Span<'static>> {
    let arrow = || Span::styled(" → ", Style::default().fg(rgb(105, 100, 135)));
    let build = |skip: usize| -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        if skip > 0 {
            spans.push(Span::styled(
                format!("✓{skip}"),
                Style::default().fg(rgb(100, 180, 100)),
            ));
        }
        for todo in &lane_todos[skip..] {
            if !spans.is_empty() {
                spans.push(arrow());
            }
            spans.extend(item_cell_spans(todo));
        }
        spans
    };
    let done_prefix = lane_todos.iter().take_while(|todo| is_done(todo)).count();
    // Keep at least the last finished task when everything is done.
    let max_skip = done_prefix.min(lane_todos.len().saturating_sub(1));
    let mut spans = build(0);
    for skip in 1..=max_skip {
        if width_of(&spans) <= width {
            break;
        }
        spans = build(skip);
    }
    super::truncate_line_with_ellipsis_to_width(&Line::from(spans), width).spans
}

/// Render todos that declare stages. Each stage opens with a plain word
/// (`now`, `next`, `later`, `done`); below it every lane is one row: the
/// group name, then that lane's tasks in order from left to right. Rows in a
/// stage run in parallel, tasks along a row run in sequence.
pub(crate) fn push_stage_lines(
    lines: &mut Vec<Line<'static>>,
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
    base_indent: &str,
    inner_width: usize,
) {
    let stages = ordered_layout(todos);
    let current = current_stage(&stages, todos);
    let name_width = stages
        .iter()
        .flat_map(|stage| stage.lanes.iter())
        .map(|lane| {
            let items: Vec<&crate::todo::TodoItem> = lane.items.iter().map(|&i| todos[i]).collect();
            width_of(&lane_name_spans(lane.group.as_deref(), &items, agents))
        })
        .max()
        .unwrap_or(0)
        .min(inner_width / 3);
    let indent = if stages.len() > 1 { "  " } else { "" };

    for (stage_index, stage) in stages.iter().enumerate() {
        // Finished phases are summarised on the header row; the body is for
        // the active phase and what comes after it. A fully finished list
        // keeps every phase so the card does not go blank.
        if stages.len() > 1 && current.is_some_and(|current| stage_index < current) {
            continue;
        }
        // `phase 1 · done`, `phase 2 · now`: the number says which phase, the
        // word says where it stands, so repeated states stay unambiguous.
        if stages.len() > 1 {
            let (word, style) = stage_word(stage_index, current);
            let mut label = Vec::new();
            if let Some(number) = stage.number {
                label.push(Span::styled(
                    format!("phase {number}"),
                    Style::default().fg(stage_color()).bold(),
                ));
                label.push(Span::styled(" · ", Style::default().fg(todo_meta_color())));
            }
            label.push(Span::styled(word, style));
            lines.push(todo_card_line(label, base_indent, inner_width));
        }
        for lane in &stage.lanes {
            let items: Vec<&crate::todo::TodoItem> = lane.items.iter().map(|&i| todos[i]).collect();
            let mut row = vec![Span::raw(indent)];
            if name_width > 0 {
                let name = lane_name_spans(lane.group.as_deref(), &items, agents);
                let name =
                    super::truncate_line_with_ellipsis_to_width(&Line::from(name), name_width)
                        .spans;
                let pad = name_width.saturating_sub(width_of(&name)) + 2;
                row.extend(name);
                row.push(Span::raw(" ".repeat(pad)));
            }
            let room = inner_width.saturating_sub(width_of(&row));
            let chain = lane_chain_spans(&items, room);
            let chain_width = width_of(&chain);
            row.extend(chain);
            let (_, note) = lane_agent_chip(&items, agents);
            let note_room = room.saturating_sub(chain_width + 2);
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
            lines.push(todo_card_line(row, base_indent, inner_width));
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
