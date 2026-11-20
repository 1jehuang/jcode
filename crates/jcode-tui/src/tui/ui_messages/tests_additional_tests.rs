use super::*;

#[test]
fn pinned_dependency_label_uses_available_width_and_palette() {
    let task = TodoCardBackgroundTask {
        task_id: "resume".into(),
        label: "Resume lint validation".into(),
        percent: Some(38.0),
    };
    let line = todo_dependency_progress(&[&task], 60);
    assert!(line.to_string().contains(&task.label));
    assert_eq!(line.spans[0].style.fg, Some(todo_meta_color()));
    for width in 0..100 {
        assert!(todo_dependency_progress(&[&task], width).width() <= width);
    }
}

#[test]
fn expanded_diff_rows_preserve_source_and_align_continuations() {
    let source = "    let result = compute(\"界界 e\u{301}\", argument);";
    let parsed = ParsedDiffLine {
        kind: DiffLineKind::Add,
        prefix: "9+ ".into(),
        content: source.into(),
        file_path: None,
    };
    let rows = expanded_diff_row(&parsed, Some("rs"), 3, 24);
    assert!(rows.len() > 1);
    assert!(extract_line_text(&rows[0]).starts_with("│   9 +     let"));
    let mut reconstructed = String::new();
    for (index, row) in rows.iter().enumerate() {
        assert!(row.width() <= 24, "{row:?}");
        assert_eq!(row.alignment, Some(Alignment::Left));
        if index > 0 {
            assert!(extract_line_text(row).starts_with("│     ↪ "));
        }
        for span in row.spans.iter().skip(if index == 0 { 2 } else { 1 }) {
            reconstructed.push_str(&span.content);
        }
    }
    assert_eq!(reconstructed, source);
    assert_eq!(rows[0].spans[1].style.fg, Some(diff_add_color()));
}
