#[test]
fn test_file_diff_cache_reuses_entry_when_signature_matches() {
    let temp = tempfile::NamedTempFile::new().expect("temp file");
    std::fs::write(temp.path(), "fn main() {}\n").expect("write file");
    let path = temp.path().to_string_lossy().to_string();

    let state = file_diff_cache();
    {
        let mut cache = state.lock().expect("cache lock");
        cache.entries.clear();
        cache.order.clear();
        let key = FileDiffCacheKey {
            file_path: path.clone(),
            msg_index: 1,
        };
        let sig = file_content_signature(&path);
        cache.insert(
            key.clone(),
            FileDiffViewCacheEntry {
                file_sig: sig.clone(),
                rows: vec![file_diff_ui::FileDiffDisplayRow {
                    prefix: String::new(),
                    text: "cached".to_string(),
                    kind: file_diff_ui::FileDiffDisplayRowKind::Placeholder,
                }],
                rendered_rows: vec![Some(Line::from("cached"))],
                first_change_line: 0,
                additions: 1,
                deletions: 0,
                file_ext: None,
            },
        );

        let cached = cache.entries.get(&key).expect("cached entry");
        assert_eq!(cached.file_sig, sig);
    }
}

#[test]
fn test_calculate_input_lines_single_line() {
    assert_eq!(calculate_input_lines("hello", 80), 1);
    assert_eq!(calculate_input_lines("hello world", 80), 1);
}

#[test]
fn test_calculate_input_lines_wrapped() {
    // 10 chars with width 5 = 2 lines
    assert_eq!(calculate_input_lines("aaaaaaaaaa", 5), 2);
    // 15 chars with width 5 = 3 lines
    assert_eq!(calculate_input_lines("aaaaaaaaaaaaaaa", 5), 3);
}

#[test]
fn test_calculate_input_lines_with_newlines() {
    // Two lines separated by newline
    assert_eq!(calculate_input_lines("hello\nworld", 80), 2);
    // Three lines
    assert_eq!(calculate_input_lines("a\nb\nc", 80), 3);
    // Trailing newline
    assert_eq!(calculate_input_lines("hello\n", 80), 2);
}

#[test]
fn test_calculate_input_lines_newlines_and_wrapping() {
    // First line wraps (10 chars / 5 = 2), second line is short (1)
    assert_eq!(calculate_input_lines("aaaaaaaaaa\nb", 5), 3);
}

#[test]
fn test_calculate_input_lines_zero_width() {
    assert_eq!(calculate_input_lines("hello", 0), 1);
}

#[test]
fn test_wrap_input_text_empty() {
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("", 0, 80, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 1);
    assert_eq!(cursor_line, 0);
    assert_eq!(cursor_col, 0);
}

#[test]
fn test_wrap_input_text_simple() {
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("hello", 5, 80, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 1);
    assert_eq!(cursor_line, 0);
    assert_eq!(cursor_col, 5); // cursor at end
}

#[test]
fn test_wrap_input_text_cursor_middle() {
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("hello world", 6, 80, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 1);
    assert_eq!(cursor_line, 0);
    assert_eq!(cursor_col, 6); // cursor at 'w'
}

#[test]
fn test_wrap_input_text_wrapping() {
    // 10 chars with width 5 = 2 lines
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("aaaaaaaaaa", 7, 5, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 2);
    assert_eq!(cursor_line, 1); // second line
    assert_eq!(cursor_col, 2); // 7 - 5 = 2
}

#[test]
fn test_wrap_input_text_with_newlines() {
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("hello\nworld", 6, 80, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 2);
    assert_eq!(cursor_line, 1); // second line (after newline)
    assert_eq!(cursor_col, 0); // at start of 'world'
}

#[test]
fn test_wrap_input_text_cursor_at_end_of_wrapped() {
    // 10 chars with width 5, cursor at position 10 (end)
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("aaaaaaaaaa", 10, 5, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 2);
    assert_eq!(cursor_line, 1);
    assert_eq!(cursor_col, 5);
}

#[test]
fn test_wrap_input_text_many_lines() {
    // Create text that spans 15 lines when wrapped to width 10
    let text = "a".repeat(150);
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text(&text, 145, 10, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 15);
    assert_eq!(cursor_line, 14); // last line
    assert_eq!(cursor_col, 5); // 145 % 10 = 5
}

#[test]
fn test_wrap_input_text_multiple_newlines() {
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("a\nb\nc\nd", 6, 80, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 4);
    assert_eq!(cursor_line, 3); // on 'd' line
    assert_eq!(cursor_col, 0);
}

#[test]
fn test_wrapped_input_line_count_respects_two_digit_prompt_width() {
    let mut app = TestState {
        input: "abcdefghijk".to_string(),
        cursor_pos: "abcdefghijk".len(),
        ..Default::default()
    };
    for _ in 0..9 {
        app.display_messages.push(DisplayMessage {
            role: "user".to_string(),
            content: "previous".to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: None,
        });
    }

    // Old layout math effectively used width 11 here (14 total - hardcoded prompt width 3),
    // which incorrectly fit this input on a single line. The real prompt is "10> ", width 4,
    // so the wrapped renderer only has 10 columns and must use 2 lines.
    assert_eq!(calculate_input_lines(app.input(), 11), 1);
    assert_eq!(input_ui::wrapped_input_line_count(&app, 14, 10), 2);
}

#[test]
fn test_compute_visible_margins_centered_respects_line_alignment() {
    let lines = vec![
        ratatui::text::Line::from("centered").centered(),
        ratatui::text::Line::from("left block").left_aligned(),
        ratatui::text::Line::from("right").right_aligned(),
    ];
    let area = Rect::new(0, 0, 20, 3);
    let margins = compute_visible_margins(&lines, &[], area, true);

    // centered: used=8 => total_margin=12 => 6/6 split
    assert_eq!(margins.left_widths[0], 6);
    assert_eq!(margins.right_widths[0], 6);

    // left-aligned: used=10 => left=0, right=10
    assert_eq!(margins.left_widths[1], 0);
    assert_eq!(margins.right_widths[1], 10);

    // right-aligned: used=5 => left=15, right=0
    assert_eq!(margins.left_widths[2], 15);
    assert_eq!(margins.right_widths[2], 0);
}

#[test]
fn test_estimate_pinned_diagram_pane_width_scales_to_height() {
    let diagram = info_widget::DiagramInfo {
        hash: 1,
        width: 800,
        height: 600,
        label: None,
    };
    let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 20, 24, Some((8, 16)));
    assert_eq!(width, 50);
}

#[test]
fn test_estimate_pinned_diagram_pane_width_respects_minimum() {
    let diagram = info_widget::DiagramInfo {
        hash: 2,
        width: 120,
        height: 120,
        label: None,
    };
    let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 10, 24, Some((8, 16)));
    assert_eq!(width, 24);
}
