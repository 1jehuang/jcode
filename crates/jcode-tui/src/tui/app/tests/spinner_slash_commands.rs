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

/// An unfocused client batches streamed text slowly, so the one-cell spinner
/// repaint has to stay available between batches. A focused client streams
/// full frames fast enough to carry the spinner and keeps the old behaviour.
#[test]
fn unfocused_streaming_keeps_the_one_cell_spinner_path() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.processing_started = Some(Instant::now());
    app.streaming.streaming_text = "partial reply".to_string();

    app.set_client_focused(true);
    assert!(
        super::run_shell::status_spinner_only_symbol(&app).is_none(),
        "focused streaming draws the spinner with its full frames"
    );

    app.set_client_focused(false);
    assert!(
        super::run_shell::status_spinner_only_symbol(&app).is_some(),
        "unfocused streaming must keep the spinner moving between text batches"
    );
}

/// Server events are batched only for an unfocused client with a turn in
/// flight. Focus, or an idle session, must read each event as it arrives.
#[test]
fn server_events_are_held_only_while_unfocused_and_processing() {
    let mut app = create_test_app();
    app.set_client_focused(false);
    assert!(!super::run_shell::hold_server_events_while_unfocused(&app));

    app.is_processing = true;
    assert!(super::run_shell::hold_server_events_while_unfocused(&app));

    app.set_client_focused(true);
    assert!(!super::run_shell::hold_server_events_while_unfocused(&app));
}
