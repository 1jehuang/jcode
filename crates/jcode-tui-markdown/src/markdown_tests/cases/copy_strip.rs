// Drag-select copy strips the frame art this renderer draws around code
// blocks, display math, and blockquotes (issue #995).

fn rendered_text(markdown: &str) -> String {
    render_markdown(markdown)
        .iter()
        .map(line_plain_text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn test_strip_rendered_frame_decorations_round_trips_rendered_frames() {
    let code = rendered_text("before\n\n```python\ndef f():\n    return 1\n```\n\nafter");
    assert!(
        code.contains("│ def f():"),
        "fixture lost its gutter: {code:?}"
    );
    assert_eq!(
        strip_rendered_frame_decorations(&code),
        "before\n\ndef f():\n    return 1\n\nafter"
    );

    let quote = rendered_text("> Outer\n>\n>> Inner\n\nafter");
    let stripped = strip_rendered_frame_decorations(&quote);
    assert!(!stripped.contains('│'), "{stripped:?}");
    assert!(stripped.contains("Outer") && stripped.contains("Inner"));

    // Centered layout indents the whole frame; the indent goes with it.
    assert_eq!(
        strip_rendered_frame_decorations("    ┌─ rust \n    │ let x = 1;\n    └─"),
        "let x = 1;"
    );
}

#[test]
fn test_strip_rendered_frame_decorations_leaves_tables_and_prose() {
    let table = rendered_text("| a | b |\n|---|---|\n| 1 | 2 |");
    assert!(
        table.contains(" │ "),
        "fixture should be a table: {table:?}"
    );
    assert_eq!(strip_rendered_frame_decorations(&table), table);
    let prose = "pipes like a│b stay";
    assert_eq!(strip_rendered_frame_decorations(prose), prose);
}

#[test]
fn test_strip_rendered_frame_decorations_keeps_box_drawing_inside_code() {
    let code = rendered_text("```text\n│ tree\n└─ leaf\n```");
    assert_eq!(
        strip_rendered_frame_decorations(&code),
        "│ tree\n└─ leaf",
        "rendered: {code:?}"
    );
}

/// Classify `rows` in `range` with the full rendered block as context, the way
/// a mid-block selection is classified in the TUI.
fn classify_range(rows: &[&str], range: std::ops::Range<usize>) -> Vec<FrameRow> {
    let mut kinds = Vec::new();
    classify_frame_rows(
        |idx| rows.get(idx).copied(),
        range,
        |_, _, kind| kinds.push(kind),
    )
    .expect("rows in range");
    kinds
}

#[test]
fn test_classify_frame_rows_mid_code_selection_keeps_literal_bars() {
    let rendered = rendered_text("```text\n│ tree\n└─ leaf\n```");
    let rows: Vec<&str> = rendered.split('\n').collect();
    let tree = rows
        .iter()
        .position(|row| row.ends_with("│ │ tree"))
        .expect("tree row");
    // Selection starts on the code row, below the `┌─` header. Only the frame
    // gutter is art: the code's own `│ ` and `└─` are content.
    let kinds = classify_range(&rows, tree..tree + 2);
    let gutter = rows[tree].find("│ ").unwrap() + 2;
    assert_eq!(
        kinds,
        vec![FrameRow::Gutter(gutter), FrameRow::Gutter(gutter)]
    );
}

#[test]
fn test_classify_frame_rows_keeps_table_cell_padding() {
    let table = rendered_text("| a | b |\n|---|---|\n|  | value |\n| x | y |");
    let rows: Vec<&str> = table.split('\n').collect();
    let empty_cell = rows
        .iter()
        .position(|row| row.trim_start().starts_with("│ value"))
        .unwrap_or_else(|| panic!("fixture should render an empty first cell: {table:?}"));
    for start in [0, empty_cell] {
        assert!(
            classify_range(&rows, start..rows.len())
                .iter()
                .all(|kind| *kind == FrameRow::Content),
            "table rows must keep their padding: {table:?}"
        );
    }
    assert_eq!(strip_rendered_frame_decorations(&table), table);
}

#[test]
fn test_classify_frame_rows_wrapped_table_rows_keep_padding() {
    // A wrapped first cell leaves its continuation row blank before the `│`.
    let table = "a        │ b\n─────────┼──\nlong cell│ v\nwraps    │  \n         │ z";
    assert_eq!(strip_rendered_frame_decorations(table), table);
}

#[test]
fn test_strip_rendered_frame_decorations_drops_quoted_math_edges() {
    let quoted = rendered_text("> before\n>\n> $$\n> x^2\n> $$\n>\n> after");
    assert!(
        quoted.contains("│ ┌─ math"),
        "fixture should nest math in a quote: {quoted:?}"
    );
    let stripped = strip_rendered_frame_decorations(&quoted);
    assert!(
        !stripped.contains('┌') && !stripped.contains('└'),
        "{stripped:?}"
    );
    assert!(!stripped.contains('│'), "{stripped:?}");
    assert!(stripped.contains("before") && stripped.contains("after"));
    assert!(stripped.contains("x²"), "{stripped:?}");
}

#[test]
fn test_strip_rendered_frame_decorations_quoted_code_keeps_literal_bars() {
    let stripped = strip_rendered_frame_decorations("│ ┌─ text \n│ │ │ tree\n│ └─\n│ after");
    assert_eq!(stripped, "│ tree\nafter");
}

#[test]
fn test_classify_frame_rows_multi_file_diff_drops_divider_and_footer() {
    let rows = [
        "┌─ diff · a",
        "│ +a",
        "├─ diff · b",
        "│ +b",
        "└─ (+2 -0 total)",
    ];
    // Selection starts below the `├─` divider and ends on the footer.
    assert_eq!(
        classify_range(&rows, 3..5),
        vec![FrameRow::Gutter(2), FrameRow::Edge]
    );
    assert_eq!(strip_rendered_frame_decorations(&rows.join("\n")), "+a\n+b");
}
