// Transcript selection across a resize (epic #1411 phase 5c).
//
// Selection endpoints are wrapped line indices, so a rewrap reinterprets them
// and the selection silently starts covering different text. Before the fix, a
// drag over TOKEN026..TOKEN028 at 100 columns copied a stray wrapped fragment
// plus TOKEN018 after resizing to 60.

fn selection_test_app() -> crate::tui::app::App {
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    app.display_messages = (0..40)
        .map(|i| DisplayMessage::assistant(format!("TOKEN{i:03} - {}", "filler ".repeat(8))))
        .collect();
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());
    app
}

/// Drag from the first to the third visible `TOKEN` line and return the copy.
fn drag_over_three_token_lines(
    app: &mut crate::tui::app::App,
    terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
) -> String {
    render_and_snap(app, terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .expect("alt+y enters copy mode");

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let mut token_lines = Vec::new();
    for abs in visible_start..visible_end {
        let text = crate::tui::ui::copy_viewport_line_text(abs).unwrap_or_default();
        if text.contains("TOKEN") {
            token_lines.push(abs);
        }
    }
    assert!(
        token_lines.len() >= 3,
        "need three visible token lines, saw {token_lines:?}"
    );
    let start_abs = token_lines[0];
    let end_abs = token_lines[2];
    let to_row = |abs: usize| layout.messages_area.y + (abs - visible_start) as u16;

    // Walk the rendered row with the same hit-test the mouse path uses, so the
    // drag lands on real cells (issue #430 pattern).
    let mut point_at = |abs: usize, row: u16| {
        (layout.messages_area.x..layout.messages_area.right())
            .filter_map(|column| {
                crate::tui::ui::copy_viewport_point_from_screen(column, row)
                    .filter(|point| point.abs_line == abs)
                    .map(|point| (column, point.column))
            })
            .collect::<Vec<_>>()
    };
    let start_cells = point_at(start_abs, to_row(start_abs));
    let end_cells = point_at(end_abs, to_row(end_abs));
    let start_x = start_cells
        .iter()
        .min_by_key(|(_, column)| *column)
        .map(|(column, _)| *column)
        .expect("start cell for the first token line");
    let end_x = end_cells
        .iter()
        .max_by_key(|(_, column)| *column)
        .map(|(column, _)| *column)
        .expect("end cell for the third token line");

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: start_x,
        row: to_row(start_abs),
        modifiers: KeyModifiers::empty(),
    });
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: end_x,
        row: to_row(end_abs),
        modifiers: KeyModifiers::empty(),
    });
    app.current_copy_selection_text().unwrap_or_default()
}

#[test]
fn transcript_selection_covers_the_same_text_after_a_resize() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();
    let mut app = selection_test_app();
    let mut wide = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();

    let before = drag_over_three_token_lines(&mut app, &mut wide);
    assert!(before.contains("TOKEN"), "fixture must select token text");

    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    assert!(app.should_redraw_after_resize());
    render_and_snap(&app, &mut narrow);
    assert!(app.rebase_selection_after_resize());

    render_and_snap(&app, &mut narrow);
    let after = app.current_copy_selection_text().unwrap_or_default();
    assert_eq!(
        after, before,
        "the selection must still cover the text the reader dragged over"
    );
}

#[test]
fn resize_without_a_selection_rebases_nothing() {
    let _lock = scroll_render_test_lock();
    let mut app = selection_test_app();
    let mut wide = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut wide);

    assert!(app.should_redraw_after_resize());
    assert!(app.pending_selection_rebase.is_none());
    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow);
    assert!(!app.rebase_selection_after_resize());
}

#[test]
fn non_transcript_selection_is_left_alone() {
    let _lock = scroll_render_test_lock();
    let mut app = selection_test_app();
    app.input = "select this draft".to_string();
    app.cursor_pos = app.input.len();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);

    let point = crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Input,
        abs_line: 0,
        column: 3,
    };
    app.copy_selection_anchor = Some(point);
    app.copy_selection_cursor = Some(point);

    assert!(app.should_redraw_after_resize());
    assert!(
        app.pending_selection_rebase.is_none(),
        "an input-pane selection is not transcript-relative"
    );
}
