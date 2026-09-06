#[test]
fn first_prompt_manual_navigation_uses_visible_start_not_tail() {
    let _render_lock = scroll_render_test_lock();
    for action in ["up", "down", "pause", "end"] {
        let (mut app, mut terminal) = create_scroll_test_app(80, 24, 0, 0);
        app.display_messages.clear();
        app.bump_display_messages_version();
        render_and_snap(&app, &mut terminal);
        let initial_y = crate::tui::ui::last_layout_snapshot()
            .unwrap()
            .input_area
            .unwrap()
            .y;
        app.display_messages.push(DisplayMessage::user(format!(
            "FIRST LINE\n{}\nLAST LINE",
            "middle line\n".repeat(60)
        )));
        app.bump_display_messages_version();
        app.is_processing = true;
        app.status = ProcessingStatus::Thinking(Instant::now());
        // Submission resumes tail following, but the initial preview must still
        // start at the beginning of this over-height prompt.
        app.follow_chat_bottom();
        render_and_snap(&app, &mut terminal);
        let start = crate::tui::ui::last_resolved_chat_scroll();
        let max = crate::tui::ui::last_max_scroll();
        assert!(start < max);
        assert!(
            crate::tui::ui::last_layout_snapshot()
                .unwrap()
                .input_area
                .unwrap()
                .y
                >= initial_y
        );
        match action {
            "up" => {
                app.scroll_up(2);
            }
            "down" => {
                assert!(app.scroll_down(2));
            }
            "pause" => app.pause_chat_auto_scroll(),
            "end" => app.follow_chat_bottom(),
            _ => unreachable!(),
        }
        for _ in 0..3 {
            render_and_snap(&app, &mut terminal);
            let expected = match action {
                "up" => start.saturating_sub(2),
                "down" => start + 2,
                "pause" => start,
                "end" => max,
                _ => unreachable!(),
            };
            assert_eq!(
                crate::tui::ui::last_resolved_chat_scroll(),
                expected,
                "{action}"
            );
        }
    }
}

#[test]
fn first_prompt_composer_floor_is_released_by_terminal_clear() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 50, 0, 0);
    app.display_messages.clear();
    app.bump_display_messages_version();
    render_and_snap(&app, &mut terminal);
    let initial_y = crate::tui::ui::last_layout_snapshot()
        .unwrap()
        .input_area
        .unwrap()
        .y;
    app.display_messages
        .push(DisplayMessage::user("hello"));
    app.bump_display_messages_version();
    render_and_snap(&app, &mut terminal);
    assert_eq!(
        crate::tui::ui::last_layout_snapshot()
            .unwrap()
            .input_area
            .unwrap()
            .y,
        initial_y
    );
    app.handle_key(KeyCode::Char('l'), KeyModifiers::CONTROL)
        .unwrap();
    render_and_snap(&app, &mut terminal);
    assert!(
        crate::tui::ui::last_layout_snapshot()
            .unwrap()
            .input_area
            .unwrap()
            .y
            < initial_y
    );
    // Returning to a short transcript after a terminal clear must not restore
    // the welcome screen's old composer floor.
    app.display_messages = vec![DisplayMessage::user("after clear")];
    app.bump_display_messages_version();
    render_and_snap(&app, &mut terminal);
    assert!(
        crate::tui::ui::last_layout_snapshot()
            .unwrap()
            .input_area
            .unwrap()
            .y
            < initial_y
    );
}
