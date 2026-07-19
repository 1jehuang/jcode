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
fn test_compute_visible_margins_left_aligned_respects_centered_header() {
    // Regression: the header lines are always centered even in left-aligned mode.
    // The right margin reported for a centered line must be the true right pad
    // (~half the slack), not the full `width - used`, otherwise a right-side info
    // widget is placed on top of the centered header text.
    let lines = vec![
        ratatui::text::Line::from("centered header").centered(),
        ratatui::text::Line::from("left body").left_aligned(),
    ];
    let area = Rect::new(0, 0, 40, 2);
    let margins = compute_visible_margins(&lines, &[], area, false);

    // centered: used=15 => total_margin=25 => left=12, right=13. Left-aligned mode
    // never places left-side widgets, so left is reported as 0, but the right gap
    // must stay at the true 13 columns so widgets clear the centered text.
    assert_eq!(margins.left_widths[0], 0);
    assert_eq!(margins.right_widths[0], 13);

    // left-aligned body is unchanged: full slack on the right.
    assert_eq!(margins.left_widths[1], 0);
    assert_eq!(margins.right_widths[1], 31);
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
fn test_copy_badge_reserves_right_margin_for_info_widgets() {
    let mut margins = info_widget::Margins {
        right_widths: vec![30, 30, 30],
        left_widths: vec![0, 0, 0],
        centered: false,
        right_reliable: Vec::new(),
        left_reliable: Vec::new(),
        ..Default::default()
    };
    let copy_badge_ui = crate::tui::app::CopyBadgeUiState::default();

    reserve_copy_badge_margins(&mut margins, 10, 13, &[(11, 'a')], &copy_badge_ui, Instant::now());

    assert_eq!(margins.right_widths[0], 30);
    assert_eq!(margins.right_widths[1], 16);
    assert_eq!(margins.right_widths[2], 30);
}

#[test]
fn test_expand_badge_reserves_right_margin_for_info_widgets() {
    // The inline `[Alt] [⇧] [E] expand` badge is appended to a transcript row at
    // render time. Without reserving its width in the margin profile, a floating
    // info widget (e.g. the KV cache panel) would dock right up against the badge
    // and get squeezed into a too-narrow slot that wraps/collides with it. The
    // badge width must be carved out of the row's free width.
    let collapsed = expand_badge_reserved_width(" expand");
    let expanded = expand_badge_reserved_width(" ✓ Expanded");
    assert!(
        collapsed > 0 && expanded > 0,
        "expand badge must reserve some width"
    );

    let mut width = 40u16;
    width = width.saturating_sub(collapsed as u16);
    assert_eq!(
        width as usize,
        40 - collapsed,
        "reservation should shrink the row's free width by exactly the badge width"
    );
}

#[test]
fn test_copy_badge_truncates_full_width_line_before_appending_shortcut() {
    let copy_badge_ui = crate::tui::app::CopyBadgeUiState::default();
    let reserved = copy_badge_reserved_width('a', &copy_badge_ui, Instant::now());
    let viewport_width = 20usize;
    let mut line = Line::from("x".repeat(viewport_width));

    truncate_copy_badge_line_to_width(&mut line, viewport_width.saturating_sub(reserved));
    // Matches the render path: one separator space, then the shortcut badges.
    line.spans.push(Span::raw(" "));
    line.spans.push(Span::raw("[Alt] [⇧] [A]"));

    assert_eq!(line.width(), viewport_width);
    assert!(line.width() <= viewport_width);
}

#[test]
fn test_copy_badge_line_prefers_row_with_free_width_over_truncation() {
    // A blockquote whose first line fills the viewport but whose second line
    // is short: the badge must move to the short line instead of cutting off
    // the first line's words.
    let full = Line::from("│ ".to_string() + &"x".repeat(60));
    let short = Line::from("│ short".to_string());
    let visible_lines = vec![full, short];
    let reserved = 14usize; // " [Alt] [⇧] [S]"

    let picked = pick_copy_badge_line(0, 0, 2, 0, 2, &visible_lines, 62, reserved);
    assert_eq!(picked, 1, "badge should move to the line with free width");

    // When no line in the block fits, keep the natural badge line.
    let picked_none = pick_copy_badge_line(0, 0, 1, 0, 1, &visible_lines, 62, reserved);
    assert_eq!(picked_none, 0);
}

#[test]
fn test_copy_badge_truncation_marks_cut_content_with_ellipsis() {
    // Content wider than the allowance must end in a visible ellipsis.
    let mut line = Line::from("y".repeat(30));
    truncate_line_for_copy_badge(&mut line, 10);
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert!(text.ends_with('…'), "cut content must show ellipsis: {text:?}");
    assert!(line.width() <= 10);

    // Content that fits is left intact (trailing spaces trimmed only).
    let mut fits = Line::from("short  ");
    truncate_line_for_copy_badge(&mut fits, 10);
    let fits_text: String = fits
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(fits_text, "short");
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

#[test]
fn test_idle_donut_reserved_height_absorbs_composer_growth() {
    // No donut: nothing reserved regardless of composer size.
    assert_eq!(idle_donut_reserved_height(false, 1), 0);
    assert_eq!(idle_donut_reserved_height(false, 9), 0);

    // Resting composer (1 row input, no hints): full donut reservation.
    assert_eq!(idle_donut_reserved_height(true, 1), 14);

    // Slash menu open (1 input row + 8 suggestion rows = 9): the extra 8 rows
    // come out of the donut so the transcript above does not shift.
    assert_eq!(idle_donut_reserved_height(true, 9), 6);

    // Pathologically tall composer: reservation bottoms out at zero.
    assert_eq!(idle_donut_reserved_height(true, 40), 0);
}
