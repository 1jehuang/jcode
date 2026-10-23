//! Live interaction geometry must not be replaced by offscreen diagnostics.
use super::*;

fn badge_rect() -> Option<Rect> {
    #[cfg(test)]
    {
        TEST_VISIBLE_EXPAND_EDIT_BADGE_RECT.with(Cell::get)
    }
    #[cfg(not(test))]
    {
        *visible_expand_edit_badge_rect_state()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

pub(super) fn transcript_overflows(
    prepared: &PreparedChatFrame,
    fixed: u16,
    available: u16,
) -> bool {
    // Document coordinates must not narrow to terminal coordinates before the
    // comparison: that wraps at 65K lines, or panics with overflow checks on.
    let started = Instant::now();
    let result =
        prepared.total_wrapped_lines().max(1) > usize::from(available.saturating_sub(fixed));
    note_prep_overflow(started.elapsed());
    result
}

#[cfg(test)]
pub(crate) fn chat_scrollbar_visible_for_tests() -> bool {
    last_chat_scrollbar_visible()
}

macro_rules! read_snapshot {
    ($test:ident, $state:ident) => {{
        #[cfg(test)]
        {
            $test.with(|value| value.borrow().clone())
        }
        #[cfg(not(test))]
        {
            $state().lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }};
}
macro_rules! restore_snapshot {
    ($test:ident, $state:ident, $value:expr) => {{
        #[cfg(test)]
        $test.with(|value| *value.borrow_mut() = $value);
        #[cfg(not(test))]
        {
            *$state().lock().unwrap_or_else(|e| e.into_inner()) = $value;
        }
    }};
}

/// Captured without holding locks across drawing. Drop also restores geometry
/// when a diagnostic exits early, so queued mouse events still hit the live UI.
pub(crate) struct InteractiveRenderState {
    copies: CopyViewportSnapshots,
    layout: Option<LayoutSnapshot>,
    frame: Option<Arc<PreparedChatFrame>>,
    targets: Vec<VisibleCopyTarget>,
    status: Option<Rect>,
    max_scroll: usize,
    total_lines: usize,
    viewport_height: usize,
    resolved_scroll: usize,
    pane_lines: usize,
    pane_scroll: usize,
    pane_max_scroll: usize,
    scrollbar: bool,
    idle_area: Option<Rect>,
    badge: (bool, Option<usize>, Option<Rect>),
    tail: (bool, bool),
    todo_areas: (Option<Rect>, Option<Rect>),
    jump_area: Option<Rect>,
    images: Vec<(Rect, u64)>,
}

pub(crate) fn preserve_interactive_render_state() -> InteractiveRenderState {
    InteractiveRenderState {
        copies: read_snapshot!(TEST_COPY_VIEWPORT, copy_viewport_state),
        layout: last_layout_snapshot(),
        frame: last_chat_frame(),
        targets: read_snapshot!(TEST_VISIBLE_COPY_TARGETS, visible_copy_targets_state),
        status: last_status_area(),
        max_scroll: last_max_scroll(),
        total_lines: last_total_wrapped_lines(),
        viewport_height: last_chat_viewport_height(),
        resolved_scroll: last_resolved_chat_scroll(),
        pane_lines: pinned_pane_total_lines(),
        pane_scroll: last_diff_pane_effective_scroll(),
        pane_max_scroll: last_diff_pane_max_scroll(),
        scrollbar: last_chat_scrollbar_visible(),
        idle_area: last_idle_animation_area(),
        badge: (
            visible_expand_edit_badge(),
            visible_expand_edit_badge_line(),
            badge_rect(),
        ),
        tail: (tail_catchup_active(), take_tail_follow_snap_request()),
        todo_areas: (
            viewport::pinned_todo_more_area(),
            viewport::pinned_todo_card_area(),
        ),
        jump_area: viewport::jump_to_bottom_area(),
        images: panel_image_preview::snapshot_regions(),
    }
}

impl Drop for InteractiveRenderState {
    fn drop(&mut self) {
        restore_snapshot!(TEST_COPY_VIEWPORT, copy_viewport_state, self.copies.clone());
        restore_snapshot!(TEST_LAST_LAYOUT, last_layout_state, self.layout);
        restore_snapshot!(
            TEST_LAST_CHAT_FRAME,
            last_chat_frame_state,
            self.frame.clone()
        );
        restore_snapshot!(TEST_LAST_STATUS_AREA, last_status_area_state, self.status);
        set_visible_copy_targets(self.targets.clone());
        set_last_max_scroll(self.max_scroll);
        set_last_total_wrapped_lines(self.total_lines);
        set_last_chat_viewport_height(self.viewport_height);
        set_last_resolved_chat_scroll(self.resolved_scroll);
        set_pinned_pane_total_lines(self.pane_lines);
        set_last_diff_pane_effective_scroll(self.pane_scroll);
        set_last_diff_pane_max_scroll(self.pane_max_scroll);
        set_last_chat_scrollbar_visible(self.scrollbar);
        record_idle_animation_area(self.idle_area);
        set_visible_expand_edit_badge(self.badge.0, self.badge.1);
        set_visible_expand_edit_badge_rect(self.badge.2);
        set_tail_catchup_active(self.tail.0);
        let _ = take_tail_follow_snap_request();
        if self.tail.1 {
            request_tail_follow_snap();
        }
        viewport::set_pinned_todo_more_area(self.todo_areas.0);
        viewport::set_pinned_todo_card_area(self.todo_areas.1);
        viewport::set_jump_to_bottom_area(self.jump_area);
        panel_image_preview::restore_regions(self.images.clone());
    }
}
