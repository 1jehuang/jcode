// Tests for copy-selection in the prompt composer (input box), issue #430:
// text being typed must be drag-selectable and copyable with the mouse, just
// like the transcript, without ever copying the prompt decoration.

/// Exercise the real draw path, not a hand-installed viewport snapshot. A
/// transcript is a usize-sized document even though the terminal uses u16 cells.
#[test]
fn test_long_transcript_u16_boundaries_keep_repeated_copy_working() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let mut app = create_test_app();
    crate::tui::info_widget::set_dock_disabled_for_test(true);
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    app.input = "composer still copies".to_string();
    app.cursor_pos = app.input.len();
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).expect("test terminal");

    // Measure the frame's header/spacer overhead once so these are actual
    // wrapped-line boundaries, rather than approximately 65K source lines.
    app.display_messages = vec![DisplayMessage::assistant(format!(
        "```text\n{}```",
        "copy boundary\n".repeat(8)
    ))];
    app.bump_display_messages_version();
    render_and_snap(&app, &mut terminal);
    let overhead = crate::tui::ui::copy_viewport_line_count().expect("chat snapshot") - 8;

    for target in [65_534usize, 65_535, 65_536, 65_537] {
        app.display_messages = vec![DisplayMessage::assistant(format!(
            "```text\n{}```",
            "copy boundary\n".repeat(target - overhead)
        ))];
        app.bump_display_messages_version();
        // Both enabled hysteresis branches and the disabled-scrollbar path
        // must finish drawing and publish copy geometry on consecutive frames.
        for scrollbar in [false, true, true, false] {
            app.chat_native_scrollbar = scrollbar;
            render_and_snap(&app, &mut terminal);
            assert_eq!(crate::tui::ui::copy_viewport_line_count(), Some(target));
            assert_eq!(
                crate::tui::ui::interactive_state::chat_scrollbar_visible_for_tests(),
                scrollbar
            );
            assert!(
                crate::tui::ui::copy_viewport_visible_range()
                    .is_some_and(|(start, end)| start < end && end <= target)
            );

            for key in [KeyCode::Enter, KeyCode::Char('y')] {
                app.enter_copy_selection_mode();
                assert!(app.select_chat_viewport_context());
                let expected = app
                    .current_copy_selection_text()
                    .expect("visible selection");
                assert!(expected.contains("copy boundary"));
                assert!(app.handle_copy_selection_key(key, KeyModifiers::empty()));
                assert_eq!(clipboard.text().as_deref(), Some(expected.as_str()));
                assert!(!app.copy_selection_mode);
                assert!(app.copy_selection_anchor.is_none());
            }

            // Mouse release must keep working after keyboard mode has cleared
            // its selection, including composer geometry late in a long session.
            let points = input_pane_screen_points(80, 24);
            let start = points
                .iter()
                .find(|(_, _, p)| p.column == 0)
                .expect("input start");
            let end = points
                .iter()
                .max_by_key(|(_, _, p)| p.column)
                .expect("input end");
            assert_eq!(
                drag_copy(&mut app, (start.0, start.1), (end.0, end.1)),
                app.input
            );
            assert!(!app.copy_selection_dragging);
            assert!(app.copy_selection_pending_anchor.is_none());
        }
    }
}

#[test]
fn test_offscreen_benchmark_preserves_live_copy_geometry_without_redraw() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "live composer".to_string();
    app.cursor_pos = app.input.len();
    app.display_messages = vec![DisplayMessage::assistant("live transcript\n".repeat(80))];
    app.bump_display_messages_version();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    render_and_snap(&app, &mut terminal);
    let layout = format!("{:?}", crate::tui::ui::last_layout_snapshot());
    let frame = crate::tui::ui::last_chat_frame().unwrap();
    let lines = crate::tui::ui::copy_viewport_line_count();
    let scroll = (
        crate::tui::ui::last_max_scroll(),
        crate::tui::ui::last_total_wrapped_lines(),
    );
    let points = input_pane_screen_points(80, 24);
    let start = points.iter().find(|(_, _, p)| p.column == 0).unwrap();
    let end = points.iter().max_by_key(|(_, _, p)| p.column).unwrap();

    let result = app.handle_debug_command(
        r#"side-panel-latency:{"iterations":8,"warmup_iterations":2,"include_samples":false}"#,
    );
    let value: serde_json::Value =
        serde_json::from_str(&result).unwrap_or_else(|e| panic!("benchmark JSON: {e}: {result}"));
    assert_eq!(value["ok"], true);
    // No live draw here: queued input must still use the original terminal.
    assert_eq!(
        format!("{:?}", crate::tui::ui::last_layout_snapshot()),
        layout
    );
    assert!(std::sync::Arc::ptr_eq(
        &frame,
        &crate::tui::ui::last_chat_frame().unwrap()
    ));
    assert_eq!(crate::tui::ui::copy_viewport_line_count(), lines);
    assert_eq!(
        (
            crate::tui::ui::last_max_scroll(),
            crate::tui::ui::last_total_wrapped_lines()
        ),
        scroll
    );
    assert_eq!(
        drag_copy(&mut app, (start.0, start.1), (end.0, end.1)),
        "live composer"
    );

    let recovered = std::panic::catch_unwind(|| {
        let _saved = crate::tui::ui::interactive_state::preserve_interactive_render_state();
        crate::tui::ui::clear_copy_viewport_snapshot();
        panic!("offscreen diagnostic failed");
    });
    assert!(recovered.is_err());
    assert_eq!(crate::tui::ui::copy_viewport_line_count(), lines);
    assert_eq!(
        crate::tui::ui::input_pane_line_text(0).as_deref(),
        Some("live composer")
    );
}

/// Scan the rendered frame for screen cells that hit-test into the composer
/// (`Input`) pane, returning `(col, row, point)` triples.
fn input_pane_screen_points(
    width: u16,
    height: u16,
) -> Vec<(u16, u16, crate::tui::CopySelectionPoint)> {
    let mut points = Vec::new();
    for row in 0..height {
        for col in 0..width {
            if let Some(point) = crate::tui::ui::copy_point_from_screen(col, row)
                && point.pane == crate::tui::CopySelectionPane::Input
            {
                points.push((col, row, point));
            }
        }
    }
    points
}

fn drag_copy(app: &mut App, start: (u16, u16), end: (u16, u16)) -> String {
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: start.0,
            row: start.1,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: end.0,
            row: end.1,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: end.0,
            row: end.1,
            modifiers: KeyModifiers::empty(),
        },
        |text| {
            *copied_for_closure.lock().unwrap() = text.to_string();
            true
        },
    );

    copied.lock().unwrap().clone()
}

#[test]
fn test_copy_first_drag_outside_composer_still_copies() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "coalesced drag draft".to_string();
    app.cursor_pos = app.input.len();
    app.display_messages = vec![DisplayMessage::assistant("transcript stays separate")];
    app.bump_display_messages_version();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    render_and_snap(&app, &mut terminal);

    let points = input_pane_screen_points(80, 24);
    let start = points.iter().find(|(_, _, p)| p.column == 0).unwrap();
    let end = points.iter().max_by_key(|(_, _, p)| p.column).unwrap();
    let below_input = (
        start.0,
        points.iter().map(|(_, row, _)| *row).max().unwrap() + 1,
    );
    assert!(below_input.1 < 24);
    let chat = (0..24u16)
        .flat_map(|row| (0..80u16).map(move |col| (col, row)))
        .find(|(col, row)| {
            crate::tui::ui::copy_point_from_screen(*col, *row)
                .is_some_and(|p| p.pane == crate::tui::CopySelectionPane::Chat)
        })
        .expect("chat point above the composer");

    // A terminal can coalesce motion so the first drag already lands outside
    // the pressed pane. No intermediate in-pane drag may be assumed.
    for (anchor, target) in [((start.0, start.1), below_input), ((end.0, end.1), chat)] {
        assert_eq!(drag_copy(&mut app, anchor, target), app.input);
        assert_eq!(
            app.current_copy_selection_pane(),
            Some(crate::tui::CopySelectionPane::Input)
        );
        assert!(!app.copy_selection_dragging);
        assert!(app.copy_selection_pending_anchor.is_none());
    }
}

#[test]
fn test_copy_first_drag_outside_chat_with_long_history_still_copies() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let mut app = create_test_app();
    app.display_messages = (0..12_000)
        .map(|index| {
            DisplayMessage::assistant(format!(
                "```text\nhistory marker {index}\n{}\n```",
                "history detail\n".repeat(5)
            ))
        })
        .collect();
    app.bump_display_messages_version();
    app.input = "composer must not leak into chat copy".to_string();
    app.cursor_pos = app.input.len();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    render_and_snap(&app, &mut terminal);
    assert!(crate::tui::ui::copy_viewport_line_count().unwrap() > u16::MAX as usize);

    let start = (0..24u16)
        .flat_map(|row| (0..80u16).map(move |col| (col, row)))
        .find(|(col, row)| {
            crate::tui::ui::copy_point_from_screen(*col, *row).is_some_and(|point| {
                point.pane == crate::tui::CopySelectionPane::Chat
                    && point.abs_line > u16::MAX as usize
                    && crate::tui::ui::copy_viewport_line_text(point.abs_line)
                        .is_some_and(|text| text.contains("history marker 11999"))
            })
        })
        .expect("last message is visible in document coordinates");
    let input_points = input_pane_screen_points(80, 24);
    let target = (input_points[0].0, input_points[0].1);
    let expected = crate::tui::ui::copy_selection_text(crate::tui::CopySelectionRange {
        start: crate::tui::ui::copy_point_from_screen(start.0, start.1).unwrap(),
        end: crate::tui::ui::copy_pane_drag_point(
            crate::tui::CopySelectionPane::Chat,
            target.0,
            target.1,
        )
        .unwrap(),
    })
    .unwrap();

    for (kind, (column, row)) in [
        (MouseEventKind::Down(MouseButton::Left), start),
        (MouseEventKind::Drag(MouseButton::Left), target),
        (MouseEventKind::Up(MouseButton::Left), target),
    ] {
        app.handle_mouse_event(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        });
    }
    assert!(expected.contains("history marker 11999"), "{expected:?}");
    assert!(!expected.contains(&app.input));
    assert_eq!(clipboard.text().as_deref(), Some(expected.as_str()));
    assert_eq!(
        app.current_copy_selection_pane(),
        Some(crate::tui::CopySelectionPane::Chat)
    );
    assert!(!app.copy_selection_dragging);
    assert!(app.copy_selection_pending_anchor.is_none());
}

#[test]
fn test_input_composer_drag_selects_and_copies_typed_text() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "select this draft".to_string();
    app.cursor_pos = app.input.len();

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    // The composer registers a copy snapshot of the typed text (no prompt).
    assert_eq!(
        crate::tui::ui::input_pane_line_text(0).as_deref(),
        Some("select this draft")
    );
    assert_eq!(crate::tui::ui::input_pane_line_count(), Some(1));

    let points = input_pane_screen_points(80, 24);
    assert!(
        !points.is_empty(),
        "composer must be hit-testable for copy selection"
    );
    let start = points
        .iter()
        .find(|(_, _, p)| p.abs_line == 0 && p.column == 0)
        .map(|(c, r, _)| (*c, *r))
        .expect("screen cell for text start");
    let end = points
        .iter()
        .filter(|(_, _, p)| p.abs_line == 0)
        .max_by_key(|(_, _, p)| p.column)
        .map(|(c, r, _)| (*c, *r))
        .expect("screen cell for text end");

    let copied = drag_copy(&mut app, start, end);
    assert_eq!(copied, "select this draft");
    assert_eq!(
        app.status_notice(),
        Some("Copied selection · highlight remains visible".to_string())
    );
    // The highlight stays visible after copying (c7afd6620), but the drag ends.
    assert!(!app.copy_selection_mode);
    assert!(!app.copy_selection_dragging);
    assert!(app.copy_selection_anchor.is_some());
    assert!(app.copy_selection_cursor.is_some());
}

#[test]
fn test_input_composer_selection_never_includes_prompt_prefix() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "no prompt here".to_string();
    app.cursor_pos = app.input.len();

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let rendered = render_and_snap(&app, &mut terminal);
    // Sanity: the prompt decoration is actually on screen ("1>" for the first prompt).
    assert!(rendered.contains("1>"), "expected prompt prefix on screen");

    let points = input_pane_screen_points(80, 24);
    let row = points.first().map(|(_, r, _)| *r).expect("composer row");

    // Start the drag on the far-left edge of the composer row: on the prompt
    // decoration itself. The selection must clamp to the typed text.
    let end = points
        .iter()
        .filter(|(_, _, p)| p.abs_line == 0)
        .max_by_key(|(_, _, p)| p.column)
        .map(|(c, r, _)| (*c, *r))
        .expect("screen cell for text end");
    let copied = drag_copy(&mut app, (0, row), end);
    assert_eq!(copied, "no prompt here");
    assert!(
        !copied.contains('>'),
        "prompt decoration must never be copied, got {copied:?}"
    );
}

#[test]
fn test_input_composer_multiline_selection_preserves_newlines() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "alpha one\nbeta two".to_string();
    app.cursor_pos = app.input.len();

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    assert_eq!(crate::tui::ui::input_pane_line_count(), Some(2));
    assert_eq!(
        crate::tui::ui::input_pane_line_text(0).as_deref(),
        Some("alpha one")
    );
    assert_eq!(
        crate::tui::ui::input_pane_line_text(1).as_deref(),
        Some("beta two")
    );

    let points = input_pane_screen_points(80, 24);
    let start = points
        .iter()
        .find(|(_, _, p)| p.abs_line == 0 && p.column == 0)
        .map(|(c, r, _)| (*c, *r))
        .expect("screen cell for first line start");
    let end = points
        .iter()
        .filter(|(_, _, p)| p.abs_line == 1)
        .max_by_key(|(_, _, p)| p.column)
        .map(|(c, r, _)| (*c, *r))
        .expect("screen cell for second line end");

    let copied = drag_copy(&mut app, start, end);
    assert_eq!(copied, "alpha one\nbeta two");
}

#[test]
fn test_input_composer_soft_wrapped_selection_copies_unwrapped_text() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    // Narrow terminal so this single logical line soft-wraps across rows.
    let text = "abcdefghij klmnopqrst uvwxyz0123456789";
    app.input = text.to_string();
    app.cursor_pos = app.input.len();

    let backend = ratatui::backend::TestBackend::new(30, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let wrapped_rows = crate::tui::ui::input_pane_line_count().expect("input snapshot");
    assert!(
        wrapped_rows >= 2,
        "expected the input to soft-wrap, got {wrapped_rows} rows"
    );

    let points = input_pane_screen_points(30, 20);
    let start = points
        .iter()
        .find(|(_, _, p)| p.abs_line == 0 && p.column == 0)
        .map(|(c, r, _)| (*c, *r))
        .expect("screen cell for wrap start");
    let last_line = wrapped_rows - 1;
    let end = points
        .iter()
        .filter(|(_, _, p)| p.abs_line == last_line)
        .max_by_key(|(_, _, p)| p.column)
        .map(|(c, r, _)| (*c, *r))
        .expect("screen cell for wrap end");

    let copied = drag_copy(&mut app, start, end);
    // A soft wrap is a rendering artifact: the copied text must be the
    // original logical line with no injected newline.
    assert_eq!(copied, text);
    assert!(!copied.contains('\n'));
}

#[test]
fn test_chat_drag_into_composer_clamps_to_chat_pane() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.display_messages = vec![DisplayMessage {
        role: "user".to_string(),
        content: "transcript prompt line".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.input = "draft under composition".to_string();
    app.cursor_pos = app.input.len();

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    // Anchor on a chat transcript cell.
    let (chat_col, chat_row, chat_point) = (0..24u16)
        .flat_map(|row| (0..80u16).map(move |col| (col, row)))
        .find_map(|(col, row)| {
            crate::tui::ui::copy_point_from_screen(col, row)
                .filter(|p| p.pane == crate::tui::CopySelectionPane::Chat)
                .map(|p| (col, row, p))
        })
        .expect("a chat cell to anchor on");
    // A second chat cell that resolves to a *different* logical point: the
    // same-cell drag-jitter guard keeps the press armed (dragging never
    // starts) while press and motion map to the identical point, so the
    // in-pane motion must genuinely move the selection cursor.
    let (chat_col2, chat_row2, _) = (0..24u16)
        .flat_map(|row| (0..80u16).map(move |col| (col, row)))
        .find_map(|(col, row)| {
            crate::tui::ui::copy_point_from_screen(col, row)
                .filter(|p| p.pane == crate::tui::CopySelectionPane::Chat && *p != chat_point)
                .map(|p| (col, row, p))
        })
        .expect("a second distinct chat cell");

    // Composer row to drag into.
    let input_points = input_pane_screen_points(80, 24);
    let (input_col, input_row) = input_points
        .iter()
        .map(|(c, r, _)| (*c, *r))
        .next()
        .expect("composer cell");

    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: chat_col,
            row: chat_row,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    // Move within the chat pane first (real drags pass through cells), then
    // into the composer.
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: chat_col2,
            row: chat_row2,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: input_col,
            row: input_row,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    // The selection must stay clamped to the chat pane.
    assert_eq!(
        app.current_copy_selection_pane(),
        Some(crate::tui::CopySelectionPane::Chat)
    );
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: input_col,
            row: input_row,
            modifiers: KeyModifiers::empty(),
        },
        |text| {
            *copied_for_closure.lock().unwrap() = text.to_string();
            true
        },
    );

    let copied = copied.lock().unwrap().clone();
    assert!(
        !copied.contains("draft under composition"),
        "cross-pane drag must not leak composer text into a chat selection, got {copied:?}"
    );
}

#[test]
fn test_input_composer_click_still_moves_caret() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "caret target".to_string();
    app.cursor_pos = 0;

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    // Click (press + release, no drag) in the middle of the typed text.
    let points = input_pane_screen_points(80, 24);
    let (col, row, point) = points
        .iter()
        .find(|(_, _, p)| p.abs_line == 0 && p.column == 6)
        .copied()
        .expect("screen cell inside the typed text");

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::empty(),
    });
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(
        app.cursor_pos, point.column,
        "plain click in the composer must reposition the caret"
    );
    // No selection was made or copied by the plain click.
    assert!(app.copy_selection_anchor.is_none());
    assert_ne!(app.status_notice(), Some("Copied selection".to_string()));
}

#[test]
fn test_input_composer_drag_then_release_copies_via_full_mouse_path() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "full path check".to_string();
    app.cursor_pos = app.input.len();

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let points = input_pane_screen_points(80, 24);
    let start = points
        .iter()
        .find(|(_, _, p)| p.abs_line == 0 && p.column == 0)
        .map(|(c, r, _)| (*c, *r))
        .expect("start cell");
    let end = points
        .iter()
        .filter(|(_, _, p)| p.abs_line == 0)
        .max_by_key(|(_, _, p)| p.column)
        .map(|(c, r, _)| (*c, *r))
        .expect("end cell");

    // Full handle_mouse_event path: press, drag, release. The release attempts
    // a real clipboard copy, which may fail in CI, but the selection path must
    // have run and reported one of the copy outcomes.
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: start.0,
        row: start.1,
        modifiers: KeyModifiers::empty(),
    });
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: end.0,
        row: end.1,
        modifiers: KeyModifiers::empty(),
    });
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: end.0,
        row: end.1,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        matches!(
            app.status_notice().as_deref(),
            Some("Copied selection · highlight remains visible") | Some("Failed to copy selection")
        ),
        "drag release over the composer must attempt a copy, got {:?}",
        app.status_notice()
    );
}
