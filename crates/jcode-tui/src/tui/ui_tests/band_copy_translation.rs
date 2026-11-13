use super::*;

// Copy-translation tests for the pinned Overview band: display rows map back
// into the wide frame through `CopyRowBinding`s, so selection and copy stay
// exact even when the band re-wraps the top rows narrow.

fn single_line_frame(lines: &[&str]) -> Arc<PreparedChatFrame> {
    let prepared = Arc::new(PreparedMessages {
        wrapped_lines: lines
            .iter()
            .map(|text| Line::from(text.to_string()))
            .collect(),
        wrapped_plain_lines: Arc::new(lines.iter().map(|text| text.to_string()).collect()),
        wrapped_copy_offsets: Arc::new(vec![0; lines.len()]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: Vec::new(),
        mermaid_pending_epoch: None,
    });
    Arc::new(PreparedChatFrame::from_single(prepared))
}

fn record_band_frame(
    prepared: Arc<PreparedChatFrame>,
    bindings: Vec<viewport::CopyRowBinding>,
    content_area: Rect,
) {
    crate::tui::ui::clear_copy_viewport_snapshot();
    crate::tui::ui::record_copy_viewport_frame_snapshot_for_test(
        prepared,
        0,
        content_area.height as usize,
        content_area,
        &vec![0u16; content_area.height as usize],
        Some(Arc::new(bindings)),
    );
}

#[test]
fn copy_point_translates_straddle_fragment_rows() {
    let _guard = viewport_snapshot_test_lock();
    // Wide line 0 wraps into two band fragments (narrow width 10); line 1
    // stays identity below the band. Display row 0 covers wide columns
    // [0, 10) of line 0, row 1 covers [10, 27) plus the re-seeded gutter.
    let prepared = single_line_frame(&["hello world, band split", "next line"]);
    let content_area = Rect::new(0, 0, 27, 4);
    let bindings = vec![
        viewport::CopyRowBinding {
            wide_line: 0,
            shift: 0,
            floor: 0,
            ceil: 10,
        },
        viewport::CopyRowBinding {
            wide_line: 0,
            shift: 1,
            floor: 10,
            ceil: 23,
        },
        viewport::CopyRowBinding {
            wide_line: 1,
            shift: 0,
            floor: 0,
            ceil: 9,
        },
        viewport::CopyRowBinding {
            wide_line: 2,
            shift: 0,
            floor: 0,
            ceil: 0,
        },
    ];
    record_band_frame(prepared, bindings, content_area);

    let point = crate::tui::ui::copy_viewport_point_from_screen(3, 0)
        .expect("row 0 is a band fragment of wide line 0");
    assert_eq!(point.abs_line, 0);
    assert_eq!(point.column, 3);

    let point = crate::tui::ui::copy_viewport_point_from_screen(0, 1)
        .expect("row 1 is the continuation fragment of wide line 0");
    assert_eq!(point.abs_line, 0);
    // The continuation row's display column 0 maps into the wide frame
    // past the fragment start (shifted past the re-seeded gutter).
    let continuation_column = point.column;
    assert!(
        continuation_column >= 10,
        "continuation column {continuation_column}"
    );
}

#[test]
fn copy_point_rejects_padding_rows() {
    let _guard = viewport_snapshot_test_lock();
    // The last display row is blank padding past the frame's end: the wide
    // line it binds to is beyond `visible_end`, so no copy point resolves.
    let prepared = single_line_frame(&["one line"]);
    let content_area = Rect::new(0, 0, 20, 3);
    let bindings = vec![
        viewport::CopyRowBinding {
            wide_line: 0,
            shift: 0,
            floor: 0,
            ceil: 7,
        },
        viewport::CopyRowBinding {
            wide_line: 1,
            shift: 0,
            floor: 0,
            ceil: 0,
        },
        viewport::CopyRowBinding {
            wide_line: 2,
            shift: 0,
            floor: 0,
            ceil: 0,
        },
    ];
    record_band_frame(prepared, bindings, content_area);

    assert!(
        crate::tui::ui::copy_viewport_point_from_screen(0, 2).is_none(),
        "padding rows must not resolve to a copy point"
    );
    assert!(
        crate::tui::ui::copy_viewport_point_from_screen(0, 1).is_none(),
        "rows past the frame's last line must not resolve either"
    );
    assert!(
        crate::tui::ui::copy_viewport_point_from_screen(0, 0).is_some(),
        "the real row still resolves"
    );
}

#[test]
fn copy_point_without_bindings_uses_plain_window() {
    let _guard = viewport_snapshot_test_lock();
    // No band: the plain scrolled window (`scroll + row`) stays authoritative.
    let prepared = single_line_frame(&["plain frame line", "second line"]);
    let content_area = Rect::new(0, 0, 20, 3);
    crate::tui::ui::clear_copy_viewport_snapshot();
    crate::tui::ui::record_copy_viewport_frame_snapshot_for_test(
        prepared,
        0,
        2,
        content_area,
        &[0u16; 3],
        None,
    );

    let point = crate::tui::ui::copy_viewport_point_from_screen(2, 0)
        .expect("row 0 maps to wide line 0 without bindings");
    assert_eq!(point.abs_line, 0);
    assert_eq!(point.column, 2);
    assert!(
        crate::tui::ui::copy_viewport_point_from_screen(0, 2).is_none(),
        "rows past visible_end stay out of range"
    );
}
