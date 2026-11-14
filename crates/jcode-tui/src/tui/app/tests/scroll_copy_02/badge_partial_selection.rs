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
    drag_copy_between_in_chat(app, clipboard, &screen, needle, needle)
}

/// Drag from the first cell of `from` to just past the last cell of `to`.
/// The two needles may sit on different rendered rows.
fn drag_copy_between_in_chat(
    app: &mut App,
    clipboard: &CapturedClipboard,
    screen: &str,
    from: &str,
    to: &str,
) -> Option<String> {
    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let locate = |needle: &str, past_end: bool| {
        let (line_idx, text) = (visible_start..visible_end)
            .find_map(|abs| {
                let text = crate::tui::ui::copy_viewport_line_text(abs).unwrap_or_default();
                text.contains(needle).then_some((abs, text))
            })
            .unwrap_or_else(|| panic!("no rendered line contains {needle:?}: {screen}"));
        let mut col = unicode_width::UnicodeWidthStr::width(&text[..text.find(needle).unwrap()]);
        if past_end {
            col += unicode_width::UnicodeWidthStr::width(needle);
        }
        let row = layout.messages_area.y + (line_idx - visible_start) as u16;
        let column = (layout.messages_area.x
            ..layout.messages_area.x + layout.messages_area.width)
            .find(|&column| {
                crate::tui::ui::copy_viewport_point_from_screen(column, row)
                    .is_some_and(|p| p.abs_line == line_idx && p.column == col)
            })
            .expect("screen x");
        (column, row)
    };
    let (start, end) = (locate(from, false), locate(to, true));
    for (kind, (column, row)) in [
        (MouseEventKind::Down(MouseButton::Left), start),
        (MouseEventKind::Drag(MouseButton::Left), end),
        (MouseEventKind::Up(MouseButton::Left), end),
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

#[test]
fn test_multi_line_drag_inside_code_block_copies_only_highlighted_lines() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_copy_test_app();
    let screen = render_and_snap(&app, &mut terminal);
    let copied = drag_copy_between_in_chat(&mut app, &clipboard, &screen, "main", "println");
    assert_eq!(copied.as_deref(), Some("main() {\n    println"));
}

#[test]
fn test_drag_from_prose_into_blockquote_stops_at_the_highlight() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_blockquote_copy_test_app();
    let screen = render_and_snap(&app, &mut terminal);
    let copied = drag_copy_between_in_chat(&mut app, &clipboard, &screen, "say", "quick")
        .expect("selection should reach the clipboard");
    assert_eq!(
        copied.lines().filter(|line| !line.is_empty()).collect::<Vec<_>>(),
        ["say:", "the quick"],
        "selection must stop at the highlight, without gutter or badge chrome: {copied:?}"
    );
}

/// Highlighting a whole framed block by hand gives the same text as its badge:
/// the code, without the `┌─ rust` / `└─` border rows or the `│ ` gutter.
#[test]
fn test_drag_over_whole_code_block_copies_code_without_frame_chrome() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_copy_test_app();
    let screen = render_and_snap(&app, &mut terminal);
    let copied = drag_copy_between_in_chat(&mut app, &clipboard, &screen, "┌─ rust", "└─");
    assert_eq!(
        copied.as_deref(),
        Some("fn main() {\n    println!(\"hello\");\n}")
    );
    let range = app.normalized_copy_selection().expect("range");
    let (chars, lines) = crate::tui::ui::copy_selection_metrics(range).expect("metrics");
    assert_eq!((chars, lines), (copied.unwrap().chars().count(), 3));
}

#[test]
fn test_drag_across_soft_wrapped_quote_copies_only_highlighted_text() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_blockquote_copy_test_app();
    let words: Vec<String> = (0..40).map(|i| format!("word{i:02}")).collect();
    app.display_messages[1].content = format!("> {}", words.join(" "));
    app.bump_display_messages_version();
    let screen = render_and_snap(&app, &mut terminal);
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let rows: Vec<String> = (visible_start..visible_end)
        .map(|abs| crate::tui::ui::copy_viewport_line_text(abs).unwrap_or_default())
        .filter(|text| text.contains("word"))
        .collect();
    assert!(rows.len() >= 2, "quote should soft-wrap: {screen}");
    // Second word of the first row through the second word of the next row.
    let nth_word = |row: &str, n: usize| {
        row.split_whitespace()
            .filter(|word| word.starts_with("word"))
            .nth(n)
            .expect("word")
            .to_string()
    };
    let (from, to) = (nth_word(&rows[0], 1), nth_word(&rows[1], 1));
    let copied = drag_copy_between_in_chat(&mut app, &clipboard, &screen, &from, &to);
    let (a, b) = (
        words.iter().position(|word| *word == from).unwrap(),
        words.iter().position(|word| *word == to).unwrap(),
    );
    assert_eq!(copied, Some(words[a..=b].join(" ")));
}

#[test]
fn test_partial_drag_inside_unicode_display_math_copies_highlighted_glyphs() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_math_copy_test_app();
    let screen = render_and_snap(&app, &mut terminal);
    assert!(screen.contains("┌─ math"), "{screen}");
    let copied = drag_copy_between_in_chat(&mut app, &clipboard, &screen, "⊤", "⊤");
    assert_eq!(copied.as_deref(), Some("⊤"));
}
