use super::*;

impl SessionPicker {
    /// Find next selectable item (skip headers)
    fn next_selectable(&self, from: usize) -> Option<usize> {
        ((from + 1)..self.items.len())
            .find(|&i| self.item_to_session.get(i).is_some_and(|x| x.is_some()))
    }

    /// Find previous selectable item (skip headers)
    fn prev_selectable(&self, from: usize) -> Option<usize> {
        (0..from)
            .rev()
            .find(|&i| self.item_to_session.get(i).is_some_and(|x| x.is_some()))
    }

    pub fn next(&mut self) {
        if self.visible_sessions.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        if let Some(next) = self.next_selectable(current) {
            self.list_state.select(Some(next));
            self.scroll_offset = 0;
            self.auto_scroll_preview = true;
        }
    }

    pub fn previous(&mut self) {
        if self.visible_sessions.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        if let Some(prev) = self.prev_selectable(current) {
            self.list_state.select(Some(prev));
            self.scroll_offset = 0;
            self.auto_scroll_preview = true;
        }
    }

    pub fn scroll_preview_down(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
    }

    pub fn scroll_preview_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    fn point_in_rect(col: u16, row: u16, rect: Rect) -> bool {
        col >= rect.x
            && col < rect.x.saturating_add(rect.width)
            && row >= rect.y
            && row < rect.y.saturating_add(rect.height)
    }

    fn mouse_scroll_amount(&mut self) -> u16 {
        let now = std::time::Instant::now();
        let amount = if let Some(last) = self.last_mouse_scroll {
            let gap = now.duration_since(last);
            if gap.as_millis() < 50 { 1 } else { 3 }
        } else {
            3
        };
        self.last_mouse_scroll = Some(now);
        amount
    }

    pub(super) fn handle_mouse_scroll(&mut self, col: u16, row: u16, kind: MouseEventKind) {
        let over_preview = self
            .last_preview_area
            .map(|r| Self::point_in_rect(col, row, r))
            .unwrap_or(false);
        let over_list = self
            .last_list_area
            .map(|r| Self::point_in_rect(col, row, r))
            .unwrap_or(false);

        if over_preview {
            let amt = self.mouse_scroll_amount();
            match kind {
                MouseEventKind::ScrollUp => self.scroll_preview_up(amt),
                MouseEventKind::ScrollDown => self.scroll_preview_down(amt),
                _ => {}
            }
            return;
        }

        if over_list {
            match kind {
                MouseEventKind::ScrollUp => self.previous(),
                MouseEventKind::ScrollDown => self.next(),
                _ => {}
            }
        }
    }

    fn focus_previous_step(&mut self) {
        match self.focus {
            PaneFocus::Sessions => self.previous(),
            PaneFocus::Preview => self.scroll_preview_up(PREVIEW_SCROLL_STEP),
        }
    }

    fn focus_next_step(&mut self) {
        match self.focus {
            PaneFocus::Sessions => self.next(),
            PaneFocus::Preview => self.scroll_preview_down(PREVIEW_SCROLL_STEP),
        }
    }

    fn focus_previous_page(&mut self) {
        match self.focus {
            PaneFocus::Sessions => {
                for _ in 0..SESSION_PAGE_STEP_COUNT {
                    self.previous();
                }
            }
            PaneFocus::Preview => self.scroll_preview_up(PREVIEW_PAGE_SCROLL),
        }
    }

    fn focus_next_page(&mut self) {
        match self.focus {
            PaneFocus::Sessions => {
                for _ in 0..SESSION_PAGE_STEP_COUNT {
                    self.next();
                }
            }
            PaneFocus::Preview => self.scroll_preview_down(PREVIEW_PAGE_SCROLL),
        }
    }

    pub(super) fn handle_focus_navigation_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        match code {
            KeyCode::Char('h') | KeyCode::Left => {
                self.focus = PaneFocus::Sessions;
                true
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.focus = PaneFocus::Preview;
                true
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    PaneFocus::Sessions => PaneFocus::Preview,
                    PaneFocus::Preview => PaneFocus::Sessions,
                };
                true
            }
            KeyCode::Down if modifiers.contains(KeyModifiers::SHIFT) => {
                self.focus_next_page();
                true
            }
            KeyCode::Up if modifiers.contains(KeyModifiers::SHIFT) => {
                self.focus_previous_page();
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.focus_next_step();
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.focus_previous_step();
                true
            }
            KeyCode::Char('J') | KeyCode::PageDown => {
                self.focus_next_page();
                true
            }
            KeyCode::Char('K') | KeyCode::PageUp => {
                self.focus_previous_page();
                true
            }
            _ => false,
        }
    }

    /// Handle mouse events when used as an overlay
    pub fn handle_overlay_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.handle_mouse_scroll(mouse.column, mouse.row, mouse.kind);
            }
            _ => {}
        }
    }
}
