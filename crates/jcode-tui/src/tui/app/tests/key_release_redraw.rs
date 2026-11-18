// Key releases (kitty keyboard protocol reports one per keystroke) must not
// force a full frame unless they change visible state.
#[test]
fn plain_key_release_changes_no_visible_state() {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let ev = |code, modifiers, kind| KeyEvent {
        code,
        modifiers,
        kind,
        state: KeyEventState::NONE,
    };
    let mut app = create_test_app();
    // Ordinary typing: press then release 'a'. No badge is shown, so the
    // release must not count as a visible change.
    app.update_copy_badge_key_event(ev(KeyCode::Char('a'), KeyModifiers::NONE, KeyEventKind::Press));
    let before = app.key_release_visible_state();
    app.update_copy_badge_key_event(ev(KeyCode::Char('a'), KeyModifiers::NONE, KeyEventKind::Release));
    assert_eq!(before, app.key_release_visible_state());

    // Releasing a held Alt does hide the badge, so that release must draw.
    app.update_copy_badge_key_event(ev(
        KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftAlt),
        KeyModifiers::ALT,
        KeyEventKind::Press,
    ));
    app.copy_badge_ui.alt_pulse_until = None;
    let before = app.key_release_visible_state();
    app.update_copy_badge_key_event(ev(
        KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftAlt),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));
    assert_ne!(before, app.key_release_visible_state());
}
