fn create_math_copy_test_app() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage {
            role: "user".to_string(),
            content: "show latex".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        
            tool_duration_ms: None,
        },
        DisplayMessage {
            role: "assistant".to_string(),
            content: "Euler $e^{i\\pi_{3}} + 1 = 0$ done.\n\n$$\n\\frac{\\partial L}{\\partial \\theta_{88}} = \\theta^\\top x\n$$".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        
            tool_duration_ms: None,
        },
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    (app, terminal)
}

#[test]
fn test_copy_selection_select_all_copies_rendered_math_as_latex() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_math_copy_test_app();

    let screen = render_and_snap(&app, &mut terminal);
    // Sanity: the screen shows the Unicode approximation, not the source.
    assert!(screen.contains("┌─ math"), "{screen}");
    assert!(screen.contains('⊤'), "\\top should render as ⊤: {screen}");
    assert!(!screen.contains("\\frac"), "{screen}");

    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();
    assert!(app.select_all_in_copy_mode());
    let selected = app
        .current_copy_selection_text()
        .expect("expected selected transcript text");
    assert!(
        selected.contains("Euler $e^{i\\pi_{3}} + 1 = 0$ done."),
        "inline math should copy as LaTeX: {selected}"
    );
    assert!(
        selected.contains("$$\n\\frac{\\partial L}{\\partial \\theta_{88}} = \\theta^\\top x\n$$"),
        "display math should copy as LaTeX: {selected}"
    );
    assert!(!selected.contains('⊤'), "{selected}");

    let range = app.normalized_copy_selection().expect("range");
    let (chars, _) = crate::tui::ui::copy_selection_metrics(range).expect("metrics");
    assert_eq!(chars, selected.chars().count());
}

#[test]
fn test_copy_selection_partial_drag_over_inline_math_copies_whole_formula() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_math_copy_test_app();
    render_and_snap(&app, &mut terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let (line_idx, text) = (visible_start..visible_end)
        .find_map(|abs| {
            let text = crate::tui::ui::copy_viewport_line_text(abs).unwrap_or_default();
            text.contains("Euler").then_some((abs, text))
        })
        .expect("prose line");
    let euler_col = unicode_width::UnicodeWidthStr::width(&text[..text.find("Euler").unwrap()]);
    // Drag from the start of "Euler" to two cells into the formula.
    let start_col = euler_col;
    let end_col = euler_col + "Euler ".len() + 2;
    let row = layout.messages_area.y + (line_idx - visible_start) as u16;
    let screen_x = |target: usize| {
        (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
            .find(|&column| {
                crate::tui::ui::copy_viewport_point_from_screen(column, row)
                    .is_some_and(|p| p.abs_line == line_idx && p.column == target)
            })
            .expect("screen x")
    };
    let (sx, ex) = (screen_x(start_col), screen_x(end_col));
    for (kind, column) in [
        (MouseEventKind::Down(MouseButton::Left), sx),
        (MouseEventKind::Drag(MouseButton::Left), ex),
    ] {
        app.handle_mouse_event(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        });
    }
    let selected = app.current_copy_selection_text().expect("selection");
    assert_eq!(selected, "Euler $e^{i\\pi_{3}} + 1 = 0$");
}

#[test]
fn test_unicode_math_copy_badge_copies_latex_to_clipboard() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_math_copy_test_app();
    let screen = render_and_snap(&app, &mut terminal);
    assert!(
        screen.contains("⇧S]"),
        "math frame should get a badge: {screen}"
    );

    app.handle_key(KeyCode::Char('S'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(app.status_notice(), Some("Copied math".to_string()));
    assert_eq!(
        clipboard.text().as_deref(),
        Some("$$\n\\frac{\\partial L}{\\partial \\theta_{88}} = \\theta^\\top x\n$$")
    );
}
