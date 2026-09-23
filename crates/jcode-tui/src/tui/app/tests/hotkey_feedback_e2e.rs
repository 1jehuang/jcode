// End-to-end tests for inline hotkey feedback: rare-hotkey notes and
// unknown-chord near-miss suggestions, driven through App::handle_key.

#[test]
fn unknown_ctrl_chord_sets_hotkey_feedback_with_suggestion() {
    let mut app = create_test_app();
    assert!(app.hotkey_feedback.is_none());

    // Ctrl+M is unbound (no control-key handler claims 'm'); the nearest
    // known hotkey is Alt+M (side panel toggle).
    app.handle_key(KeyCode::Char('m'), KeyModifiers::CONTROL)
        .unwrap();

    let (message, _) = app
        .hotkey_feedback
        .clone()
        .expect("unknown chord should set feedback");
    assert!(message.contains("Ctrl+M"), "{message}");
    assert!(message.contains("isn't bound"), "{message}");
    assert!(
        message.contains(&jcode_tui_core::keybind::alt_chord("M")),
        "{message}"
    );
    assert!(message.contains("side panel"), "{message}");

    // The renderer consumes the trait accessor; it must surface the same text
    // (and expire it later) so the notification line actually shows it.
    {
        use crate::tui::TuiState as _;
        let visible = app
            .hotkey_feedback()
            .expect("trait accessor should expose fresh feedback");
        assert_eq!(visible, message);
    }
}

#[test]
fn rare_known_hotkey_sets_feedback_and_repeats_stop_once_familiar() {
    let mut app = create_test_app();

    // Ctrl+T toggles queue mode; a fresh JCODE_HOME has no usage history, so
    // the first press is "rare" and should explain itself.
    app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL)
        .unwrap();
    let (message, _) = app
        .hotkey_feedback
        .clone()
        .expect("first use of a known hotkey should set feedback");
    assert!(message.contains("Ctrl+T"), "{message}");
    assert!(message.contains("queue mode"), "{message}");

    // After enough uses the action becomes familiar and the note stops.
    for _ in 0..8 {
        app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL)
            .unwrap();
    }
    app.hotkey_feedback = None;
    app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(
        app.hotkey_feedback.is_none(),
        "familiar hotkeys should not re-announce"
    );
}

#[test]
fn plain_typing_never_sets_hotkey_feedback() {
    let mut app = create_test_app();
    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('I'), KeyModifiers::SHIFT)
        .unwrap();
    assert!(app.hotkey_feedback.is_none());
    assert_eq!(app.input, "hI");
}

#[test]
fn unknown_chord_notice_is_rate_limited_per_chord() {
    let mut app = create_test_app();

    // Ctrl+; is unbound with no near suggestion.
    for _ in 0..6 {
        app.handle_key(KeyCode::Char(';'), KeyModifiers::CONTROL)
            .unwrap();
        // Reset the time-based limiter so only the per-chord cap applies.
        app.last_unknown_hotkey_notice = None;
    }
    assert!(app.unknown_hotkey_seen.get("Ctrl+;").copied().unwrap_or(0) <= 3);
}
