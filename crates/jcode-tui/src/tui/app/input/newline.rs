//! Newline insertion in the composer, including a keymap-independent fallback.
//!
//! jcode's newline chords (Shift+Enter and Option/Alt+Enter) both require the
//! terminal to disambiguate a modified Enter, which in practice means the kitty
//! keyboard protocol. macOS terminals that do not implement it (Terminal.app,
//! and iTerm2 without "Report modifiers using CSI u") send a bare CR for every
//! Enter chord, so those users had no way at all to type a multi-line prompt.
//!
//! The trailing-backslash continuation covers them: it depends only on the
//! draft text, so it works on every terminal and every platform.

use crossterm::event::{KeyCode, KeyModifiers};

use super::super::App;
use super::insert_input_text;

/// Handles every way an Enter press can insert a newline instead of sending.
///
/// Returns true when the key was consumed as a newline.
pub(in crate::tui::app) fn enter_inserts_newline(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    if code != KeyCode::Enter {
        return false;
    }
    if modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) {
        insert_input_text(app, "\n");
        return true;
    }
    consume_backslash_continuation(app)
}

/// A trailing backslash at the cursor turns Enter into a newline.
///
/// The backslash itself is removed, matching shell line-continuation muscle
/// memory. An escaped backslash (`\\`) is literal text and still submits.
fn consume_backslash_continuation(app: &mut App) -> bool {
    if app.cursor_pos != app.input.len() || !app.input.ends_with('\\') {
        return false;
    }
    let trailing = app.input.len() - app.input.trim_end_matches('\\').len();
    if trailing % 2 == 0 {
        return false;
    }
    app.remember_input_undo_state();
    app.input.pop();
    app.cursor_pos = app.input.len();
    insert_input_text(app, "\n");
    true
}
