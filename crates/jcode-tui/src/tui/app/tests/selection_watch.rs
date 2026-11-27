#[test]
fn selection_watch_delete_or_replace_then_paste_sends_current_payloads() {
    let a = "A1\nA2\nA3\nA4\nA5";
    let b = "B1\nB2\nB3\nB4\nB5";
    let c = "C1\nC2\nC3\nC4\nC5";
    for replacement in [false, true] {
        for key in [KeyCode::Backspace, KeyCode::Delete] {
            let mut app = create_test_app();
            super::input::handle_text_paste(&mut app, a.into());
            super::input::insert_input_text(&mut app, " | ");
            let start = app.cursor_pos;
            super::input::handle_text_paste(&mut app, b.into());
            let original = app.input.clone();
            app.set_input_selection(start, app.input.len());
            app.handle_key(
                if replacement { KeyCode::Char('X') } else { key },
                KeyModifiers::NONE,
            )
            .unwrap();
            super::input::handle_text_paste(&mut app, c.into());
            assert_eq!(
                super::input::expand_paste_placeholders(&app, &app.input),
                format!("{a} | {}{c}", if replacement { "X" } else { "" })
            );
            app.undo_input_change();
            app.undo_input_change();
            assert_eq!(app.input, original);
            let prepared = super::input::take_prepared_input(&mut app);
            assert_eq!(prepared.expanded, format!("{a} | {b}"));
        }
    }
}

#[test]
fn selection_watch_copy_mode_multi_click_copies_without_editable_selection() {
    let _render_lock = scroll_render_test_lock();
    let rt = tokio::runtime::Runtime::new().unwrap();
    for mode in 0..3 {
        for clicks in [2, 3] {
            let clipboard = CapturedClipboard::new();
            let input = "hello 界alpha world";
            let (mut app, mut terminal) = rendered_composer_app(input);
            app.enter_copy_selection_mode();
            let caret = app.cursor_pos;
            render_and_snap(&app, &mut terminal);
            let cell = composer_cell_for(&app, 80, 24, 9);
            let plain_bg = terminal.backend().buffer()[cell].bg;
            for _ in 0..clicks {
                left_click(&mut app, cell);
            }
            render_and_snap(&app, &mut terminal);
            assert_ne!(terminal.backend().buffer()[cell].bg, plain_bg);
            let expected = if clicks == 2 { "界alpha" } else { input };
            assert_eq!(app.current_copy_selection_text().as_deref(), Some(expected));
            assert_eq!(app.input_selection(), None);
            assert_eq!(app.cursor_pos, caret);
            assert!(app.copy_selection_mode);
            // Copy mode must absorb editing keys without deleting the highlight.
            app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
                .unwrap();
            assert_eq!(app.input, input);
            assert_eq!(app.current_copy_selection_text().as_deref(), Some(expected));
            rt.block_on(async {
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                match mode {
                    0 => app
                        .handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
                        .unwrap(),
                    1 => app
                        .handle_remote_key(KeyCode::Char('c'), KeyModifiers::CONTROL, &mut remote)
                        .await
                        .unwrap(),
                    _ => super::remote::handle_disconnected_key(
                        &mut app,
                        KeyCode::Char('c'),
                        KeyModifiers::CONTROL,
                    )
                    .unwrap(),
                }
            });
            assert_eq!(clipboard.text().as_deref(), Some(expected), "mode={mode}");
            assert_eq!(app.input, input);
            assert!(!app.copy_selection_mode);
            assert!(app.quit_pending.is_none());
        }
    }
}

#[test]
fn selection_watch_copy_mode_word_across_soft_wrap_keeps_raw_range() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let word = "界alpha".repeat(16);
    let input = format!("first line\nhello {word} tail");
    let (mut app, mut terminal) = rendered_composer_app(&input);
    app.enter_copy_selection_mode();
    render_and_snap(&app, &mut terminal);
    let (column, row, _) = input_pane_screen_points(80, 24)
        .into_iter()
        .find(|(_, _, point)| point.abs_line == 2 && point.column == 5)
        .unwrap();
    for _ in 0..2 {
        left_click(&mut app, (column, row));
    }
    assert_eq!(
        app.current_copy_selection_text().as_deref(),
        Some(word.as_str())
    );
    assert_eq!(app.input_selection(), None);
    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(clipboard.text().as_deref(), Some(word.as_str()));
    assert_eq!(app.input, input);
}
