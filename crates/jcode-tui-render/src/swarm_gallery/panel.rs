//! Swarm panel (list + detail) and compact summary for the info-widget.

use ratatui::prelude::*;

use jcode_tui_style::color::rgb;

use super::render::members_to_tiles;
use super::types::GalleryMember;
use super::util::{
    clamp_line_to_width, disp_w, display_index_to_tile_index, is_active_status, role_color,
    role_glyph, sort_members_for_display, status_accent, truncate_label,
};

pub fn render_swarm_panel(
    members: &[GalleryMember],
    selected: usize,
    focused: bool,
    width: usize,
    max_height: usize,
    rename_hint: Option<&str>,
) -> Vec<Line<'static>> {
    if members.is_empty() || width < 8 || max_height < 3 {
        return Vec::new();
    }
    let tiles = members_to_tiles(members);
    // members_to_tiles re-sorts; mirror that ordering for the list so the
    // selected index lines up with what is shown.
    let ordered = sort_members_for_display(members);
    let selected = selected.min(ordered.len().saturating_sub(1));

    let active = members
        .iter()
        .filter(|m| is_active_status(&m.status))
        .count();
    let mut out: Vec<Line<'static>> = Vec::new();
    out.push(panel_header(members.len(), active, focused));

    // ---- Detail viewport for the selected agent ----
    let rename_budget = if rename_hint.is_some() { 1 } else { 0 };
    let detail_budget = if max_height >= 7 {
        ((max_height - rename_budget) / 2).max(3)
    } else {
        0
    };
    let list_budget = max_height
        .saturating_sub(1)
        .saturating_sub(detail_budget)
        .saturating_sub(rename_budget);

    // ---- Agent list ----
    let list_rows = list_budget.min(ordered.len());
    // Scroll the list so the selection stays visible.
    let first = if selected >= list_rows {
        selected + 1 - list_rows
    } else {
        0
    };
    for (idx, member) in ordered
        .iter()
        .enumerate()
        .skip(first)
        .take(list_rows.max(1))
    {
        out.push(list_row(member, idx == selected, focused, width));
    }

    // ---- Detail viewport for the selected agent ----
    if detail_budget >= 3
        && let Some(tile) = tiles.get(display_index_to_tile_index(&ordered, members, selected))
    {
        let detail = crate::swarm_tiles::render_single_tile(tile, width, detail_budget, focused);
        out.extend(detail);
    }

    // ---- Rename input line ----
    if let Some(buffer) = rename_hint {
        let cursor = "▌";
        let label = format!("Rename: {buffer}{cursor}");
        let label_w = unicode_width::UnicodeWidthStr::width(label.as_str());
        let mut spans = vec![Span::styled(label, Style::default().fg(rgb(200, 200, 210)))];
        // Pad to width
        if label_w < width {
            spans.push(Span::styled(
                " ".repeat(width - label_w),
                Style::default().fg(rgb(60, 60, 70)),
            ));
        }
        out.push(Line::from(spans));
    }

    // Hard bound: the header carries a fixed hint and list rows budget by
    // display width, but never let any line exceed the panel width.
    for line in &mut out {
        clamp_line_to_width(line, width);
    }

    out
}

/// Render the compact swarm summary: at most two lines for the info-widget
/// margins.
///
/// ```text
/// 🐝 2/4 agents · nodes 5/12 · ⚠1
/// ▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁   (green done · yellow running · dim rest)
/// ```
///
/// Line 1 tallies active/total agents, then (width permitting, dropped
/// right-to-left) done/total task-graph nodes and an attention count. Line 2
/// is the plan progress bar: green cells are done nodes, yellow cells are
/// running nodes, dim cells are the remainder. The bar is omitted when there
/// is no plan or `max_height` < 2.
pub fn render_swarm_compact(
    members: &[GalleryMember],
    plan: Option<(u32, u32, u32)>,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() || width < 8 || max_height == 0 {
        return Vec::new();
    }
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

    let sep_style = Style::default().fg(rgb(80, 80, 90));
    let mut spans: Vec<Span<'static>> = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!("{active}/{} agents", members.len()),
            Style::default().fg(if active > 0 {
                rgb(255, 200, 100)
            } else {
                rgb(120, 120, 130)
            }),
        ),
    ];
    let mut used: usize = spans.iter().map(|s| disp_w(&s.content)).sum();
    if let Some((done, _running, total)) = plan {
        let text = format!("nodes {done}/{total}");
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
    let mut out = vec![Line::from(spans)];

    if let Some((done, running, total)) = plan
        && total > 0
        && max_height >= 2
    {
        out.push(plan_progress_bar(done, running, total, width));
    }

    out.truncate(max_height);
    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}

/// The compact widget's plan bar: green = done, yellow = running, dim = the
/// rest. Rendered as a low-profile underline (▁) rather than full-height
/// blocks. Non-empty classes always get at least one cell so tiny progress is
/// visible, and the bar never exceeds `width` cells.
fn plan_progress_bar(done: u32, running: u32, total: u32, width: usize) -> Line<'static> {
    const CELL: &str = "▁";
    let cells = width.max(1);
    let total = total.max(1) as usize;
    let done = (done as usize).min(total);
    let running = (running as usize).min(total - done);

    let mut done_w = done * cells / total;
    if done > 0 {
        done_w = done_w.clamp(1, cells);
    }
    let mut running_w = ((done + running) * cells / total).saturating_sub(done_w);
    if running > 0 {
        running_w = running_w.clamp(1, cells - done_w);
    }
    let empty_w = cells - done_w - running_w;

    let mut spans: Vec<Span<'static>> = Vec::new();
    if done_w > 0 {
        spans.push(Span::styled(
            CELL.repeat(done_w),
            Style::default().fg(rgb(100, 200, 100)),
        ));
    }
    if running_w > 0 {
        spans.push(Span::styled(
            CELL.repeat(running_w),
            Style::default().fg(rgb(255, 200, 100)),
        ));
    }
    if empty_w > 0 {
        spans.push(Span::styled(
            CELL.repeat(empty_w),
            Style::default().fg(rgb(60, 60, 70)),
        ));
    }
    Line::from(spans)
}

fn panel_header(total: usize, active: usize, focused: bool) -> Line<'static> {
    let agent_color = if focused {
        rgb(220, 220, 230) // bright white when focused
    } else {
        rgb(160, 160, 170) // dim gray when unfocused
    };
    let active_color = if focused {
        if active > 0 {
            rgb(255, 200, 100) // accent gold when focused and active
        } else {
            rgb(150, 150, 160)
        }
    } else {
        rgb(140, 140, 150) // dim when unfocused
    };

    let mut spans = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!("{} agent{}", total, if total == 1 { "" } else { "s" }),
            Style::default().fg(agent_color),
        ),
    ];
    if active > 0 {
        spans.push(Span::styled(
            format!(" · {active} active"),
            Style::default().fg(active_color),
        ));
    }
    if focused {
        spans.push(Span::styled(
            "  (j/k select · o pop out · esc)",
            Style::default().fg(rgb(100, 100, 110)),
        ));
    }
    Line::from(spans)
}

/// One row in the agent list: a selection marker, optional role glyph, the
/// label, a status badge, and an age hint, all bounded to `width`.
fn list_row(member: &GalleryMember, selected: bool, focused: bool, width: usize) -> Line<'static> {
    let accent =
        role_color(member.role.as_deref()).unwrap_or_else(|| status_accent(&member.status));
    let marker = if selected { "▸ " } else { "  " };
    let glyph = role_glyph(member.role.as_deref())
        .map(|g| format!("{g} "))
        .unwrap_or_default();

    // Badge + age live on the right; build them first to know how much room the
    // label gets.
    let badge = format!("[{}]", member.status);
    let age = member
        .body
        .iter()
        .rev()
        .find_map(|l| l.strip_prefix("· ").map(|s| s.trim_end_matches(" ago")))
        .map(|a| a.to_string());

    let marker_w = 2;
    let glyph_w = disp_w(&glyph);
    let badge_w = disp_w(&badge);
    let age_w = age.as_ref().map(|a| disp_w(a) + 1).unwrap_or(0);
    // Reserve: marker + glyph + label + space + badge + space + age.
    let reserved = marker_w + glyph_w + 1 + badge_w + age_w + 1;
    let label_budget = width.saturating_sub(reserved).max(4);
    let label = truncate_label(&member.label, label_budget);
    let label_w = disp_w(&label);

    let label_style = if selected && focused {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default().fg(rgb(235, 235, 245))
    } else {
        Style::default().fg(rgb(170, 170, 180))
    };
    let marker_style = if selected && focused {
        Style::default().fg(accent)
    } else if selected {
        Style::default().fg(rgb(150, 150, 160))
    } else {
        Style::default().fg(rgb(90, 90, 100))
    };

    // Compute filler so the badge/age right-align.
    let used = marker_w + glyph_w + label_w;
    let right_w = badge_w + age_w;
    let filler = width.saturating_sub(used + right_w).max(1);

    let mut spans = vec![Span::styled(marker.to_string(), marker_style)];
    if !glyph.is_empty() {
        spans.push(Span::styled(glyph, Style::default().fg(accent)));
    }
    spans.push(Span::styled(label, label_style));
    spans.push(Span::raw(" ".repeat(filler)));
    spans.push(Span::styled(badge, Style::default().fg(accent)));
    if let Some(age) = age {
        spans.push(Span::styled(
            format!(" {age}"),
            Style::default().fg(rgb(110, 110, 120)),
        ));
    }
    Line::from(spans)
}
