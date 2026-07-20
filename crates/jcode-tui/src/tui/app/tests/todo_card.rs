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
        user_intention: Some("keep the plan state visible".to_string()),
        alignment_score: Some(95),
        hill_climbability: Some(score),
        objective: Some("readable card".to_string()),
        feedback_loop: Some("inspect the frame".to_string()),
        end_to_end_ownership: Some(90),
    };

    crate::todo::save_todos(&session_id, &todos).unwrap();
    crate::todo::save_goals(&session_id, &[goal(70)]).unwrap();
    app.toggle_todo_card();
    let card = app
        .display_messages
        .iter()
        .find(|message| message.role == "todos")
        .expect("todo card pushed");
    assert!(card.content.contains("\"hill_climbability\":70"));
    assert!(card.content.contains("\"alignment_score\":95"));

    crate::todo::save_goals(&session_id, &[goal(95)]).unwrap();
    assert!(app.refresh_todo_card_if_needed());
    let card = app
        .display_messages
        .iter()
        .find(|message| message.role == "todos")
        .expect("todo card still present");
    assert!(card.content.contains("\"hill_climbability\":95"));

    let _ = crate::todo::save_todos(&session_id, &[]);
    let _ = crate::todo::save_goals(&session_id, &[]);
}
