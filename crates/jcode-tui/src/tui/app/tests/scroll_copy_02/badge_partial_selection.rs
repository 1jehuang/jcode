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
