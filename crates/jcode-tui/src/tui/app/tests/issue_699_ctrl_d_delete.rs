// Issue #699: Ctrl+D must forward-delete a character while the input line has
// text (readline/terminal convention), and only quit on an empty input line.

#[test]
fn test_ctrl_d_deletes_character_under_cursor() {
    let mut app = create_test_app();
    app.input = "hello".to_string();
    app.cursor_pos = 1;

    app.handle_key(KeyCode::Char('d'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input, "hllo");
    assert_eq!(app.cursor_pos, 1);
    assert!(
        app.quit_pending.is_none(),
        "Ctrl+D with text in the input must not arm quit"
    );
}

#[test]
fn test_ctrl_d_at_end_of_non_empty_input_does_not_quit() {
    let mut app = create_test_app();
    app.input = "hello".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Char('d'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input, "hello", "nothing to delete forward");
    assert!(
        app.quit_pending.is_none(),
        "Ctrl+D must never quit while the input line has pending text"
    );
}

#[test]
fn test_ctrl_d_on_empty_input_still_requests_quit() {
    let mut app = create_test_app();
    assert!(app.input.is_empty());

    app.handle_key(KeyCode::Char('d'), KeyModifiers::CONTROL)
        .unwrap();

    assert!(
        app.quit_pending.is_some() || app.cancel_requested,
        "Ctrl+D on an empty line keeps the existing interrupt/quit behavior"
    );
}

#[test]
fn test_ctrl_d_deletes_multibyte_character() {
    let mut app = create_test_app();
    app.input = "héllo".to_string();
    app.cursor_pos = 1;

    app.handle_key(KeyCode::Char('d'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input, "hllo");
}
