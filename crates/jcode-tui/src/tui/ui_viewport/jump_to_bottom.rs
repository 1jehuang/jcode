use super::*;

/// Smallest chat viewport (rows) that still shows the "Jump to bottom" pill.
/// Below this the 3-row box would hide too much of the transcript, and the
/// `↓N` indicator is drawn instead.
const JUMP_TO_BOTTOM_MIN_VIEWPORT_ROWS: u16 = 10;

/// Screen rect and content line of the "Jump to bottom" pill, or `None` when
/// it doesn't fit. The pill is a 3-row rounded box on the last rows of the
/// chat viewport (directly above the input), centered in the chat column.
/// Optional parts (line count, then key hint) are dropped when narrow.
fn jump_to_bottom_pill_rect(
    area: Rect,
    top_band_lines: u16,
    key_label: Option<&str>,
    lines_below: usize,
) -> Option<(Rect, Line<'static>)> {
    const PILL_HEIGHT: u16 = 3;
    if area.height < JUMP_TO_BOTTOM_MIN_VIEWPORT_ROWS
        || area.height.saturating_sub(top_band_lines) < PILL_HEIGHT * 2
    {
        return None;
    }
    let key_span = key_label.map(|label| format!(" ({label})"));
    let count = format!(
        " · {lines_below} {} below",
        if lines_below == 1 { "line" } else { "lines" }
    );
    let build = |with_key: bool, with_count: bool| -> Line<'static> {
        let mut spans = vec![Span::styled(
            "Jump to bottom",
            Style::default().fg(accent_color()).bold(),
        )];
        if with_key && let Some(key) = &key_span {
            spans.push(Span::styled(key.clone(), Style::default().fg(dim_color())));
        }
        spans.push(Span::styled(
            " ↓",
            Style::default().fg(accent_color()).bold(),
        ));
        if with_count {
            spans.push(Span::styled(
                count.clone(),
                Style::default().fg(dim_color()),
            ));
        }
        Line::from(spans)
    };
    // Border (2) + one space of padding on each side (2).
    let chrome = 4u16;
    let line = [(true, true), (true, false), (false, false)]
        .into_iter()
        .map(|(key, count)| build(key, count))
        .find(|line| line.width().saturating_add(usize::from(chrome)) <= usize::from(area.width))?;
    let width = (line.width() as u16).saturating_add(chrome);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + area.height - PILL_HEIGHT,
        width,
        height: PILL_HEIGHT,
    };
    Some((rect, line))
}

static JUMP_TO_BOTTOM_AREA: std::sync::Mutex<Option<Rect>> = std::sync::Mutex::new(None);

/// Publish the on-screen rect of the "Jump to bottom" pill for the mouse
/// handler. `None` whenever the pill isn't drawn this frame.
pub(crate) fn set_jump_to_bottom_area(area: Option<Rect>) {
    if super::super::secondary_render_active() {
        return;
    }
    *JUMP_TO_BOTTOM_AREA
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = area;
}

pub(crate) fn jump_to_bottom_area() -> Option<Rect> {
    *JUMP_TO_BOTTOM_AREA
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub(super) fn draw(
    frame: &mut Frame,
    app: &dyn TuiState,
    area: Rect,
    top_band_lines: u16,
    scroll: usize,
    max_scroll: usize,
    margins: &mut info_widget::Margins,
) -> bool {
    let lines_below = max_scroll.saturating_sub(scroll);
    let jump_pill = if app.auto_scroll_paused() && lines_below > 0 {
        jump_to_bottom_pill_rect(
            area,
            top_band_lines,
            app.jump_to_bottom_key_label().as_deref(),
            lines_below,
        )
    } else {
        None
    };
    set_jump_to_bottom_area(jump_pill.as_ref().map(|(rect, _)| *rect));

    if let Some((pill_rect, pill_line)) = jump_pill {
        clear_area(frame, pill_rect);
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(accent_color()));
        frame.render_widget(
            Paragraph::new(pill_line)
                .alignment(Alignment::Center)
                .block(block),
            pill_rect,
        );
        // Keep info widgets from docking on top of the pill.
        let first = pill_rect.y.saturating_sub(area.y) as usize;
        for row in first..first.saturating_add(pill_rect.height as usize) {
            if let Some(width) = margins.right_widths.get_mut(row) {
                *width = 0;
            }
            if let Some(width) = margins.left_widths.get_mut(row) {
                *width = 0;
            }
        }
        true
    } else {
        false
    }
}

pub(super) fn resolve_tail_follow_scroll(max_scroll: usize, viewport_height: usize) -> usize {
    if super::take_tail_follow_snap_request() {
        super::set_tail_catchup_active(false);
        return max_scroll;
    }
    if !crate::perf::tui_policy().enable_decorative_animations {
        super::set_tail_catchup_active(false);
        return max_scroll;
    }
    let prev = super::last_resolved_chat_scroll();
    // Only animate forward catch-up from an established position. Backward
    // motion (content shrank) and first frames snap directly.
    if prev == 0 || max_scroll <= prev {
        super::set_tail_catchup_active(false);
        return max_scroll;
    }
    let jump = max_scroll - prev;
    // Never let the live tail drift more than a viewport behind (e.g. a huge
    // paste); cap the lag so the catch-up is at most one screen.
    let max_lag = viewport_height.max(TAIL_CATCHUP_MIN_JUMP);
    if jump <= TAIL_CATCHUP_MIN_JUMP {
        super::set_tail_catchup_active(false);
        return max_scroll;
    }
    let floor = max_scroll.saturating_sub(max_lag);
    let next = prev.max(floor).saturating_add(TAIL_CATCHUP_MAX_STEP);
    if next >= max_scroll {
        super::set_tail_catchup_active(false);
        max_scroll
    } else {
        super::set_tail_catchup_active(true);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pill_fits_cell_width_and_drops_optional_text_when_narrow() {
        for width in 0..90 {
            let area = Rect::new(7, 11, width, 12);
            if let Some((rect, line)) = jump_to_bottom_pill_rect(area, 2, Some("Alt+界"), 12) {
                assert_eq!(rect.intersection(area), rect);
                assert!(line.width() <= usize::from(rect.width.saturating_sub(2)));
            }
        }
        let (_, narrow) =
            jump_to_bottom_pill_rect(Rect::new(0, 0, 22, 12), 0, Some("Alt+界"), 12).unwrap();
        assert_eq!(narrow.to_string(), "Jump to bottom ↓");
        let (_, wide) =
            jump_to_bottom_pill_rect(Rect::new(0, 0, 70, 12), 0, Some("Alt+界"), 12).unwrap();
        assert!(wide.to_string().contains("Alt+界"));
        assert!(wide.to_string().contains("12 lines below"));
    }
}
