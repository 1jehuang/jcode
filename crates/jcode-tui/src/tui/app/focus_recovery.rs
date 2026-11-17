use super::App;

impl App {
    /// Everything a focus-gained handler must do, in one place.
    ///
    /// A ConPTY host can move or resize its pane while the client is unfocused,
    /// and ratatui only emits diffs against its own last buffer. Unless every row
    /// is repainted, the composer stays anchored to rows the pane no longer shows
    /// (#1812). Keeping the sequence here also stops three copies of it drifting.
    pub(super) fn handle_client_focus_gained(&mut self) {
        crate::tui::reapply_configured_terminal_modes_after_focus();
        self.note_client_focus(true);
        // Returns whether the notice was queued; a false here is not actionable.
        let _notified = self.set_client_focused(true);
        #[cfg(windows)]
        self.force_viewport_repaint_after_shift();
    }
}
