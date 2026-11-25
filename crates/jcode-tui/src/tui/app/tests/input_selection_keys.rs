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

#[test]
fn selection_review_undo_replacement_paste_restores_the_original_payload() {
    let original = "one\ntwo\nthree\nfour\nfive";
    for replacement in ["a\nb\nc\nd\ne", "a\nb\nc\nd\ne\nf"] {
        let mut app = create_test_app();
        super::input::handle_text_paste(&mut app, original.into());
        let label = app.input.clone();
        app.set_input_selection(0, app.input.len());
        super::input::handle_text_paste(&mut app, replacement.into());
        assert_eq!(
            super::input::expand_paste_placeholders(&app, &app.input),
            replacement
        );
        app.undo_input_change();
        assert_eq!(app.input, label);
        assert_eq!(app.pasted_contents, vec![original]);
        let prepared = super::input::take_prepared_input(&mut app);
        assert_eq!(prepared.expanded, original);
    }
}

#[test]
fn selection_review_held_edge_ticks_extend_the_editable_range_and_highlight() {
    let _render_lock = scroll_render_test_lock();
    for upward in [false, true] {
        let mut app = create_test_app();
        app.input = (0..60)
            .map(|n| format!("line {n:02} 界"))
            .collect::<Vec<_>>()
            .join("\n");
        app.cursor_pos = if upward { app.input.len() } else { 0 };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 24)).unwrap();
        render_and_snap(&app, &mut terminal);
        let points = input_pane_screen_points(40, 24);
        let (start, edge) = if upward {
            (points.last().unwrap(), points.first().unwrap())
        } else {
            (points.first().unwrap(), points.last().unwrap())
        };
        let plain_bg = terminal.backend().buffer()[(start.0, start.1)].bg;
        for (kind, point) in [
            (MouseEventKind::Down(MouseButton::Left), start),
            (MouseEventKind::Drag(MouseButton::Left), edge),
        ] {
            app.handle_mouse_event(MouseEvent {
                kind,
                column: point.0,
                row: point.1,
                modifiers: KeyModifiers::NONE,
            });
        }
        render_and_snap(&app, &mut terminal);
        for _ in 0..4 {
            assert!(app.progress_copy_selection_edge_autoscroll());
            assert_eq!(
                app.input_selection_text(),
                app.current_copy_selection_text(),
                "upward={upward}"
            );
            render_and_snap(&app, &mut terminal);
            let endpoint = app.copy_selection_cursor.unwrap();
            let cell = input_pane_screen_points(40, 24)
                .into_iter()
                .find(|(_, _, point)| point.abs_line == endpoint.abs_line && point.column == 1)
                .unwrap();
            assert_eq!(terminal.backend().buffer()[(cell.0, cell.1)].symbol(), "i");
            assert_ne!(
                terminal.backend().buffer()[(cell.0, cell.1)].bg,
                plain_bg,
                "held edge row must be highlighted: upward={upward}"
            );
        }
    }
}
