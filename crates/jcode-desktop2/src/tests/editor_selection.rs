//! Editor regressions for selection-aware deletion.
//!
//! Kept beside the other split test modules so `editor.rs` stays inside the
//! code-size budget, per this directory's "no file grows unbounded" rule.

use crate::editor::Editor;

/// Regression for #728: a word-delete with an active selection used to
/// shorten the buffer while leaving `anchor` pointing past the new end.
/// The next ordinary Backspace then sliced out of bounds and panicked
/// across winit's Objective-C `key_down` boundary, aborting the process.
#[test]
fn word_delete_with_a_selection_does_not_leave_a_stale_anchor() {
    // The reporter's exact sequence: type "hello world", Shift+Left,
    // Option+Backspace, Backspace.
    let mut editor = Editor::with_text("hello world");
    editor.extend_left();
    assert!(editor.selection().is_some(), "expected an active selection");

    editor.delete_word_back();
    assert!(
        editor.selection().is_none(),
        "word-delete must consume the selection, not leave a stale anchor"
    );
    assert!(
        editor.cursor() <= editor.text().len(),
        "cursor {} past buffer {:?}",
        editor.cursor(),
        editor.text()
    );

    // Must not panic.
    editor.delete_back();
    assert!(editor.cursor() <= editor.text().len());
}

#[test]
fn word_delete_forward_with_a_selection_replaces_the_selection() {
    let mut editor = Editor::with_text("hello world");
    editor.move_to_start();
    editor.extend_right();
    assert!(editor.selection().is_some());

    editor.delete_word_forward();
    assert_eq!(editor.text(), "ello world");
    assert!(editor.selection().is_none());

    editor.delete_back();
    assert!(editor.cursor() <= editor.text().len());
}
