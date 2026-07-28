// Issue #544: stray Enter after a bracketed paste must not submit.
#[test]
fn bare_enter_immediately_after_paste_does_not_submit() {
    // Windows Terminal / conhost sends a separate bare Enter key event after
    // a bracketed paste ending with \n; it must not submit the chat (#544).
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = create_test_app();
    crate::tui::app::input::handle_paste(&mut app, "hello world\n".to_string());
    assert_eq!(
        app.input,
        "hello world\n".trim_end_matches('\n').to_owned() + "\n"
    );

    app.handle_key_press_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(
        !app.is_processing,
        "Enter right after paste must not submit"
    );
    assert!(
        !app.input.is_empty(),
        "input should be preserved after paste"
    );

    // A later, human-timed Enter still submits.
    crate::tui::app::input::paste_guard_expire_for_test();
    app.handle_key_press_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(app.input.is_empty(), "later Enter should submit normally");
}

// Keymap-independent multi-line input: a trailing backslash makes Enter insert
// a newline instead of submitting.
//
// Motivation: both newline chords (Shift+Enter, Option/Alt+Enter) require the
// terminal to disambiguate modified Enter via the kitty keyboard protocol.
// macOS Terminal.app and default-profile iTerm2 send a bare CR instead, so
// those users previously had no way to type a multi-line prompt.

#[test]
fn trailing_backslash_enter_inserts_newline_instead_of_submitting() {
    let mut app = create_test_app();
    app.set_input_for_test("first\\");
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("enter should be handled");

    assert_eq!(app.input, "first\n");
    assert_eq!(app.cursor_pos, app.input.len());
    assert!(
        app.display_messages().is_empty(),
        "the draft must not be submitted: {:?}",
        app.display_messages()
    );
}

#[test]
fn plain_enter_still_submits_without_a_trailing_backslash() {
    let mut app = create_test_app();
    app.set_input_for_test("first");
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("enter should be handled");

    assert!(app.input.is_empty(), "input should be consumed by submit");
}

#[test]
fn escaped_double_backslash_is_literal_text_and_still_submits() {
    let mut app = create_test_app();
    app.set_input_for_test("path\\\\");
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("enter should be handled");

    assert!(
        app.input.is_empty(),
        "an escaped backslash is literal, so Enter submits: {:?}",
        app.input
    );
}

#[test]
fn backslash_continuation_only_applies_at_the_end_of_the_draft() {
    let mut app = create_test_app();
    app.set_input_for_test("a\\b");
    app.cursor_pos = 2; // just after the backslash, not at end

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("enter should be handled");

    assert!(
        app.input.is_empty(),
        "mid-draft backslash must not become a continuation: {:?}",
        app.input
    );
}

#[test]
fn repeated_continuations_build_a_multi_line_draft() {
    let mut app = create_test_app();
    for line in ["one\\", "two\\"] {
        for c in line.chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .expect("char should be handled");
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .expect("enter should be handled");
    }
    for c in "three".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .expect("char should be handled");
    }

    assert_eq!(app.input, "one\ntwo\nthree");
}
