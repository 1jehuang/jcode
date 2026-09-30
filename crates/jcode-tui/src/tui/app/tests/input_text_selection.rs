// Editable text selection in the prompt composer: double-click selects a word,
// triple-click a line, Shift+arrows extend, and the selection can be cut,
// copied, deleted, or typed over (like Claude Code's input box).

fn composer_cell_for(app: &App, width: u16, height: u16, column: usize) -> (u16, u16) {
    let _ = app;
    input_pane_screen_points(width, height)
        .into_iter()
        .find(|(_, _, p)| p.abs_line == 0 && p.column == column)
        .map(|(c, r, _)| (c, r))
        .expect("screen cell for composer column")
}

fn left_click(app: &mut App, cell: (u16, u16)) {
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_mouse_event(MouseEvent {
            kind,
            column: cell.0,
            row: cell.1,
            modifiers: KeyModifiers::empty(),
        });
    }
}

fn rendered_composer_app(input: &str) -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    app.input = input.to_string();
    app.cursor_pos = app.input.len();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);
    (app, terminal)
}

#[test]
fn test_input_double_click_selects_word_and_ctrl_x_cuts_it() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, _terminal) = rendered_composer_app("hello big world");

    // Double-click on the 'i' of "big".
    let cell = composer_cell_for(&app, 80, 24, 7);
    left_click(&mut app, cell);
    left_click(&mut app, cell);

    assert_eq!(app.input_selection(), Some((6, 9)));
    assert_eq!(app.input_selection_text().as_deref(), Some("big"));

    app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input, "hello  world");
    assert_eq!(app.cursor_pos, 6);
    assert_eq!(app.input_selection(), None);
    assert_eq!(clipboard.text().as_deref(), Some("big"));
}

#[test]
fn test_input_triple_click_selects_logical_line() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = rendered_composer_app("first line\nsecond line");

    let cell = composer_cell_for(&app, 80, 24, 2);
    left_click(&mut app, cell);
    left_click(&mut app, cell);
    left_click(&mut app, cell);

    assert_eq!(app.input_selection_text().as_deref(), Some("first line"));
}

#[test]
fn test_input_single_click_after_double_click_clears_selection() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = rendered_composer_app("hello big world");

    let cell = composer_cell_for(&app, 80, 24, 7);
    left_click(&mut app, cell);
    left_click(&mut app, cell);
    assert!(app.input_selection().is_some());

    // A click far enough away (a different cell) is a fresh single click.
    let other = composer_cell_for(&app, 80, 24, 1);
    left_click(&mut app, other);
    assert_eq!(app.input_selection(), None);
    assert_eq!(app.cursor_pos, 1);
}

#[test]
fn test_input_typing_replaces_selection_and_undo_restores() {
    let mut app = create_test_app();
    app.input = "hello big world".to_string();
    app.set_input_selection(6, 9);

    app.handle_key(KeyCode::Char('X'), KeyModifiers::SHIFT)
        .unwrap();
    assert_eq!(app.input, "hello X world");
    assert_eq!(app.cursor_pos, 7);
    assert_eq!(app.input_selection(), None);

    // One undo step restores the replaced text.
    app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input, "hello big world");
}

#[test]
fn test_input_backspace_and_delete_remove_selection() {
    let mut app = create_test_app();
    app.input = "hello big world".to_string();
    app.set_input_selection(9, 6);
    app.handle_key(KeyCode::Backspace, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.input, "hello  world");
    assert_eq!(app.cursor_pos, 6);

    app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input, "hello big world");

    app.set_input_selection(0, 6);
    app.handle_key(KeyCode::Delete, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.input, "big world");
}

#[test]
fn test_input_paste_replaces_selection() {
    let mut app = create_test_app();
    app.input = "hello big world".to_string();
    app.set_input_selection(6, 9);
    super::input::handle_text_paste(&mut app, "small".to_string());
    assert_eq!(app.input, "hello small world");
    assert_eq!(app.input_selection(), None);
}

#[test]
fn test_input_ctrl_c_with_selection_copies_without_clearing_or_quitting() {
    let clipboard = CapturedClipboard::new();
    let mut app = create_test_app();
    app.input = "hello big world".to_string();
    app.set_input_selection(6, 9);

    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input, "hello big world");
    assert!(app.quit_pending.is_none());
    assert_eq!(clipboard.text().as_deref(), Some("big"));
    // The selection stays so it can still be cut or typed over.
    assert_eq!(app.input_selection(), Some((6, 9)));
}

#[test]
fn test_input_ctrl_c_without_selection_still_clears_input() {
    let mut app = create_test_app();
    app.input = "draft".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();

    assert!(app.input.is_empty());
    assert_eq!(
        app.status_notice(),
        Some("Input cleared. Press Ctrl+C again to quit".to_string())
    );
}

#[test]
fn test_input_ctrl_x_without_selection_still_cuts_whole_line() {
    let clipboard = CapturedClipboard::new();
    let mut app = create_test_app();
    app.input = "whole line".to_string();
    app.cursor_pos = 3;
    app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.input.is_empty());
    assert_eq!(clipboard.text().as_deref(), Some("whole line"));
}

#[test]
fn test_input_shift_arrows_extend_selection() {
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Left, KeyModifiers::SHIFT).unwrap();
    app.handle_key(KeyCode::Left, KeyModifiers::SHIFT).unwrap();
    assert_eq!(app.input_selection_text().as_deref(), Some("ld"));

    app.handle_key(KeyCode::Home, KeyModifiers::SHIFT).unwrap();
    assert_eq!(app.input_selection_text().as_deref(), Some("hello world"));

    app.handle_key(KeyCode::Right, KeyModifiers::SHIFT).unwrap();
    assert_eq!(app.input_selection_text().as_deref(), Some("ello world"));

    // Shrinking back to the anchor leaves no selection.
    app.handle_key(KeyCode::End, KeyModifiers::SHIFT).unwrap();
    assert_eq!(app.input_selection(), None);
    assert_eq!(app.cursor_pos, app.input.len());
}

#[test]
fn test_input_plain_arrows_and_esc_clear_selection_without_editing() {
    let mut app = create_test_app();
    app.input = "hello big world".to_string();

    app.set_input_selection(6, 9);
    app.handle_key(KeyCode::Left, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input_selection(), None);
    assert_eq!(app.cursor_pos, 6, "Left collapses to the selection start");

    app.set_input_selection(6, 9);
    app.handle_key(KeyCode::Right, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input_selection(), None);
    assert_eq!(app.cursor_pos, 9, "Right collapses to the selection end");

    app.set_input_selection(6, 9);
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input_selection(), None);
    assert_eq!(app.input, "hello big world", "Esc only drops the selection");
}

#[test]
fn test_input_drag_selection_becomes_editable() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = rendered_composer_app("select this draft");

    let start = composer_cell_for(&app, 80, 24, 7);
    let end = composer_cell_for(&app, 80, 24, 11);
    // Full mouse path: press, drag, release over the composer text.
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
    assert_eq!(app.input_selection_text().as_deref(), Some("this"));

    app.handle_key(KeyCode::Backspace, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.input, "select  draft");
}

#[test]
fn test_input_selection_is_highlighted_when_rendered() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = rendered_composer_app("hello big world");
    app.set_input_selection(6, 9);
    render_and_snap(&app, &mut terminal);

    let buffer = terminal.backend().buffer().clone();
    let area = buffer.area;
    let row = (0..area.height)
        .find(|&y| {
            let line: String = (0..area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect();
            line.contains("hello big world")
        })
        .expect("composer row");
    let line: String = (0..area.width)
        .map(|x| buffer[(x, row)].symbol().to_string())
        .collect();
    let h_col = line.find("hello big world").unwrap() as u16;
    let b_col = h_col + 6;
    let plain_bg = buffer[(h_col, row)].bg;
    for x in b_col..b_col + 3 {
        assert_ne!(
            buffer[(x, row)].bg, plain_bg,
            "selected cell {x} must be highlighted"
        );
    }
    assert_eq!(buffer[(b_col + 3, row)].bg, plain_bg, "space after the word stays plain");
    assert_eq!(buffer[(b_col - 1, row)].bg, plain_bg, "space before the word stays plain");
}

#[test]
fn test_remote_input_selection_cut_and_copy() {
    let clipboard = CapturedClipboard::new();
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.input = "hello big world".to_string();
    app.set_input_selection(6, 9);
    rt.block_on(app.handle_remote_key(KeyCode::Char('c'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert!(app.quit_pending.is_none(), "Ctrl+C over a selection copies");
    assert_eq!(clipboard.text().as_deref(), Some("big"));

    rt.block_on(app.handle_remote_key(KeyCode::Char('x'), KeyModifiers::SUPER, &mut remote))
        .unwrap();
    assert_eq!(app.input, "hello  world");

    app.set_input_selection(0, 5);
    rt.block_on(app.handle_remote_key(KeyCode::Char('y'), KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert_eq!(app.input, "y  world");
}
