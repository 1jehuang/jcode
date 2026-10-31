//! Full conversation canvas with a bounded, positive-space resume control card.
use super::*;

pub(super) fn panel_area(area: Rect, header_rows: u16) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let width = ((area.width as u32 * 36 / 100) as u16)
        .clamp(30, 52)
        .min(area.width.saturating_sub(12).max(1));
    let top = header_rows
        .saturating_add(1)
        .min(area.height.saturating_sub(4));
    let available = area.height.saturating_sub(top).saturating_sub(2).max(1);
    let height = ((area.height as u32 * 3 / 5) as u16)
        .clamp(6, 24)
        .min(available);
    Rect::new(
        area.right()
            .saturating_sub(width + area.width.saturating_sub(width).min(2)),
        area.y + top,
        width,
        height,
    )
}

impl SessionPicker {
    pub(super) fn render_resume_canvas(&mut self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(Clear, area);
        // Measure after polling so a completed load moves the card this frame.
        let _ = self.poll_preview_load();
        self.ensure_selected_preview_loading();
        let header_rows = self.selected_session().map_or(3, |session| {
            super::super::ui::header::build_preview_header(session, area.width).len() as u16
        });
        let panel = panel_area(area, header_rows);
        self.reserved_panel_area = Some(panel);
        self.last_preview_area = Some(area);
        self.render_preview(frame, area);
        let panel = self.reserved_panel_area.unwrap_or(panel);

        // Search and session selection share one bounded overlay card. Painting
        // it last leaves the underlying session's wrapping and scroll unchanged.
        frame.render_widget(Clear, panel);
        let banner_rows = u16::from(self.crashed_sessions.is_some() && panel.height > 5);
        if banner_rows > 0 {
            self.render_crash_banner(frame, Rect { height: 1, ..panel });
        }
        let search = Rect {
            y: panel.y + banner_rows,
            height: u16::from(panel.height > banner_rows),
            ..panel
        };
        let query = if self.search_query.is_empty() && !self.search_active {
            Line::from(vec![
                Span::styled(" / ", Style::default().fg(rgb(186, 139, 255))),
                Span::styled("Search sessions", Style::default().fg(rgb(130, 130, 145))),
            ])
        } else {
            Line::from(vec![
                Span::styled(" 🔍 ", Style::default().fg(rgb(186, 139, 255))),
                Span::styled(
                    self.search_query.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if self.search_active { "▎" } else { "" },
                    Style::default().fg(rgb(186, 139, 255)),
                ),
            ])
        };
        let query =
            jcode_tui_render::truncate_line_with_ellipsis_to_width(&query, search.width as usize);
        frame.render_widget(
            Paragraph::new(query).style(Style::default().bg(rgb(25, 25, 30))),
            search,
        );
        let controls_height = banner_rows + search.height;
        let list = Rect {
            y: panel.y + controls_height,
            height: panel.height.saturating_sub(controls_height),
            ..panel
        };
        self.last_list_area = Some(list);
        self.render_session_list(frame, list);
        self.render_claude_takeover_confirmation(frame);
    }
}
