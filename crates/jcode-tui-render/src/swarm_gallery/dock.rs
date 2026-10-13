//! Swarm dock: narrow, vertical agent list for the info-widget margins.

use ratatui::prelude::*;

use jcode_tui_style::color::rgb;

use super::types::GalleryMember;
use super::util::{
    clamp_line_to_width, disp_w, is_active_status, role_color, role_glyph,
    sort_members_for_display, status_accent, status_glyph, truncate_label,
};

/// info-widget margins (~20-38 inner columns).
///
/// Layout (all lines bounded to `width`, at most `max_height` lines):
/// ```text
/// 🐝 2/4 active · plan 3/7 · ⚠1
/// ▸ ★ coordinator      ⠋ 4/9
///   │ ⚙ bash cargo build 47s
///   │ carving gallery band
///   researcher         ⠙ 2/5
///   reviewer           ✓
///   +2 more
///   j/k · enter · esc
/// ```
///
/// The selected agent (clamped into range) gets a short live tail from its
/// `body` directly beneath its row: up to 2 lines, or 4 when `focused`. When
/// not all agents fit, the list windows around the selection and a `+N more`
/// line reports the rest. The hint line appears only when `focused`.
pub fn render_swarm_dock(
    members: &[GalleryMember],
    selected: usize,
    focused: bool,
    plan: Option<(u32, u32)>,
    spinner_frame: usize,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() || width < 12 || max_height < 2 {
        return Vec::new();
    }
    let ordered = sort_members_for_display(members);
    let selected = selected.min(ordered.len() - 1);
    let active = members
        .iter()
        .filter(|m| is_active_status(&m.status))
        .count();
    let attention = members
        .iter()
        .filter(|m| {
            matches!(
                m.status.as_str(),
                "blocked" | "failed" | "crashed" | "waiting_network"
            )
        })
        .count();

    let mut out: Vec<Line<'static>> = Vec::new();
    out.push(dock_header(members.len(), active, attention, plan, width));

    // ---- Line budget: header (emitted) + rows + selected tail + hints ----
    let hint_rows = usize::from(focused);
    let budget = max_height.saturating_sub(1 + hint_rows);
    let n = ordered.len();
    let tail_want = if focused { 4 } else { 2 };
    // Prefer listing every agent; leftover lines become the selected tail.
    // When agents alone overflow, window around the selection and spend one
    // line on the "+N more" marker.
    let (list_rows, tail_rows) = if n <= budget {
        (n, budget.saturating_sub(n).min(tail_want))
    } else {
        (budget.saturating_sub(1).max(1), 0)
    };
    let hidden = n.saturating_sub(list_rows);
    let first = if selected >= list_rows {
        selected + 1 - list_rows
    } else {
        0
    };

    let tail_lines = if tail_rows > 0 {
        dock_tail_lines(ordered[selected], width, tail_rows)
    } else {
        Vec::new()
    };

    for (idx, member) in ordered.iter().enumerate().skip(first).take(list_rows) {
        let is_sel = idx == selected;
        out.push(dock_row(member, is_sel, focused, spinner_frame, width));
        if is_sel {
            out.extend(tail_lines.iter().cloned());
        }
    }
    if hidden > 0 {
        out.push(Line::from(Span::styled(
            format!("  +{hidden} more"),
            Style::default().fg(rgb(130, 130, 140)),
        )));
    }

    if focused {
        out.push(Line::from(vec![
            Span::styled("  j/k", Style::default().fg(rgb(150, 170, 210))),
            Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))),
            Span::styled("enter", Style::default().fg(rgb(150, 170, 210))),
            Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))),
            Span::styled("esc", Style::default().fg(rgb(150, 170, 210))),
        ]));
    }

    // Hard bounds: never exceed the height budget (tiny budgets can otherwise
    // overflow via the header + marker + hint fixed rows) or the width.
    out.truncate(max_height);
    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}

/// Dock header: bee + active tally, then plan progress and attention count,
/// dropped right-to-left when the width is too tight.
fn dock_header(
    total: usize,
    active: usize,
    attention: usize,
    plan: Option<(u32, u32)>,
    width: usize,
) -> Line<'static> {
    let sep_style = Style::default().fg(rgb(80, 80, 90));
    let mut spans: Vec<Span<'static>> = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!("{active}/{total} active"),
            Style::default().fg(if active > 0 {
                rgb(255, 200, 100)
            } else {
                rgb(120, 120, 130)
            }),
        ),
    ];
    let mut used: usize = spans.iter().map(|s| disp_w(&s.content)).sum();
    if let Some((done, total)) = plan {
        let text = format!("plan {done}/{total}");
        if used + 3 + disp_w(&text) <= width {
            used += 3 + disp_w(&text);
            spans.push(Span::styled(" · ", sep_style));
            spans.push(Span::styled(text, Style::default().fg(rgb(160, 160, 170))));
        }
    }
    if attention > 0 {
        let text = format!("⚠{attention}");
        if used + 3 + disp_w(&text) <= width {
            spans.push(Span::styled(" · ", sep_style));
            spans.push(Span::styled(text, Style::default().fg(rgb(255, 170, 80))));
        }
    }
    Line::from(spans)
}

/// One dock row: selection marker, optional role glyph, label, then a
/// right-aligned status glyph and optional todo counter.
fn dock_row(
    member: &GalleryMember,
    selected: bool,
    focused: bool,
    spinner_frame: usize,
    width: usize,
) -> Line<'static> {
    let accent =
        role_color(member.role.as_deref()).unwrap_or_else(|| status_accent(&member.status));
    let active = is_active_status(&member.status);
    let marker = if selected { "▸ " } else { "  " };
    let glyph = role_glyph(member.role.as_deref())
        .map(|g| format!("{g} "))
        .unwrap_or_default();
    let status = status_glyph(&member.status, spinner_frame);
    let todo = member.todo.map(|(d, t)| format!(" {d}/{t}"));

    let right_w = disp_w(status) + todo.as_ref().map(|t| disp_w(t)).unwrap_or(0);
    let fixed = 2 + disp_w(&glyph) + 1 + right_w;
    let label = truncate_label(&member.label, width.saturating_sub(fixed).max(4));
    let filler = width
        .saturating_sub(2 + disp_w(&glyph) + disp_w(&label) + right_w)
        .max(1);

    let mut label_style = if active {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(rgb(110, 110, 125))
    };
    if selected {
        label_style = label_style.add_modifier(Modifier::UNDERLINED);
    }
    if selected && focused {
        label_style = label_style.add_modifier(Modifier::REVERSED);
    }
    let mut spans = vec![Span::styled(
        marker.to_string(),
        Style::default().fg(if selected { accent } else { rgb(90, 90, 100) }),
    )];
    if !glyph.is_empty() {
        let glyph_style = if active {
            Style::default().fg(accent)
        } else {
            Style::default().fg(rgb(110, 110, 125))
        };
        spans.push(Span::styled(glyph, glyph_style));
    }
    spans.push(Span::styled(label, label_style));
    spans.push(Span::raw(" ".repeat(filler)));
    let status_style = if active {
        Style::default().fg(accent)
    } else {
        Style::default().fg(rgb(110, 110, 125))
    };
    spans.push(Span::styled(status.to_string(), status_style));
    if let Some(todo) = todo {
        spans.push(Span::styled(todo, Style::default().fg(rgb(130, 130, 140))));
    }
    Line::from(spans)
}

/// The selected agent's live tail: the last `rows` non-meta body lines,
/// dim, behind a `│` gutter.
fn dock_tail_lines(member: &GalleryMember, width: usize, rows: usize) -> Vec<Line<'static>> {
    let text_budget = width.saturating_sub(4);
    member
        .body
        .iter()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('·'))
        .rev()
        .take(rows)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|l| {
            Line::from(vec![
                Span::styled("  │ ", Style::default().fg(rgb(80, 80, 90))),
                Span::styled(
                    truncate_label(l, text_budget),
                    Style::default().fg(rgb(160, 160, 170)),
                ),
            ])
        })
        .collect()
}
