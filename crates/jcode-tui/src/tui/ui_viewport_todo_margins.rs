//! Pinned todo bands and viewport margins.

use super::*;

/// Pinned todo band for an arbitrary todo payload. Shared by the live chat
/// viewport and the `/resume` preview so both render the band identically.
pub(crate) fn pinned_todo_band_lines_for_payload(
    payload: Option<&str>,
    expanded: bool,
    diff_mode: crate::config::DiffDisplayMode,
    width: u16,
    viewport_height: u16,
) -> (Vec<Line<'static>>, Option<usize>) {
    if width < 16 || viewport_height < 3 {
        return (Vec::new(), None);
    }

    let card_lines = if crate::config::config().display.pin_todos {
        payload.map_or_else(Vec::new, |payload| {
            // Distinct title keeps the line cache from sharing entries with
            // the width-capped inline transcript card.
            let mut msg = crate::tui::DisplayMessage::todos(payload.to_string());
            msg.title = Some("Pinned todos".to_string());
            super::super::messages::get_cached_message_lines(
                &msg,
                width,
                diff_mode,
                super::super::messages::render_pinned_todos_message,
            )
        })
    } else {
        Vec::new()
    };
    PINNED_TODO_LAST_MAX.store(0, std::sync::atomic::Ordering::Relaxed);
    if card_lines.is_empty() {
        return (Vec::new(), None);
    }
    let mut card_lines = card_lines;
    super::super::messages::animate_lane_spinners(&mut card_lines);

    // Band budget: about a third of the viewport.
    let budget = ((viewport_height as usize) / 3).clamp(2, 12);
    let content_budget = budget.max(2);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let has_more = card_lines.len() > content_budget && !expanded;
    let mut more_line = None;
    if has_more {
        let shown = content_budget.saturating_sub(1);
        let hidden = card_lines.len() - shown;
        lines.extend(card_lines.into_iter().take(shown));
        more_line = Some(lines.len());
        lines.push(Line::from(Span::styled(
            format!("  … +{} more (todo)", hidden),
            Style::default().fg(dim_color()),
        )));
    } else {
        if expanded {
            // Expanded bands get a larger budget but stay bounded so a long plan
            // cannot swallow the transcript. Overflow scrolls with the wheel.
            let expanded_budget = ((viewport_height as usize) * 2 / 3).max(content_budget);
            let window = expanded_budget.saturating_sub(1).max(1);
            let total = card_lines.len();
            let max_offset = total.saturating_sub(window);
            let offset = pinned_todo_scroll().min(max_offset);
            PINNED_TODO_LAST_MAX.store(max_offset, std::sync::atomic::Ordering::Relaxed);
            more_line = Some(lines.len());
            let label = if max_offset > 0 {
                format!(
                    "  ▴ Collapse todos ({}-{} of {}, scroll)",
                    offset + 1,
                    (offset + window).min(total),
                    total
                )
            } else {
                "  ▴ Collapse todos".to_string()
            };
            lines.push(Line::from(Span::styled(
                label,
                Style::default().fg(dim_color()),
            )));
            lines.extend(card_lines.into_iter().skip(offset).take(window));
            return (lines, more_line);
        }
        lines.extend(card_lines);
    }
    (lines, more_line)
}

pub(crate) fn compute_visible_margins(
    lines: &[Line],
    visible_user_indices: &[usize],
    area: Rect,
    centered: bool,
) -> info_widget::Margins {
    let visible_height = area.height as usize;
    let mut visible_user_cursor = 0usize;

    let mut right_widths = Vec::with_capacity(visible_height);
    let mut left_widths = Vec::with_capacity(visible_height);

    for row in 0..visible_height {
        while visible_user_cursor < visible_user_indices.len()
            && visible_user_indices[visible_user_cursor] < row
        {
            visible_user_cursor += 1;
        }
        let is_user_line = visible_user_cursor < visible_user_indices.len()
            && visible_user_indices[visible_user_cursor] == row;

        if row < lines.len() {
            let mut used = lines[row].width().min(area.width as usize) as u16;
            if is_user_line && area.width > 0 {
                used = used.saturating_add(1).min(area.width);
            }

            // Compute the true free space on each side from the line's *rendered*
            // alignment. This matters even in left-aligned mode: the header lines
            // (`server:`/`client:`/model/version, auth status, mcp list, changelog
            // box, etc.) are always centered regardless of mode, so a centered line
            // of width `used` leaves only ~half the slack on the right. Reporting
            // the full `area.width - used` here would let a right-side info widget
            // overlap the centered header text.
            let total_margin = area.width.saturating_sub(used);
            let default_alignment = if centered {
                Alignment::Center
            } else {
                Alignment::Left
            };
            let effective_alignment = lines[row].alignment.unwrap_or(default_alignment);
            let (left_margin, right_margin) = match effective_alignment {
                Alignment::Left => (0, total_margin),
                Alignment::Center => {
                    let left = total_margin / 2;
                    let right = total_margin.saturating_sub(left);
                    (left, right)
                }
                Alignment::Right => (total_margin, 0),
            };

            if centered {
                left_widths.push(left_margin);
                right_widths.push(right_margin);
            } else {
                // Left-aligned mode never places left-side widgets (content is
                // flush-left), so the left gap is reported as 0; the right gap
                // still respects per-line alignment so widgets clear the header.
                left_widths.push(0);
                right_widths.push(right_margin);
            }
        } else if centered {
            let half = area.width / 2;
            left_widths.push(half);
            right_widths.push(area.width.saturating_sub(half));
        } else {
            left_widths.push(0);
            right_widths.push(area.width);
        }
    }

    info_widget::Margins {
        right_widths,
        left_widths,
        centered,
        ..Default::default()
    }
}
