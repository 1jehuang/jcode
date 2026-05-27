use super::*;
use crate::tui::ui::input_ui;
use ratatui::layout::Rect;

#[derive(Clone, Debug, PartialEq, Eq)]
struct MouseScrollTraceState {
    chat_offset: usize,
    auto_scroll_paused: bool,
    mouse_queue: i16,
    mouse_target: Option<MouseScrollTarget>,
    diff_offset: usize,
    diff_auto_scroll: bool,
    diagram_focus: bool,
    diagram_x: i32,
    diagram_y: i32,
    diagram_zoom: u8,
    help_scroll: Option<usize>,
    changelog_scroll: Option<usize>,
}

impl MouseScrollTraceState {
    fn capture(app: &App) -> Self {
        Self {
            chat_offset: app.scroll_offset,
            auto_scroll_paused: app.auto_scroll_paused,
            mouse_queue: app.mouse_scroll_queue,
            mouse_target: app.mouse_scroll_target,
            diff_offset: app.diff_pane_scroll,
            diff_auto_scroll: app.diff_pane_auto_scroll,
            diagram_focus: app.diagram_focus,
            diagram_x: app.diagram_scroll_x,
            diagram_y: app.diagram_scroll_y,
            diagram_zoom: app.diagram_zoom,
            help_scroll: app.help_scroll,
            changelog_scroll: app.changelog_scroll,
        }
    }

    fn summary(&self) -> String {
        format!(
            "chat={} auto={} queue={} target={:?} diff={} diff_auto={} diagram_focus={} diagram=({},{} @ {}%) help={:?} changelog={:?}",
            self.chat_offset,
            self.auto_scroll_paused,
            self.mouse_queue,
            self.mouse_target,
            self.diff_offset,
            self.diff_auto_scroll,
            self.diagram_focus,
            self.diagram_x,
            self.diagram_y,
            self.diagram_zoom,
            self.help_scroll,
            self.changelog_scroll,
        )
    }
}

fn tui_mouse_scroll_trace_enabled() -> bool {
    std::env::var_os("JCODE_TUI_SCROLL_TRACE").is_some()
}

fn is_mouse_scroll_kind(kind: MouseEventKind) -> bool {
    matches!(
        kind,
        MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
    )
}

impl App {
    const MOUSE_SCROLL_INTENT_LINES: i16 = 3;
    const MOUSE_SCROLL_MAX_QUEUE: i16 = 24;

    fn log_mouse_scroll_trace(
        &self,
        mouse: MouseEvent,
        decision: &str,
        scroll_only: bool,
        before: Option<&MouseScrollTraceState>,
    ) {
        let after = MouseScrollTraceState::capture(self);
        let before_summary = before
            .map(MouseScrollTraceState::summary)
            .unwrap_or_else(|| "unrecorded".to_string());
        let changed = before.is_some_and(|before| before != &after);
        let layout = super::super::ui::last_layout_snapshot();
        let over_messages = layout.as_ref().is_some_and(|layout| {
            super::super::layout_utils::point_in_rect(mouse.column, mouse.row, layout.messages_area)
        });
        let over_diagram = layout
            .as_ref()
            .and_then(|layout| layout.diagram_area)
            .is_some_and(|area| {
                super::super::layout_utils::point_in_rect(mouse.column, mouse.row, area)
            });
        let over_diff = layout
            .as_ref()
            .and_then(|layout| layout.diff_pane_area)
            .is_some_and(|area| {
                super::super::layout_utils::point_in_rect(mouse.column, mouse.row, area)
            });

        crate::logging::event_info(
            "TUI_MOUSE_SCROLL",
            [
                ("kind", format!("{:?}", mouse.kind)),
                ("column", mouse.column.to_string()),
                ("row", mouse.row.to_string()),
                ("modifiers", format!("{:?}", mouse.modifiers)),
                ("decision", decision.to_string()),
                ("scroll_only", scroll_only.to_string()),
                ("changed", changed.to_string()),
                ("over_messages", over_messages.to_string()),
                ("over_diagram", over_diagram.to_string()),
                ("over_diff", over_diff.to_string()),
                (
                    "side_panel_visible",
                    self.side_panel.focused_page().is_some().to_string(),
                ),
                ("before", before_summary),
                ("after", after.summary()),
            ],
        );
    }

    fn current_visible_diagram_hash(&self) -> Option<u64> {
        if self.diagram_mode != crate::config::DiagramDisplayMode::Pinned
            || !self.diagram_pane_enabled
        {
            return None;
        }
        if self.side_panel.focused_page().is_some()
            && self.diagram_pane_position == crate::config::DiagramPanePosition::Side
        {
            return None;
        }
        let diagrams = crate::tui::mermaid::get_active_diagrams();
        diagrams
            .get(self.diagram_index.min(diagrams.len().saturating_sub(1)))
            .map(|diagram| diagram.hash)
    }

    pub(super) fn reset_diagram_view_to_fit(&mut self) {
        self.diagram_scroll_x = 0;
        self.diagram_scroll_y = 0;
        self.diagram_zoom = 100;
    }

    pub(super) fn sync_diagram_fit_context(&mut self) {
        let current_hash = self.current_visible_diagram_hash();
        if current_hash != self.last_visible_diagram_hash {
            self.reset_diagram_view_to_fit();
            self.last_visible_diagram_hash = current_hash;
        }
    }

    pub(super) fn handle_diagram_geometry_change(&mut self) {
        self.reset_diagram_view_to_fit();
        if self.side_panel.focused_page().is_some() {
            self.diff_pane_scroll_x = 0;
        }
        crate::tui::mermaid::clear_image_state();
        crate::tui::clear_side_panel_render_caches();
        self.last_visible_diagram_hash = self.current_visible_diagram_hash();
    }

    pub(super) fn try_open_link_at(&mut self, column: u16, row: u16) -> bool {
        self.try_open_link_at_with(column, row, |url| open::that_detached(url))
    }

    pub(super) fn try_open_link_at_with<F, E>(
        &mut self,
        column: u16,
        row: u16,
        mut open_url: F,
    ) -> bool
    where
        F: FnMut(&str) -> Result<(), E>,
        E: std::fmt::Display,
    {
        let Some(url) = super::super::ui::link_target_from_screen(column, row) else {
            return false;
        };

        match open_url(&url) {
            Ok(()) => self.set_status_notice(format!("Opened link: {}", url)),
            Err(e) => self.set_status_notice(format!("Failed to open link: {}", e)),
        }
        true
    }

    pub(super) fn scroll_max_estimate(&self) -> usize {
        let renderer_max = super::super::ui::last_max_scroll();
        let Some(layout) = super::super::ui::last_layout_snapshot() else {
            return renderer_max.max(
                self.display_messages
                    .len()
                    .saturating_mul(100)
                    .saturating_add(self.streaming_text.len()),
            );
        };

        // In the steady state the renderer has already computed the exact scroll
        // extent from the prepared/cached transcript. Avoid re-walking and
        // measuring every message on each scroll input, which is noticeable in
        // very long sessions. The estimate below is only needed while streaming
        // can make LAST_MAX_SCROLL stale between frames.
        if renderer_max > 0 && !self.is_processing && self.streaming_text.is_empty() {
            return renderer_max;
        }

        // While streaming, input can arrive after new text has been appended but before the next
        // full frame recomputes LAST_MAX_SCROLL.  Using only the stale rendered max makes the first
        // scroll-up convert from bottom-follow mode to an absolute offset that is too close to the
        // top, so the viewport appears to jump/shift as the transcript grows.  Keep the renderer's
        // exact value when available, but never let the estimate fall behind the current transcript.
        let width = layout.messages_area.width.max(1) as usize;
        let viewport = layout.messages_area.height as usize;
        let estimated_lines = self.estimated_chat_wrapped_lines(width);
        let estimated_max = estimated_lines.saturating_sub(viewport);
        renderer_max.max(estimated_max)
    }

    fn estimated_chat_wrapped_lines(&self, width: usize) -> usize {
        use unicode_width::UnicodeWidthStr;

        fn wrapped_text_lines(text: &str, width: usize) -> usize {
            if text.is_empty() {
                return 0;
            }
            text.lines()
                .map(|line| UnicodeWidthStr::width(line).max(1).div_ceil(width))
                .sum::<usize>()
                .max(1)
        }

        let message_lines = self
            .display_messages
            .iter()
            .map(|message| wrapped_text_lines(&message.content, width))
            .sum::<usize>();
        message_lines.saturating_add(wrapped_text_lines(&self.streaming_text, width))
    }

    pub(super) fn diagram_available(&self) -> bool {
        self.diagram_mode == crate::config::DiagramDisplayMode::Pinned
            && self.diagram_pane_enabled
            && !crate::tui::mermaid::get_active_diagrams().is_empty()
    }

    pub(super) fn normalize_diagram_state(&mut self) {
        if self.diagram_mode != crate::config::DiagramDisplayMode::Pinned {
            self.diagram_focus = false;
            self.diagram_index = 0;
            self.diagram_scroll_x = 0;
            self.diagram_scroll_y = 0;
            self.last_visible_diagram_hash = None;
            return;
        }
        if !self.diagram_pane_enabled {
            self.diagram_focus = false;
        }

        let diagram_count = crate::tui::mermaid::get_active_diagrams().len();
        if diagram_count == 0 {
            self.diagram_focus = false;
            self.diagram_index = 0;
            self.diagram_scroll_x = 0;
            self.diagram_scroll_y = 0;
            self.last_visible_diagram_hash = None;
            return;
        }

        if self.diagram_index >= diagram_count {
            self.diagram_index = 0;
            self.diagram_scroll_x = 0;
            self.diagram_scroll_y = 0;
        }

        self.last_visible_diagram_hash = self.current_visible_diagram_hash();
    }

    pub(super) fn set_diagram_focus(&mut self, focus: bool) {
        if self.diagram_focus == focus {
            return;
        }
        self.diagram_focus = focus;
        self.diff_pane_focus = false;
        if focus {
            self.set_status_notice("Focus: diagram (hjkl pan, [/] zoom, +/- resize)");
        } else {
            self.set_status_notice("Focus: chat");
        }
    }

    pub(super) fn diff_pane_visible(&self) -> bool {
        self.diff_mode.has_side_pane() || self.side_panel.focused_page().is_some()
    }

    pub(super) fn set_diff_pane_focus(&mut self, focus: bool) {
        if self.diff_pane_focus == focus {
            return;
        }
        self.diff_pane_focus = focus;
        self.diagram_focus = false;
        if focus {
            if self.side_panel.focused_page_id.as_deref()
                == Some(super::split_view::SPLIT_VIEW_PAGE_ID)
            {
                self.set_status_notice(
                    "Focus: split view (j/k scroll, Esc to return, Ctrl+H back to chat)",
                );
            } else if self.side_panel.focused_page().is_some() {
                self.set_status_notice(
                    "Focus: side pane (j/k scroll, h/l pan diagrams, Esc to return)",
                );
            } else {
                self.set_status_notice("Focus: side pane (j/k scroll, Esc to return)");
            }
        } else {
            self.set_status_notice("Focus: chat");
        }
    }

    pub(super) fn pan_diff_pane_x(&mut self, dx: i32) {
        self.diff_pane_scroll_x = self
            .diff_pane_scroll_x
            .saturating_add(dx)
            .clamp(-4096, 4096);
    }

    pub(super) fn adjust_side_panel_image_zoom(&mut self, delta_percent: i16) {
        let current = self.side_panel_image_zoom_percent as i16;
        let next = current.saturating_add(delta_percent).clamp(25, 250) as u8;
        if next == self.side_panel_image_zoom_percent {
            return;
        }
        self.side_panel_image_zoom_percent = next;
        self.diff_pane_scroll_x = 0;
        crate::tui::clear_side_panel_render_caches();
        crate::tui::mermaid::clear_image_state();
        self.set_status_notice(format!("Side image zoom: {}%", next));
    }

    pub(super) fn reset_side_panel_image_zoom(&mut self) {
        if self.side_panel_image_zoom_percent == 100 {
            return;
        }
        self.side_panel_image_zoom_percent = 100;
        self.diff_pane_scroll_x = 0;
        crate::tui::clear_side_panel_render_caches();
        crate::tui::mermaid::clear_image_state();
        self.set_status_notice("Side image zoom: fit".to_string());
    }

    pub(super) fn handle_diff_pane_focus_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        if !self.diff_pane_focus || modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }

        let line_amount = self.side_pane_line_scroll_amount();
        let page_amount = self.side_pane_page_scroll_amount();

        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.diff_pane_scroll = self.diff_pane_scroll.saturating_add(line_amount);
                self.diff_pane_auto_scroll = false;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.diff_pane_scroll = self.diff_pane_scroll.saturating_sub(line_amount);
                self.diff_pane_auto_scroll = false;
            }
            KeyCode::Char('d') | KeyCode::PageDown => {
                self.diff_pane_scroll = self.diff_pane_scroll.saturating_add(page_amount);
                self.diff_pane_auto_scroll = false;
            }
            KeyCode::Char('u') | KeyCode::PageUp => {
                self.diff_pane_scroll = self.diff_pane_scroll.saturating_sub(page_amount);
                self.diff_pane_auto_scroll = false;
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.diff_pane_scroll = 0;
                self.diff_pane_auto_scroll = false;
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.diff_pane_scroll = usize::MAX;
                self.diff_pane_auto_scroll = true;
            }
            KeyCode::Tab if self.side_panel.focused_page().is_some() => {
                self.focus_adjacent_side_panel_page(1);
            }
            KeyCode::BackTab if self.side_panel.focused_page().is_some() => {
                self.focus_adjacent_side_panel_page(-1);
            }
            KeyCode::Char('h') | KeyCode::Left if self.side_panel.focused_page().is_some() => {
                self.pan_diff_pane_x(-4);
            }
            KeyCode::Char('l') | KeyCode::Right if self.side_panel.focused_page().is_some() => {
                self.pan_diff_pane_x(4);
            }
            KeyCode::Char('+') | KeyCode::Char('=') if self.side_panel.focused_page().is_some() => {
                self.adjust_side_panel_image_zoom(10);
            }
            KeyCode::Char('-') if self.side_panel.focused_page().is_some() => {
                self.adjust_side_panel_image_zoom(-10);
            }
            KeyCode::Char('0') if self.side_panel.focused_page().is_some() => {
                self.reset_side_panel_image_zoom();
            }
            KeyCode::Esc => {
                self.set_diff_pane_focus(false);
            }
            _ => {}
        }

        true
    }

    fn focus_adjacent_side_panel_page(&mut self, delta: isize) {
        let page_count = self.side_panel.pages.len();
        if page_count < 2 {
            return;
        }

        let current_index = self
            .side_panel
            .focused_page_id
            .as_deref()
            .and_then(|focused_id| {
                self.side_panel
                    .pages
                    .iter()
                    .position(|page| page.id == focused_id)
            })
            .unwrap_or(0);
        let next_index = (current_index as isize + delta).rem_euclid(page_count as isize) as usize;

        let next_id = self.side_panel.pages[next_index].id.clone();
        self.side_panel.focused_page_id = Some(next_id.clone());
        self.last_side_panel_focus_id = Some(next_id);
        self.diff_pane_scroll = 0;
        self.diff_pane_auto_scroll = true;
        crate::tui::clear_side_panel_render_caches();
    }

    fn side_pane_has_visual_images(&self) -> bool {
        if self.side_panel_user_hidden {
            return false;
        }
        self.side_pane_has_visual_images_ignoring_user_hidden()
    }

    fn side_pane_has_visual_images_ignoring_user_hidden(&self) -> bool {
        if !self.pin_images || self.side_panel.focused_page().is_some() || self.diff_mode.is_file()
        {
            return false;
        }

        if self.is_remote {
            !self.remote_side_pane_images.is_empty()
        } else {
            crate::session::has_rendered_images(&self.session)
        }
    }

    fn side_pane_line_scroll_amount(&self) -> usize {
        if self.side_pane_has_visual_images() {
            1
        } else {
            3
        }
    }

    fn side_pane_page_scroll_amount(&self) -> usize {
        if self.side_pane_has_visual_images() {
            8
        } else {
            20
        }
    }

    pub(super) fn enqueue_mouse_scroll(&mut self, target: MouseScrollTarget, direction: i16) {
        if direction == 0 {
            return;
        }

        let trace_scroll = tui_mouse_scroll_trace_enabled();
        let before_queue = self.mouse_scroll_queue;
        let before_target = self.mouse_scroll_target;
        if self.mouse_scroll_target != Some(target) {
            self.mouse_scroll_target = Some(target);
            self.mouse_scroll_queue = 0;
        }

        self.last_mouse_scroll = Some(Instant::now());
        let delta = direction * Self::MOUSE_SCROLL_INTENT_LINES;
        self.mouse_scroll_queue = self
            .mouse_scroll_queue
            .saturating_add(delta)
            .clamp(-Self::MOUSE_SCROLL_MAX_QUEUE, Self::MOUSE_SCROLL_MAX_QUEUE);
        if trace_scroll {
            crate::logging::event_info(
                "TUI_MOUSE_SCROLL_QUEUE",
                [
                    ("target", format!("{:?}", target)),
                    ("direction", direction.to_string()),
                    ("delta", delta.to_string()),
                    ("before_queue", before_queue.to_string()),
                    ("before_target", format!("{:?}", before_target)),
                    ("after_queue", self.mouse_scroll_queue.to_string()),
                    ("after_target", format!("{:?}", self.mouse_scroll_target)),
                ],
            );
        }
        self.drain_mouse_scroll_animation(Self::MOUSE_SCROLL_INTENT_LINES as usize);
    }

    fn mouse_scroll_drain_amount(&self) -> usize {
        let queued = self.mouse_scroll_queue.unsigned_abs() as usize;

        if queued >= 6 {
            3
        } else if queued >= 3 {
            2
        } else {
            1
        }
    }

    fn drain_mouse_scroll_animation(&mut self, max_steps: usize) {
        let Some(target) = self.mouse_scroll_target else {
            self.mouse_scroll_queue = 0;
            return;
        };
        if self.mouse_scroll_queue == 0 || max_steps == 0 {
            if self.mouse_scroll_queue == 0 {
                self.mouse_scroll_target = None;
            }
            return;
        }

        let direction = self.mouse_scroll_queue.signum();
        let steps = max_steps.min(self.mouse_scroll_queue.unsigned_abs() as usize);
        let before_queue = self.mouse_scroll_queue;
        let before = tui_mouse_scroll_trace_enabled().then(|| MouseScrollTraceState::capture(self));

        for _ in 0..steps {
            if !self.apply_mouse_scroll_step(target, direction) {
                self.mouse_scroll_queue = 0;
                self.mouse_scroll_target = None;
                if let Some(before) = before.as_ref() {
                    crate::logging::event_info(
                        "TUI_MOUSE_SCROLL_DRAIN",
                        [
                            ("target", format!("{:?}", target)),
                            ("direction", direction.to_string()),
                            ("steps", steps.to_string()),
                            ("before_queue", before_queue.to_string()),
                            ("after_queue", self.mouse_scroll_queue.to_string()),
                            ("stopped_early", "true".to_string()),
                            ("before", before.summary()),
                            ("after", MouseScrollTraceState::capture(self).summary()),
                        ],
                    );
                }
                return;
            }
        }

        self.mouse_scroll_queue -= direction * steps as i16;
        if self.mouse_scroll_queue == 0 {
            self.mouse_scroll_target = None;
        }
        if let Some(before) = before.as_ref() {
            crate::logging::event_info(
                "TUI_MOUSE_SCROLL_DRAIN",
                [
                    ("target", format!("{:?}", target)),
                    ("direction", direction.to_string()),
                    ("steps", steps.to_string()),
                    ("before_queue", before_queue.to_string()),
                    ("after_queue", self.mouse_scroll_queue.to_string()),
                    ("stopped_early", "false".to_string()),
                    ("before", before.summary()),
                    ("after", MouseScrollTraceState::capture(self).summary()),
                ],
            );
        }
    }

    fn apply_mouse_scroll_step(&mut self, target: MouseScrollTarget, direction: i16) -> bool {
        match target {
            MouseScrollTarget::Chat => {
                if direction < 0 {
                    self.scroll_up(1);
                } else {
                    self.scroll_down(1);
                }
                true
            }
            MouseScrollTarget::SidePane => {
                let current = if self.diff_pane_scroll == usize::MAX {
                    super::super::ui::last_diff_pane_effective_scroll()
                } else {
                    self.diff_pane_scroll
                };
                self.diff_pane_scroll = if direction < 0 {
                    current.saturating_sub(1)
                } else {
                    current.saturating_add(1)
                };
                self.diff_pane_auto_scroll = false;
                true
            }
            MouseScrollTarget::HelpOverlay => {
                let Some(current) = self.help_scroll else {
                    return false;
                };
                self.help_scroll = Some(if direction < 0 {
                    current.saturating_sub(1)
                } else {
                    current.saturating_add(1)
                });
                true
            }
            MouseScrollTarget::ChangelogOverlay => {
                let Some(current) = self.changelog_scroll else {
                    return false;
                };
                self.changelog_scroll = Some(if direction < 0 {
                    current.saturating_sub(1)
                } else {
                    current.saturating_add(1)
                });
                true
            }
        }
    }

    pub(super) fn progress_mouse_scroll_animation(&mut self) {
        self.drain_mouse_scroll_animation(self.mouse_scroll_drain_amount());
    }

    pub(super) fn cycle_diagram(&mut self, direction: i32) {
        let diagrams = crate::tui::mermaid::get_active_diagrams();
        let count = diagrams.len();
        if count == 0 {
            return;
        }
        let current = self.diagram_index.min(count - 1);
        let next = if direction < 0 {
            if current == 0 { count - 1 } else { current - 1 }
        } else if current + 1 >= count {
            0
        } else {
            current + 1
        };
        self.diagram_index = next;
        self.reset_diagram_view_to_fit();
        self.last_visible_diagram_hash = diagrams.get(next).map(|diagram| diagram.hash);
        self.set_status_notice(format!("Diagram {}/{}", next + 1, count));
    }

    pub(super) fn pan_diagram(&mut self, dx: i32, dy: i32) {
        self.diagram_scroll_x = (self.diagram_scroll_x + dx).max(0);
        self.diagram_scroll_y = (self.diagram_scroll_y + dy).max(0);
    }

    pub(super) const DIAGRAM_PANE_ANIM_DURATION: f32 = 0.15;

    fn diagram_pane_ratio_limits(&self) -> (u8, u8) {
        match self.diagram_pane_position {
            crate::config::DiagramPanePosition::Side => (25, 100),
            crate::config::DiagramPanePosition::Top => (20, 100),
        }
    }

    fn set_diagram_pane_ratio(&mut self, next: i16, animate: bool, announce: bool) {
        let (min_ratio, max_ratio) = self.diagram_pane_ratio_limits();
        let next = next.clamp(min_ratio as i16, max_ratio as i16) as u8;
        let current_target = self.diagram_pane_ratio_target;
        if next == current_target {
            if !animate {
                self.diagram_pane_ratio = next;
                self.diagram_pane_ratio_from = next;
                self.diagram_pane_anim_start = None;
            }
            return;
        }

        if animate {
            self.diagram_pane_ratio_from = self.animated_diagram_pane_ratio();
            self.diagram_pane_ratio_target = next;
            self.diagram_pane_anim_start = Some(Instant::now());
        } else {
            self.diagram_pane_ratio = next;
            self.diagram_pane_ratio_from = next;
            self.diagram_pane_ratio_target = next;
            self.diagram_pane_anim_start = None;
        }

        self.handle_diagram_geometry_change();

        if announce {
            self.set_status_notice(format!("Diagram pane: {}%", next));
        }
    }

    pub(super) fn animated_diagram_pane_ratio(&self) -> u8 {
        let Some(start) = self.diagram_pane_anim_start else {
            return self.diagram_pane_ratio_target;
        };
        let elapsed = start.elapsed().as_secs_f32();
        let t = (elapsed / Self::DIAGRAM_PANE_ANIM_DURATION).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);
        let from = self.diagram_pane_ratio_from as f32;
        let to = self.diagram_pane_ratio_target as f32;
        (from + (to - from) * t).round() as u8
    }

    pub(super) fn adjust_diagram_pane_ratio(&mut self, delta: i8) {
        let next = self.diagram_pane_ratio_target as i16 + delta as i16;
        self.set_diagram_pane_ratio(next, true, true);
    }

    pub(super) fn set_diagram_pane_ratio_immediate(&mut self, next: u8) {
        self.set_diagram_pane_ratio(next as i16, false, false);
    }

    pub(super) fn set_side_panel_ratio_preset(&mut self, next: u8) {
        self.set_diagram_pane_ratio(next as i16, false, false);
        self.set_status_notice(format!("Side panel: {}%", self.diagram_pane_ratio_target));
    }

    pub(super) fn toggle_side_panel(&mut self) {
        if self.side_panel_user_hidden {
            self.side_panel_user_hidden = false;
            if self.side_panel.pages.is_empty() {
                if self.side_pane_has_visual_images_ignoring_user_hidden() {
                    self.sync_diagram_fit_context();
                    self.set_status_notice("Image side panel: ON");
                } else {
                    self.toggle_diagram_pane();
                }
                return;
            }
        }

        if self.side_pane_has_visual_images() {
            self.side_panel_user_hidden = true;
            self.set_diff_pane_focus(false);
            self.sync_diagram_fit_context();
            self.set_status_notice("Image side panel: OFF");
            return;
        }

        if self.side_panel.pages.is_empty() {
            self.toggle_diagram_pane();
            return;
        }

        if self.side_panel.focused_page().is_some() {
            self.last_side_panel_focus_id = self.side_panel.focused_page_id.clone();
            self.side_panel.focused_page_id = None;
            self.side_panel_user_hidden = true;
            if !self.diff_pane_visible() {
                self.set_diff_pane_focus(false);
            }
            self.sync_diagram_fit_context();
            self.set_status_notice("Side panel: OFF");
            return;
        }

        let restore_id = self
            .last_side_panel_focus_id
            .as_deref()
            .filter(|id| self.side_panel.pages.iter().any(|page| page.id == *id))
            .map(str::to_owned)
            .or_else(|| self.side_panel.pages.first().map(|page| page.id.clone()));

        let Some(restore_id) = restore_id else {
            self.toggle_diagram_pane();
            return;
        };

        self.side_panel.focused_page_id = Some(restore_id.clone());
        self.last_side_panel_focus_id = Some(restore_id);
        self.side_panel_user_hidden = false;
        self.sync_diagram_fit_context();
        let status = self
            .side_panel
            .focused_page()
            .map(|page| format!("Side panel: {}", page.title))
            .unwrap_or_else(|| "Side panel: ON".to_string());
        self.set_status_notice(status);
    }

    pub(super) fn adjust_diagram_zoom(&mut self, delta: i8) {
        let next = (self.diagram_zoom as i16 + delta as i16).clamp(50, 200) as u8;
        if next != self.diagram_zoom {
            self.diagram_zoom = next;
            self.set_status_notice(format!("Diagram zoom: {}%", next));
        }
    }

    pub(super) fn toggle_diagram_pane(&mut self) {
        if self.diagram_mode != crate::config::DiagramDisplayMode::Pinned {
            self.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
        }
        super::super::markdown::set_diagram_mode_override(Some(self.diagram_mode));
        self.diagram_pane_enabled = !self.diagram_pane_enabled;
        if !self.diagram_pane_enabled {
            self.diagram_focus = false;
        }
        let status = if self.diagram_pane_enabled {
            "Diagram pane: ON"
        } else {
            "Diagram pane: OFF"
        };
        self.set_status_notice(status);
    }

    pub(super) fn toggle_diagram_pane_position(&mut self) {
        use crate::config::DiagramPanePosition;
        self.diagram_pane_position = match self.diagram_pane_position {
            DiagramPanePosition::Side => DiagramPanePosition::Top,
            DiagramPanePosition::Top => DiagramPanePosition::Side,
        };
        let (min_ratio, max_ratio) = self.diagram_pane_ratio_limits();
        self.diagram_pane_ratio_target = self.diagram_pane_ratio_target.clamp(min_ratio, max_ratio);
        self.diagram_pane_ratio = self.diagram_pane_ratio_target;
        self.diagram_pane_ratio_from = self.diagram_pane_ratio_target;
        self.diagram_pane_anim_start = None;
        self.handle_diagram_geometry_change();
        let label = match self.diagram_pane_position {
            DiagramPanePosition::Side => "side",
            DiagramPanePosition::Top => "top",
        };
        self.set_status_notice(format!("Diagram pane: {}", label));
    }

    pub(super) fn pop_out_diagram(&mut self) {
        let diagrams = super::super::mermaid::get_active_diagrams();
        let total = diagrams.len();
        if total == 0 {
            self.set_status_notice("No diagrams to open");
            return;
        }
        let index = self.diagram_index.min(total - 1);
        let diagram = &diagrams[index];
        if let Some(path) = super::super::mermaid::get_cached_path(diagram.hash) {
            if path.exists() {
                match open::that_detached(&path) {
                    Ok(_) => self.set_status_notice(format!(
                        "Opened diagram {}/{} in viewer",
                        index + 1,
                        total
                    )),
                    Err(e) => self.set_status_notice(format!("Failed to open: {}", e)),
                }
            } else {
                self.set_status_notice("Diagram image not found on disk");
            }
        } else {
            self.set_status_notice("Diagram not cached");
        }
    }

    pub(super) fn handle_diagram_ctrl_key(
        &mut self,
        code: KeyCode,
        diagram_available: bool,
    ) -> bool {
        if diagram_available {
            match code {
                KeyCode::Left => {
                    if !self.diagram_focus {
                        return false;
                    }
                    self.cycle_diagram(-1);
                    return true;
                }
                KeyCode::Right => {
                    if !self.diagram_focus {
                        return false;
                    }
                    self.cycle_diagram(1);
                    return true;
                }
                KeyCode::Char('h') => {
                    if !self.diagram_focus {
                        return false;
                    }
                    self.set_diagram_focus(false);
                    return true;
                }
                KeyCode::Char('l') => {
                    self.set_diagram_focus(true);
                    return true;
                }
                _ => {}
            }
        }
        if self.diff_pane_visible() {
            match code {
                KeyCode::Char('l') => {
                    self.set_diff_pane_focus(true);
                    return true;
                }
                KeyCode::Char('h') => {
                    self.set_diff_pane_focus(false);
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    pub(super) fn ctrl_prompt_rank(code: &KeyCode, modifiers: KeyModifiers) -> Option<usize> {
        if !modifiers.contains(KeyModifiers::CONTROL)
            || modifiers.contains(KeyModifiers::ALT)
            || modifiers.contains(KeyModifiers::SHIFT)
        {
            return None;
        }
        match code {
            KeyCode::Char(c) if ('5'..='9').contains(c) => Some((*c as u8 - b'0') as usize),
            _ => None,
        }
    }

    pub(super) fn ctrl_side_panel_ratio_preset(
        code: &KeyCode,
        modifiers: KeyModifiers,
    ) -> Option<u8> {
        if !modifiers.contains(KeyModifiers::CONTROL)
            || modifiers.contains(KeyModifiers::ALT)
            || modifiers.contains(KeyModifiers::SHIFT)
        {
            return None;
        }
        match code {
            KeyCode::Char('1') => Some(25),
            KeyCode::Char('2') => Some(50),
            KeyCode::Char('3') => Some(75),
            KeyCode::Char('4') => Some(100),
            _ => None,
        }
    }

    pub(super) fn handle_diagram_focus_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        diagram_available: bool,
    ) -> bool {
        if !diagram_available || !self.diagram_focus || modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }

        match code {
            KeyCode::Char('h') | KeyCode::Left => self.pan_diagram(-4, 0),
            KeyCode::Char('l') | KeyCode::Right => self.pan_diagram(4, 0),
            KeyCode::Char('k') | KeyCode::Up => self.pan_diagram(0, -3),
            KeyCode::Char('j') | KeyCode::Down => self.pan_diagram(0, 3),
            KeyCode::Char('+') | KeyCode::Char('=') => self.adjust_diagram_pane_ratio(5),
            KeyCode::Char('-') | KeyCode::Char('_') => self.adjust_diagram_pane_ratio(-5),
            KeyCode::Char(']') => self.adjust_diagram_zoom(10),
            KeyCode::Char('[') => self.adjust_diagram_zoom(-10),
            KeyCode::Char('o') => self.pop_out_diagram(),
            KeyCode::Esc => {
                self.set_diagram_focus(false);
            }
            _ => {}
        }

        true
    }

    /// Returns true if this was a scroll-only event (safe to defer redraw during streaming)
    pub(super) fn handle_mouse_event(&mut self, mouse: MouseEvent) -> bool {
        let trace_scroll = tui_mouse_scroll_trace_enabled() && is_mouse_scroll_kind(mouse.kind);
        let trace_before = trace_scroll.then(|| MouseScrollTraceState::capture(self));
        macro_rules! finish_mouse_event {
            ($scroll_only:expr, $decision:expr) => {{
                let scroll_only = $scroll_only;
                if trace_scroll {
                    self.log_mouse_scroll_trace(
                        mouse,
                        $decision,
                        scroll_only,
                        trace_before.as_ref(),
                    );
                }
                return scroll_only;
            }};
        }

        if self.changelog_scroll.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.enqueue_mouse_scroll(MouseScrollTarget::ChangelogOverlay, -1);
                    finish_mouse_event!(true, "changelog_overlay_scroll_up");
                }
                MouseEventKind::ScrollDown => {
                    self.enqueue_mouse_scroll(MouseScrollTarget::ChangelogOverlay, 1);
                    finish_mouse_event!(true, "changelog_overlay_scroll_down");
                }
                _ => finish_mouse_event!(false, "changelog_overlay_non_scroll"),
            }
        }

        if self.help_scroll.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.enqueue_mouse_scroll(MouseScrollTarget::HelpOverlay, -1);
                    finish_mouse_event!(true, "help_overlay_scroll_up");
                }
                MouseEventKind::ScrollDown => {
                    self.enqueue_mouse_scroll(MouseScrollTarget::HelpOverlay, 1);
                    finish_mouse_event!(true, "help_overlay_scroll_down");
                }
                _ => finish_mouse_event!(false, "help_overlay_non_scroll"),
            }
        }

        if let Some(ref picker_cell) = self.session_picker_overlay {
            picker_cell.borrow_mut().handle_overlay_mouse(mouse);
            finish_mouse_event!(false, "session_picker_overlay");
        }
        if let Some(ref picker_cell) = self.login_picker_overlay {
            picker_cell.borrow_mut().handle_overlay_mouse(mouse);
            finish_mouse_event!(false, "login_picker_overlay");
        }
        if let Some(ref picker_cell) = self.account_picker_overlay {
            picker_cell.borrow_mut().handle_overlay_mouse(mouse);
            finish_mouse_event!(false, "account_picker_overlay");
        }
        self.normalize_diagram_state();
        let diagram_available = self.diagram_available();
        let layout = super::super::ui::last_layout_snapshot();
        let mut over_diagram = false;
        let mut over_diff_pane = false;
        let mut on_diagram_border = false;
        let mut input_area: Option<Rect> = None;
        let mut current_messages_area: Option<Rect> = None;
        let mut current_diagram_area: Option<Rect> = None;
        let mut terminal_width: u16 = 0;
        let mut terminal_height: u16 = 0;
        if let Some(layout) = layout {
            current_messages_area = Some(layout.messages_area);
            current_diagram_area = layout.diagram_area;
            input_area = layout.input_area;
            terminal_width =
                layout.messages_area.width + layout.diagram_area.map(|a| a.width).unwrap_or(0);
            terminal_height =
                layout.messages_area.height + layout.diagram_area.map(|a| a.height).unwrap_or(0);
            if let Some(diagram_area) = layout.diagram_area {
                over_diagram = super::super::layout_utils::point_in_rect(
                    mouse.column,
                    mouse.row,
                    diagram_area,
                );
                let is_side = matches!(
                    self.diagram_pane_position,
                    crate::config::DiagramPanePosition::Side
                );
                if is_side {
                    let border_x = diagram_area.x;
                    on_diagram_border = mouse.column >= border_x.saturating_sub(1)
                        && mouse.column <= border_x.saturating_add(1);
                } else {
                    let border_y = diagram_area.y.saturating_add(diagram_area.height);
                    on_diagram_border = mouse.row >= border_y.saturating_sub(1)
                        && mouse.row <= border_y.saturating_add(1);
                }
            }
            if let Some(diff_area) = layout.diff_pane_area {
                over_diff_pane =
                    super::super::layout_utils::point_in_rect(mouse.column, mouse.row, diff_area);
            }
            if diagram_available && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                if on_diagram_border {
                    self.diagram_pane_dragging = true;
                } else if over_diagram {
                    self.set_diagram_focus(true);
                } else {
                    self.set_diagram_focus(false);
                }
            }
        }

        let clicked_main_chat = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && !over_diff_pane
            && !over_diagram
            && !on_diagram_border;
        if clicked_main_chat {
            self.set_diff_pane_focus(false);
        }

        if let Some(scroll_only) = self.handle_copy_selection_mouse(mouse) {
            finish_mouse_event!(scroll_only, "copy_selection");
        }

        let clicked_input_cursor = if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            input_area.and_then(|area| {
                input_ui::input_cursor_pos_from_screen(
                    self,
                    area,
                    input_ui::next_input_prompt_number(self),
                    mouse.column,
                    mouse.row,
                )
            })
        } else {
            None
        };
        if let Some(cursor_pos) = clicked_input_cursor {
            self.cursor_pos = cursor_pos.min(self.input.len());
            self.reset_tab_completion();
            finish_mouse_event!(false, "input_cursor_click");
        }

        if self.diagram_pane_dragging {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    if diagram_available {
                        self.diagram_pane_anim_start = None;
                        let is_side = matches!(
                            self.diagram_pane_position,
                            crate::config::DiagramPanePosition::Side
                        );
                        let new_ratio = if is_side {
                            if let (Some(messages_area), Some(diagram_area)) =
                                (current_messages_area, current_diagram_area)
                            {
                                let right_edge = diagram_area.x.saturating_add(diagram_area.width);
                                let total_width = right_edge.saturating_sub(messages_area.x);
                                let desired_width = right_edge.saturating_sub(mouse.column);
                                if desired_width == diagram_area.width || total_width == 0 {
                                    self.diagram_pane_ratio_target
                                } else {
                                    ((desired_width as u32 * 100) / total_width as u32) as u8
                                }
                            } else if terminal_width > 0 {
                                ((terminal_width.saturating_sub(mouse.column)) as u32 * 100
                                    / terminal_width as u32) as u8
                            } else {
                                self.diagram_pane_ratio_target
                            }
                        } else if !is_side && terminal_height > 0 {
                            (mouse.row as u32 * 100 / terminal_height as u32) as u8
                        } else {
                            self.diagram_pane_ratio_target
                        };
                        self.set_diagram_pane_ratio_immediate(new_ratio);
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.diagram_pane_dragging = false;
                }
                _ => {}
            }
            finish_mouse_event!(false, "diagram_dragging");
        }

        let mut handled_scroll = false;
        let mut immediate_redraw = false;
        if diagram_available
            && over_diagram
            && matches!(
                mouse.kind,
                MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
                    | MouseEventKind::ScrollLeft
                    | MouseEventKind::ScrollRight
            )
        {
            if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.adjust_diagram_zoom(10),
                    MouseEventKind::ScrollDown => self.adjust_diagram_zoom(-10),
                    _ => {}
                }
                self.set_diagram_focus(true);
                handled_scroll = true;
            } else if self.diagram_focus {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.pan_diagram(0, -1),
                    MouseEventKind::ScrollDown => self.pan_diagram(0, 1),
                    MouseEventKind::ScrollLeft => self.pan_diagram(-1, 0),
                    MouseEventKind::ScrollRight => self.pan_diagram(1, 0),
                    _ => {}
                }
                handled_scroll = true;
            }
        }

        if !handled_scroll
            && over_diff_pane
            && self.diff_pane_visible()
            && matches!(
                mouse.kind,
                MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
                    | MouseEventKind::ScrollLeft
                    | MouseEventKind::ScrollRight
            )
        {
            // Keep hover-scroll focus behavior for the shared right pane so users can keep typing
            // in chat while inspecting pinned content. But when the side panel is visible, redraw
            // immediately so scroll/pan feels responsive instead of waiting for the next tick.
            let side_panel_visible = self.side_panel.focused_page().is_some();
            if side_panel_visible && mouse.modifiers.contains(KeyModifiers::CONTROL) {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.adjust_side_panel_image_zoom(10),
                    MouseEventKind::ScrollDown => self.adjust_side_panel_image_zoom(-10),
                    _ => {}
                }
            } else {
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        self.enqueue_mouse_scroll(MouseScrollTarget::SidePane, -1);
                    }
                    MouseEventKind::ScrollDown => {
                        self.enqueue_mouse_scroll(MouseScrollTarget::SidePane, 1);
                    }
                    MouseEventKind::ScrollLeft if self.side_panel.focused_page().is_some() => {
                        self.pan_diff_pane_x(-1);
                    }
                    MouseEventKind::ScrollRight if self.side_panel.focused_page().is_some() => {
                        self.pan_diff_pane_x(1);
                    }
                    _ => {}
                }
            }
            immediate_redraw = side_panel_visible;
            handled_scroll = true;
        }

        if handled_scroll {
            finish_mouse_event!(!immediate_redraw, "pane_or_focused_diagram_scroll");
        }

        if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left))
            && self.try_open_link_at(mouse.column, mouse.row)
        {
            finish_mouse_event!(false, "open_link");
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.enqueue_mouse_scroll(MouseScrollTarget::Chat, -1);
                finish_mouse_event!(false, "chat_scroll_up");
            }
            MouseEventKind::ScrollDown => {
                self.enqueue_mouse_scroll(MouseScrollTarget::Chat, 1);
                finish_mouse_event!(false, "chat_scroll_down");
            }
            _ => {
                finish_mouse_event!(false, "unhandled_non_scroll");
            }
        }
    }

    pub(super) fn scroll_up(&mut self, amount: usize) {
        let max = self.scroll_max_estimate();
        if !self.auto_scroll_paused {
            let rendered_max = super::super::ui::last_max_scroll();
            let current_abs = max.saturating_sub(self.scroll_offset);
            self.scroll_offset = current_abs.saturating_sub(amount);
            if rendered_max > 0 {
                self.scroll_offset = self.scroll_offset.min(rendered_max.saturating_sub(amount));
            }
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        }
        self.auto_scroll_paused = true;
        self.maybe_queue_compacted_history_load();
    }

    pub(super) fn pause_chat_auto_scroll(&mut self) {
        if self.auto_scroll_paused {
            return;
        }

        let max = self.scroll_max_estimate();

        self.scroll_offset = max.saturating_sub(self.scroll_offset.min(max));
        self.auto_scroll_paused = true;
    }

    pub(super) fn scroll_down(&mut self, amount: usize) {
        if !self.auto_scroll_paused {
            return;
        }
        let max = self.scroll_max_estimate();
        let rendered_max = super::super::ui::last_max_scroll();
        let bottom_threshold = if rendered_max > 0 {
            rendered_max.min(max)
        } else {
            max
        };
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
        if self.scroll_offset >= bottom_threshold {
            self.follow_chat_bottom();
        } else {
            self.scroll_offset = self.scroll_offset.min(max);
        }
    }

    pub(super) fn follow_chat_bottom(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll_paused = false;
    }

    pub(super) fn debug_scroll_up(&mut self, amount: usize) {
        self.scroll_up(amount);
    }

    pub(super) fn debug_scroll_down(&mut self, amount: usize) {
        self.scroll_down(amount);
    }

    pub(super) fn debug_scroll_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll_paused = true;
    }

    pub(super) fn debug_scroll_bottom(&mut self) {
        self.follow_chat_bottom();
    }
}
