//! Editor, grapheme and paste tests for the `ask_user` form.

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use super::*;
use crate::tui::ask_form::state::{EditorKind, MAX_TEXT_CODE_POINTS};

#[test]
fn editor_captures_shortcut_letters_as_text() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    for character in ['n', 'p', '1', ' '] {
        assert_eq!(
            key_with(
                &context,
                KeyCode::Char(character),
                KeyModifiers::NONE,
                Some(&character.to_string())
            ),
            KeyOutcome::Consumed
        );
    }
    active(|form| {
        assert_eq!(form.questions[0].notes, "np1 ");
        assert_eq!(
            form.editor.map(|editor| editor.kind),
            Some(EditorKind::Notes)
        );
    });
}

#[test]
fn text_input_is_preferred_over_the_key_code() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    key_with(&context, KeyCode::Char('x'), KeyModifiers::NONE, Some("é"));
    active(|form| assert_eq!(form.questions[0].notes, "é"));
}

#[test]
fn grapheme_edits_move_and_delete_whole_graphemes() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));

    // Combined grapheme: e + combining acute.
    key_with(
        &context,
        KeyCode::Char('e'),
        KeyModifiers::NONE,
        Some("e\u{301}"),
    );
    // Emoji ZWJ sequence typed as one paste.
    let family = "👨‍👩‍👧‍👦";
    assert!(paste_text(&context, family));
    active(|form| assert_eq!(form.questions[0].notes, format!("e\u{301}{family}")));

    // Left moves over the whole family, right back to the end.
    key(&context, KeyCode::Left);
    active(|form| {
        assert_eq!(form.questions[0].notes_cursor, "e\u{301}".len());
    });
    key(&context, KeyCode::Right);
    active(|form| {
        assert_eq!(
            form.questions[0].notes_cursor,
            format!("e\u{301}{family}").len()
        );
    });

    // Backspace removes the family in one edit.
    key(&context, KeyCode::Backspace);
    active(|form| assert_eq!(form.questions[0].notes, "e\u{301}"));

    // Backspace removes the combined grapheme in one edit.
    key(&context, KeyCode::Backspace);
    active(|form| {
        assert!(form.questions[0].notes.is_empty());
        assert_eq!(form.questions[0].notes_cursor, 0);
    });
}

#[test]
fn home_and_end_move_to_line_boundaries() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    assert!(paste_text(&context, "ab\ncd"));
    // Cursor is at the end (byte 5).
    key(&context, KeyCode::Home);
    active(|form| assert_eq!(form.questions[0].notes_cursor, 3));
    key(&context, KeyCode::End);
    active(|form| assert_eq!(form.questions[0].notes_cursor, 5));
    key(&context, KeyCode::Home);
    key(&context, KeyCode::Backspace);
    active(|form| assert_eq!(form.questions[0].notes, "abcd"));
}

#[test]
fn paste_into_editor_preserves_newlines() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    assert!(paste_text(&context, "line 1\nline 2"));
    active(|form| {
        assert_eq!(form.questions[0].notes, "line 1\nline 2");
        assert_eq!(form.questions[0].notes_cursor, "line 1\nline 2".len());
    });
}

#[test]
fn paste_opens_the_selected_other_editor() {
    reset();
    let context = install_default();
    // Highlight Other (third row; q1 has two normal options).
    key(&context, KeyCode::Down);
    key(&context, KeyCode::Down);
    assert!(paste_text(&context, "typed answer"));
    active(|form| {
        assert_eq!(form.questions[0].confirmed.as_deref(), Some("__other__"));
        assert_eq!(form.questions[0].other_text, "typed answer");
        assert_eq!(
            form.editor,
            Some(crate::tui::ask_form::state::EditorFocus {
                question: 0,
                kind: EditorKind::Other
            })
        );
    });
}

#[test]
fn paste_without_a_text_target_returns_false_and_explains_where_it_went() {
    reset();
    let context = install_default();
    // Highlight a normal option: no editor, no Other selection.
    assert!(!paste_text(&context, "composer text"));
    active(|form| {
        assert!(form.questions[0].other_text.is_empty());
        assert!(
            form.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("composer"))
        );
    });
}

#[test]
fn over_limit_paste_is_rejected_and_preserves_the_draft() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    assert!(paste_text(&context, "keep"));
    let oversized = "x".repeat(MAX_TEXT_CODE_POINTS);
    // Consumed (the form must not hand it to the composer) but rejected.
    assert!(paste_text(&context, &oversized));
    active(|form| {
        assert_eq!(form.questions[0].notes, "keep");
        assert!(
            form.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Text limit"))
        );
    });
}

#[test]
fn hidden_editor_cannot_take_paste() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    assert!(paste_text(&context, "visible"));

    let blocked = ViewContext::from_probe(&blocked_probe("session"));
    assert!(!paste_text(&blocked, "hidden"));
    active(|form| assert_eq!(form.questions[0].notes, "visible"));
}
