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

/// State-space sweep for the #728 class: after *any* mutating operation with an
/// active selection, the editor must be internally consistent.
///
/// The original bug was a single method forgetting to clear `anchor`, which
/// only surfaced on the *next* keystroke. Checking the invariant across every
/// mutator catches that whole family rather than the two instances that were
/// reported, and would have caught #728 on whichever method regressed.
#[test]
fn no_mutator_leaves_a_selection_pointing_past_the_buffer() {
    type Op = (&'static str, fn(&mut Editor));
    let ops: &[Op] = &[
        ("delete_back", |e| e.delete_back()),
        ("delete_forward", |e| e.delete_forward()),
        ("delete_word_back", |e| e.delete_word_back()),
        ("delete_word_forward", |e| e.delete_word_forward()),
        ("kill_to_start", |e| {
            e.kill_to_start();
        }),
        ("kill_to_end", |e| {
            e.kill_to_end();
        }),
        ("cut_line", |e| {
            e.cut_line();
        }),
        ("clear", |e| e.clear()),
        ("insert", |e| e.insert_str("X")),
    ];

    // Selections built by different motions, including ones anchored at either
    // end and spanning multibyte text.
    let selections: &[Op] = &[
        ("extend_left", |e| e.extend_left()),
        ("extend_right", |e| {
            e.move_to_start();
            e.extend_right();
        }),
        ("extend_word_left", |e| e.extend_word_left()),
        ("select_all", |e| e.select_all()),
    ];

    for text in ["hello world", "héllo wörld", "a", "one two three"] {
        for (sel_name, make_selection) in selections {
            for (op_name, op) in ops {
                let mut editor = Editor::with_text(text);
                make_selection(&mut editor);
                op(&mut editor);

                let len = editor.text().len();
                assert!(
                    editor.cursor() <= len,
                    "{op_name} after {sel_name} on {text:?}: cursor {} past buffer {:?}",
                    editor.cursor(),
                    editor.text()
                );
                if let Some((start, end)) = editor.selection() {
                    assert!(
                        start <= end && end <= len,
                        "{op_name} after {sel_name} on {text:?}: stale selection \
                         {start}..{end} over {:?}",
                        editor.text()
                    );
                }

                // The real symptom was the *next* keystroke panicking, so drive
                // one more edit and require it not to panic.
                editor.delete_back();
                editor.insert_str("z");
                assert!(editor.cursor() <= editor.text().len());
            }
        }
    }
}
