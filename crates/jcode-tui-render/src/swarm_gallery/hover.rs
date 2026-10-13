//! Hovered-agent detail viewport: transcript tail and todo list.

use ratatui::prelude::*;

use jcode_tui_style::color::rgb;

use super::types::{GalleryMember, GalleryToolIntent};
use super::util::{
    format_cost, format_elapsed_short, format_tokens, role_color, status_accent, status_glyph,
    truncate_label,
};

/// Render the expanded detail viewport for the hovered agent in the focused
/// strip: a header, a tail of the agent's live transcript, and its todo list.
///
/// Layout (at most `budget` lines, fewer when there is less content):
/// ```text
///    ⠙ researcher · thinking · 12s
///    │ Editing crates/jcode-tui/src/tui/ui.rs
///    │   carving the gallery band off chat_area
///    ├ todos 3/9
///    │ ✓ wire the bus tap
///    │ ▸ carve the gallery band
///    │ · run the ui tests
/// ```
/// Todos get at most half the budget (they are skimmable); the transcript tail
/// takes the rest. Every line is truncated to `width`.
pub(crate) fn render_hovered_detail(
    m: &GalleryMember,
    spinner_frame: usize,
    width: usize,
    budget: usize,
) -> Vec<Line<'static>> {
    let accent = role_color(m.role.as_deref()).unwrap_or_else(|| status_accent(&m.status));
    let dim = rgb(120, 120, 130);
    const GUTTER: &str = "   ";

    // The age hint the adapter appends ('·'-prefixed meta) moves into the header.
    let age: Option<String> = m
        .body
        .iter()
        .rev()
        .find(|l| l.trim_start().starts_with('·'))
        .map(|l| l.trim().trim_start_matches('·').trim().to_string());

    let mut out: Vec<Line<'static>> = Vec::new();

    // ---- Header ----
    let mut header: Vec<Span<'static>> = vec![
        Span::raw(GUTTER),
        Span::styled(
            format!("{} {}", status_glyph(&m.status, spinner_frame), m.label),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" · {}", m.status), Style::default().fg(dim)),
    ];
    if let Some(age) = age {
        header.push(Span::styled(format!(" · {age}"), Style::default().fg(dim)));
    }
    out.push(Line::from(header));
    out.extend(hovered_detail_body(
        m,
        Some(spinner_frame),
        width,
        budget.saturating_sub(1),
        true,
    ));
    out
}

/// The body of the hovered-agent detail viewport: a tail of the agent's live
/// transcript plus its todo list, gutter-indented, without the header row.
/// Used directly by the vertical strip (where the selected agent's row already
/// serves as the header) and by [`render_hovered_detail`].
pub(crate) fn hovered_detail_body(
    m: &GalleryMember,
    spinner_frame: Option<usize>,
    width: usize,
    budget: usize,
    show_member_rail: bool,
) -> Vec<Line<'static>> {
    let dim = rgb(120, 120, 130);
    let text_fg = rgb(190, 190, 200);
    let gutter_fg = rgb(80, 80, 90);
    const GUTTER: &str = "   ";
    const BAR: &str = "│   ";

    if budget == 0 {
        return Vec::new();
    }

    let mut out: Vec<Line<'static>> = Vec::new();

    // ---- Expanded metadata lines (visible when this is the focused agent) ----
    // Show elapsed time, effort level, and auth method as compact indicator
    // lines above the todo card so they are always visible in the detail pane.
    let detail_fg = rgb(110, 110, 125);
    if let Some(secs) = m.elapsed_secs
        && out.len() < budget
    {
        out.push(Line::from(vec![
            Span::raw(GUTTER),
            Span::styled(
                if show_member_rail { BAR } else { "    " },
                Style::default().fg(gutter_fg),
            ),
            Span::styled("  ⏱ ", Style::default().fg(detail_fg)),
            Span::raw(format_elapsed_short(secs)),
        ]));
    }
    if let Some(ref effort) = m.effort
        && !effort.trim().is_empty()
        && out.len() < budget
    {
        out.push(Line::from(vec![
            Span::raw(GUTTER),
            Span::styled(
                if show_member_rail { BAR } else { "    " },
                Style::default().fg(gutter_fg),
            ),
            Span::styled("  ⚡ ", Style::default().fg(detail_fg)),
            Span::raw(format!("effort: {effort}")),
        ]));
    }
    if let Some(ref auth) = m.auth_method
        && !auth.trim().is_empty()
        && out.len() < budget
    {
        out.push(Line::from(vec![
            Span::raw(GUTTER),
            Span::styled(
                if show_member_rail { BAR } else { "    " },
                Style::default().fg(gutter_fg),
            ),
            Span::styled("  🔑 ", Style::default().fg(detail_fg)),
            Span::raw(format!("auth: {auth}")),
        ]));
    }

    // ---- Performance metrics ----
    if let Some(tokens) = m.input_tokens
        && out.len() < budget
    {
        out.push(Line::from(vec![
            Span::raw(GUTTER),
            Span::styled(
                if show_member_rail { BAR } else { "    " },
                Style::default().fg(gutter_fg),
            ),
            Span::styled("  📥 ", Style::default().fg(detail_fg)),
            Span::raw(format_tokens(tokens)),
        ]));
    }
    if let Some(tokens) = m.output_tokens
        && out.len() < budget
    {
        out.push(Line::from(vec![
            Span::raw(GUTTER),
            Span::styled(
                if show_member_rail { BAR } else { "    " },
                Style::default().fg(gutter_fg),
            ),
            Span::styled("  📤 ", Style::default().fg(detail_fg)),
            Span::raw(format_tokens(tokens)),
        ]));
    }
    if let Some(depth) = m.queue_depth
        && depth > 0
        && out.len() < budget
    {
        out.push(Line::from(vec![
            Span::raw(GUTTER),
            Span::styled(
                if show_member_rail { BAR } else { "    " },
                Style::default().fg(gutter_fg),
            ),
            Span::styled("  ⚠️  ", Style::default().fg(rgb(255, 180, 50))),
            Span::raw(format!("{depth} tasks queued")),
        ]));
    }
    if let Some(cents) = m.cost_cents
        && out.len() < budget
    {
        out.push(Line::from(vec![
            Span::raw(GUTTER),
            Span::styled(
                if show_member_rail { BAR } else { "    " },
                Style::default().fg(gutter_fg),
            ),
            Span::styled("  💰 ", Style::default().fg(detail_fg)),
            Span::raw(format_cost(cents)),
        ]));
    }

    // ---- Todo card ----
    // Show a sliding window of four item names. Tool activity belongs to the
    // active item and is nested immediately below it, newest at the bottom.
    if !m.todo_items.is_empty() {
        const TODO_WINDOW: usize = 4;
        const TOOL_WINDOW: usize = 3;
        let active = m
            .todo_items
            .iter()
            .position(|t| t.status == "in_progress")
            .or_else(|| m.todo_items.iter().position(|t| t.status != "completed"))
            .unwrap_or_else(|| m.todo_items.len().saturating_sub(1));
        let shown = TODO_WINDOW.min(m.todo_items.len());
        let start = active
            .saturating_sub(1)
            .min(m.todo_items.len().saturating_sub(shown));
        let text_budget = width.saturating_sub(GUTTER.len() + BAR.len() + 2);

        for (visible_idx, (idx, todo)) in m
            .todo_items
            .iter()
            .enumerate()
            .skip(start)
            .take(shown)
            .enumerate()
        {
            if out.len() >= budget {
                break;
            }
            let (glyph, glyph_fg, emph) = match todo.status.as_str() {
                "completed" => ("✓".to_string(), rgb(100, 200, 100), false),
                "in_progress" => (
                    spinner_frame
                        .map(|frame| status_glyph("running", frame))
                        .unwrap_or("●")
                        .to_string(),
                    rgb(255, 200, 100),
                    true,
                ),
                _ => ("○".to_string(), dim, false),
            };
            let mut style = Style::default().fg(if emph { text_fg } else { dim });
            if emph {
                style = style.add_modifier(Modifier::BOLD);
            }
            let todo_rail = if !show_member_rail {
                "    "
            } else if visible_idx + 1 == shown {
                "└─  "
            } else {
                BAR
            };
            out.push(Line::from(vec![
                Span::raw(GUTTER),
                Span::styled(todo_rail, Style::default().fg(gutter_fg)),
                Span::styled(format!("{glyph} "), Style::default().fg(glyph_fg)),
                Span::styled(truncate_label(&todo.content, text_budget), style),
            ]));

            if idx == active {
                let tools: Vec<&GalleryToolIntent> = todo
                    .tool_intents
                    .iter()
                    .rev()
                    .take(TOOL_WINDOW)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let tool_count = tools.len();
                for (tool_idx, tool) in tools.into_iter().enumerate() {
                    if out.len() >= budget {
                        break;
                    }
                    let (tool_glyph, fg) = match tool.status.as_str() {
                        "running" => (
                            spinner_frame
                                .map(|frame| status_glyph("running", frame))
                                .unwrap_or("●"),
                            rgb(255, 200, 100),
                        ),
                        "error" => ("✗", rgb(230, 100, 100)),
                        _ => ("✓", rgb(100, 200, 100)),
                    };
                    let branch = if tool_idx + 1 == tool_count {
                        "└─"
                    } else {
                        "├─"
                    };
                    let progress = tool
                        .progress
                        .as_ref()
                        .map(|(current, total, _unit)| format!(" · {current}/{total}"))
                        .unwrap_or_default();
                    let label = format!("{} · {}{progress}", tool.tool_name, tool.intent);
                    let nested_budget = width.saturating_sub(GUTTER.len() + BAR.len() + 7);
                    out.push(Line::from(vec![
                        Span::raw(GUTTER),
                        Span::styled(
                            if show_member_rail {
                                "│     "
                            } else {
                                "      "
                            },
                            Style::default().fg(gutter_fg),
                        ),
                        Span::styled(format!("{branch} "), Style::default().fg(gutter_fg)),
                        Span::styled(format!("{tool_glyph} "), Style::default().fg(fg)),
                        Span::styled(
                            truncate_label(&label, nested_budget),
                            Style::default().fg(rgb(155, 155, 165)),
                        ),
                    ]));
                }
            }
        }
        return out;
    }

    // No todos yet: retain the live transcript fallback.
    let transcript: Vec<&str> = m
        .body
        .iter()
        .map(|l| l.as_str())
        .filter(|l| !l.trim_start().starts_with('·'))
        .collect();
    let text_budget = width.saturating_sub(GUTTER.len() + BAR.len());
    let shown: Vec<&str> = transcript
        .iter()
        .copied()
        .rev()
        .take(budget)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if shown.iter().all(|l| l.trim().is_empty()) {
        out.push(Line::from(vec![
            Span::raw(GUTTER),
            Span::styled(
                if show_member_rail { BAR } else { "    " },
                Style::default().fg(gutter_fg),
            ),
            Span::styled(format!("[{}]", m.status), Style::default().fg(dim)),
        ]));
    } else {
        for line in shown {
            out.push(Line::from(vec![
                Span::raw(GUTTER),
                Span::styled(
                    if show_member_rail { BAR } else { "    " },
                    Style::default().fg(gutter_fg),
                ),
                Span::styled(
                    truncate_label(line, text_budget),
                    Style::default().fg(text_fg),
                ),
            ]));
        }
    }

    out
}
