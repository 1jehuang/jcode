//! Compacted display-history replacement.

use super::*;

impl App {
    pub(in super::super) fn apply_compacted_history_window(
        &mut self,
        mut messages: Vec<DisplayMessage>,
        images: Vec<crate::session::RenderedImage>,
        total_messages: usize,
        visible_messages: usize,
        remaining_messages: usize,
        hidden_user_prompts: usize,
    ) {
        let recent_history = self.recent_history_window_active();
        // A load that arrives while the reader follows the tail (the Alt+E
        // prompt-history backfill) must not pull them up to the old history.
        let following_tail = !self.auto_scroll_paused && self.pending_history_anchor.is_none();
        compact_display_messages_for_storage(&mut messages);
        carry_over_tool_durations(&self.display_messages, &mut messages);
        self.display_messages = messages;
        self.remote_side_pane_images = images;
        self.invalidate_side_pane_images_signature();
        self.compacted_history_lazy = CompactedHistoryLazyState {
            total_messages,
            visible_messages,
            remaining_messages,
            hidden_user_prompts,
            pending_request_visible: None,
        };
        if recent_history {
            // Once the current transcript is fully visible, its original
            // compaction marker (if any) becomes the next lazy history source.
            self.sync_compacted_history_lazy_from_display_messages();
        }
        self.auto_scroll_paused = true;
        // Older messages are prepended above the current view. If the reader had
        // an anchor captured (they scrolled up to trigger this load), leave the
        // scroll position for the next render to resolve so the content under
        // them stays put instead of teleporting to the new absolute top. Only
        // fall back to the top when there is no anchor to honor.
        if self.pending_history_anchor.is_none() {
            self.scroll_offset = 0;
        }
        if following_tail {
            self.auto_scroll_paused = false;
        }
        self.bump_display_messages_version();
        self.note_runtime_memory_event_force(
            "compacted_history_loaded",
            "display_history_lazy_window",
        );
        if remaining_messages > 0 {
            self.set_status_notice(format!(
                "Loaded {} compacted messages · {} older hidden",
                visible_messages, remaining_messages
            ));
        } else if total_messages > 0 {
            self.set_status_notice(format!("Loaded all {} compacted messages", total_messages));
        }
    }
}
