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
        // Onboarding actions form a short list above the resumable sessions.
        if self.onboarding_review_recent_project_highlighted() {
            self.onboarding_action = Some(OnboardingAction::StartNewSession);
            return;
        }
        if self.onboarding_start_new_highlighted() {
            self.onboarding_action = None;
            if self.visible_sessions.is_empty() {
                // Nothing below the actions, so stay on the last action.
                self.onboarding_action = Some(OnboardingAction::StartNewSession);
            }
            return;
        }
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
        // Onboarding: already on the first action -> nothing above it.
        if self.onboarding_review_recent_project_highlighted() {
            return;
        }
        if self.onboarding_start_new_highlighted() {
            self.onboarding_action = Some(OnboardingAction::ReviewRecentProject);
            return;
        }
        if self.visible_sessions.is_empty() {
            // Onboarding picker with no transcripts: Up lands on the final
            // onboarding action.
            if self.onboarding_banner.is_some() {
                self.onboarding_action = Some(OnboardingAction::StartNewSession);
            }
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        if let Some(prev) = self.prev_selectable(current) {
            self.list_state.select(Some(prev));
            self.scroll_offset = 0;
            self.auto_scroll_preview = true;
        } else if self.onboarding_banner.is_some() {
            // At the top of the session list in onboarding mode -> move up to
            // the final onboarding action.
            self.onboarding_action = Some(OnboardingAction::StartNewSession);
        }
    }

    /// Scroll the preview down by `amount` lines. The offset is clamped to the
    /// real maximum by `render_preview` on the next frame; this returns whether
    /// the offset changed relative to the last rendered maximum so the shared
    /// mouse-scroll momentum can stop draining once the bottom is reached.
    pub fn scroll_preview_down(&mut self, amount: u16) -> bool {
        let before = self.scroll_offset;
        let target = before.saturating_add(amount);
        // Clamp to the last rendered maximum when we have one (mouse momentum and
        // post-first-render scrolling); otherwise advance freely and let the next
        // render clamp (keyboard paging before the first preview render).
        self.scroll_offset = if self.preview_max_scroll > 0 {
            target.min(self.preview_max_scroll)
        } else {
            target
        };
        self.scroll_offset != before
    }

    /// Scroll the preview up by `amount` lines. Returns whether the offset moved.
    pub fn scroll_preview_up(&mut self, amount: u16) -> bool {
        let before = self.scroll_offset;
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        self.scroll_offset != before
    }

    fn point_in_rect(col: u16, row: u16, rect: Rect) -> bool {
        col >= rect.x
            && col < rect.x.saturating_add(rect.width)
            && row >= rect.y
            && row < rect.y.saturating_add(rect.height)
    }

    /// Whether a screen coordinate falls inside the preview pane (used by the
    /// host App to route wheel events into the shared scroll-momentum queue).
    pub fn mouse_over_preview(&self, col: u16, row: u16) -> bool {
        self.last_preview_area
            .map(|r| Self::point_in_rect(col, row, r))
            .unwrap_or(false)
    }

    fn mouse_scroll_amount(&mut self) -> u16 {
        // One wheel notch advances the preview by the same number of lines as the
        // main chat viewport, so the standalone picker feels consistent with the
        // in-app overlay.
        crate::tui::WHEEL_LINES as u16
    }

    pub(super) fn handle_mouse_scroll(&mut self, col: u16, row: u16, kind: MouseEventKind) {
        let over_preview = self.mouse_over_preview(col, row);
        let over_list = self
            .last_list_area
            .map(|r| Self::point_in_rect(col, row, r))
            .unwrap_or(false);

        if over_preview {
            let amt = self.mouse_scroll_amount();
            match kind {
                MouseEventKind::ScrollUp => {
                    self.scroll_preview_up(amt);
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_preview_down(amt);
                }
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
            PaneFocus::Preview => {
                self.scroll_preview_up(PREVIEW_SCROLL_STEP);
            }
        }
    }

    fn focus_next_step(&mut self) {
        match self.focus {
            PaneFocus::Sessions => self.next(),
            PaneFocus::Preview => {
                self.scroll_preview_down(PREVIEW_SCROLL_STEP);
            }
        }
    }

    fn focus_previous_page(&mut self) {
        match self.focus {
            PaneFocus::Sessions => {
                for _ in 0..SESSION_PAGE_STEP_COUNT {
                    self.previous();
                }
            }
            PaneFocus::Preview => {
                self.scroll_preview_up(PREVIEW_PAGE_SCROLL);
            }
        }
    }

    fn focus_next_page(&mut self) {
        match self.focus {
            PaneFocus::Sessions => {
                for _ in 0..SESSION_PAGE_STEP_COUNT {
                    self.next();
                }
            }
            PaneFocus::Preview => {
                self.scroll_preview_down(PREVIEW_PAGE_SCROLL);
            }
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

    /// Apply a single line of preview scroll in `direction` (-1 up, +1 down) and
    /// report whether the offset actually moved. Used by the host App's shared
    /// mouse-scroll momentum so the picker preview scrolls with the same smooth
    /// easing as the main chat viewport.
    pub fn apply_preview_scroll_step(&mut self, direction: i16) -> bool {
        if direction < 0 {
            self.scroll_preview_up(1)
        } else if direction > 0 {
            self.scroll_preview_down(1)
        } else {
            false
        }
    }

    /// Whether the list pane (not the preview) is the wheel target for a screen
    /// coordinate. The host App steps the selection directly for the list since
    /// it is discrete, and routes preview wheels through scroll momentum.
    pub fn mouse_over_list(&self, col: u16, row: u16) -> bool {
        self.last_list_area
            .map(|r| Self::point_in_rect(col, row, r))
            .unwrap_or(false)
    }

    /// Step the session selection for a list-pane wheel event.
    pub fn step_list_selection(&mut self, direction: i16) {
        if direction < 0 {
            self.previous();
        } else if direction > 0 {
            self.next();
        }
    }

    /// Current preview scroll offset, exposed for tests that assert scroll
    /// movement through the host App's shared momentum.
    #[cfg(test)]
    pub fn preview_scroll_offset_for_test(&self) -> u16 {
        self.scroll_offset
    }
}
