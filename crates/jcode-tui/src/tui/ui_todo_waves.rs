//! Wave rails and swarm agent chips for todo cards.
//!
//! Time flows down, parallel work flows across. A todo `wave` groups items
//! that may run at the same time. Waves with a single item render exactly like
//! a plain list, so a fully sequential plan looks unchanged. Only waves with
//! two or more items get a rail (`┏ ┃ ┗`) and a gap in the header pips.
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

/// Position of a todo inside its wave run, used to pick the rail glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RailPos {
    None,
    First,
    Middle,
    Last,
}

/// Consecutive todos that share a wave form one run. Runs of length one (or
/// todos without a wave) get no rail.
pub(crate) fn wave_rail_positions(todos: &[&crate::todo::TodoItem]) -> Vec<RailPos> {
    let mut positions = vec![RailPos::None; todos.len()];
    let mut start = 0;
    while start < todos.len() {
        let wave = todos[start].wave;
        let mut end = start + 1;
        while wave.is_some() && end < todos.len() && todos[end].wave == wave {
            end += 1;
        }
        if end - start >= 2 {
            positions[start] = RailPos::First;
            for pos in positions.iter_mut().take(end - 1).skip(start + 1) {
                *pos = RailPos::Middle;
            }
            positions[end - 1] = RailPos::Last;
        }
        start = end;
    }
    positions
}

pub(crate) fn has_parallel_waves(todos: &[&crate::todo::TodoItem]) -> bool {
    wave_rail_positions(todos)
        .iter()
        .any(|pos| *pos != RailPos::None)
}

fn rail_color() -> Color {
    rgb(150, 130, 210)
}

fn rail_span(pos: RailPos, any_rails: bool) -> Option<Span<'static>> {
    if !any_rails {
        return None;
    }
    let glyph = match pos {
        RailPos::None => " ",
        RailPos::First => "┏",
        RailPos::Middle => "┃",
        RailPos::Last => "┗",
    };
    Some(Span::styled(
        format!("{glyph} "),
        Style::default().fg(rail_color()),
    ))
}

/// Header pips with a space between wave runs that contain parallel work, e.g.
/// `●● ●●○ ○`. A fully sequential list produces the same pips as before.
pub(crate) fn push_wave_pips(
    spans: &mut Vec<Span<'static>>,
    todos: &[&crate::todo::TodoItem],
    max_pips: usize,
) {
    if todos.len() > max_pips.max(12) {
        push_todo_status_pips(spans, todos.iter().copied(), max_pips);
        return;
    }
    let positions = wave_rail_positions(todos);
    for (index, (todo, pos)) in todos.iter().zip(&positions).enumerate() {
        let starts_run = matches!(pos, RailPos::First);
        let after_run = index > 0 && matches!(positions[index - 1], RailPos::Last);
        if index > 0 && (starts_run || after_run) {
            spans.push(Span::raw(" "));
        }
        let (glyph, color) = match todo.status.as_str() {
            "completed" => ("●", rgb(100, 180, 100)),
            "in_progress" => ("●", asap_color()),
            _ => ("○", rgb(90, 90, 105)),
        };
        spans.push(Span::styled(glyph, Style::default().fg(color)));
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

/// One todo row with an optional wave rail and agent chip.
///
/// Width priority when space runs out: the activity note shrinks first, then
/// the task text; the status glyph, rail, and agent emoji always stay visible.
pub(crate) fn render_wave_item_line(
    todo: &crate::todo::TodoItem,
    pos: RailPos,
    any_rails: bool,
    agents: &[TodoCardAgent],
    base_indent: &str,
    inner_width: usize,
) -> Line<'static> {
    let assigned = todo
        .assigned_to
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    if !any_rails && assigned.is_none() {
        return render_todo_card_item_line(todo, base_indent, inner_width);
    }

    // Agent rows drop the confidence suffix: the agent state is the more
    // useful signal in a width-constrained row.
    let mut spans = todo_card_item_spans(todo, assigned.is_none());
    if let Some(rail) = rail_span(pos, any_rails)
        && let Some(gutter) = spans.first_mut()
    {
        *gutter = rail;
    }
    let Some(assigned) = assigned else {
        return todo_card_line(spans, base_indent, inner_width);
    };

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
            if let Some(activity) = agent.activity.as_deref() {
                note.push_str(activity);
            }
            if let Some(age) = agent.age_secs.filter(|age| *age >= 2) {
                if !note.is_empty() {
                    note.push_str(" · ");
                }
                note.push_str(&jcode_tui_render::swarm_gallery::humanize_age(age));
            }
        }
    }

    let width_of =
        |spans: &[Span<'static>]| -> usize { spans.iter().map(|span| span.content.width()).sum() };
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
#[path = "ui_todo_waves_tests.rs"]
mod tests;
