// Tests for copy-selection in the prompt composer (input box), issue #430:
// text being typed must be drag-selectable and copyable with the mouse, just
// like the transcript, without ever copying the prompt decoration.

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

fn drag_copy(
    app: &mut App,
    start: (u16, u16),
    end: (u16, u16),
) -> String {
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
    assert_eq!(app.status_notice(), Some("Copied selection · highlight remains visible".to_string()));
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

// --- Edge auto-scroll while drag-selecting in the composer ---
//
// A long draft scrolls inside the composer (caret-follow, max 10 visible
// wrapped rows). Selecting it with the mouse must keep scrolling while the
// drag is held at the top/bottom edge, browser-style, so the whole prompt can
// be selected with one held drag (the fix for this issue).

/// 16 one-line rows in a 24-row terminal: the composer shows 10, the draft is
/// scrollable, and the caret-follow view starts pinned to the bottom.
fn create_scrollable_input_app() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>, usize) {
    let mut app = create_test_app();
    let line_count = 16usize;
    app.input = (0..line_count)
        .map(|i| format!("prompt line {i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.cursor_pos = app.input.len();

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);
    (app, terminal, line_count)
}

fn composer_mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

/// Press on the bottom composer row, drag one row up (to start a real drag),
/// then drag onto the given edge row so the edge hot zone engages.
fn drag_composer_to_edge(app: &mut App, points: &[(u16, u16, crate::tui::CopySelectionPoint)], to_top: bool) {
    let rows = points.iter().map(|(_, r, _)| *r).collect::<Vec<_>>();
    let (top_row, bottom_row) = (
        rows.iter().copied().min().expect("composer top row"),
        rows.iter().copied().max().expect("composer bottom row"),
    );
    let (edge_row, near_row) = if to_top { (top_row, bottom_row) } else { (bottom_row, top_row) };
    // Anchor at the far end of the text on the press row (rightmost cell of
    // the bottom row when dragging up): a press at the left margin clamps to
    // column 0 and would leave the final line unselected after release.
    let press = points
        .iter()
        .filter(|(_, r, _)| *r == near_row)
        .max_by_key(|(_, _, p)| p.column)
        .map(|(c, r, _)| (*c, *r))
        .expect("cell on the far composer row");
    let arm = points
        .iter()
        .find(|(c, r, _)| (*r == near_row || *r == edge_row) && (*c, *r) != press)
        .map(|(c, r, _)| (*c, *r))
        .expect("distinct cell to arm the drag");
    let edge_col = points
        .iter()
        .find(|(_, r, _)| *r == edge_row)
        .map(|(c, _, _)| *c)
        .expect("cell on the edge row");

    app.handle_copy_selection_mouse_with(
        composer_mouse(MouseEventKind::Down(MouseButton::Left), press.0, press.1),
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        composer_mouse(MouseEventKind::Drag(MouseButton::Left), arm.0, arm.1),
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        composer_mouse(MouseEventKind::Drag(MouseButton::Left), edge_col, edge_row),
        |_| true,
    );
}

#[test]
fn test_input_composer_drag_to_top_edge_autoscrolls_and_selects_whole_prompt() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal, line_count) = create_scrollable_input_app();

    // Precondition: the draft overflows the composer and caret-follow starts
    // pinned to the bottom, so only the tail is selectable without the fix.
    let (scroll, visible_end) =
        crate::tui::ui::input_pane_visible_range().expect("input snapshot");
    assert!(scroll > 0, "draft must overflow the composer (scroll={scroll})");
    assert_eq!(visible_end, line_count);

    let points = input_pane_screen_points(80, 24);
    drag_composer_to_edge(&mut app, &points, true);
    assert_eq!(
        app.copy_selection_edge_autoscroll,
        Some((crate::tui::CopySelectionPane::Input, true))
    );

    // The tick-driven autoscroll keeps scrolling while the button is held at
    // the top edge until the composer reaches its top scroll bound.
    let mut ticks = 0;
    while app.progress_copy_selection_edge_autoscroll() {
        render_and_snap(&app, &mut terminal);
        ticks += 1;
        assert!(ticks < 100, "autoscroll never reached the top bound");
    }
    render_and_snap(&app, &mut terminal);
    assert_eq!(
        crate::tui::ui::input_pane_visible_range().expect("input snapshot").0,
        0,
        "composer must be scrolled to the top after the drag"
    );
    assert_eq!(app.input_copy_scroll_offset, Some(0));
    assert_eq!(
        app.copy_selection_cursor.map(|point| point.abs_line),
        Some(0),
        "selection must extend to the first line while held at the edge"
    );

    // Releasing at the top edge copies the entire draft, head to tail.
    let top_row = points.iter().map(|(_, r, _)| *r).min().expect("top row");
    let top_col = points
        .iter()
        .find(|(_, r, _)| *r == top_row)
        .map(|(c, _, _)| *c)
        .expect("top row cell");
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();
    app.handle_copy_selection_mouse_with(
        composer_mouse(MouseEventKind::Up(MouseButton::Left), top_col, top_row),
        |text| {
            *copied_for_closure.lock().unwrap() = text.to_string();
            true
        },
    );
    let expected = (0..line_count)
        .map(|i| format!("prompt line {i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(copied.lock().unwrap().clone(), expected);
    // The highlight (and its scrolled view) remains visible after the copy.
    assert_eq!(app.input_copy_scroll_offset, Some(0));
    assert!(!app.copy_selection_dragging);

    // Any key hands control back to typing: the composer follows the caret
    // again so the edit is visible.
    app.handle_key(KeyCode::Char('z'), KeyModifiers::empty())
        .expect("key handled");
    assert_eq!(app.input_copy_scroll_offset, None);
    assert!(app.input.ends_with('z'), "typing must resume after the drag");
}

#[test]
fn test_input_composer_edge_autoscroll_reverses_when_drag_reaches_bottom() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal, line_count) = create_scrollable_input_app();

    // Scroll to the top first (see the previous test), then keep the button
    // held and drag down to the bottom edge: the autoscroll must reverse and
    // pull the view back down to the tail.
    let points = input_pane_screen_points(80, 24);
    drag_composer_to_edge(&mut app, &points, true);
    let mut ticks = 0;
    while app.progress_copy_selection_edge_autoscroll() {
        render_and_snap(&app, &mut terminal);
        ticks += 1;
        assert!(ticks < 100);
    }
    render_and_snap(&app, &mut terminal);

    let bottom_row = points.iter().map(|(_, r, _)| *r).max().expect("bottom row");
    let bottom_col = points
        .iter()
        .find(|(_, r, _)| *r == bottom_row)
        .map(|(c, _, _)| *c)
        .expect("bottom row cell");
    app.handle_copy_selection_mouse_with(
        composer_mouse(MouseEventKind::Drag(MouseButton::Left), bottom_col, bottom_row),
        |_| true,
    );
    assert_eq!(
        app.copy_selection_edge_autoscroll,
        Some((crate::tui::CopySelectionPane::Input, false)),
        "dragging the held selection to the bottom edge must arm downward autoscroll"
    );

    let mut ticks = 0;
    while app.progress_copy_selection_edge_autoscroll() {
        render_and_snap(&app, &mut terminal);
        ticks += 1;
        assert!(ticks < 100, "autoscroll never reached the bottom bound");
    }
    render_and_snap(&app, &mut terminal);
    let (scroll, visible_end) =
        crate::tui::ui::input_pane_visible_range().expect("input snapshot");
    assert_eq!(
        visible_end, line_count,
        "composer must be pinned to the bottom after downward autoscroll"
    );
    assert!(scroll > 0, "view must have scrolled back down (scroll={scroll})");

    // Release disarms the autoscroll and copies.
    app.handle_copy_selection_mouse_with(
        composer_mouse(MouseEventKind::Up(MouseButton::Left), bottom_col, bottom_row),
        |_| true,
    );
    assert_eq!(app.copy_selection_edge_autoscroll, None);
    assert!(
        matches!(
            app.status_notice().as_deref(),
            Some("Copied selection · highlight remains visible")
                | Some("Failed to copy selection")
                | Some("Selection is empty")
        ),
        "release must resolve the drag, got {:?}",
        app.status_notice()
    );
}

#[test]
fn test_input_composer_scroll_override_clears_on_press_text_and_mode_exit() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal, _line_count) = create_scrollable_input_app();

    // Simulate an armed drag scroll: the composer shows the draft's head.
    app.input_copy_scroll_offset = Some(0);
    render_and_snap(&app, &mut terminal);
    assert_eq!(
        crate::tui::ui::input_pane_visible_range().expect("input snapshot").0,
        0
    );

    // A fresh mouse press returns the composer to caret-follow so the click's
    // caret placement is visible.
    let points = input_pane_screen_points(80, 24);
    let (col, row) = points.first().map(|(c, r, _)| (*c, *r)).expect("composer cell");
    app.handle_copy_selection_mouse_with(
        composer_mouse(MouseEventKind::Down(MouseButton::Left), col, row),
        |_| true,
    );
    assert_eq!(app.input_copy_scroll_offset, None);

    // Text insertion (typing, paste, drops) also resumes caret-follow, even
    // when it runs from a non-key path.
    app.input_copy_scroll_offset = Some(0);
    super::input::handle_text_input(&mut app, "x");
    assert_eq!(app.input_copy_scroll_offset, None);
    assert!(app.input.ends_with('x'));

    // Leaving copy-selection mode resets it as well.
    app.input_copy_scroll_offset = Some(0);
    app.enter_copy_selection_mode();
    app.exit_copy_selection_mode();
    assert_eq!(app.input_copy_scroll_offset, None);
}
