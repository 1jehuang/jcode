use super::*;
use crate::tui::session_picker::{SessionInfo, SessionSource};

fn session(id: &str, opening: Option<&str>) -> SessionInfo {
    let now = chrono::Utc::now();
    SessionInfo {
        id: id.into(),
        parent_id: None,
        short_name: id.into(),
        icon: "x".into(),
        title: "metadata".into(),
        message_count: 1,
        user_message_count: 1,
        assistant_message_count: 0,
        created_at: now,
        last_message_time: if id == "queued-stale" {
            now + chrono::Duration::seconds(1)
        } else {
            now
        },
        last_active_at: None,
        working_dir: None,
        model: None,
        provider_key: None,
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: jcode_session_types::SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 1,
        first_user_prompt: opening.map(str::to_owned),
        messages_preview: Vec::new(),
        search_index: if id == "queued-stale" {
            "needle deployment configuration rollback"
        } else {
            "metadata"
        }
        .into(),
        server_name: None,
        server_icon: None,
        source: SessionSource::Jcode,
        resume_target: session_picker::ResumeTarget::JcodeSession {
            session_id: id.into(),
        },
        external_path: None,
        todo_statuses: Vec::new(),
    }
}

#[test]
fn picker_tick_submits_best_search_result_without_second_key() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.session_picker_mode = SessionPickerMode::Resume;
    app.session_picker_overlay = Some(RefCell::new(session_picker::SessionPicker::new(vec![
        session("queued-stale", None),
        session(
            "queued-best",
            Some("needle deployment configuration rollback"),
        ),
    ])));
    app.handle_session_picker_key(KeyCode::Char('/'), KeyModifiers::NONE)
        .unwrap();
    // Opening-request rank intentionally applies only to >=4-word requests.
    // Typing preserves the pre-query row. The worker discovers the exact
    // opening request only after Enter is queued.
    for ch in "needle deployment configuration rollback".chars() {
        app.handle_session_picker_key(KeyCode::Char(ch), KeyModifiers::NONE)
            .unwrap();
    }
    assert_eq!(
        app.session_picker_overlay
            .as_ref()
            .unwrap()
            .borrow()
            .selected_session()
            .unwrap()
            .id,
        "queued-stale"
    );
    let modifiers = match crate::config::config().keybindings.session_picker_enter {
        crate::config::SessionPickerResumeAction::CurrentTerminal => KeyModifiers::NONE,
        crate::config::SessionPickerResumeAction::NewTerminal => KeyModifiers::CONTROL,
    };
    app.handle_session_picker_key(KeyCode::Enter, modifiers)
        .unwrap();
    assert!(app.workspace_client.take_pending_resume_session().is_none());
    assert!(
        app.session_picker_overlay
            .as_ref()
            .unwrap()
            .borrow()
            .search_submission_pending()
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "queued Enter never submitted"
        );
        assert!(app.poll_session_picker_search_submission());
        if let Some(id) = app.workspace_client.take_pending_resume_session() {
            assert_eq!(id, "queued-best");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(app.session_picker_overlay.is_none());
}
