//! Full conversation canvas with a bounded, positive-space resume control card.
use super::*;

pub(super) fn panel_area(area: Rect) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let width = ((area.width as u32 * 36 / 100) as u16)
        .clamp(30, 52)
        .min(area.width.saturating_sub(12).max(1));
    // Leave room for the shared product/session header, but never follow the
    // selected transcript's variable prompt and todo bands. This is a control
    // overlay: its position and size must stay put while browsing sessions.
    let min_top = 5.min(area.height.saturating_sub(4));
    let available = area.height.saturating_sub(min_top).saturating_sub(2).max(1);
    let height = ((area.height as u32 * 4 / 5) as u16).max(6).min(available);
    let top = ((area.height - height) / 2).max(min_top);
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
        self.poll_preview_load();
        self.ensure_selected_preview_loading();
        let panel = panel_area(area);
        self.reserved_panel_area = Some(panel);
        self.last_preview_area = Some(area);
        self.render_preview(frame, area);

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
        let (query, continuation): (Line, Vec<String>) =
            if self.search_query.is_empty() && !self.search_active {
                (
                    Line::from(vec![
                        Span::styled(" / ", Style::default().fg(rgb(186, 139, 255))),
                        Span::styled("Search sessions", Style::default().fg(rgb(130, 130, 145))),
                    ]),
                    Vec::new(),
                )
            } else {
                // Qualifier keys are advertised in this card: discoverability for
                // `model:`/`saved:`/etc. beats a blank tail. The card is bounded
                // (30-52 cols), so the key list wraps onto continuation rows below
                // the query instead of clipping; a long query shrinks the first
                // row's budget and the wrap absorbs the rest (Ctrl+U clears).
                let qualifier_keys = [
                    "filters:",
                    "model:",
                    "provider:",
                    "source:",
                    "dir:",
                    "saved:",
                    "debug:",
                    "canary:",
                    "after:",
                    "before:",
                ];
                let state_suffix = if self.search_active {
                    "  Esc to clear"
                } else {
                    "  / to edit"
                };
                let used_cells = 4
                    + self.search_query.chars().count()
                    + usize::from(self.search_active)
                    + state_suffix.chars().count();
                let first_budget = (search.width as usize).saturating_sub(used_cells);
                let mut hint_lines: Vec<String> = Vec::new();
                let mut current = String::new();
                let mut budget = first_budget;
                for key in qualifier_keys {
                    let piece_len = key.chars().count() + usize::from(!current.is_empty());
                    if piece_len <= budget {
                        if !current.is_empty() {
                            current.push(' ');
                        }
                        current.push_str(key);
                        budget -= piece_len;
                    } else {
                        if !current.is_empty() {
                            hint_lines.push(std::mem::take(&mut current));
                            budget = search.width as usize;
                        }
                        current = key.to_string();
                        budget = budget.saturating_sub(key.chars().count());
                    }
                }
                if !current.is_empty() {
                    hint_lines.push(current);
                }
                let search_row_hint = if first_budget > 0 {
                    hint_lines.first().cloned().unwrap_or_default()
                } else {
                    String::new()
                };
                let continuation: Vec<String> = if first_budget > 0 {
                    hint_lines.iter().skip(1).cloned().collect()
                } else {
                    hint_lines.clone()
                };
                let query_line = Line::from(vec![
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
                    Span::styled(state_suffix, Style::default().fg(rgb(60, 60, 60))),
                    Span::styled(search_row_hint, Style::default().fg(rgb(60, 60, 60))),
                ]);
                (query_line, continuation)
            };
        let query =
            jcode_tui_render::truncate_line_with_ellipsis_to_width(&query, search.width as usize);
        frame.render_widget(
            Paragraph::new(query).style(Style::default().bg(rgb(25, 25, 30))),
            search,
        );
        let mut hint_rows = 0u16;
        if panel.height > banner_rows + 1 {
            for (i, hint_line) in continuation.iter().enumerate() {
                let row = Rect {
                    y: search.y + 1 + i as u16,
                    height: 1,
                    ..search
                };
                if row.y >= panel.bottom() {
                    break;
                }
                let line = jcode_tui_render::truncate_line_with_ellipsis_to_width(
                    &Line::from(Span::styled(
                        hint_line.clone(),
                        Style::default().fg(rgb(60, 60, 60)),
                    )),
                    search.width as usize,
                );
                frame.render_widget(
                    Paragraph::new(line).style(Style::default().bg(rgb(25, 25, 30))),
                    row,
                );
                hint_rows += 1;
            }
        }
        let controls_height = banner_rows + search.height + hint_rows;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_card_uses_eighty_percent_height_without_a_row_cap() {
        for height in [40, 50, 80, 100, 160] {
            let area = Rect::new(3, 7, 120, height);
            let panel = panel_area(area);
            assert_eq!(panel.height, height * 4 / 5);
            assert_eq!(panel.y - area.y, ((height - panel.height) / 2).max(5));
            assert!(panel.x > area.x + area.width / 2);
        }
    }

    #[test]
    fn resume_card_stays_within_small_terminal_bounds() {
        for width in 0..65 {
            for height in 0..50 {
                let area = Rect::new(3, 7, width, height);
                let panel = panel_area(area);
                assert!(panel.x >= area.x && panel.right() <= area.right());
                assert!(panel.y >= area.y && panel.bottom() <= area.bottom());
                if width > 0 && height >= 9 {
                    assert!(panel.y >= area.y + 5);
                    assert!(panel.bottom() <= area.bottom() - 2);
                }
            }
        }
    }
}
