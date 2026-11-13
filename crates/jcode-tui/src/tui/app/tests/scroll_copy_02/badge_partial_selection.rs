/// Drag-select `needle` inside the rendered chat (press, drag, release) and
/// return what reached the clipboard.
fn drag_copy_needle_in_chat(
    app: &mut App,
    terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
    clipboard: &CapturedClipboard,
    needle: &str,
) -> Option<String> {
    let screen = render_and_snap(app, terminal);
    assert!(
        screen.contains("⇧S]"),
        "block should carry a copy badge: {screen}"
    );
    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let (line_idx, text) = (visible_start..visible_end)
        .find_map(|abs| {
            let text = crate::tui::ui::copy_viewport_line_text(abs).unwrap_or_default();
            text.contains(needle).then_some((abs, text))
        })
        .unwrap_or_else(|| panic!("no rendered line contains {needle:?}: {screen}"));
    let start_col = unicode_width::UnicodeWidthStr::width(&text[..text.find(needle).unwrap()]);
    let end_col = start_col + unicode_width::UnicodeWidthStr::width(needle);
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
        (MouseEventKind::Up(MouseButton::Left), ex),
    ] {
        app.handle_mouse_event(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        });
    }
    clipboard.text()
}

#[test]
fn test_partial_drag_inside_code_block_copies_only_highlighted_text() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_copy_test_app();
    let copied = drag_copy_needle_in_chat(&mut app, &mut terminal, &clipboard, "println");
    assert_eq!(copied.as_deref(), Some("println"));
}

#[test]
fn test_partial_drag_inside_blockquote_copies_only_highlighted_text() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_blockquote_copy_test_app();
    let copied = drag_copy_needle_in_chat(&mut app, &mut terminal, &clipboard, "brown");
    assert_eq!(copied.as_deref(), Some("brown"));
}

#[test]
fn test_partial_drag_inside_error_copies_only_highlighted_text() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_error_copy_test_app();
    let copied = drag_copy_needle_in_chat(&mut app, &mut terminal, &clipboard, "denied");
    assert_eq!(copied.as_deref(), Some("denied"));
}

#[test]
fn test_partial_drag_inside_tool_output_copies_only_highlighted_text() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_tool_failed_output_copy_test_app();
    // The tool row renders collapsed, so select part of the badge line itself.
    let needle = "secret";
    let copied = drag_copy_needle_in_chat(&mut app, &mut terminal, &clipboard, needle);
    assert_eq!(copied.as_deref(), Some(needle));
}

/// With `display.latex_rendering = "none"` math is shown as its literal LaTeX
/// source, so what is highlighted is exactly what can be copied. A partial
/// drag must not expand to the whole formula the way rendered math does.
#[test]
fn test_partial_drag_inside_raw_latex_copies_only_highlighted_text() {
    // App setup also takes the render lock, so always acquire env first.
    let _env = LatexCommandEnv::new();
    let _render_lock = scroll_render_test_lock();
    crate::env::set_var("JCODE_LATEX_RENDERING", "none");
    crate::config::Config::invalidate_cache();
    crate::tui::markdown::install_jcode_markdown_hooks();

    for needle in ["\\rho_{41}", "\\sigma^{17}"] {
        let clipboard = CapturedClipboard::new();
        let (mut app, mut terminal) = create_math_copy_test_app();
        // Unique formulas so no other test's cached render of the same
        // markdown (in a different LaTeX mode) can be reused here.
        app.display_messages[1].content =
            "Raw $a + \\rho_{41} = b$ inline.\n\n$$\nq = \\sigma^{17} + r\n$$".to_string();
        app.bump_display_messages_version();
        let screen = render_and_snap(&app, &mut terminal);
        assert!(
            screen.contains("math (raw)") && screen.contains("$a + \\rho_{41} = b$"),
            "math should be shown as raw source: {screen}"
        );
        let copied = drag_copy_needle_in_chat(&mut app, &mut terminal, &clipboard, needle);
        assert_eq!(copied.as_deref(), Some(needle));
    }
}
