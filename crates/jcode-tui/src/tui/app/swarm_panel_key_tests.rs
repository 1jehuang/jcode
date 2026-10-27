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
        swarm_panel_action_for_key(KeyCode::Enter, KeyModifiers::ALT),
        Some(SwarmPanelAction::Enter)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('b'), KeyModifiers::ALT),
        Some(SwarmPanelAction::ReturnToParent)
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
        swarm_panel_action_for_key(KeyCode::Enter, KeyModifiers::NONE),
        None,
        "plain enter must remain chat input"
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

#[test]
fn entering_selected_agent_queues_resume_and_return_queues_parent() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.set_swarm_panel_focus(true);
    app.debug_force_inline_gallery = true;
    app.remote_session_id = Some("parent-session".to_string());
    app.remote_swarm_members = vec![crate::protocol::SwarmMemberStatus {
        session_id: "worker-session".to_string(),
        friendly_name: Some("worker-fox".to_string()),
        role: Some("agent".to_string()),
        task_label: None,
        status: "running".to_string(),
        detail: None,
        status_age_secs: None,
        is_headless: None,
        live_attachments: None,
        report_back_to_session_id: Some("parent-session".to_string()),
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: Default::default(),
    }];

    app.enter_selected_swarm_agent();
    assert_eq!(
        app.workspace_client.take_pending_resume_session().as_deref(),
        Some("worker-session")
    );
    assert_eq!(app.swarm_parent_session_id.as_deref(), Some("parent-session"));
    assert!(!app.swarm_panel_focused);

    app.return_to_swarm_parent();
    assert_eq!(
        app.workspace_client.take_pending_resume_session().as_deref(),
        Some("parent-session")
    );
    assert_eq!(app.swarm_parent_session_id, None);
}
