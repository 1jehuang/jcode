//! Collapsed paste blocks in the composer.
//!
//! A multi-line paste is shown as a numbered `[Paste #N - M lines]`
//! placeholder so the composer stays readable. The raw text lives in
//! `App::pasted_contents` and is restored on submit, when queueing, and by the
//! expand hotkey (Alt+X). Prompt history always records the expanded text.

use super::super::App;
use super::insert_input_text;

/// Placeholder shown in the composer for the paste stored at `paste_number`
/// (1-based) with `content`.
pub(in crate::tui::app) fn paste_placeholder(paste_number: usize, content: &str) -> String {
    let line_count = content.lines().count().max(1);
    format!(
        "[Paste #{paste_number} - {line_count} line{}]",
        if line_count == 1 { "" } else { "s" }
    )
}

pub(in crate::tui::app) fn handle_text_paste(app: &mut App, text: String) {
    crate::logging::info(&format!(
        "Text paste: {} chars, {} lines",
        text.len(),
        text.lines().count()
    ));

    // Single-line pastes stay inline (including a trailing newline); only
    // genuinely multi-line blocks collapse.
    if text.lines().count() <= 1 {
        insert_input_text(app, &text);
        return;
    }
    if expand_matching_paste(app, &text) {
        return;
    }

    let paste_number = app.pasted_contents.len() + 1;
    let placeholder = paste_placeholder(paste_number, &text);
    app.pasted_contents.push(text);
    insert_input_text(app, &placeholder);
    app.set_status_notice("Pasted text collapsed - Alt+X to expand");
}

/// Re-pasting text that is already stored expands its own placeholder in place
/// instead of storing a duplicate.
fn expand_matching_paste(app: &mut App, text: &str) -> bool {
    let Some(content_index) = app
        .pasted_contents
        .iter()
        .rposition(|content| content == text)
    else {
        return false;
    };

    let placeholder = paste_placeholder(content_index + 1, text);
    let Some(placeholder_start) = app.input.rfind(placeholder.as_str()) else {
        return false;
    };

    replace_placeholder_with_text(app, placeholder_start, placeholder.len(), text);
    app.pasted_contents.remove(content_index);
    renumber_paste_placeholders(app, content_index);
    true
}

/// The paste placeholder closest before the cursor, as `(byte_start, index)`
/// into `pasted_contents`.
fn placeholder_before_cursor(app: &App) -> Option<(usize, usize)> {
    let mut target: Option<(usize, usize)> = None;
    for (index, content) in app.pasted_contents.iter().enumerate() {
        let placeholder = paste_placeholder(index + 1, content);
        for (start, _) in app.input.match_indices(&placeholder) {
            let closer = match target {
                Some((best, _)) => start > best,
                None => true,
            };
            if start <= app.cursor_pos && closer {
                target = Some((start, index));
            }
        }
    }
    target
}

/// True when Alt+X has a collapsed paste it can expand right now.
pub(in crate::tui::app) fn has_paste_to_expand_at_cursor(app: &App) -> bool {
    placeholder_before_cursor(app).is_some()
}

/// Alt+X chord shared by every dispatcher: Alt with the `x` key, including the
/// Option+X Unicode forms some macOS terminals insert, but not Ctrl+Alt+X or
/// other command chords that dispatch elsewhere.
fn is_alt_x_chord(
    code: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    if !modifiers.contains(KeyModifiers::ALT)
        || modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::META)
    {
        return false;
    }
    matches!(code, KeyCode::Char('x'))
        || crate::tui::keybind::shortcut_char_for_macos_option_key(code, modifiers) == Some('x')
}

/// Contextual Alt+X expansion shared by the local and remote dispatchers.
///
/// The todo-card toggle is bound to Alt+X by default and runs before the
/// generic Alt handler in both paths. A collapsed paste placeholder sitting
/// before the cursor is the more specific target, so it claims the chord;
/// with no placeholder this returns false and the toggle keeps working.
/// Returns true when the key was consumed.
pub(in crate::tui::app) fn try_expand_paste_on_alt_x(
    app: &mut App,
    code: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) -> bool {
    if !is_alt_x_chord(code, modifiers) || !has_paste_to_expand_at_cursor(app) {
        return false;
    }
    expand_paste_at_cursor(app);
    true
}

/// Alt+X: expand the paste placeholder closest before the cursor back into its
/// raw multi-line text. Returns false when the composer holds no paste.
pub(in crate::tui::app) fn expand_paste_at_cursor(app: &mut App) -> bool {
    if app.pasted_contents.is_empty() {
        return false;
    }

    let Some((start, content_index)) = placeholder_before_cursor(app) else {
        app.set_status_notice("No paste placeholder before the cursor to expand");
        return true;
    };

    let text = app.pasted_contents[content_index].clone();
    let placeholder_len = paste_placeholder(content_index + 1, &text).len();
    replace_placeholder_with_text(app, start, placeholder_len, &text);
    app.pasted_contents.remove(content_index);
    renumber_paste_placeholders(app, content_index);
    app.set_status_notice("Expanded paste");
    true
}

fn replace_placeholder_with_text(app: &mut App, start: usize, placeholder_len: usize, text: &str) {
    app.follow_chat_bottom_for_typing();
    app.remember_input_undo_state();
    app.input
        .replace_range(start..start + placeholder_len, text);
    app.cursor_pos = start + text.len();
    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
}

/// Deleting a stored paste shifts every later paste down one number, so rewrite
/// the remaining placeholders to keep them matching their stored text.
fn renumber_paste_placeholders(app: &mut App, removed_index: usize) {
    for index in removed_index..app.pasted_contents.len() {
        let old = paste_placeholder(index + 2, &app.pasted_contents[index]);
        let new = paste_placeholder(index + 1, &app.pasted_contents[index]);
        let Some(start) = app.input.rfind(&old) else {
            continue;
        };
        let end = start + old.len();
        if app.cursor_pos >= end {
            app.cursor_pos -= old.len() - new.len();
        } else if app.cursor_pos > start {
            app.cursor_pos = start + new.len();
        }
        app.input.replace_range(start..end, &new);
    }
}

pub(in crate::tui::app) fn expand_paste_placeholders(app: &App, input: &str) -> String {
    expand_paste_placeholders_with(&app.pasted_contents, input)
}

pub(in crate::tui::app) fn expand_paste_placeholders_with(
    pasted_contents: &[String],
    input: &str,
) -> String {
    let mut result = input.to_string();
    for (index, content) in pasted_contents.iter().enumerate().rev() {
        let placeholder = paste_placeholder(index + 1, content);
        if let Some(start) = result.rfind(&placeholder) {
            result.replace_range(start..start + placeholder.len(), content);
        }
    }
    result
}
