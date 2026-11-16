use super::*;

#[test]
fn session_picker_resume_action_keeps_overlay_open() {
    let mut app = create_test_app();
    app.session_picker_mode = SessionPickerMode::CatchUp;
    app.session_picker_overlay = Some(RefCell::new(
        crate::tui::session_picker::SessionPicker::new(vec![
            crate::tui::session_picker::SessionInfo {
                id: "session_keep_open".to_string(),
                parent_id: None,
                short_name: "keep-open".to_string(),
                icon: "k".to_string(),
                title: "Keep Open".to_string(),
                message_count: 1,
                user_message_count: 1,
                assistant_message_count: 0,
                created_at: chrono::Utc::now(),
                last_message_time: chrono::Utc::now(),
                last_active_at: None,
                working_dir: None,
                model: None,
                provider_key: None,
                is_canary: false,
                is_debug: false,
                saved: false,
                save_label: None,
                status: crate::session::SessionStatus::Closed,
                needs_catchup: false,
                estimated_tokens: 0,
                first_user_prompt: None,
                messages_preview: Vec::new(),
                search_index: "keep-open keep open".to_string(),
                server_name: None,
                server_icon: None,
                source: crate::tui::session_picker::SessionSource::Jcode,
                resume_target: crate::tui::session_picker::ResumeTarget::JcodeSession {
                    session_id: "session_keep_open".to_string(),
                },
                external_path: None,
                todo_statuses: Vec::new(),
            },
        ]),
    ));

    app.handle_session_picker_key(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
    )
    .expect("session picker enter should succeed");

    assert!(app.session_picker_overlay.is_some());
}

#[test]
fn session_picker_enter_queues_current_terminal_resume_and_closes_overlay() {
    let mut app = create_test_app();
    app.session_picker_mode = SessionPickerMode::Resume;
    app.session_picker_overlay = Some(RefCell::new(
        crate::tui::session_picker::SessionPicker::new(vec![
            crate::tui::session_picker::SessionInfo {
                id: "session_here_123".to_string(),
                parent_id: None,
                short_name: "here".to_string(),
                icon: "h".to_string(),
                title: "Here".to_string(),
                message_count: 1,
                user_message_count: 1,
                assistant_message_count: 0,
                created_at: chrono::Utc::now(),
                last_message_time: chrono::Utc::now(),
                last_active_at: None,
                working_dir: None,
                model: None,
                provider_key: None,
                is_canary: false,
                is_debug: false,
                saved: false,
                save_label: None,
                status: crate::session::SessionStatus::Closed,
                needs_catchup: false,
                estimated_tokens: 0,
                first_user_prompt: None,
                messages_preview: Vec::new(),
                search_index: "here".to_string(),
                server_name: None,
                server_icon: None,
                source: crate::tui::session_picker::SessionSource::Jcode,
                resume_target: crate::tui::session_picker::ResumeTarget::JcodeSession {
                    session_id: "session_here_123".to_string(),
                },
                external_path: None,
                todo_statuses: Vec::new(),
            },
        ]),
    ));

    app.handle_session_picker_key(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
    )
    .expect("session picker enter should succeed");

    assert!(app.session_picker_overlay.is_none());
    assert_eq!(
        app.workspace_client
            .take_pending_resume_session()
            .as_deref(),
        Some("session_here_123")
    );
}
