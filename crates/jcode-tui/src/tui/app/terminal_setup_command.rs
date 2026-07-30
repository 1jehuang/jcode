//! The `/terminal-setup` command: make Shift+Enter actually work.
//!
//! See [`crate::tui::terminal_setup`] for why terminals cannot report
//! Shift+Enter by default. This module is the user-facing half: it reports
//! whether the current terminal can report the chord, and applies the fix when
//! configuration can provide one.

use jcode_tui_messages::DisplayMessage;

use super::App;
use crate::tui::terminal_setup::{self, Applied};

impl App {
    pub(super) fn handle_terminal_setup_command(&mut self, trimmed: &str) -> bool {
        if trimmed != "/terminal-setup" {
            return false;
        }

        // If the terminal already reports modified Enter, Shift+Enter works and
        // changing config would be noise.
        if terminal_setup::supports_modified_enter_reporting() == Some(true) {
            self.push_display_message(DisplayMessage::system(
                "✓ Shift+Enter already works: this terminal reports modified Enter \
                 via the kitty keyboard protocol.\n\
                 Press Shift+Enter to insert a newline; Enter submits."
                    .to_string(),
            ));
            return true;
        }

        let inside_tmux = std::env::var_os("TMUX").is_some();
        let term_program = std::env::var("TERM_PROGRAM").ok();
        let target = terminal_setup::diagnose(term_program.as_deref(), inside_tmux);

        let home = match dirs::home_dir() {
            Some(home) => home,
            None => {
                self.push_display_message(DisplayMessage::error(
                    "Cannot locate your home directory, so terminal config cannot be updated."
                        .to_string(),
                ));
                return true;
            }
        };

        match terminal_setup::apply(target, &home) {
            Ok(Applied::Changed { detail }) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "✓ Configured {} so Shift+Enter is reported distinctly from Enter.\n\
                     {detail}\n{}",
                    target.label(),
                    target.activation_note()
                )));
            }
            Ok(Applied::AlreadyConfigured) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "{} is already configured for Shift+Enter.\n\
                     If the chord still submits, {}",
                    target.label(),
                    target.activation_note()
                )));
            }
            Ok(Applied::Manual { message }) => {
                self.push_display_message(DisplayMessage::system(message));
            }
            Err(err) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to update {} configuration: {err}",
                    target.label()
                )));
            }
        }
        true
    }
}
