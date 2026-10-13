//! Swarm strip rendering: vertical (accordion) agent strip view.

use ratatui::prelude::*;

use jcode_tui_style::color::rgb;

use super::hover::hovered_detail_body;
use super::strip::SwarmStripHint;
use super::types::GalleryMember;
use super::util::{
    card_status_label, clamp_line_to_width, disp_w, format_elapsed, format_model, is_active_status,
    role_color, sort_members_for_display, status_accent, status_glyph, truncate_label,
};

/// Minimum width for a task label chip (from strip.rs shared constant).
const CHIP_TASK_MIN_W: usize = 6;

/// Render the vertical (accordion-style) swarm strip.
///
/// Each agent occupies one row: icon + status glyph + name + optional task
/// label + optional todo count. The selected agent expands its detail
/// directly beneath its row; a keybinding hint line closes the strip. All
/// bounded by `max_height` total lines.
///
/// ```text
/// 🐝 ⠙ 🦊 · wire the auth flow 3/9            2/3 active · alt+n controls
///  ▸ ⠹ 🐝 bee · audit the webhook path
///    │ checking the signing secret path
///    │ ▸ verify replay protection
///    ✓ 🐅 · support/contact page
///    alt+↑/↓ select · alt+o open · alt+shift+p prompt · esc exit
/// ```
#[allow(clippy::too_many_arguments)]
pub fn render_swarm_strip_vertical(
    members: &[GalleryMember],
    selected: usize,
    focused: bool,
    hints: &[SwarmStripHint],
    enter_hint: Option<&str>,
    spinner_frame: usize,
    width: usize,
    max_rows: usize,
    max_height: usize,
    batch_selected: Option<&std::collections::HashSet<usize>>,
) -> Vec<Line<'static>> {
    if members.is_empty() || width < 8 || max_rows == 0 {
        return Vec::new();
    }
    let ordered = sort_members_for_display(members);
    let selected = selected.min(ordered.len().saturating_sub(1));
    let active = members
        .iter()
        .filter(|m| is_active_status(&m.status))
        .count();

    // Row budget: reserve one row for "+N more" when not everything fits.
    let shown = if ordered.len() <= max_rows {
        ordered.len()
    } else {
        max_rows.saturating_sub(1).max(1)
    };
    let hidden = ordered.len() - shown;

    // Keep the selected agent visible: window the list around the selection.
    let start = if selected >= shown {
        selected + 1 - shown
    } else {
        0
    };

    // ---- First-row right tail: "M/N active" plus the controls hint. ----
    let tally = format!("{active}/{} active", members.len());
    let tally_w = disp_w(&tally);
    let hint_text = if focused { None } else { enter_hint };
    let hint_sep = " · ";
    let gap = 2usize;
    let hint_w = hint_text.map(|h| disp_w(h) + disp_w(hint_sep)).unwrap_or(0);

    const LEAD: &str = "🐝 ";
    const INDENT: &str = "   ";
    let lead_w = disp_w(LEAD);

    let mut out: Vec<Line<'static>> = Vec::new();
    // Where the selected agent's row landed in `out` (focused accordion).
    let mut selected_row_at: Option<usize> = None;
    for (row, m) in ordered.iter().enumerate().skip(start).take(shown) {
        let first = out.is_empty();
        let is_sel = row == selected;
        let color = role_color(m.role.as_deref()).unwrap_or_else(|| status_accent(&m.status));
        let glyph = status_glyph(&m.status, spinner_frame);
        let todo = m.todo.map(|(done, total)| format!("{done}/{total}"));

        let mut spans: Vec<Span<'static>> = Vec::new();
        if first {
            spans.push(Span::styled(
                LEAD.to_string(),
                Style::default().fg(rgb(255, 200, 100)),
            ));
        } else {
            spans.push(Span::raw(INDENT));
        }

        // In batch mode, prepend a checkbox for each agent.
        if let Some(selected_set) = batch_selected {
            let checked = selected_set.contains(&(start + row));
            let check = if checked { "[\u{2713}] " } else { "[ ] " };
            let check_color = if checked {
                rgb(100, 200, 100)
            } else {
                rgb(100, 100, 110)
            };
            spans.push(Span::styled(
                check.to_string(),
                Style::default().fg(check_color),
            ));
        }

        // Right tail only on the first row; degrade by dropping hint first.
        let (row_tail_w, show_hint) = if first {
            if lead_w + gap + tally_w + hint_w + 16 <= width && hint_w > 0 {
                (tally_w + hint_w, true)
            } else if lead_w + gap + tally_w + 12 <= width {
                (tally_w, false)
            } else {
                (0, false)
            }
        } else {
            (0, false)
        };

        let body_budget = width
            .saturating_sub(lead_w)
            .saturating_sub(if row_tail_w > 0 { row_tail_w + gap } else { 0 });

        // Expanded workers use the full-width information card header approved
        // for the inline swarm view. Compact rows retain the older chip layout.
        if focused && is_sel && !m.todo_items.is_empty() {
            let left = format!("{} ", m.label);
            spans.push(Span::styled(
                left.clone(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));

            let mut metadata = vec![format!(
                "{} {}",
                status_glyph(&m.status, spinner_frame),
                card_status_label(&m.status)
            )];
            if let Some((done, total)) = m.todo {
                metadata.push(format!("Todo {done}/{total}"));
            }
            if let Some(elapsed) = m.elapsed_secs {
                metadata.push(format_elapsed(elapsed));
            }
            if let Some(model) = m.model.as_deref().filter(|model| !model.trim().is_empty()) {
                metadata.push(format_model(model));
            }

            let mut tail = metadata.join(" · ");
            while metadata.len() > 1 && lead_w + disp_w(&left) + gap + disp_w(&tail) > width {
                metadata.pop();
                tail = metadata.join(" · ");
            }
            let consumed = lead_w + disp_w(&left);
            if consumed + gap + disp_w(&tail) <= width {
                spans.push(Span::raw(" ".repeat(width - consumed - disp_w(&tail))));
                spans.push(Span::styled(tail, Style::default().fg(rgb(150, 150, 160))));
            }
            if is_sel {
                selected_row_at = Some(out.len());
            }
            out.push(Line::from(spans));
            continue;
        }

        // <glyph> [icon ]<name>[ · task][ done/total]
        let row_active = is_active_status(&m.status);
        let mut style = if row_active {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(rgb(110, 110, 125))
        };
        let is_batch_row = batch_selected.is_some_and(|set| set.contains(&(start + row)));
        if is_batch_row {
            style = style.bg(rgb(20, 30, 40));
        }
        if is_sel {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        // The assigned animal icon already identifies the worker. Keep compact
        // strip rows emoji-only so names such as "sauropod" do not consume the
        // task-label space; the expanded chat card carries the full identity.
        let ident = match m.icon.as_deref().filter(|i| !i.is_empty()) {
            Some(icon) => icon.to_string(),
            None => m.label.clone(),
        };
        let marker = if focused {
            if is_sel { "▸ " } else { "  " }
        } else {
            ""
        };
        let head = format!("{marker}{glyph} {ident}");
        let head_w = disp_w(&head);
        let todo_w = todo.as_ref().map(|t| disp_w(t) + 1).unwrap_or(0);
        let mut used = head_w;
        spans.push(Span::styled(head, style));

        if let Some(task) = m.task.as_deref().filter(|t| !t.trim().is_empty()) {
            let avail = body_budget.saturating_sub(used + todo_w + disp_w(" · "));
            if avail >= CHIP_TASK_MIN_W {
                let label = truncate_label(task, avail);
                used += disp_w(" · ") + disp_w(&label);
                spans.push(Span::styled(
                    format!(" · {label}"),
                    Style::default().fg(rgb(150, 150, 160)),
                ));
            }
        }
        if let Some(todo) = &todo
            && used + todo_w <= body_budget
        {
            used += todo_w;
            spans.push(Span::styled(
                format!(" {todo}"),
                Style::default().fg(rgb(130, 130, 140)),
            ));
        }

        // ---- Right-align the first-row tail ----
        if row_tail_w > 0 {
            let consumed = lead_w + used;
            if consumed + gap + row_tail_w <= width {
                let pad = width - consumed - row_tail_w;
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(
                    tally.clone(),
                    Style::default().fg(if active > 0 {
                        rgb(255, 200, 100)
                    } else {
                        rgb(120, 120, 130)
                    }),
                ));
                if show_hint && let Some(hint) = hint_text {
                    spans.push(Span::styled(
                        hint_sep.to_string(),
                        Style::default().fg(rgb(80, 80, 90)),
                    ));
                    spans.push(Span::styled(
                        hint.to_string(),
                        Style::default().fg(rgb(110, 130, 170)),
                    ));
                }
            }
        }
        if is_sel {
            selected_row_at = Some(out.len());
        }
        out.push(Line::from(spans));
    }

    if hidden > 0 {
        out.push(Line::from(vec![
            Span::raw(INDENT),
            Span::styled(
                format!("+{hidden} more"),
                Style::default().fg(rgb(140, 140, 150)),
            ),
        ]));
    }

    // ---- Accordion detail under the selected row ----
    // Expanded details belong to the explicitly focused control surface. The
    // always-visible live card is rendered in chat beneath the spawning tool
    // call, so the persistent status strip stays compact.
    if focused {
        let hint_rows = usize::from(focused && !hints.is_empty());
        let detail_budget = max_height.saturating_sub(out.len() + hint_rows);
        if let (Some(m), Some(at)) = (ordered.get(selected), selected_row_at)
            && detail_budget >= 1
        {
            // Insert directly beneath the selected row so the list expands in
            // place (accordion) instead of jumping to a detached pane below.
            let detail = hovered_detail_body(m, Some(spinner_frame), width, detail_budget, true);
            for (i, line) in detail.into_iter().enumerate() {
                out.insert(at + 1 + i, line);
            }
        }
        if focused && !hints.is_empty() {
            let mut hint_spans: Vec<Span<'static>> = vec![Span::raw(INDENT)];
            for (i, h) in hints.iter().enumerate() {
                if i > 0 {
                    hint_spans.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))));
                }
                hint_spans.push(Span::styled(
                    h.key.clone(),
                    Style::default().fg(rgb(150, 170, 210)),
                ));
                hint_spans.push(Span::raw(" "));
                hint_spans.push(Span::styled(
                    h.label.clone(),
                    Style::default().fg(rgb(120, 120, 130)),
                ));
            }
            out.push(Line::from(hint_spans));
        }
    }

    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}
