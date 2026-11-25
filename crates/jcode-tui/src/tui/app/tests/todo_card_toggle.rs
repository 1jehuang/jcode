#[test]
fn pinned_todo_body_clicks_follow_long_history_without_toggling_other_rows() {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn click(app: &mut App, column: u16, row: u16) {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse_event(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
        }
    }

    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = crate::tui::ui::render_state_test_lock();
    let _pin = PinTodosEnvGuard::enable();
    let mut app = create_test_app();
    app.chat_native_scrollbar = false;
    app.input = "composer stays editable".to_string();
    app.cursor_pos = app.input.len();
    app.pinned_todos_payload = Some(
        serde_json::json!({"todos": (0..12)
        .map(|i| pinned_band_todo(&format!("t{i}"), &format!("body task {i}"), "pending"))
        .collect::<Vec<_>>()})
        .to_string(),
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 36)).unwrap();

    for lines in [100, 65_580] {
        app.display_messages = vec![DisplayMessage::assistant(format!(
            "```text\n{}```",
            "TRANSCRIPT_ROW\n".repeat(lines)
        ))];
        app.bump_display_messages_version();
        app.pinned_todos_expanded = false;
        let compact = render_and_snap(&app, &mut terminal);
        let body_y = compact
            .lines()
            .position(|row| row.contains("body task 0"))
            .expect("body row") as u16;
        let card = crate::tui::ui::viewport::pinned_todo_card_area().expect("expandable card");
        click(&mut app, card.x + 3, body_y);
        assert!(
            app.pinned_todos_expanded,
            "body click should expand: {compact}"
        );

        app.push_display_message(DisplayMessage::assistant("append while expanded"));
        let expanded = render_and_snap(&app, &mut terminal);
        assert!(app.pinned_todos_expanded);
        let card = crate::tui::ui::viewport::pinned_todo_card_area().expect("expanded card");
        let control = crate::tui::ui::viewport::pinned_todo_more_area().expect("collapse control");
        assert_eq!(
            control.bottom(),
            card.bottom(),
            "control belongs at the bottom"
        );
        assert!(
            expanded
                .lines()
                .nth(control.y as usize)
                .unwrap()
                .contains("Collapse todos")
        );
        let layout = crate::tui::ui::last_layout_snapshot().expect("layout");
        assert_eq!(card.intersection(layout.messages_area), card);
        let input = layout.input_area.expect("composer");
        click(&mut app, input.x + 2, input.y);
        assert!(app.pinned_todos_expanded, "composer click toggled todos");
        let transcript_y = expanded
            .lines()
            .position(|row| row.contains("TRANSCRIPT_ROW"))
            .expect("transcript row") as u16;
        click(&mut app, card.x + 3, transcript_y);
        assert!(app.pinned_todos_expanded, "transcript click toggled todos");
        let body_y = expanded
            .lines()
            .position(|row| row.contains("body task 0"))
            .expect("expanded body") as u16;
        click(&mut app, card.x + 3, body_y);
        assert!(!app.pinned_todos_expanded, "body click should collapse");
    }
    crate::tui::ui::viewport::set_pinned_todo_more_area_for_test(None);
    crate::tui::ui::viewport::publish_pinned_todo_scroll_for_test();
}

#[test]
fn hidden_todo_card_does_not_catch_composer_clicks_after_clear() {
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = crate::tui::ui::render_state_test_lock();
    let _pin = PinTodosEnvGuard::enable();
    let mut app = create_test_app();
    app.pinned_todos_payload = Some(
        serde_json::json!({"todos": (0..12)
        .map(|i| pinned_band_todo(&format!("t{i}"), &format!("Todo {i}"), "pending"))
        .collect::<Vec<_>>()})
        .to_string(),
    );
    app.display_messages = vec![DisplayMessage::assistant(
        "history before clear\n".repeat(30),
    )];
    app.bump_display_messages_version();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 25)).unwrap();
    render_and_snap(&app, &mut terminal);
    assert!(crate::tui::ui::viewport::pinned_todo_card_area().is_some());
    app.handle_key(KeyCode::Char('l'), KeyModifiers::CONTROL)
        .unwrap();
    app.input = "composer row\n".repeat(5);
    app.cursor_pos = app.input.len();
    let cleared = render_and_snap(&app, &mut terminal);
    assert!(app.terminal_clear_collapsed(), "{cleared}");
    assert!(
        crate::tui::ui::viewport::pinned_todo_card_area().is_none(),
        "hidden card has no target"
    );
    assert!(crate::tui::ui::viewport::pinned_todo_more_area().is_none());
    let input = crate::tui::ui::last_layout_snapshot()
        .unwrap()
        .input_area
        .unwrap();
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: input.x + 2,
        row: input.y + 1,
        modifiers: KeyModifiers::NONE,
    });
    assert!(
        !app.pinned_todos_expanded,
        "composer click toggled hidden todos"
    );
    app.scroll_up(1);
    render_and_snap(&app, &mut terminal);
    assert!(
        crate::tui::ui::viewport::pinned_todo_card_area().is_some(),
        "restoring history republishes the card"
    );
}
