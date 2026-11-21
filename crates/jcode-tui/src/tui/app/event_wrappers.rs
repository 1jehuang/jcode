use super::*;
use crate::tui::backend;

impl App {
    pub(super) fn handle_server_event(
        &mut self,
        event: crate::protocol::ServerEvent,
        remote: &mut impl backend::RemoteEventState,
    ) -> bool {
        remote::handle_server_event(self, event, remote)
    }

    #[cfg(test)]
    pub(super) fn handle_remote_char_input(&mut self, c: char) {
        remote::handle_remote_char_input(self, c);
    }

    /// Handle keyboard input in remote mode
    #[cfg(test)]
    pub(super) async fn handle_remote_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        remote: &mut backend::RemoteConnection,
    ) -> Result<()> {
        remote::handle_remote_key(self, code, modifiers, remote).await
    }

    /// Process turn while still accepting input for queueing
    pub(super) async fn process_turn_with_input(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
        bus_receiver: &mut tokio::sync::broadcast::Receiver<crate::bus::BusEvent>,
    ) {
        local::process_turn_with_input(self, terminal, event_stream, bus_receiver).await;
    }

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

    /// Repaint every cell after an event that can move the physical viewport
    /// without a geometry change (issue #1812).
    ///
    /// Windows ConPTY hosts such as herdr can repaint, zoom or scroll their own
    /// pane while the console window rect crossterm reports stays the same, so
    /// ratatui's diff would anchor the frame to rows the pane no longer shows.
    /// No-op off Windows: native terminals report geometry faithfully.
    pub(super) fn force_viewport_repaint_after_shift(&mut self) {
        if cfg!(windows) {
            self.force_full_redraw = true;
        }
    }

    /// Remove a keyboard-protocol report that reached the composer as text.
    #[cfg(windows)]
    pub(super) fn scrub_leaked_report_after_input(&mut self) {
        let Some(fragment) = crate::tui::keyboard_protocol::scrub_leaked_report(
            &mut self.input,
            &mut self.cursor_pos,
        ) else {
            return;
        };
        crate::logging::info(&format!(
            "Stripped a keyboard-protocol report that arrived as text: {fragment:?}"
        ));
        // The picker filter was built before the strip, so a report arriving
        // after `/model ` could leave the picker matching nothing.
        self.sync_model_picker_preview_from_input();
        self.reset_tab_completion();
    }
}
