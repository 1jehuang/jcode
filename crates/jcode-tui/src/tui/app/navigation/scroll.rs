use super::*;

impl App {
    /// Scroll the chat transcript up by `amount` lines.
    ///
    /// Returns `true` if the stored scroll position actually changed. Callers
    /// (e.g. the mouse-wheel queue) rely on this to avoid accumulating
    /// "phantom" scroll once the viewport is already pinned to the top.
    pub(in crate::tui::app) fn scroll_up(&mut self, amount: usize) -> bool {
        // A user scroll supersedes a pending resize anchor.
        self.pending_resize_anchor = None;
        // Leaving the collapsed terminal-clear screen: drop the trailing Ctrl+L
        // spacer so scrolling up reveals the transcript immediately instead of
        // first having to travel back through a viewport of blank rows.
        if self.terminal_clear_collapsed() {
            self.display_messages.pop();
            self.bump_display_messages_version();
            self.request_full_repaint();
        }
        // While older compacted history is still settling on screen, the renderer
        // is anchored to a distance-from-bottom rather than `scroll_offset`. Keep
        // scrolling continuous by moving the anchor itself instead of a stale
        // offset the renderer is currently ignoring.
        if let Some(mut anchor) = self.pending_history_anchor {
            let total = crate::tui::ui::last_total_wrapped_lines();
            anchor.lines_from_bottom = anchor
                .lines_from_bottom
                .saturating_add(amount)
                .min(total.max(anchor.lines_from_bottom));
            self.pending_history_anchor = Some(anchor);
            self.auto_scroll_paused = true;
            self.maybe_queue_compacted_history_load();
            // Force a full repaint: ratatui's diff does not re-emit the trailing
            // cell after a wide grapheme (emoji/CJK) when the symbol is unchanged,
            // so terminals like kitty/foot leave a stale "ghost" char from the
            // previous frame. See ratatui issue #2357. Buffer invalidation re-emits
            // every cell without the ED2 clear escape that made images flicker
            // during scroll (issue #404).
            self.request_full_repaint();
            return true;
        }
        let before = (self.scroll_offset, self.auto_scroll_paused);
        let max = self.scroll_max_estimate();
        if !self.auto_scroll_paused {
            let rendered_max = crate::tui::ui::last_max_scroll();
            let current_abs = max.saturating_sub(self.scroll_offset);
            self.scroll_offset = current_abs.saturating_sub(amount);
            if rendered_max > 0 {
                self.scroll_offset = self.scroll_offset.min(rendered_max.saturating_sub(amount));
            }
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        }
        self.auto_scroll_paused = true;
        // If the upward scroll bottomed out against the top of the currently
        // loaded content, fold the unsatisfied intent into the prefetch as
        // overshoot so the newly loaded history scrolls into view smoothly.
        let overshoot = if self.scroll_offset == 0 { amount } else { 0 };
        self.maybe_queue_compacted_history_load_with_overshoot(overshoot);
        let changed = before != (self.scroll_offset, self.auto_scroll_paused);
        if changed {
            // See note above (ratatui #2357): force a clean repaint on scroll so
            // wide-grapheme trailing cells cannot leave a ghost character.
            self.request_full_repaint();
        }
        changed
    }

    /// Scroll the chat transcript down by `amount` lines.
    ///
    /// Returns `true` if the stored scroll position actually changed. When the
    /// view is already following the bottom this is a no-op and returns
    /// `false`, so the mouse-wheel queue does not accumulate phantom scroll
    /// that would later have to be undone before scrolling up moves the view.
    pub(in crate::tui::app) fn scroll_down(&mut self, amount: usize) -> bool {
        // A user scroll supersedes a pending resize anchor.
        self.pending_resize_anchor = None;
        // Mirror `scroll_up`: while an older-history prepend is still settling,
        // the renderer is anchored to distance-from-bottom, so move the anchor
        // toward the bottom instead of a stale `scroll_offset`.
        if let Some(mut anchor) = self.pending_history_anchor {
            if anchor.lines_from_bottom == 0 {
                return false;
            }
            anchor.lines_from_bottom = anchor.lines_from_bottom.saturating_sub(amount);
            self.pending_history_anchor = Some(anchor);
            // ratatui #2357: clean repaint on scroll to avoid wide-grapheme ghosts.
            self.request_full_repaint();
            return true;
        }
        if !self.auto_scroll_paused {
            // Already pinned to the bottom: nothing further to scroll.
            return false;
        }
        let before = self.scroll_offset;
        let max = self.scroll_max_estimate();
        let rendered_max = crate::tui::ui::last_max_scroll();
        // The renderer's exact extent is the authoritative ceiling. Only fall
        // back to the (possibly inflated) estimate while streaming can leave
        // `rendered_max` stale at 0 even though there is content to scroll.
        let bottom_threshold = if rendered_max > 0 {
            rendered_max.min(max)
        } else if self.is_processing || !self.streaming.streaming_text.is_empty() {
            max
        } else {
            // Not streaming and nothing to scroll: we are already at the bottom.
            0
        };
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
        let changed = if self.scroll_offset >= bottom_threshold {
            self.follow_chat_bottom();
            true
        } else {
            // Never let the stored offset grow past the largest offset that
            // still moves the rendered viewport. Otherwise scrolling down at
            // (or near) the bottom silently accumulates "phantom" offset that
            // later has to be undone before scrolling up moves the view again.
            self.scroll_offset = self.scroll_offset.min(bottom_threshold);
            self.scroll_offset != before
        };
        if changed {
            // ratatui #2357: clean repaint on scroll to avoid wide-grapheme ghosts.
            self.request_full_repaint();
        }
        changed
    }

    pub(in crate::tui::app) fn follow_chat_bottom(&mut self) {
        self.pending_history_anchor = None;
        // Resuming the tail drops any reading position captured for a resize.
        self.pending_resize_anchor = None;
        self.scroll_offset = 0;
        self.auto_scroll_paused = false;
        crate::tui::ui::request_tail_follow_snap();
    }

    /// `scroll_to_bottom` hotkey / "Jump to bottom" pill: go to the live tail
    /// and resume auto-follow. Unlike the Ctrl+G bookmark this is one-way, so
    /// any stashed bookmark is dropped.
    pub(in crate::tui::app) fn jump_to_chat_bottom(&mut self) {
        self.scroll_bookmark = None;
        self.follow_chat_bottom();
    }
}
