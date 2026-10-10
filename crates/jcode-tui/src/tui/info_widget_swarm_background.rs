use super::frame::{self, Framed};
use super::{BackgroundInfo, InfoWidgetData, SwarmInfo, truncate_smart};
use crate::protocol::SwarmMemberStatus;
use crate::tui::color_support::rgb;
use ratatui::prelude::*;
use unicode_width::UnicodeWidthStr;

pub(super) fn render_swarm_widget(data: &InfoWidgetData, inner: Rect) -> Framed {
    let Some(info) = &data.swarm_info else {
        return Framed::default();
    };

    // Dock mode: this session manages agents.
    if !info.managed_members.is_empty() {
        return render_swarm_dock(info, inner);
    }

    let title = render_swarm_stats_line(info);
    let mut lines: Vec<Line> = Vec::new();

    if info.members.is_empty()
        && let Some(status) = &info.subagent_status
    {
        lines.push(Line::from(vec![
            Span::styled("▶ ", Style::default().fg(rgb(255, 200, 100))),
            Span::styled(
                truncate_smart(status, inner.width.saturating_sub(4) as usize),
                Style::default().fg(rgb(200, 200, 210)),
            ),
        ]));
    }

    let max_names = inner.height.saturating_sub(lines.len() as u16) as usize;
    let max_name_len = inner.width.saturating_sub(6) as usize;
    if !info.members.is_empty() {
        for member in info.members.iter().take(max_names.min(3)) {
            lines.push(swarm_member_line(member, max_name_len));
        }
    } else {
        for name in info.session_names.iter().take(max_names.min(3)) {
            lines.push(render_swarm_name_line(name, max_name_len));
        }
    }

    Framed::body(lines).title(title)
}

/// Border layout: `⏳ Background · 3 running` top-left, one compact row per
/// task (status glyph, label, progress bar, percent) in the body, `+N more`
/// bottom-left.
pub(super) fn render_background_widget(data: &InfoWidgetData, inner: Rect) -> Framed {
    let Some(info) = &data.background_info else {
        return Framed::default();
    };
    let Some(summary) = background_summary(info) else {
        return Framed::default();
    };
    let max_rows = (inner.height as usize).clamp(1, WIDGET_MAX_TASK_ROWS);
    let (rows, hidden) = background_task_rows(info, inner.width as usize, "", max_rows);
    let mut framed = Framed::body(rows).title(Line::from(vec![
        Span::styled("⏳ ", Style::default().fg(rgb(180, 140, 255))),
        frame::label(summary),
    ]));
    if hidden > 0 {
        framed = framed.footer(frame::more(hidden));
    }
    framed
}

pub(super) fn render_background_compact(info: &BackgroundInfo, width: usize) -> Vec<Line<'static>> {
    render_background_lines(info, width)
}

fn swarm_member_label(member: &SwarmMemberStatus) -> String {
    member
        .friendly_name
        .clone()
        .unwrap_or_else(|| member.session_id.chars().take(8).collect())
}

fn swarm_status_style(status: &str) -> (Color, &'static str) {
    match status {
        "spawned" => (rgb(140, 140, 150), "○"),
        "ready" => (rgb(120, 180, 120), "●"),
        "running" => (rgb(255, 200, 100), "▶"),
        "blocked" => (rgb(255, 170, 80), "⏸"),
        "failed" => (rgb(255, 100, 100), "✗"),
        "completed" => (rgb(100, 200, 100), "✓"),
        "stopped" => (rgb(140, 140, 150), "■"),
        "crashed" => (rgb(255, 80, 80), "!"),
        _ => (rgb(140, 140, 150), "·"),
    }
}

fn swarm_role_prefix(member: &SwarmMemberStatus) -> &'static str {
    match member.role.as_deref() {
        Some("coordinator") => "★ ",
        _ => "  ",
    }
}

fn swarm_member_line(member: &SwarmMemberStatus, max_width: usize) -> Line<'static> {
    let name = swarm_member_label(member);
    let mut detail = member.detail.clone().unwrap_or_default();
    if !detail.is_empty() {
        detail = format!(" - {}", detail);
    }
    let role_prefix = swarm_role_prefix(member);
    let line_text = truncate_smart(&format!("{} {}{}", name, member.status, detail), max_width);
    let (color, icon) = swarm_status_style(&member.status);
    Line::from(vec![
        Span::styled(
            role_prefix.to_string(),
            Style::default().fg(rgb(255, 200, 100)),
        ),
        Span::styled(format!("{} ", icon), Style::default().fg(color)),
        Span::styled(line_text, Style::default().fg(rgb(140, 140, 150))),
    ])
}

fn render_swarm_stats_line(info: &SwarmInfo) -> Line<'static> {
    let mut stats_parts: Vec<Span> =
        vec![Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100)))];

    if info.session_count > 0 {
        stats_parts.push(Span::styled(
            format!("{}s", info.session_count),
            Style::default().fg(rgb(160, 160, 170)),
        ));
    }
    if let Some(clients) = info.client_count {
        if info.session_count > 0 {
            stats_parts.push(Span::styled(" · ", Style::default().fg(rgb(100, 100, 110))));
        }
        stats_parts.push(Span::styled(
            format!("{}c", clients),
            Style::default().fg(rgb(160, 160, 170)),
        ));
    }

    Line::from(stats_parts)
}

fn render_swarm_name_line(name: &str, max_name_len: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled("  · ", Style::default().fg(rgb(100, 100, 110))),
        Span::styled(
            truncate_smart(name, max_name_len),
            Style::default().fg(rgb(140, 140, 150)),
        ),
    ])
}

fn render_background_lines(info: &BackgroundInfo, width: usize) -> Vec<Line<'static>> {
    let Some(summary) = background_summary(info) else {
        return Vec::new();
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("⏳ ", Style::default().fg(rgb(180, 140, 255))),
        Span::styled(summary, Style::default().fg(rgb(160, 160, 170))),
    ])];
    let (rows, hidden) = background_task_rows(info, width, "  ", COMPACT_MAX_TASK_ROWS);
    lines.extend(rows);
    if hidden > 0 {
        lines.push(Line::from(vec![
            Span::styled("   ", Style::default().fg(rgb(100, 100, 110))),
            Span::styled(
                format!("+{} more", hidden),
                Style::default().fg(rgb(140, 140, 150)),
            ),
        ]));
    }
    lines
}

/// Most task rows the standalone widget lists before `+N more`.
const WIDGET_MAX_TASK_ROWS: usize = 6;

/// Most task rows the compact (Overview) form lists before `+N more`.
const COMPACT_MAX_TASK_ROWS: usize = 3;

/// Task rows plus how many were left out. Prefers the session-scoped
/// `rows` (which carry status and percent). Falls back to plain running task
/// names when only the process-local manager knows about tasks.
fn background_task_rows(
    info: &BackgroundInfo,
    width: usize,
    indent: &'static str,
    max_rows: usize,
) -> (Vec<Line<'static>>, usize) {
    let row_width = width.saturating_sub(UnicodeWidthStr::width(indent)).max(12);
    if !info.rows.is_empty() {
        // Most recently active task first.
        let total = info.rows.len();
        let shown = total.min(max_rows);
        let lines = info
            .rows
            .iter()
            .rev()
            .take(shown)
            .map(|task| {
                let mut line = background_task_row_line(task, row_width);
                if !indent.is_empty() {
                    line.spans.insert(0, Span::raw(indent));
                }
                line
            })
            .collect();
        return (lines, total - shown);
    }

    let mut lines = Vec::new();
    let shown = info.running_tasks.len().min(max_rows);
    for (index, task) in info.running_tasks.iter().take(shown).enumerate() {
        let detail = if index == 0 {
            info.progress_detail.as_deref()
        } else {
            None
        };
        let row_text = if let Some(detail) = detail {
            truncate_smart(
                &format!("{} · {}", task, detail),
                row_width.saturating_sub(2),
            )
        } else {
            truncate_smart(task, row_width.saturating_sub(2))
        };
        lines.push(Line::from(vec![
            Span::raw(indent),
            Span::styled("◌ ", Style::default().fg(rgb(180, 140, 255))),
            Span::styled(row_text, Style::default().fg(rgb(180, 180, 190))),
        ]));
    }

    (lines, info.running_tasks.len().saturating_sub(shown))
}

/// One task row: `◌ cargo test  ━━╺─── 42%`. Completed tasks show `✓` with a
/// full bar, failed tasks `×` with `failed`.
pub(crate) fn background_task_row_line(
    task: &crate::tui::BackgroundTaskRow,
    width: usize,
) -> Line<'static> {
    use crate::tui::BackgroundTaskRowStatus as Status;
    const BAR_WIDTH: usize = 6;
    let dim = rgb(110, 110, 120);
    let (icon, task_color, percent) = match task.status {
        Status::Running => (
            "◌",
            rgb(180, 140, 255),
            task.percent.unwrap_or(0.0).clamp(0.0, 100.0),
        ),
        Status::Completed => ("✓", Color::Green, 100.0),
        Status::Failed => (
            "×",
            Color::Red,
            task.percent.unwrap_or(0.0).clamp(0.0, 100.0),
        ),
    };
    let status_label = if task.status == Status::Failed {
        "failed".to_string()
    } else {
        format!("{}%", percent.round() as u8)
    };
    let filled = ((percent / 100.0) * BAR_WIDTH as f32).round() as usize;
    let (active_bar, remaining_bar) = if task.status == Status::Failed {
        (
            "━".repeat(filled.min(BAR_WIDTH)),
            "─".repeat(BAR_WIDTH.saturating_sub(filled)),
        )
    } else if filled >= BAR_WIDTH {
        ("━".repeat(BAR_WIDTH), String::new())
    } else {
        (
            format!("{}╺", "━".repeat(filled)),
            "─".repeat(BAR_WIDTH.saturating_sub(filled + 1)),
        )
    };

    // icon + space + label + two spaces + bar + space + status
    let fixed_width = UnicodeWidthStr::width(
        format!("◌   {}{} {}", active_bar, remaining_bar, status_label).as_str(),
    );
    let max_label_width = width.saturating_sub(fixed_width).max(1);
    let label = truncate_task_label(&task.label, max_label_width);

    Line::from(vec![
        Span::styled(icon, Style::default().fg(task_color)),
        Span::raw(" "),
        Span::styled(label, Style::default().fg(rgb(190, 190, 200))),
        Span::raw("  "),
        Span::styled(active_bar, Style::default().fg(task_color)),
        Span::styled(remaining_bar, Style::default().fg(dim)),
        Span::styled(format!(" {}", status_label), Style::default().fg(dim)),
    ])
}

fn truncate_task_label(label: &str, max_width: usize) -> String {
    let label = label.replace(['\r', '\n'], " ");
    if UnicodeWidthStr::width(label.as_str()) <= max_width {
        return label;
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let mut truncated = String::new();
    let mut used = 0usize;
    for ch in label.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w + 1 > max_width {
            break;
        }
        used += w;
        truncated.push(ch);
    }
    truncated.push('…');
    truncated
}

fn background_summary(info: &BackgroundInfo) -> Option<String> {
    if !info.has_content() {
        return None;
    }
    let running = if info.rows.is_empty() {
        info.running_count
    } else {
        info.rows
            .iter()
            .filter(|row| row.status == crate::tui::BackgroundTaskRowStatus::Running)
            .count()
    };
    if running == 0 {
        return Some("Background".to_string());
    }
    Some(format!("Background · {} running", running))
}

/// Most agents the dock lists before collapsing the rest into the footer.
const DOCK_MAX_ROWS: usize = 6;

fn dock_status_rank(status: &str) -> u8 {
    match status {
        "blocked" | "waiting_network" | "failed" | "crashed" => 0,
        "running" | "streaming" | "thinking" => 1,
        "ready" | "spawned" => 2,
        _ => 3, // completed, done, stopped
    }
}

fn dock_is_finished(status: &str) -> bool {
    dock_status_rank(status) == 3
}

fn dock_is_attention(status: &str) -> bool {
    dock_status_rank(status) == 0
}

/// What the agent is doing, in the fewest words available: the error or
/// blocker when it needs attention, otherwise the latest status detail, the
/// last line it streamed, or the task it was spawned for.
fn dock_activity(member: &SwarmMemberStatus) -> String {
    let nonempty = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let tail = member
        .output_tail
        .as_deref()
        .and_then(|t| t.lines().rev().map(str::trim).find(|l| !l.is_empty()))
        .map(str::to_string);
    let detail = nonempty(&member.detail);
    let task = nonempty(&member.task_label);
    let picked = if dock_is_finished(&member.status) {
        task.or(detail)
    } else if dock_is_attention(&member.status) {
        detail.or(task).or(tail)
    } else {
        detail.or(tail).or(task)
    };
    picked.unwrap_or_else(|| member.status.clone())
}

/// Swarm dock. Border layout: `🐝 Swarm 2/4 active` top-left, attention count
/// top-right, overflow bottom-left, task-graph meter bottom-right. Body: one
/// row per agent, attention first, then working, idle, and finished.
///
/// ```text
/// ╭ 🐝 Swarm 2/4 active ──────────── ⚠ 1 ╮
/// │✗ reviewer  cargo test failed: 3 …    │
/// │⠋ builder   wiring commits widget  2/5│
/// │⠋ ★ lead    waiting on reviewer    1/3│
/// │✓ research  map the swarm code      5m│
/// ╰ +2 more ──────────── nodes 4/9 ▰▰▰▱▱ ╯
/// ```
fn render_swarm_dock(info: &SwarmInfo, inner: Rect) -> Framed {
    use jcode_tui_render::swarm_gallery::{humanize_age, status_accent, status_glyph};
    let members = &info.managed_members;
    let width = inner.width as usize;

    let mut order: Vec<&SwarmMemberStatus> = members.iter().collect();
    order.sort_by(|a, b| {
        dock_status_rank(&a.status)
            .cmp(&dock_status_rank(&b.status))
            .then_with(|| {
                let coord = |m: &SwarmMemberStatus| m.role.as_deref() != Some("coordinator");
                coord(a).cmp(&coord(b))
            })
            .then_with(|| a.session_id.cmp(&b.session_id))
    });

    let active = members
        .iter()
        .filter(|m| jcode_tui_render::swarm_gallery::is_active_status(&m.status))
        .count();
    let attention = members
        .iter()
        .filter(|m| dock_is_attention(&m.status))
        .count();
    let finished = members
        .iter()
        .filter(|m| dock_is_finished(&m.status))
        .count();

    let rows = (inner.height as usize)
        .clamp(1, DOCK_MAX_ROWS)
        .min(order.len());
    let shown = &order[..rows];

    let name_w = shown
        .iter()
        .map(|m| {
            let star = if m.role.as_deref() == Some("coordinator") {
                2
            } else {
                0
            };
            UnicodeWidthStr::width(swarm_member_label(m).as_str()) + star
        })
        .max()
        .unwrap_or(0)
        .min(12);

    let mut lines = Vec::with_capacity(rows);
    for member in shown {
        let accent = status_accent(&member.status);
        let glyph = status_glyph(&member.status, info.spinner_frame);
        let finished = dock_is_finished(&member.status);

        // Working agents show todo progress; finished or idle ones show how
        // long ago they last changed.
        let right = match member.todo_progress {
            Some((done, total)) if total > 0 && !finished => format!("{done}/{total}"),
            _ => member
                .status_age_secs
                .map(humanize_age)
                .filter(|a| a != "now")
                .unwrap_or_default(),
        };

        let star = member.role.as_deref() == Some("coordinator");
        let label_budget = name_w - if star { 2 } else { 0 };
        let label = truncate_smart(&swarm_member_label(member), label_budget.max(1));
        let label_pad = name_w
            .saturating_sub(UnicodeWidthStr::width(label.as_str()) + if star { 2 } else { 0 });

        // glyph + space + name + gap, then activity, then " right".
        let right_w = UnicodeWidthStr::width(right.as_str());
        let fixed = 2 + name_w + 2 + if right_w > 0 { right_w + 1 } else { 0 };
        let activity_w = width.saturating_sub(fixed);

        let mut spans = vec![Span::styled(
            format!("{glyph} "),
            Style::default().fg(accent),
        )];
        if star {
            spans.push(Span::styled("★ ", Style::default().fg(rgb(255, 200, 100))));
        }
        spans.push(Span::styled(
            format!("{label}{}  ", " ".repeat(label_pad)),
            Style::default().fg(if finished {
                rgb(130, 130, 140)
            } else {
                rgb(210, 210, 220)
            }),
        ));
        let activity = if activity_w >= 4 {
            truncate_smart(&dock_activity(member), activity_w)
        } else {
            String::new()
        };
        let activity_color = if dock_is_attention(&member.status) {
            rgb(255, 170, 110)
        } else if finished {
            rgb(110, 110, 120)
        } else {
            rgb(160, 160, 170)
        };
        let activity_len = UnicodeWidthStr::width(activity.as_str());
        spans.push(Span::styled(activity, Style::default().fg(activity_color)));
        if right_w > 0 && activity_w >= 4 {
            let pad = activity_w.saturating_sub(activity_len) + 1;
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(right, Style::default().fg(rgb(120, 120, 130))));
        }
        let line = Line::from(spans);
        lines.push(if line.width() <= width {
            line
        } else {
            frame::fit(&line, width).unwrap_or_default()
        });
    }

    let mut title = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        frame::label("Swarm "),
    ];
    if active > 0 {
        title.push(Span::styled(
            format!("{active}/{} active", members.len()),
            Style::default().fg(rgb(255, 200, 100)),
        ));
    } else if finished == members.len() {
        title.push(Span::styled(
            format!("{finished} done"),
            Style::default().fg(rgb(100, 200, 100)),
        ));
    } else {
        title.push(frame::dim(format!("{} idle", members.len())));
    }
    let mut framed = Framed::body(lines).title(Line::from(title));

    if attention > 0 {
        framed = framed.title_right(Span::styled(
            format!("⚠ {attention}"),
            Style::default().fg(rgb(255, 170, 80)).bold(),
        ));
    }

    let hidden = order.len() - rows;
    if hidden > 0 {
        framed = framed.footer(frame::more(hidden));
    }

    if let Some((done, running, total)) = info.plan_progress
        && total > 0
    {
        framed = framed.footer_right(plan_meter(done, running, total));
    }
    framed
}

/// `nodes 4/9 ▰▰▰▰▱▱▱▱` with green done, amber running, dim remainder.
fn plan_meter(done: u32, running: u32, total: u32) -> Line<'static> {
    const CELLS: u32 = 8;
    let done = done.min(total);
    let running = running.min(total - done);
    let done_c = (done * CELLS / total).max(u32::from(done > 0));
    let run_c = (((done + running) * CELLS / total).saturating_sub(done_c))
        .max(u32::from(running > 0))
        .min(CELLS - done_c);
    let rest = CELLS - done_c - run_c;
    Line::from(vec![
        frame::dim(format!("nodes {done}/{total} ")),
        Span::styled(
            "▰".repeat(done_c as usize),
            Style::default().fg(rgb(100, 200, 100)),
        ),
        Span::styled(
            "▰".repeat(run_c as usize),
            Style::default().fg(rgb(255, 200, 100)),
        ),
        Span::styled(
            "▱".repeat(rest as usize),
            Style::default().fg(rgb(80, 80, 90)),
        ),
    ])
}

/// Overview Swarm section: a compact projection of the swarm dock. One row
/// per agent (attention first), a header line carrying the same
/// `N/M active` headline, and a trailing plan-meter line when a plan is
/// active. Mirrors [`compact_swarm_height`].
pub(super) fn render_swarm_compact(info: &SwarmInfo, width: usize) -> Vec<Line<'static>> {
    use jcode_tui_render::swarm_gallery::{status_accent, status_glyph};
    let members = &info.managed_members;
    if members.is_empty() {
        return Vec::new();
    }

    let mut order: Vec<&SwarmMemberStatus> = members.iter().collect();
    order.sort_by(|a, b| {
        dock_status_rank(&a.status)
            .cmp(&dock_status_rank(&b.status))
            .then_with(|| {
                let coord = |m: &SwarmMemberStatus| m.role.as_deref() != Some("coordinator");
                coord(a).cmp(&coord(b))
            })
            .then_with(|| a.session_id.cmp(&b.session_id))
    });

    let active = members
        .iter()
        .filter(|m| jcode_tui_render::swarm_gallery::is_active_status(&m.status))
        .count();
    let attention = members
        .iter()
        .filter(|m| dock_is_attention(&m.status))
        .count();

    let shown = &order[..order.len().min(OVERVIEW_SWARM_MAX_ROWS)];

    let mut lines = Vec::with_capacity(shown.len() + 2);

    // Header: `Swarm N/M active ⚠K`, same headline the dock border carries.
    let mut header = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!("Swarm {}/{} active", active, members.len()),
            Style::default().fg(rgb(180, 180, 190)).bold(),
        ),
    ];
    if attention > 0 {
        header.push(Span::styled(
            format!("  ⚠ {attention}"),
            Style::default().fg(rgb(255, 170, 80)).bold(),
        ));
    }
    lines.push(Line::from(header));

    for member in shown {
        let accent = status_accent(&member.status);
        let glyph = status_glyph(&member.status, info.spinner_frame);
        let name = swarm_member_label(member);
        let star = if member.role.as_deref() == Some("coordinator") {
            "★ "
        } else {
            ""
        };
        let detail = dock_activity(member);
        let right = match member.todo_progress {
            Some((done, total)) if total > 0 && !dock_is_finished(&member.status) => {
                format!("{done}/{total}")
            }
            _ => member
                .status_age_secs
                .and_then(|s| {
                    let age = jcode_tui_render::swarm_gallery::humanize_age(s);
                    (age != "now").then_some(age)
                })
                .unwrap_or_default(),
        };
        let fixed = 2
            + UnicodeWidthStr::width(star)
            + UnicodeWidthStr::width(name.as_str())
            + 2
            + if right.is_empty() {
                0
            } else {
                UnicodeWidthStr::width(right.as_str()) + 1
            };
        let activity_w = width.saturating_sub(fixed);
        let activity = if activity_w >= 4 {
            truncate_smart(&detail, activity_w)
        } else {
            String::new()
        };
        let mut spans = vec![
            Span::styled(format!("{glyph} "), Style::default().fg(accent)),
            Span::styled(star.to_string(), Style::default().fg(rgb(255, 200, 100))),
            Span::styled(name, Style::default().fg(rgb(210, 210, 220))),
            Span::raw("  "),
        ];
        let activity_len = UnicodeWidthStr::width(activity.as_str());
        spans.push(Span::styled(
            activity,
            Style::default().fg(rgb(160, 160, 170)),
        ));
        if !right.is_empty() && activity_w >= 4 {
            let pad = activity_w.saturating_sub(activity_len) + 1;
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(right, Style::default().fg(rgb(120, 120, 130))));
        }
        let line = Line::from(spans);
        lines.push(if line.width() <= width {
            line
        } else {
            frame::fit(&line, width).unwrap_or_default()
        });
    }

    let hidden = order.len().saturating_sub(OVERVIEW_SWARM_MAX_ROWS);
    if hidden > 0 {
        lines.push(Line::from(vec![Span::styled(
            format!("  +{hidden} more"),
            Style::default().fg(rgb(110, 110, 125)),
        )]));
    }

    if let Some((done, running, total)) = info.plan_progress
        && total > 0
    {
        lines.push(plan_meter(done, running, total));
    }

    lines
}

/// Agents the overview Swarm section lists before collapsing the rest.
pub(crate) const OVERVIEW_SWARM_MAX_ROWS: usize = 3;
