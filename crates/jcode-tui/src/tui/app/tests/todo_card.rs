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

    assert!(super::commands::handle_session_command(&mut app, "/todos"));
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
    assert!(super::commands::handle_session_command(&mut app, "/todo"));
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
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(70)),
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
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(80)),
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    }];
    let goal = |score| crate::todo::TodoGoal {
        group: None,
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(score)),
        feedback_loop: Some("inspect the frame".to_string()),
        delivery_state: Some(crate::todo::DeliveryState::from_legacy_score(90)),
        ..Default::default()
    };

    let plan = crate::todo::TodoPlan {
        user_intention: Some("keep the plan state visible".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::from_legacy_score(95)),
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
    assert!(card.content.contains("\"closed_feedback_loop\":\"usable\""));
    assert!(
        card.content
            .contains("\"understands_user_intent\":\"partial\"")
    );

    crate::todo::save_goals(&session_id, &[goal(95)]).unwrap();
    assert!(app.refresh_todo_card_if_needed());
    let card = app
        .display_messages
        .iter()
        .find(|message| message.role == "todos")
        .expect("todo card still present");
    assert!(card.content.contains("\"closed_feedback_loop\":\"strong\""));

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
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(80)),
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

    fn disable() -> Self {
        crate::env::set_var("JCODE_PIN_TODOS", "0");
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
    let _pin_guard = PinTodosEnvGuard::disable();
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
fn pinned_todos_are_omitted_from_info_widgets() {
    let _env_lock = crate::storage::lock_test_env();
    let _pin = PinTodosEnvGuard::enable();
    let app = create_test_app();
    let session_id = app.session.id.clone();
    crate::todo::save_todos(
        &session_id,
        &[pinned_band_todo("t1", "only in pinned band", "pending")],
    )
    .unwrap();

    let info = app.info_widget_data();
    assert!(info.todos.is_empty());
    assert!(info.todo_goals.is_empty());
    assert!(!info.has_data_for(crate::tui::info_widget::WidgetKind::Todos));

    crate::todo::save_todos(&session_id, &[]).unwrap();
}

#[test]
fn pinned_todos_hide_todo_tool_messages_from_the_transcript() {
    let _env_lock = crate::storage::lock_test_env();
    let _pin = PinTodosEnvGuard::enable();
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    crate::todo::save_todos(
        &session_id,
        &[pinned_band_todo("pinned", "PINNED_ONLY", "in_progress")],
    )
    .unwrap();
    app.refresh_pinned_todos_now();
    app.display_messages = vec![
        DisplayMessage::tool(
            "duplicate todo transcript card",
            crate::message::ToolCall {
                id: "todo-tool".to_string(),
                name: "todo".to_string(),
                input: serde_json::json!({"todos": []}),
                intent: None,
                thought_signature: None,
            },
        ),
        DisplayMessage::tool(
            "ordinary tool remains visible",
            crate::message::ToolCall {
                id: "read-tool".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file_path": "README.md"}),
                intent: None,
                thought_signature: None,
            },
        ),
    ];
    app.bump_display_messages_version();
    app.session.short_name = Some("test".to_string());
    let backend = ratatui::backend::TestBackend::new(80, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let transcript = render_and_snap(&app, &mut terminal);
    assert!(!transcript.contains("duplicate todo transcript card"));
    assert!(transcript.contains("PINNED_ONLY"), "{transcript}");
    let _ = crate::todo::save_todos(&session_id, &[]);
}

#[test]
fn pinned_todo_band_renders_below_sticky_prompt_without_separator() {
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

    app.auto_scroll_paused = true;
    let top_text = render_and_snap(&app, &mut terminal);
    assert!(
        top_text.lines().take(6).any(|row| row.contains("pinned band item")),
        "pinned todo should remain visible at the top of scrollback, got:\n{}",
        top_text
    );

    app.auto_scroll_paused = false;
    let text = render_and_snap(&app, &mut terminal);

    let first_rows = text.lines().take(6).collect::<Vec<_>>();
    let prompt_row = first_rows
        .iter()
        .position(|row| row.contains("kick off the work"))
        .expect("sticky prompt should be visible");
    let todo_row = first_rows
        .iter()
        .position(|row| row.contains("pinned band item"))
        .expect("pinned todo should be visible");
    assert!(
        prompt_row < todo_row,
        "pinned todo band should render below the sticky prompt, got:\n{}",
        text
    );
    assert!(
        !first_rows.iter().any(|row| row.contains("────")),
        "pinned todo band should not render a horizontal separator, got:\n{}",
        text
    );

    let _ = crate::todo::save_todos(&session_id, &[]);
}

#[test]
fn background_task_rows_render_without_todos_or_transcript_cards() {
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = crate::tui::ui::render_state_test_lock();
    let mut app = create_test_app();
    app.session.short_name = Some("test".to_string());
    app.push_display_message(DisplayMessage::assistant("ordinary transcript content"));
    app.upsert_running_background_task(
        "running".to_string(),
        "cargo test".to_string(),
        Some(42.0),
    );
    app.finish_background_task(
        "done".to_string(),
        "release build".to_string(),
        crate::tui::BackgroundTaskRowStatus::Completed,
    );
    app.finish_background_task(
        "failed".to_string(),
        "integration tests".to_string(),
        crate::tui::BackgroundTaskRowStatus::Failed,
    );

    let backend = ratatui::backend::TestBackend::new(80, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let rendered = render_and_snap(&app, &mut terminal);

    assert!(
        rendered.contains("✓ bg release build  ━━━━━━ 100%"),
        "missing completed task row:\n{rendered}"
    );
    assert!(
        rendered.contains("× bg integration tests  ────── failed"),
        "missing failed task row:\n{rendered}"
    );
    assert!(
        !rendered.contains("◌ bg cargo test"),
        "only the two most recent task rows should render:\n{rendered}"
    );
    assert!(!rendered.contains("Background tasks"));
    assert!(!rendered.contains("Background task started"));
    assert!(!rendered.contains("Background task progress"));
    assert!(!rendered.contains("Background task completed"));
}

#[test]
fn background_task_rows_retain_the_two_most_recently_active_tasks() {
    let mut app = create_test_app();
    app.upsert_running_background_task("first".to_string(), "first task".to_string(), None);
    app.upsert_running_background_task("second".to_string(), "second task".to_string(), None);
    app.upsert_running_background_task(
        "first".to_string(),
        "first task updated".to_string(),
        Some(50.0),
    );
    app.upsert_running_background_task("third".to_string(), "third task".to_string(), None);

    assert_eq!(
        app.background_task_rows_ref()
            .iter()
            .map(|row| row.task_id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "third"]
    );
}

#[test]
fn indeterminate_background_update_preserves_last_known_percent() {
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = crate::tui::ui::render_state_test_lock();
    let mut app = create_test_app();
    app.session.short_name = Some("test".to_string());
    app.push_display_message(DisplayMessage::assistant("ordinary transcript content"));
    app.upsert_running_background_task(
        "build".to_string(),
        "cargo build".to_string(),
        Some(42.0),
    );

    // A later phase-only parser update carries no percentage. It should update
    // activity/label state without resetting the visible bar to zero.
    app.upsert_running_background_task("build".to_string(), "Compiling jcode".to_string(), None);

    let row = app
        .background_task_rows_ref()
        .iter()
        .find(|row| row.task_id == "build")
        .expect("background row should remain present");
    assert_eq!(row.percent, Some(42.0));
    assert_eq!(row.label, "Compiling jcode");

    let backend = ratatui::backend::TestBackend::new(80, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let rendered = render_and_snap(&app, &mut terminal);
    assert!(
        rendered.contains("Compiling jcode") && rendered.contains("42%"),
        "phase-only update reset or hid the visible percentage:\n{rendered}"
    );
}

#[test]
fn completed_background_tasks_clear_after_they_stop_being_relevant() {
    let mut app = create_test_app();
    app.finish_background_task(
        "failed".to_string(),
        "integration tests".to_string(),
        crate::tui::BackgroundTaskRowStatus::Failed,
    );
    app.upsert_running_background_task("running".to_string(), "cargo test".to_string(), None);
    app.finish_background_task(
        "done".to_string(),
        "release build".to_string(),
        crate::tui::BackgroundTaskRowStatus::Completed,
    );

    app.background_task_rows
        .iter_mut()
        .find(|row| row.task_id == "done")
        .unwrap()
        .completed_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(13));

    assert!(app.prune_irrelevant_background_tasks());
    assert_eq!(
        app.background_task_rows_ref()
            .iter()
            .map(|row| row.task_id.as_str())
            .collect::<Vec<_>>(),
        vec!["running"]
    );
    assert!(!app.prune_irrelevant_background_tasks());
}

#[test]
fn clicking_pinned_todo_more_row_expands_the_band() {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let mut app = create_test_app();
    app.pinned_todos_expanded = false;
    crate::tui::ui::viewport::set_pinned_todo_more_area_for_test(Some(ratatui::layout::Rect {
        x: 2,
        y: 4,
        width: 20,
        height: 1,
    }));

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 8,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });

    assert!(app.pinned_todos_expanded);
}
