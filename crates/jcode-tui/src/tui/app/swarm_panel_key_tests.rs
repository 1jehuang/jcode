use super::{SwarmPanelAction, swarm_panel_action_for_key};
use crossterm::event::{KeyCode, KeyModifiers};

/// Plain typing (letters, space, enter, arrows without alt) must pass
/// through so the user can keep writing into the chat input while the
/// panel is focused.
#[test]
fn plain_typing_is_not_captured() {
    for code in [
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Char('o'),
        KeyCode::Char('g'),
        KeyCode::Char('G'),
        KeyCode::Char(' '),
        KeyCode::Enter,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Backspace,
    ] {
        let mods = if code == KeyCode::Char('G') {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        assert_eq!(
            swarm_panel_action_for_key(code, mods),
            None,
            "{code:?} must pass through to the chat input"
        );
    }
}

#[test]
fn alt_chords_drive_the_panel() {
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Down, KeyModifiers::ALT),
        Some(SwarmPanelAction::SelectNext)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Up, KeyModifiers::ALT),
        Some(SwarmPanelAction::SelectPrev)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('j'), KeyModifiers::ALT),
        Some(SwarmPanelAction::SelectNext)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('k'), KeyModifiers::ALT),
        Some(SwarmPanelAction::SelectPrev)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('o'), KeyModifiers::ALT),
        Some(SwarmPanelAction::PopOut)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Enter, KeyModifiers::ALT),
        Some(SwarmPanelAction::PopOut)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('P'), KeyModifiers::ALT | KeyModifiers::SHIFT),
        Some(SwarmPanelAction::OpenPrompt)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('p'), KeyModifiers::ALT | KeyModifiers::SHIFT),
        Some(SwarmPanelAction::OpenPrompt)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Esc, KeyModifiers::NONE),
        Some(SwarmPanelAction::Exit)
    );
}

#[test]
fn ctrl_chords_pass_through() {
    for code in [KeyCode::Char('j'), KeyCode::Char('o'), KeyCode::Down] {
        assert_eq!(
            swarm_panel_action_for_key(code, KeyModifiers::CONTROL),
            None,
            "{code:?}+ctrl belongs to other handlers"
        );
    }
}
