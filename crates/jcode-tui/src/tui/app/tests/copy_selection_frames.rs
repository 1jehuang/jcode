// Drag-select copy over transcript code blocks and quotes must not include the
// `│ ` gutters or `┌─`/`└─` frame edges (issue #995), and must keep literal
// characters that only look like frame art.

/// Render `content` as the assistant message and return the screen.
fn render_copy_fixture(
    app: &mut App,
    terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
    content: &str,
) -> String {
    app.display_messages[1].content = content.to_string();
    app.bump_display_messages_version();
    render_and_snap(app, terminal)
}

fn row_containing(screen: &str, needle: &str) -> u16 {
    screen
        .lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not on screen:\n{screen}")) as u16
}

/// Screen column of the `nth` (0-based) char of `needle` on `row`.
fn column_of(screen: &str, row: u16, needle: &str, nth: usize) -> u16 {
    let line = screen.lines().nth(row as usize).expect("row on screen");
    let byte = line.find(needle).expect("needle on row");
    let col = unicode_width::UnicodeWidthStr::width(&line[..byte]);
    let skip: usize = needle
        .chars()
        .take(nth)
        .map(|ch| unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum();
    (col + skip) as u16
}

/// Drag from `(start_x, start_row)` to `(end_x, end_row)`, release, and return
/// the copied text. Also checks that the status metrics match it.
fn drag_copy_framed(app: &mut App, start: (u16, u16), end: (u16, u16)) -> String {
    for (kind, (column, row)) in [
        (MouseEventKind::Down(MouseButton::Left), start),
        (MouseEventKind::Drag(MouseButton::Left), end),
    ] {
        app.handle_copy_selection_mouse_with(
            MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::empty(),
            },
            |_| true,
        );
    }
    let range = app
        .normalized_copy_selection()
        .expect("normalized selection range");
    let metrics = crate::tui::ui::copy_selection_metrics(range).expect("selection metrics");
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();
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
    let text = copied.lock().unwrap().clone();
    assert_eq!(
        metrics,
        (text.chars().count(), text.split('\n').count().max(1)),
        "status metrics should match copied text {text:?}"
    );
    text
}

#[test]
fn test_copy_selection_drag_over_code_block_and_quote_excludes_frame() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let screen = render_copy_fixture(
        &mut app,
        &mut terminal,
        "Intro line.\n\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\n> quoted one\n> quoted two\n\nAfter.",
    );
    let start_row = row_containing(&screen, "Intro line.");
    let end_row = row_containing(&screen, "After.");
    let layout = crate::tui::ui::last_layout_snapshot().expect("layout");
    let columns = layout.messages_area.x..layout.messages_area.x + layout.messages_area.width;
    // Leftmost selectable cell of the first row and rightmost of the last row,
    // so the drag spans every code/quote gutter in between.
    let start_x = columns
        .clone()
        .find(|&column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, start_row).is_some()
        })
        .expect("selectable start column");
    let end_x = columns
        .rev()
        .find(|&column| crate::tui::ui::copy_viewport_point_from_screen(column, end_row).is_some())
        .expect("selectable end column");
    let text = drag_copy_framed(&mut app, (start_x, start_row), (end_x, end_row));
    assert!(
        screen.contains("│ fn main() {"),
        "fixture should draw a gutter:\n{screen}"
    );
    assert!(!text.contains('│'), "copied text has gutter: {text:?}");
    assert!(
        !text.contains("┌─") && !text.contains("└─"),
        "copied frame edge: {text:?}"
    );
    assert!(
        text.contains("fn main() {\n    println!(\"hello\");\n}"),
        "code indentation should survive: {text:?}"
    );
    assert!(text.contains("quoted one"), "quote text missing: {text:?}");
    assert!(
        text.contains("Intro line.") && text.contains("After."),
        "{text:?}"
    );
}

#[test]
fn test_copy_selection_starting_inside_code_keeps_literal_bars() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let screen = render_copy_fixture(
        &mut app,
        &mut terminal,
        "Tree:\n\n```text\n│ tree\n└─ leaf\n```\n\nAfter.",
    );
    let tree_row = row_containing(&screen, "│ │ tree");
    let leaf_row = row_containing(&screen, "│ └─ leaf");

    // Starts on the code row below the `┌─` header, at the frame gutter.
    let gutter_x = column_of(&screen, tree_row, "│ │ tree", 0);
    let leaf_end = column_of(&screen, leaf_row, "leaf", 4);
    assert_eq!(
        drag_copy_framed(&mut app, (gutter_x, tree_row), (leaf_end, leaf_row)),
        "│ tree\n└─ leaf"
    );

    // Selecting only the code content keeps its leading bar.
    let code_x = column_of(&screen, tree_row, "│ │ tree", 2);
    let tree_end = column_of(&screen, tree_row, "tree", 4);
    assert_eq!(
        drag_copy_framed(&mut app, (code_x, tree_row), (tree_end, tree_row)),
        "│ tree"
    );
}

#[test]
fn test_copy_selection_inline_code_frame_chars_are_content() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let screen = render_copy_fixture(
        &mut app,
        &mut terminal,
        "Before.\n\n`│ tree` row\n\n`┌─ title` heading\n\nAfter.",
    );
    let tree_row = row_containing(&screen, "│ tree row");
    let title_row = row_containing(&screen, "┌─ title heading");
    let start_x = column_of(&screen, tree_row, "│ tree row", 0);
    let end_x = column_of(&screen, title_row, "heading", 7);
    assert_eq!(
        drag_copy_framed(&mut app, (start_x, tree_row), (end_x, title_row)),
        "│ tree row\n\n┌─ title heading"
    );
}

#[test]
fn test_copy_selection_quoted_math_excludes_frame_edges() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let screen = render_copy_fixture(
        &mut app,
        &mut terminal,
        "> before\n>\n> $$\n> x^2\n> $$\n>\n> after",
    );
    let start_row = row_containing(&screen, "before");
    let end_row = row_containing(&screen, "after");
    let text = drag_copy_framed(
        &mut app,
        (column_of(&screen, start_row, "before", 0), start_row),
        (column_of(&screen, end_row, "after", 5), end_row),
    );
    assert!(!text.contains('┌') && !text.contains('└'), "{text:?}");
    assert!(!text.contains('│'), "{text:?}");
    assert!(
        text.starts_with("before") && text.ends_with("after"),
        "{text:?}"
    );
}

#[test]
fn test_copy_selection_table_keeps_empty_cell_padding() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let screen = render_copy_fixture(
        &mut app,
        &mut terminal,
        "| key | val |\n|---|---|\n|  | value |\n| k2 | v2 |",
    );
    let empty_row = row_containing(&screen, "│ value");
    let last_row = row_containing(&screen, "v2");
    // Start at the row's first selectable cell so the empty cell's padding,
    // which renders as spaces before the `│`, is inside the selection.
    let layout = crate::tui::ui::last_layout_snapshot().expect("layout");
    let start_x = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .find(|&column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, empty_row).is_some()
        })
        .expect("selectable start column");
    let text = drag_copy_framed(
        &mut app,
        (start_x, empty_row),
        (column_of(&screen, last_row, "v2", 2), last_row),
    );
    let first = text.lines().next().unwrap_or_default();
    assert!(
        first.trim_start().starts_with("│ value") && first.starts_with(' '),
        "empty cell padding or separator dropped: {text:?}"
    );
    assert!(text.contains("│ v2"), "{text:?}");
}
