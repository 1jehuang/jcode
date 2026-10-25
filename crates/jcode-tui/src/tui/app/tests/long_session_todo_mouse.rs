#[test]
fn pinned_todo_click_targets_follow_long_history_growth() {
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
    let todos: Vec<_> = (0..12)
        .map(|i| {
            pinned_band_todo(
                &format!("t{i}"),
                &format!("long history task {i}"),
                "pending",
            )
        })
        .collect();
    let (mut app, _) = render_pinned_band_fixture(&todos, 206, 52);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(206, 52)).unwrap();

    // Grow the same App past the 16-bit document-row boundary, then keep
    // appending while expanded. Hit targets must stay in screen coordinates.
    for lines in [100, 65_580] {
        app.display_messages[1].content = "long history transcript row\n".repeat(lines);
        app.bump_display_messages_version();
        app.auto_scroll_paused = false;
        app.scroll_offset = usize::MAX;
        crate::tui::ui::request_tail_follow_snap();
        let compact = render_and_snap(&app, &mut terminal);
        let control = crate::tui::ui::viewport::pinned_todo_more_area().expect("expand target");
        let row = compact
            .lines()
            .nth(usize::from(control.y))
            .expect("visible control row");
        assert!(row.contains("more (todo)"), "{control:?}: {row}");
        click(&mut app, control.x, control.y);
        assert!(app.pinned_todos_expanded);

        app.push_display_message(DisplayMessage::assistant("append while todos are expanded"));
        crate::tui::ui::request_tail_follow_snap();
        let expanded = render_and_snap(&app, &mut terminal);
        assert!(
            app.pinned_todos_expanded,
            "stream growth must not collapse todos"
        );
        assert!(expanded.contains("long history task 11"), "{expanded}");
        let control = crate::tui::ui::viewport::pinned_todo_more_area().expect("collapse target");
        let row = expanded
            .lines()
            .nth(usize::from(control.y))
            .expect("visible collapse row");
        assert!(row.contains("collapse (todo)"), "{control:?}: {row}");
        let layout = crate::tui::ui::last_layout_snapshot().expect("layout");
        let card = crate::tui::ui::viewport::pinned_todo_card_area().expect("card target");
        assert_eq!(card.intersection(layout.messages_area), card);
        let input = layout.input_area.expect("input area");
        assert!(input.width > 2 && input.height > 0, "{input:?}");
        click(&mut app, input.x + 2, input.y + input.height / 2);
        assert!(
            app.pinned_todos_expanded,
            "input click must not toggle todos"
        );
        click(&mut app, control.x, control.y);
        assert!(!app.pinned_todos_expanded);
    }
    crate::tui::ui::viewport::set_pinned_todo_more_area_for_test(None);
    crate::tui::ui::viewport::set_pinned_todo_card_area_for_test(None);
}
