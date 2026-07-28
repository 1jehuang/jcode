//! Newline insertion in the composer, including a keymap-independent fallback.
//!
//! Shift+Enter needs the terminal to disambiguate a modified Enter, which in
//! practice means the kitty keyboard protocol: every Enter chord is otherwise
//! the same byte (`0x0d`). Most modern terminals support it and jcode requests
//! it at startup, so Shift+Enter works out of the box there. `/terminal-setup`
//! fixes the cases that need configuration (tmux, WezTerm) and explains the ones
//! that cannot be fixed (Terminal.app). See `docs/SHIFT_ENTER.md`.
//!
//! Option/Alt+Enter arrives as `ESC` + `CR`, which does not need the protocol,
//! so it works on more terminals but depends on the Option-as-Meta setting.
//!
//! The trailing-backslash continuation is the universal fallback: it depends
//! only on the draft text, so it works on every terminal and platform.

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
    if trailing.is_multiple_of(2) {
        return false;
    }
    app.remember_input_undo_state();
    app.input.pop();
    app.cursor_pos = app.input.len();
    hint_terminal_setup_once(app);
    insert_input_text(app, "\n");
    true
}

/// Point the user at `/terminal-setup` the first time they need the fallback.
///
/// Reaching for a trailing backslash means Shift+Enter did not work for them.
/// That is a terminal configuration problem with a real fix, so say so once
/// rather than letting them assume the workaround is the only option. Shown a
/// single time per session to stay out of the way.
fn hint_terminal_setup_once(app: &mut App) {
    if app.terminal_setup_hint_shown_this_session {
        return;
    }
    app.terminal_setup_hint_shown_this_session = true;
    app.set_status_notice(
        "Tip: run /terminal-setup to make Shift+Enter insert newlines".to_string(),
    );
}
