// Tests for the inline chat todo card (`/todos` command + todo-card hotkey).

#[test]
fn toggle_todo_card_pushes_then_dismisses_trailing_card() {
    let mut app = create_test_app();
    assert!(!app.display_messages.iter().any(|m| m.role == "todos"));

    app.toggle_todo_card();
    assert_eq!(
        app.display_messages
            .iter()
            .filter(|m| m.role == "todos")
            .count(),
        1
    );
    assert_eq!(
        app.display_messages.last().map(|m| m.role.as_str()),
        Some("todos")
    );

    // Toggling again while the card is the trailing message dismisses it.
    app.toggle_todo_card();
    assert!(!app.display_messages.iter().any(|m| m.role == "todos"));
}

#[test]
fn toggle_todo_card_moves_stale_card_to_bottom_instead_of_stacking() {
    let mut app = create_test_app();
    app.toggle_todo_card();
    app.push_display_message(DisplayMessage::system("later activity".to_string()));

    // Card exists but is no longer trailing: toggling re-shows at the bottom.
    app.toggle_todo_card();
    let card_count = app
        .display_messages
        .iter()
        .filter(|m| m.role == "todos")
        .count();
    assert_eq!(card_count, 1, "the transcript keeps at most one todo card");
    assert_eq!(
        app.display_messages.last().map(|m| m.role.as_str()),
        Some("todos")
    );
}

#[test]
fn todos_command_defaults_to_card_and_panel_subcommand_keeps_side_panel() {
    let mut app = create_test_app();

    assert!(super::commands::handle_session_command(
        &mut app, "/todos"
    ));
    assert!(app.display_messages.iter().any(|m| m.role == "todos"));
    assert!(!app.todos_view_enabled());

    assert!(super::commands::handle_session_command(
        &mut app,
        "/todos panel"
    ));
    assert!(app.todos_view_enabled());

    assert!(super::commands::handle_session_command(
        &mut app,
        "/todos off"
    ));
    assert!(!app.todos_view_enabled());
}

#[test]
fn todo_alias_shows_card() {
    let mut app = create_test_app();
    assert!(super::commands::handle_session_command(
        &mut app, "/todo"
    ));
    assert!(app.display_messages.iter().any(|m| m.role == "todos"));
}

#[test]
fn refresh_todo_card_updates_content_when_todos_change() {
    let _env_lock = crate::storage::lock_test_env();
    let mut app = create_test_app();
    let session_id = app.session.id.clone();

    let todo = |content: &str, status: &str| crate::todo::TodoItem {
        id: "t1".to_string(),
        content: content.to_string(),
        status: status.to_string(),
        priority: "high".to_string(),
        group: None,
        confidence: Some(70),
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    };

    crate::todo::save_todos(&session_id, &[todo("write the card", "pending")]).unwrap();
    app.toggle_todo_card();
    let card = app
        .display_messages
        .iter()
        .find(|m| m.role == "todos")
        .expect("todo card pushed");
    assert!(card.content.contains("write the card"));
    assert!(card.content.contains("\"goals\""));

    // Unchanged todos: refresh is a no-op.
    assert!(!app.refresh_todo_card_if_needed());

    crate::todo::save_todos(&session_id, &[todo("write the card", "completed")]).unwrap();
    assert!(app.refresh_todo_card_if_needed());
    let card = app
        .display_messages
        .iter()
        .find(|m| m.role == "todos")
        .expect("todo card still present");
    assert!(card.content.contains("completed"));

    // Cleanup the persisted todo file for this throwaway session.
    let _ = crate::todo::save_todos(&session_id, &[]);
}

#[test]
fn refresh_todo_card_updates_content_when_goal_scores_change() {
    let _env_lock = crate::storage::lock_test_env();
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    let todos = [crate::todo::TodoItem {
        id: "t1".to_string(),
        content: "render scores".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: None,
        confidence: Some(80),
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    }];
    let goal = |score| crate::todo::TodoGoal {
        group: None,
        closed_feedback_loop: Some(score),
        feedback_loop: Some("inspect the frame".to_string()),
        end_to_end_ownership: Some(90),
        ..Default::default()
    };

    let plan = crate::todo::TodoPlan {
        user_intention: Some("keep the plan state visible".to_string()),
        understands_user_intent: Some(95),
        ..Default::default()
    };

    crate::todo::save_todos(&session_id, &todos).unwrap();
    crate::todo::save_goals(&session_id, &[goal(70)]).unwrap();
    crate::todo::save_plan(&session_id, &plan).unwrap();
    app.toggle_todo_card();
    let card = app
        .display_messages
        .iter()
        .find(|message| message.role == "todos")
        .expect("todo card pushed");
    assert!(card.content.contains("\"closed_feedback_loop\":70"));
    assert!(card.content.contains("\"understands_user_intent\":95"));

    crate::todo::save_goals(&session_id, &[goal(95)]).unwrap();
    assert!(app.refresh_todo_card_if_needed());
    let card = app
        .display_messages
        .iter()
        .find(|message| message.role == "todos")
        .expect("todo card still present");
    assert!(card.content.contains("\"closed_feedback_loop\":95"));

    let _ = crate::todo::save_todos(&session_id, &[]);
    let _ = crate::todo::save_goals(&session_id, &[]);
    let _ = crate::todo::save_plan(&session_id, &crate::todo::TodoPlan::default());
}

/// Simple todo used by the pinned-band tests.
fn pinned_band_todo(id: &str, content: &str, status: &str) -> crate::todo::TodoItem {
    crate::todo::TodoItem {
        id: id.to_string(),
        content: content.to_string(),
        status: status.to_string(),
        priority: "high".to_string(),
        group: None,
        confidence: Some(80),
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    }
}

/// RAII guard for the JCODE_PIN_TODOS env override used by the band tests.
struct PinTodosEnvGuard;

impl PinTodosEnvGuard {
    fn enable() -> Self {
        crate::env::set_var("JCODE_PIN_TODOS", "1");
        // jcode-base's config cache throttles env re-checks (the zero
        // interval under cfg!(test) applies only when jcode-base itself is
        // the crate under test), so force a reload or a sibling test's
        // JCODE_PIN_TODOS state leaks into this one for up to 500ms.
        crate::config::invalidate_config_cache();
        Self
    }
}

impl Drop for PinTodosEnvGuard {
    fn drop(&mut self) {
        crate::env::remove_var("JCODE_PIN_TODOS");
        // See enable(): flush the removal too, so later tests that expect
        // pin_todos off do not observe this test's stale cached config.
        crate::config::invalidate_config_cache();
    }
}

#[test]
fn pinned_todos_payload_stays_empty_when_config_off() {
    let _env_lock = crate::storage::lock_test_env();
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    crate::todo::save_todos(&session_id, &[pinned_band_todo("t1", "pin me", "pending")]).unwrap();

    // display.pin_todos defaults to false: no payload, no redraw churn.
    assert!(!app.refresh_pinned_todos_if_needed());
    assert!(app.pinned_todos_payload_ref().is_none());

    let _ = crate::todo::save_todos(&session_id, &[]);
}

#[test]
fn pinned_todos_payload_refreshes_and_clears_with_config_and_todos() {
    let _env_lock = crate::storage::lock_test_env();
    let _pin = PinTodosEnvGuard::enable();
    let mut app = create_test_app();
    let session_id = app.session.id.clone();

    // No todos yet: enabled but nothing to pin.
    app.refresh_pinned_todos_now();
    assert!(app.pinned_todos_payload_ref().is_none());

    crate::todo::save_todos(&session_id, &[pinned_band_todo("t1", "pin me", "pending")]).unwrap();
    app.refresh_pinned_todos_now();
    let payload = app
        .pinned_todos_payload_ref()
        .expect("payload populated when enabled with todos");
    assert!(payload.contains("pin me"));

    // Unchanged todos within the throttle window: no redraw.
    assert!(!app.refresh_pinned_todos_if_needed());

    // Todos cleared: payload clears too.
    crate::todo::save_todos(&session_id, &[]).unwrap();
    app.refresh_pinned_todos_now();
    assert!(app.pinned_todos_payload_ref().is_none());
}

#[test]
fn pinned_todo_band_renders_at_top_when_scrolled() {
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = crate::tui::ui::render_state_test_lock();
    let _pin = PinTodosEnvGuard::enable();
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    crate::todo::save_todos(
        &session_id,
        &[pinned_band_todo("t1", "pinned band item", "in_progress")],
    )
    .unwrap();
    app.refresh_pinned_todos_now();
    assert!(app.pinned_todos_payload_ref().is_some());

    app.display_messages = vec![
        DisplayMessage {
            role: "user".to_string(),
            content: "kick off the work".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
        DisplayMessage {
            role: "assistant".to_string(),
            content: App::build_scroll_test_content(0, 40, None),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(60, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let text = render_and_snap(&app, &mut terminal);

    let first_rows = text.lines().take(4).collect::<Vec<_>>().join(" ");
    assert!(
        first_rows.contains("pinned band item"),
        "pinned todo band should render at the top of the scrolled viewport, got:\n{}",
        text
    );

    let _ = crate::todo::save_todos(&session_id, &[]);
}
