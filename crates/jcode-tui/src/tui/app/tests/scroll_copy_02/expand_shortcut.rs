#[test]
fn test_expand_badge_shortcut_toggles_inline_diff_and_pulses_key() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = create_copy_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('E'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ));

    assert_eq!(app.diff_mode, crate::config::DiffDisplayMode::FullInline);
    assert!(app.copy_badge_ui().key_active.is_some());
}
