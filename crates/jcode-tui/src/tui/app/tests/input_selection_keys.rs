#[test]
fn editable_composer_shift_home_then_delete_removes_selected_text() {
    let mut app = create_test_app();
    app.input = "alpha beta".into();
    app.cursor_pos = app.input.len();
    app.handle_key(KeyCode::Home, KeyModifiers::SHIFT).unwrap();
    app.handle_key(KeyCode::Delete, KeyModifiers::NONE).unwrap();
    assert!(app.input.is_empty(), "Shift+Home selection was not deleted");
    app.undo_input_change();
    assert_eq!(app.input, "alpha beta");
}

#[test]
fn editable_composer_shift_left_then_typing_replaces_selection_once() {
    let mut app = create_test_app();
    app.input = "alpha beta".into();
    app.cursor_pos = app.input.len();
    app.handle_key(KeyCode::Left, KeyModifiers::SHIFT).unwrap();
    app.handle_key(KeyCode::Left, KeyModifiers::SHIFT).unwrap();
    app.handle_key(KeyCode::Char('X'), KeyModifiers::SHIFT)
        .unwrap();
    assert_eq!(app.input, "alpha beX");
    app.undo_input_change();
    assert_eq!(app.input, "alpha beta");
}

#[test]
fn editable_composer_double_click_then_cut_uses_the_word_under_the_pointer() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let mut app = create_test_app();
    app.input = "hello big world".into();
    app.cursor_pos = app.input.len();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    render_and_snap(&app, &mut terminal);
    let (column, row, _) = input_pane_screen_points(80, 24)
        .into_iter()
        .find(|(_, _, point)| point.abs_line == 0 && point.column == 7)
        .unwrap();
    for _ in 0..2 {
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
    app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input, "hello  world");
    assert_eq!(clipboard.text().as_deref(), Some("big"));
}

#[test]
fn editable_composer_disconnected_keys_keep_selection_editable() {
    let mut app = create_test_app();
    app.input = "offline draft".into();
    app.cursor_pos = app.input.len();
    super::remote::handle_disconnected_key(&mut app, KeyCode::Home, KeyModifiers::SHIFT).unwrap();
    super::remote::handle_disconnected_key(&mut app, KeyCode::Delete, KeyModifiers::NONE).unwrap();
    assert!(app.input.is_empty());
    app.undo_input_change();
    assert_eq!(app.input, "offline draft");
}

#[test]
fn editable_composer_release_endpoint_stays_editable_after_copy_failure() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = rendered_composer_app("select this draft");
    let start = composer_cell_for(&app, 80, 24, 7);
    let drag = composer_cell_for(&app, 80, 24, 9);
    let release = composer_cell_for(&app, 80, 24, 11);
    for (kind, cell) in [
        (MouseEventKind::Down(MouseButton::Left), start),
        (MouseEventKind::Drag(MouseButton::Left), drag),
    ] {
        app.handle_mouse_event(MouseEvent {
            kind,
            column: cell.0,
            row: cell.1,
            modifiers: KeyModifiers::NONE,
        });
    }
    assert_eq!(app.input_selection_text().as_deref(), Some("th"));
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: release.0,
            row: release.1,
            modifiers: KeyModifiers::NONE,
        },
        |text| {
            assert_eq!(text, "this");
            false
        },
    );
    assert_eq!(app.input_selection_text().as_deref(), Some("this"));
    app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
        .unwrap();
    assert_eq!(app.input, "select  draft");
    app.undo_input_change();
    assert_eq!(app.input, "select this draft");
}
