//! Display-only compact DM fixture for isolated visual validation.

use super::*;

impl App {
    pub(super) fn show_debug_dm_fixture(&mut self) -> String {
        // No model prompt or agent delivery is sent.
        self.display_messages = vec![
            DisplayMessage::user("Compact agent messages"),
            DisplayMessage::background_task(
                "🐝 **Swarm await started** · watching `fox, sheep`\n\nWaiting in the background.",
            ),
            DisplayMessage::swarm(
                "DM from fox",
                "The parser changes are ready for validation.",
            ),
            DisplayMessage::swarm(
                "DM from sheep",
                "The regression tests pass and the patch is ready.",
            ),
            DisplayMessage::assistant("An intervening reply starts a new DM row."),
            DisplayMessage::swarm("DM from owl", "The final validation is complete."),
        ];
        self.bump_display_messages_version();
        self.scroll_offset = 0;
        self.auto_scroll_paused = false;
        self.request_full_redraw();
        "OK: compact DM fixture ready".to_string()
    }
}
