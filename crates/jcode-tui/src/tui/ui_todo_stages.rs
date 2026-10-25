//! Stage rows and swarm agent chips for todo cards.
//!
//! Layout comes from [`crate::todo::stages::stage_layout`], the same function
//! `run_plan` uses, so the card always matches execution order. The header row
//! labels each phase with its pips (`phase 1 ●●●  phase 2 ○○`). Below it each
//! lane (todo group) is one row: its worker's todo-status strip, its name,
//! then its coordinator todo titles left to right. If worker todos are not
//! available, the strip falls back to the coordinator items. Lanes run in
//! parallel within a phase, titles along a row in
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
    /// Global swarm navigation digit, assigned by session id in display order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub status: String,
    /// Short live activity note (current tool intent or status detail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_secs: Option<u64>,
    /// Seconds since the worker's last tool event or token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_age_secs: Option<u64>,
    /// Ordered statuses of the worker's own todos, not the coordinator's lane tasks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub todo_statuses: Vec<String>,
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
            shortcut: None,
            name: member.friendly_name.clone(),
            status: member.status.clone(),
            activity,
            age_secs: member.status_age_secs,
            activity_age_secs: member.runtime.activity_age_secs,
            todo_statuses: if member.runtime.todo_statuses.is_empty() {
                member
                    .todo_items
                    .iter()
                    .map(|item| item.status.clone())
                    .collect()
            } else {
                member.runtime.todo_statuses.clone()
            },
        }
    }

    fn shortcut_span(&self) -> Option<Span<'static>> {
        self.shortcut.filter(|digit| *digit <= 9).map(|digit| {
            Span::styled(
                format!(" [alt+{digit}]"),
                Style::default().fg(todo_meta_color()),
            )
        })
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

/// Header roster of swarm agents, split into who is still on the job and who
/// has finished: `🦊🐑  · ✓ 🦀🐢`. Working agents (running, queued, blocked,
/// failed) come first with their state marks; agents whose work is done are
/// grouped after a dim `· ✓`. An agent counts as done when every todo it owns
/// is finished, or its status is terminal-success.
pub(crate) fn agent_roster_spans(
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
) -> Vec<Span<'static>> {
    let mut order: Vec<(String, String)> = Vec::new();
    for todo in todos {
        let Some(assigned) = todo.assigned_to.as_deref() else {
            continue;
        };
        // Lane headings carry ownership. Plain lists retain only owners not
        // already represented by an active task row.
        if todos.iter().any(|item| item.stage.is_some())
            || todos.iter().any(|item| {
                (item.status == "in_progress"
                    || item
                        .group
                        .as_deref()
                        .is_some_and(|group| !group.trim().is_empty()))
                    && item.assigned_to.as_deref().is_some_and(|owner| {
                        owner == assigned
                            || agents
                                .iter()
                                .any(|agent| agent.matches(assigned) && agent.matches(owner))
                    })
            })
        {
            continue;
        }
        let agent = agents.iter().find(|agent| agent.matches(assigned));
        let key = agent
            .map(|agent| agent.session_id.clone())
            .unwrap_or_else(|| assigned.to_string());
        if !order.iter().any(|(existing, _)| *existing == key) {
            order.push((key, assigned.to_string()));
        }
    }
    let mut working = Vec::new();
    let mut done = Vec::new();
    for (_, assigned) in &order {
        let agent = agents.iter().find(|agent| agent.matches(assigned));
        let icon = agent
            .map(TodoCardAgent::icon)
            .unwrap_or_else(|| fallback_icon(assigned));
        let owns_open_work = todos.iter().any(|todo| {
            !is_done(todo)
                && todo.assigned_to.as_deref().is_some_and(|owner| {
                    owner == assigned || agent.is_some_and(|agent| agent.matches(owner))
                })
        });
        let status = agent.map(|agent| agent.status.as_str()).unwrap_or("");
        let finished = matches!(status, "completed" | "done" | "stopped")
            || (!owns_open_work && !matches!(status, "failed" | "crashed"));
        if finished {
            done.push(Span::raw(icon));
        } else {
            working.push(Span::raw(icon));
            if let Some((mark, color)) = agent_state_mark(status) {
                working.push(Span::styled(mark, Style::default().fg(color)));
            }
        }
    }
    let mut spans = Vec::new();
    if working.is_empty() && done.is_empty() {
        return spans;
    }
    spans.push(Span::raw("  "));
    spans.extend(working.iter().cloned());
    if !done.is_empty() {
        let dim = Style::default().fg(later_phase_color());
        spans.push(Span::styled(
            if working.is_empty() {
                "✓ "
            } else {
                "  · ✓ "
            },
            dim,
        ));
        spans.extend(done);
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
    status_pip_span(&todo.status)
}

fn status_pip_span(status: &str) -> Span<'static> {
    let (glyph, color) = match status {
        "completed" => ("●", rgb(100, 180, 100)),
        "in_progress" => ("●", asap_color()),
        _ => ("○", rgb(90, 90, 105)),
    };
    Span::styled(glyph, Style::default().fg(color))
}

/// Placeholder written into cached card lines for a dot whose worker is live.
/// [`animate_lane_spinners`] swaps it for the current spinner frame on every
/// draw, so the line cache stays valid while the dot still moves. A narrow
/// one-cell glyph that is also a sensible static fallback (`◍`).
pub(crate) const LIVE_DOT_SENTINEL: &str = "\u{25CD}";

/// Worker states that mean the agent is actively running right now.
fn is_live_status(status: &str) -> bool {
    jcode_tui_render::swarm_gallery::is_active_status(status)
}

/// No tool event or token for this long reads as "quiet" (amber).
const QUIET_AFTER_SECS: u64 = 120;

/// Replace live-dot placeholders with the wall-clock spinner frame. Cheap:
/// only lines that contain the placeholder are touched.
pub(crate) fn animate_lane_spinners(lines: &mut [Line<'static>]) {
    let frame = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| {
            (d.as_millis() / jcode_tui_render::swarm_gallery::STRIP_SPINNER_FRAME_MS as u128)
                as usize
        })
        .unwrap_or(0);
    let frames = jcode_tui_render::swarm_gallery::STRIP_SPINNER_FRAMES;
    let glyph = if crate::perf::tui_policy().enable_decorative_animations {
        frames[frame % frames.len()]
    } else {
        "●"
    };
    for line in lines {
        for span in &mut line.spans {
            if span.content.contains(LIVE_DOT_SENTINEL) {
                span.content = span.content.replace(LIVE_DOT_SENTINEL, glyph).into();
            }
        }
    }
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
    let label_style = |index: usize| phase_style(index, current);
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
            if let Some(badge) = agent.shortcut_span() {
                spans.push(badge);
            }
            if let Some((mark, color)) = agent_state_mark(&agent.status) {
                spans.push(Span::styled(mark, Style::default().fg(color)));
            }
            if note.is_empty() && todo.status == "in_progress" {
                note = agent_note(agent, lane_todos);
            }
        }
    }
    (spans, note)
}

fn activity_matches_title(activity: &str, todos: &[&crate::todo::TodoItem]) -> bool {
    let normalized = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let activity = normalized(activity);
    todos
        .iter()
        .any(|todo| activity == normalized(&todo.content))
}

fn agent_note(agent: &TodoCardAgent, todos: &[&crate::todo::TodoItem]) -> String {
    let mut note = agent
        .activity
        .clone()
        .filter(|activity| !activity_matches_title(activity, todos))
        .unwrap_or_default();
    if let Some(age) = agent.age_secs.filter(|age| *age >= 2) {
        if !note.is_empty() {
            note.push_str(" · ");
        }
        note.push_str(&jcode_tui_render::swarm_gallery::humanize_age(age));
    }
    note
}

/// Style for everything that belongs to one phase: its header label and the
/// names of its lanes share it, so a lane visibly belongs to its phase. The
/// active phase is bold, finished phases normal, later phases dimmed.
fn phase_style(index: usize, current: Option<usize>) -> Style {
    match current {
        Some(current) if index == current => Style::default().fg(stage_color()).bold(),
        Some(current) if index > current => Style::default().fg(later_phase_color()),
        _ => Style::default().fg(stage_color()),
    }
}

fn later_phase_color() -> Color {
    rgb(105, 100, 135)
}

/// Group name (styled like its phase label) followed by the worker emoji.
fn lane_name_spans(
    group: Option<&str>,
    style: Style,
    lane_todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
) -> Vec<Span<'static>> {
    let mut name = Vec::new();
    if let Some(group) = group {
        name.push(Span::styled(group.to_string(), style));
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

fn lane_item_statuses<'a>(
    lane_todos: &[&'a crate::todo::TodoItem],
    agents: &'a [TodoCardAgent],
) -> Vec<(&'a str, bool)> {
    let mut seen = std::collections::HashSet::new();
    let mut statuses = Vec::new();
    for todo in lane_todos {
        let agent = todo
            .assigned_to
            .as_deref()
            .and_then(|key| agents.iter().find(|agent| agent.matches(key)))
            .filter(|agent| !agent.todo_statuses.is_empty());
        if let Some(agent) = agent {
            // Several coordinator todos may point at the same worker.
            if seen.insert(agent.session_id.as_str()) {
                statuses.extend(
                    agent
                        .todo_statuses
                        .iter()
                        .map(|status| (status.as_str(), is_live_status(&agent.status))),
                );
            }
        } else {
            statuses.push((
                todo.status.as_str(),
                lane_live_agent(&[todo], agents).is_some(),
            ));
        }
    }
    statuses
}

/// One dot per worker todo, falling back to coordinator todos without a worker list.
fn lane_strip_spans(statuses: &[(&str, bool)], strip_width: usize) -> Vec<Span<'static>> {
    let strip: Vec<Span<'static>> = statuses
        .iter()
        .map(|(status, live)| {
            let mut span = status_pip_span(status);
            if *live && *status == "in_progress" {
                span.content = LIVE_DOT_SENTINEL.into();
            }
            span
        })
        .collect();
    let mut strip =
        super::truncate_line_with_ellipsis_to_width(&Line::from(strip), strip_width).spans;
    strip.push(Span::raw(
        " ".repeat(strip_width.saturating_sub(width_of(&strip)) + 2),
    ));
    strip
}

/// The lane's live worker, if any (first assigned agent that is running).
fn lane_live_agent<'a>(
    lane_todos: &[&crate::todo::TodoItem],
    agents: &'a [TodoCardAgent],
) -> Option<&'a TodoCardAgent> {
    lane_todos
        .iter()
        .filter(|todo| todo.status == "in_progress")
        .filter_map(|todo| todo.assigned_to.as_deref())
        .filter_map(|assigned| agents.iter().find(|agent| agent.matches(assigned)))
        .find(|agent| is_live_status(&agent.status))
}

/// Lane-end note: the worker's current intent and how long since it last did
/// anything, e.g. `running justrust test · 12s`. Past [`QUIET_AFTER_SECS`]
/// the age reads `quiet 3m` in amber. Failed / blocked workers say so.
fn lane_note_spans(
    lane_todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
) -> Vec<Span<'static>> {
    let meta = Style::default().fg(todo_meta_color());
    let assigned: Vec<&TodoCardAgent> = lane_todos
        .iter()
        .filter(|todo| !is_done(todo))
        .filter_map(|todo| todo.assigned_to.as_deref())
        .filter_map(|assigned| agents.iter().find(|agent| agent.matches(assigned)))
        .collect();
    if let Some(agent) = assigned
        .iter()
        .find(|agent| matches!(agent.status.as_str(), "failed" | "crashed"))
    {
        let reason = agent
            .activity
            .clone()
            .filter(|activity| !activity_matches_title(activity, lane_todos))
            .unwrap_or_else(|| "failed".into());
        return vec![
            Span::styled(
                format!("✗ {reason}"),
                Style::default().fg(todo_failure_color()),
            ),
            Span::styled(" · swarm retry", Style::default().fg(todo_meta_color())),
        ];
    }
    if let Some(agent) = assigned
        .iter()
        .find(|agent| matches!(agent.status.as_str(), "blocked" | "waiting_network"))
    {
        let reason = agent
            .activity
            .clone()
            .filter(|activity| !activity_matches_title(activity, lane_todos))
            .unwrap_or_else(|| "needs input".into());
        return vec![Span::styled(
            format!("⏸ {reason}"),
            Style::default().fg(rgb(225, 165, 90)),
        )];
    }
    let Some(agent) = lane_live_agent(lane_todos, agents) else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    if let Some(activity) = agent
        .activity
        .as_deref()
        .filter(|a| !a.trim().is_empty() && !activity_matches_title(a, lane_todos))
    {
        spans.push(Span::styled(activity.to_string(), meta));
    }
    let age = agent.activity_age_secs.or(agent.age_secs);
    if let Some(age) = age {
        let sep = if spans.is_empty() { "" } else { " · " };
        if age >= QUIET_AFTER_SECS {
            spans.push(Span::styled(sep, meta));
            spans.push(Span::styled(
                format!(
                    "quiet {}",
                    jcode_tui_render::swarm_gallery::humanize_age(age)
                ),
                Style::default().fg(rgb(225, 165, 90)),
            ));
        } else if age >= 2 {
            spans.push(Span::styled(
                format!(
                    "{sep}{}",
                    jcode_tui_render::swarm_gallery::humanize_age(age)
                ),
                meta,
            ));
        }
    }
    spans
}

/// Non-phased lanes show ownership once at the heading, never on each task.
pub(crate) fn render_group_header(
    label: &str,
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
    base_indent: &str,
    inner_width: usize,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        label.to_string(),
        Style::default().fg(todo_group_color()).bold(),
    )];
    let has_group = todos.iter().any(|todo| {
        todo.group
            .as_deref()
            .is_some_and(|group| !group.trim().is_empty())
    });
    if has_group {
        let (chip, _) = lane_agent_chip(todos, agents);
        if !chip.is_empty() {
            spans.push(Span::raw(" "));
            spans.extend(chip);
        }
    }
    spans.push(Span::raw("  "));
    let room = inner_width.saturating_sub(width_of(&spans));
    push_stage_pips(&mut spans, todos, room);
    if has_group {
        let note = lane_note_spans(todos, agents);
        if !note.is_empty() {
            spans.push(Span::raw("  "));
            spans.extend(note);
        }
    }
    todo_card_line(spans, base_indent, inner_width)
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
/// row, so the body has no heading rows. Every phase is listed: finished and
/// active phases in their phase colour, phases not yet started dimmed.
pub(crate) fn push_stage_lines(
    lines: &mut Vec<Line<'static>>,
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
    background_tasks: &[TodoCardBackgroundTask],
    base_indent: &str,
    inner_width: usize,
) {
    let stages = ordered_layout(todos);
    let current = current_stage(&stages, todos);
    let lanes: Vec<(usize, &crate::todo::stages::TodoLane)> = stages
        .iter()
        .enumerate()
        .flat_map(|(index, stage)| stage.lanes.iter().map(move |lane| (index, lane)))
        .collect();
    let items_of = |lane: &crate::todo::stages::TodoLane| -> Vec<&crate::todo::TodoItem> {
        lane.items.iter().map(|&i| todos[i]).collect()
    };
    let strip_width = lanes
        .iter()
        .map(|(_, lane)| lane_item_statuses(&items_of(lane), agents).len())
        .max()
        .unwrap_or(0)
        .min((inner_width / 3).max(1));
    let name_width = lanes
        .iter()
        .map(|(_, lane)| {
            width_of(&lane_name_spans(
                lane.group.as_deref(),
                Style::default(),
                &items_of(lane),
                agents,
            ))
        })
        .max()
        .unwrap_or(0)
        .min(inner_width / 3);

    for (phase, lane) in lanes {
        // A lane wears its phase's colour: bold while its phase is active,
        // dimmed (with its titles and dots) while the phase has not started.
        let later = current.is_some_and(|current| phase > current);
        let items = items_of(lane);
        // Expand a waiting lane so its dependency progress belongs to the
        // corresponding todo rather than an ambiguous lane-wide status dot.
        if items
            .iter()
            .any(|todo| !todo_running_dependencies(todo, background_tasks).is_empty())
        {
            for todo in &items {
                lines.push(render_todo_item_line_with_background(
                    todo,
                    agents,
                    background_tasks,
                    base_indent,
                    inner_width,
                ));
            }
            continue;
        }
        let statuses = lane_item_statuses(&items, agents);
        let mut row = lane_strip_spans(&statuses, strip_width);
        if name_width > 0 {
            let name = lane_name_spans(
                lane.group.as_deref(),
                phase_style(phase, current),
                &items,
                agents,
            );
            let name =
                super::truncate_line_with_ellipsis_to_width(&Line::from(name), name_width).spans;
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
                super::truncate_line_with_ellipsis_to_width(&Line::from(note_line), space).spans,
            );
        }
        if later {
            row = row.into_iter().map(dim_span).collect();
        }
        lines.push(todo_card_line(row, base_indent, inner_width));
    }
}

/// Grey out a span for a phase that has not started yet, keeping emoji intact.
fn dim_span(span: Span<'static>) -> Span<'static> {
    let style = span.style.fg(later_phase_color()).not_bold();
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
    if todo.status != "in_progress"
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
    let icon = agent
        .map(TodoCardAgent::icon)
        .unwrap_or_else(|| fallback_icon(assigned));
    let mut chip = vec![Span::raw("  "), Span::raw(icon)];
    let mut note = String::new();
    if let Some(agent) = agent {
        if let Some(badge) = agent.shortcut_span() {
            chip.push(badge);
        }
        if let Some((mark, color)) = agent_state_mark(&agent.status) {
            chip.push(Span::styled(mark, Style::default().fg(color)));
        }
        if todo.status != "completed" {
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

/// `⏸ phase 2 blocked` when the active phase has a lane whose worker failed:
/// later phases cannot start until it is retried or reassigned.
pub(crate) fn blocked_phase_spans(
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
) -> Vec<Span<'static>> {
    if !crate::todo::stages::has_stages(todos.iter().copied()) {
        return Vec::new();
    }
    let stages = ordered_layout(todos);
    let Some(current) = current_stage(&stages, todos) else {
        return Vec::new();
    };
    let stage = &stages[current];
    let failed = stage.item_indices().any(|i| {
        !is_done(todos[i])
            && todos[i].assigned_to.as_deref().is_some_and(|owner| {
                agents.iter().any(|agent| {
                    agent.matches(owner) && matches!(agent.status.as_str(), "failed" | "crashed")
                })
            })
    });
    if !failed {
        return Vec::new();
    }
    let label = match stage.number {
        Some(n) => format!("  ⏸ phase {n} blocked"),
        None => "  ⏸ blocked".to_string(),
    };
    vec![Span::styled(
        label,
        Style::default().fg(todo_failure_color()),
    )]
}
