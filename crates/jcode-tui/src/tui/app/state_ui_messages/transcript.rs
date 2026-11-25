use super::*;

impl App {
    pub(in crate::tui::app) fn clear_display_messages(&mut self) {
        self.capture_visible_prompt_history();
        self.compacted_history_lazy = CompactedHistoryLazyState::default();
        // The transcript is about to be discarded; forget where the live reasoning
        // block started so a stale offset can't slice the new stream.
        self.reasoning_block_start = None;
        self.turn_reasoning_traces.clear();
        if !self.display_messages.is_empty() {
            self.display_messages.clear();
            self.bump_display_messages_version();
        }
        self.adopt_prompt_history_display();
    }
}
