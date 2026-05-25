use super::App;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

impl App {
    const COPY_VIEWPORT_CONTEXT_LINES: usize = 4;

    pub(super) fn enter_copy_selection_mode(&mut self) {
        self.copy_selection_mode = true;
        self.copy_selection_dragging = false;
        self.copy_selection_pending_anchor = None;
        self.diff_pane_focus = false;
        self.diagram_focus = false;
    }

    pub(super) fn exit_copy_selection_mode(&mut self) {
        self.copy_selection_mode = false;
        self.copy_selection_dragging = false;
        self.copy_selection_pending_anchor = None;
        self.copy_selection_anchor = None;
        self.copy_selection_cursor = None;
        self.copy_selection_goal_column = None;
    }

    pub(super) fn toggle_copy_selection_mode(&mut self) {
        if self.copy_selection_mode {
            self.exit_copy_selection_mode();
        } else {
            self.enter_copy_selection_mode();
        }
    }

    pub(super) fn current_copy_selection_pane(&self) -> Option<crate::tui::CopySelectionPane> {
        self.copy_selection_cursor
            .or(self.copy_selection_anchor)
            .map(|point| point.pane)
    }

    pub(super) fn normalized_copy_selection(&self) -> Option<crate::tui::CopySelectionRange> {
        let anchor = self.copy_selection_anchor?;
        let cursor = self.copy_selection_cursor?;
        if anchor.pane != cursor.pane {
            return None;
        }
        if (anchor.abs_line, anchor.column) <= (cursor.abs_line, cursor.column) {
            Some(crate::tui::CopySelectionRange {
                start: anchor,
                end: cursor,
            })
        } else {
            Some(crate::tui::CopySelectionRange {
                start: cursor,
                end: anchor,
            })
        }
    }

    pub(super) fn current_copy_selection_text(&self) -> Option<String> {
        let range = self.normalized_copy_selection()?;
        crate::tui::ui::copy_selection_text(range)
    }

    fn line_text(pane: crate::tui::CopySelectionPane, abs_line: usize) -> Option<String> {
        match pane {
            crate::tui::CopySelectionPane::Chat => {
                crate::tui::ui::copy_viewport_line_text(abs_line)
            }
            crate::tui::CopySelectionPane::SidePane => {
                crate::tui::ui::side_pane_line_text(abs_line)
            }
        }
    }

    fn line_width(pane: crate::tui::CopySelectionPane, abs_line: usize) -> Option<usize> {
        Self::line_text(pane, abs_line).map(|text| UnicodeWidthStr::width(text.as_str()))
    }

    fn line_count(pane: crate::tui::CopySelectionPane) -> Option<usize> {
        match pane {
            crate::tui::CopySelectionPane::Chat => crate::tui::ui::copy_viewport_line_count(),
            crate::tui::CopySelectionPane::SidePane => crate::tui::ui::side_pane_line_count(),
        }
    }

    fn clamp_point(
        mut point: crate::tui::CopySelectionPoint,
    ) -> Option<crate::tui::CopySelectionPoint> {
        let line_count = Self::line_count(point.pane)?;
        if line_count == 0 {
            return None;
        }
        point.abs_line = point.abs_line.min(line_count.saturating_sub(1));
        point.column = point
            .column
            .min(Self::line_width(point.pane, point.abs_line).unwrap_or(0));
        Some(point)
    }

    fn preferred_copy_pane(&self) -> crate::tui::CopySelectionPane {
        self.current_copy_selection_pane()
            .or_else(|| {
                self.diff_pane_focus
                    .then_some(crate::tui::CopySelectionPane::SidePane)
            })
            .unwrap_or(crate::tui::CopySelectionPane::Chat)
    }

    fn first_visible_copy_point(
        pane: crate::tui::CopySelectionPane,
    ) -> Option<crate::tui::CopySelectionPoint> {
        crate::tui::ui::copy_pane_first_visible_point(pane)
    }

    fn default_copy_point(&self) -> Option<crate::tui::CopySelectionPoint> {
        self.copy_selection_cursor
            .or(self.copy_selection_anchor)
            .and_then(Self::clamp_point)
            .or_else(|| Self::first_visible_copy_point(self.preferred_copy_pane()))
            .or_else(|| Self::first_visible_copy_point(crate::tui::CopySelectionPane::Chat))
            .or_else(|| Self::first_visible_copy_point(crate::tui::CopySelectionPane::SidePane))
    }

    fn note_copy_selection_activity(&mut self, pane: crate::tui::CopySelectionPane) {
        match pane {
            crate::tui::CopySelectionPane::Chat => {
                self.pause_chat_auto_scroll();
            }
            crate::tui::CopySelectionPane::SidePane => {
                self.diff_pane_auto_scroll = false;
            }
        }
    }

    fn collapse_selection_to(&mut self, point: crate::tui::CopySelectionPoint) {
        self.note_copy_selection_activity(point.pane);
        self.copy_selection_anchor = Some(point);
        self.copy_selection_cursor = Some(point);
        self.copy_selection_goal_column = Some(point.column);
    }

    fn extend_selection_to(&mut self, point: crate::tui::CopySelectionPoint) {
        self.note_copy_selection_activity(point.pane);
        if self.copy_selection_anchor.is_none()
            || self
                .copy_selection_anchor
                .is_some_and(|anchor| anchor.pane != point.pane)
        {
            self.copy_selection_anchor = Some(point);
        }
        self.copy_selection_cursor = Some(point);
        self.copy_selection_goal_column = Some(point.column);
    }

    fn update_selection_with_point(&mut self, point: crate::tui::CopySelectionPoint, extend: bool) {
        let Some(point) = Self::clamp_point(point) else {
            return;
        };
        if extend {
            self.extend_selection_to(point);
        } else {
            self.collapse_selection_to(point);
        }
    }

    fn display_col_to_prev_boundary(text: &str, current: usize) -> usize {
        let mut width = 0usize;
        let mut prev = 0usize;
        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width >= current {
                break;
            }
            prev = width;
            width = width.saturating_add(ch_width);
        }
        prev
    }

    fn display_col_to_next_boundary(text: &str, current: usize) -> usize {
        let mut width = 0usize;
        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            let next = width.saturating_add(ch_width);
            if width >= current {
                return next;
            }
            if next > current {
                return next;
            }
            width = next;
        }
        UnicodeWidthStr::width(text)
    }

    fn move_copy_selection_horizontally(&mut self, direction: i32, extend: bool) -> bool {
        let Some(mut point) = self.default_copy_point() else {
            return false;
        };
        let Some(text) = Self::line_text(point.pane, point.abs_line) else {
            return false;
        };
        point.column = if direction < 0 {
            Self::display_col_to_prev_boundary(&text, point.column)
        } else {
            Self::display_col_to_next_boundary(&text, point.column)
        };
        self.update_selection_with_point(point, extend);
        true
    }

    fn move_copy_selection_vertically(&mut self, delta: i32, extend: bool) -> bool {
        let Some(mut point) = self.default_copy_point() else {
            return false;
        };
        let Some(line_count) = Self::line_count(point.pane) else {
            return false;
        };
        if line_count == 0 {
            return false;
        }
        let goal = self.copy_selection_goal_column.unwrap_or(point.column);
        let next_line =
            (point.abs_line as i32 + delta).clamp(0, line_count.saturating_sub(1) as i32) as usize;
        point.abs_line = next_line;
        point.column = goal.min(Self::line_width(point.pane, next_line).unwrap_or(0));
        self.update_selection_with_point(point, extend);
        true
    }

    fn move_copy_selection_to_line_edge(&mut self, end: bool, extend: bool) -> bool {
        let Some(mut point) = self.default_copy_point() else {
            return false;
        };
        point.column = if end {
            Self::line_width(point.pane, point.abs_line).unwrap_or(0)
        } else {
            0
        };
        self.update_selection_with_point(point, extend);
        true
    }

    fn move_copy_selection_to_document_edge(&mut self, end: bool, extend: bool) -> bool {
        let pane = self
            .default_copy_point()
            .map(|point| point.pane)
            .unwrap_or_else(|| self.preferred_copy_pane());
        let Some(line_count) = Self::line_count(pane) else {
            return false;
        };
        if line_count == 0 {
            return false;
        }
        let abs_line = if end { line_count - 1 } else { 0 };
        let point = crate::tui::CopySelectionPoint {
            pane,
            abs_line,
            column: if end {
                Self::line_width(pane, abs_line).unwrap_or(0)
            } else {
                0
            },
        };
        self.update_selection_with_point(point, extend);
        true
    }

    pub(super) fn select_all_in_copy_mode(&mut self) -> bool {
        let pane = self
            .default_copy_point()
            .map(|point| point.pane)
            .unwrap_or_else(|| self.preferred_copy_pane());
        let Some(line_count) = Self::line_count(pane) else {
            return false;
        };
        if line_count == 0 {
            return false;
        }
        self.copy_selection_anchor = Some(crate::tui::CopySelectionPoint {
            pane,
            abs_line: 0,
            column: 0,
        });
        let last_line = line_count - 1;
        let end_point = crate::tui::CopySelectionPoint {
            pane,
            abs_line: last_line,
            column: Self::line_width(pane, last_line).unwrap_or(0),
        };
        self.copy_selection_cursor = Some(end_point);
        self.copy_selection_goal_column = Some(end_point.column);
        self.note_copy_selection_activity(pane);
        true
    }

    pub(super) fn select_chat_viewport_context(&mut self) -> bool {
        let (visible_start, visible_end) = match crate::tui::ui::copy_viewport_visible_range() {
            Some(range) => range,
            None => return false,
        };
        let Some(line_count) = crate::tui::ui::copy_viewport_line_count() else {
            return false;
        };
        if line_count == 0 || visible_start >= visible_end {
            return false;
        }

        let context = Self::COPY_VIEWPORT_CONTEXT_LINES;
        let start_line = visible_start.saturating_sub(context);
        let end_line = visible_end
            .saturating_add(context)
            .saturating_sub(1)
            .min(line_count.saturating_sub(1));

        self.copy_selection_anchor = Some(crate::tui::CopySelectionPoint {
            pane: crate::tui::CopySelectionPane::Chat,
            abs_line: start_line,
            column: 0,
        });
        let end_point = crate::tui::CopySelectionPoint {
            pane: crate::tui::CopySelectionPane::Chat,
            abs_line: end_line,
            column: crate::tui::ui::copy_viewport_line_text(end_line)
                .map(|text| UnicodeWidthStr::width(text.as_str()))
                .unwrap_or(0),
        };
        self.copy_selection_cursor = Some(end_point);
        self.copy_selection_goal_column = Some(end_point.column);
        self.note_copy_selection_activity(crate::tui::CopySelectionPane::Chat);
        true
    }

    pub(super) fn copy_chat_viewport_context_to_clipboard(&mut self) -> bool {
        self.copy_chat_viewport_context_to_clipboard_with(super::helpers::copy_to_clipboard)
    }

    pub(super) fn copy_chat_viewport_context_to_clipboard_with<F>(&mut self, copy_text: F) -> bool
    where
        F: FnOnce(&str) -> bool,
    {
        if !self.select_chat_viewport_context() {
            self.set_status_notice("Nothing visible to copy");
            self.exit_copy_selection_mode();
            return false;
        }

        let text = self.current_copy_selection_text().unwrap_or_default();
        if text.is_empty() {
            self.set_status_notice("Nothing visible to copy");
            self.exit_copy_selection_mode();
            return false;
        }

        let success = copy_text(&text);
        if success {
            self.set_status_notice("Copied viewport context");
        } else {
            self.set_status_notice("Failed to copy viewport context");
        }
        self.exit_copy_selection_mode();
        success
    }

    pub(super) fn copy_current_selection_to_clipboard(&mut self) -> bool {
        self.copy_current_selection_to_clipboard_with(super::helpers::copy_to_clipboard)
    }

    pub(super) fn copy_current_selection_to_clipboard_with<F>(&mut self, copy_text: F) -> bool
    where
        F: FnOnce(&str) -> bool,
    {
        let text = self.current_copy_selection_text().unwrap_or_default();
        if text.is_empty() {
            self.set_status_notice("Selection is empty");
            return false;
        }
        let success = copy_text(&text);
        if success {
            self.set_status_notice("Copied selection");
            self.exit_copy_selection_mode();
        } else {
            self.set_status_notice("Failed to copy selection");
        }
        success
    }

    pub(super) fn handle_copy_selection_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        let extend = modifiers.contains(KeyModifiers::SHIFT);
        match code {
            KeyCode::Esc => {
                self.exit_copy_selection_mode();
                true
            }
            KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.copy_chat_viewport_context_to_clipboard();
                true
            }
            KeyCode::Enter => {
                self.copy_current_selection_to_clipboard();
                true
            }
            KeyCode::Char('y') | KeyCode::Char('c') => {
                self.copy_current_selection_to_clipboard();
                true
            }
            KeyCode::Char('a') if !modifiers.contains(KeyModifiers::CONTROL) => {
                self.select_all_in_copy_mode();
                true
            }
            KeyCode::Left | KeyCode::Char('h') => self.move_copy_selection_horizontally(-1, extend),
            KeyCode::Right | KeyCode::Char('l') => self.move_copy_selection_horizontally(1, extend),
            KeyCode::Up | KeyCode::Char('k') => self.move_copy_selection_vertically(-1, extend),
            KeyCode::Down | KeyCode::Char('j') => self.move_copy_selection_vertically(1, extend),
            KeyCode::PageUp => self.move_copy_selection_vertically(-10, extend),
            KeyCode::PageDown => self.move_copy_selection_vertically(10, extend),
            KeyCode::Home => self.move_copy_selection_to_line_edge(false, extend),
            KeyCode::End => self.move_copy_selection_to_line_edge(true, extend),
            KeyCode::Char('g') => self.move_copy_selection_to_document_edge(false, extend),
            KeyCode::Char('G') => self.move_copy_selection_to_document_edge(true, extend),
            _ => false,
        }
    }

    fn scroll_copy_selection_pane(
        &mut self,
        pane: crate::tui::CopySelectionPane,
        upward: bool,
    ) -> bool {
        match pane {
            crate::tui::CopySelectionPane::Chat => {
                self.enqueue_mouse_scroll(
                    super::MouseScrollTarget::Chat,
                    if upward { -1 } else { 1 },
                );
            }
            crate::tui::CopySelectionPane::SidePane => {
                self.enqueue_mouse_scroll(
                    super::MouseScrollTarget::SidePane,
                    if upward { -1 } else { 1 },
                );
            }
        }
        true
    }

    pub(super) fn handle_copy_selection_mouse(&mut self, mouse: MouseEvent) -> Option<bool> {
        self.handle_copy_selection_mouse_with(mouse, super::helpers::copy_to_clipboard)
    }

    pub(super) fn handle_copy_selection_mouse_with<F>(
        &mut self,
        mouse: MouseEvent,
        copy_text: F,
    ) -> Option<bool>
    where
        F: FnOnce(&str) -> bool,
    {
        let point = crate::tui::ui::copy_point_from_screen(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let point = point?;
                if self.copy_selection_mode {
                    self.copy_selection_dragging = true;
                    self.copy_selection_pending_anchor = None;
                    self.update_selection_with_point(point, false);
                } else {
                    self.copy_selection_pending_anchor = Some(point);
                    self.copy_selection_dragging = false;
                    self.copy_selection_anchor = None;
                    self.copy_selection_cursor = None;
                    self.copy_selection_goal_column = None;
                }
                Some(false)
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if !self.copy_selection_dragging {
                    let pending = self.copy_selection_pending_anchor?;
                    let point = point.filter(|point| point.pane == pending.pane)?;
                    self.copy_selection_pending_anchor = None;
                    self.copy_selection_dragging = true;
                    self.collapse_selection_to(pending);
                    self.update_selection_with_point(point, true);
                    return Some(false);
                }
                if let Some(point) =
                    point.filter(|point| Some(point.pane) == self.current_copy_selection_pane())
                {
                    self.update_selection_with_point(point, true);
                }
                Some(false)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let had_pending = self.copy_selection_pending_anchor.take().is_some();
                if !self.copy_selection_dragging {
                    return if self.copy_selection_mode || had_pending {
                        Some(false)
                    } else {
                        None
                    };
                }
                self.copy_selection_dragging = false;
                if let Some(point) =
                    point.filter(|point| Some(point.pane) == self.current_copy_selection_pane())
                {
                    self.update_selection_with_point(point, true);
                }
                if self.copy_selection_mode {
                    return Some(false);
                }
                if !self.copy_current_selection_to_clipboard_with(copy_text) {
                    self.exit_copy_selection_mode();
                }
                Some(false)
            }
            MouseEventKind::ScrollUp => {
                if !(self.copy_selection_mode
                    || self.copy_selection_dragging
                    || self.copy_selection_pending_anchor.is_some())
                {
                    return None;
                }
                point
                    .map(|point| self.scroll_copy_selection_pane(point.pane, true))
                    .or_else(|| {
                        self.copy_selection_dragging
                            .then(|| self.current_copy_selection_pane())
                            .flatten()
                            .map(|pane| self.scroll_copy_selection_pane(pane, true))
                    })
            }
            MouseEventKind::ScrollDown => {
                if !(self.copy_selection_mode
                    || self.copy_selection_dragging
                    || self.copy_selection_pending_anchor.is_some())
                {
                    return None;
                }
                point
                    .map(|point| self.scroll_copy_selection_pane(point.pane, false))
                    .or_else(|| {
                        self.copy_selection_dragging
                            .then(|| self.current_copy_selection_pane())
                            .flatten()
                            .map(|pane| self.scroll_copy_selection_pane(pane, false))
                    })
            }
            _ => None,
        }
    }
}
