#[test]
fn slash_palette_remains_navigable_while_streaming_spinner_is_active() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.processing_started = Some(Instant::now());
    app.input = "/".to_string();
    app.cursor_pos = app.input.len();

    let suggestions = app.command_suggestions();
    assert!(suggestions.len() > 1, "slash palette should be open");
    assert!(
        super::run_shell::status_spinner_only_symbol(&app).is_none(),
        "the one-cell spinner fast path must yield to the slash palette overlay"
    );

    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .expect("navigate slash suggestions");
    assert_eq!(app.command_suggestion_selected, 1);

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("accept slash suggestion");
    assert_ne!(app.input, "/");
    assert!(app.input.starts_with('/'));
    assert!(!app.cancel_requested, "palette input must not interrupt the turn");
}
