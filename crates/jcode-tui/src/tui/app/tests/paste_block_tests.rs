// Tests for numbered paste blocks: single-prompt submission and Alt+X expansion.

#[test]
fn multi_line_paste_collapses_to_one_numbered_placeholder() {
    let mut app = create_test_app();
    let text = "alpha\nbeta\ngamma\ndelta".to_string();

    app.handle_paste(text.clone());

    assert_eq!(app.input(), "[Paste #1 - 4 lines]");
    assert_eq!(app.pasted_contents, vec![text.clone()]);
    assert_eq!(
        crate::tui::app::input::expand_paste_placeholders(&app, &app.input),
        text,
        "the collapsed block must still expand to the pasted text"
    );
}

#[test]
fn single_line_paste_stays_inline_without_a_placeholder() {
    let mut app = create_test_app();

    app.handle_paste("just one line".to_string());

    assert_eq!(app.input(), "just one line");
    assert!(app.pasted_contents.is_empty());
}

#[test]
fn submitting_a_paste_sends_one_prompt_and_records_expanded_history() {
    let mut app = create_test_app();
    app.persisted_prompt_history = Some(Vec::new());
    let pasted = "first line\nsecond line\nthird line".to_string();

    app.handle_paste(pasted.clone());
    assert_eq!(app.input(), "[Paste #1 - 3 lines]");

    app.submit_input();

    let user_messages: Vec<&DisplayMessage> = app
        .display_messages()
        .iter()
        .filter(|message| message.role == "user")
        .collect();
    assert_eq!(
        user_messages.len(),
        1,
        "a collapsed paste must submit exactly one prompt"
    );
    assert_eq!(user_messages[0].content, pasted);
    assert_eq!(
        app.persisted_prompt_history.as_deref(),
        Some(&[pasted.clone()][..]),
        "prompt history must hold the expanded text, never the placeholder"
    );
    let session_user_texts: Vec<&str> = app
        .session
        .messages
        .iter()
        .filter(|message| message.role == Role::User)
        .filter_map(|message| match message.content.first() {
            Some(ContentBlock::Text { text, .. }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        session_user_texts
            .iter()
            .filter(|text| **text == pasted)
            .count(),
        1,
        "the real multi-line text must be sent as one prompt: {session_user_texts:?}"
    );
    assert!(
        !session_user_texts
            .iter()
            .any(|text| text.contains("[Paste #")),
        "the placeholder must never reach the session"
    );
}

#[test]
fn alt_x_expands_the_placeholder_back_into_raw_text() {
    let mut app = create_test_app();
    let pasted = "one\ntwo\nthree".to_string();

    app.handle_paste(pasted.clone());
    assert_eq!(app.input(), "[Paste #1 - 3 lines]");

    app.handle_key(KeyCode::Char('x'), KeyModifiers::ALT)
        .unwrap();

    assert_eq!(app.input(), pasted);
    assert_eq!(app.cursor_pos, pasted.len());
    assert!(app.pasted_contents.is_empty());
    assert_eq!(app.status_notice(), Some("Expanded paste".to_string()));
}
