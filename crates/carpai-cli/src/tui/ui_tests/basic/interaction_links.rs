#[test]
fn test_link_target_from_screen_detects_chat_url() {
    let _lock = viewport_snapshot_test_lock();
    record_test_chat_snapshot("Docs: https://example.com/docs).");

    assert_eq!(
        link_target_from_screen(10, 0),
        Some("https://example.com/docs".to_string())
    );
}

#[test]
fn test_link_target_from_screen_detects_side_pane_url() {
    let _lock = viewport_snapshot_test_lock();
    clear_copy_viewport_snapshot();
    record_side_pane_snapshot(
        &[Line::from("See https://example.com/side for details")],
        0,
        1,
        Rect::new(40, 0, 40, 5),
    );

    assert_eq!(
        link_target_from_screen(45, 0),
        Some("https://example.com/side".to_string())
    );
}

#[test]
fn test_link_target_from_screen_returns_none_without_url() {
    let _lock = viewport_snapshot_test_lock();
    record_test_chat_snapshot("No links here");
    assert_eq!(link_target_from_screen(3, 0), None);
}

#[test]
fn test_prompt_entry_animation_detects_newly_visible_prompt_line() {
    reset_prompt_viewport_state_for_test();

    // First frame initializes viewport history and should not animate.
    update_prompt_entry_animation(&[5, 20], 0, 10, 1000);
    assert!(active_prompt_entry_animation(1000).is_none());

    // Scrolling down brings line 20 into view and should trigger animation.
    update_prompt_entry_animation(&[5, 20], 15, 25, 1100);
    let anim = active_prompt_entry_animation(1100).expect("expected active prompt animation");
    assert_eq!(anim.line_idx, 20);
}

#[test]
fn test_prompt_entry_animation_expires_after_window() {
    reset_prompt_viewport_state_for_test();

    update_prompt_entry_animation(&[5, 20], 0, 10, 2000);
    update_prompt_entry_animation(&[5, 20], 15, 25, 2100);

    assert!(active_prompt_entry_animation(2100).is_some());
    assert!(
        active_prompt_entry_animation(2100 + PROMPT_ENTRY_ANIMATION_MS + 1).is_none(),
        "animation should expire after configured duration"
    );
}

#[test]
fn test_prompt_entry_bg_color_pulses_then_fades() {
    let base = user_bg();
    let early = prompt_entry_bg_color(base, 0.15);
    let peak = prompt_entry_bg_color(base, 0.45);
    let late = prompt_entry_bg_color(base, 0.95);

    assert_ne!(early, base);
    assert_ne!(peak, base);
    assert_ne!(late, peak);
}

#[test]
fn test_prompt_entry_shimmer_color_moves_across_positions() {
    let base = user_text();
    let left_early = prompt_entry_shimmer_color(base, 0.1, 0.1);
    let right_early = prompt_entry_shimmer_color(base, 0.9, 0.1);
    let left_late = prompt_entry_shimmer_color(base, 0.1, 0.8);
    let right_late = prompt_entry_shimmer_color(base, 0.9, 0.8);

    assert_ne!(left_early, right_early);
    assert_ne!(left_late, right_late);
    assert_ne!(left_early, left_late);
}

#[test]
fn test_active_file_diff_context_resolves_visible_edit() {
    let prepared = PreparedMessages {
        wrapped_lines: vec![Line::from("a"); 20],
        wrapped_plain_lines: Arc::new(vec!["a".to_string(); 20]),
        wrapped_copy_offsets: Arc::new(vec![0; 20]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: vec![
            EditToolRange {
                edit_index: 0,
                msg_index: 3,
                file_path: "src/one.rs".to_string(),
                start_line: 2,
                end_line: 5,
            },
            EditToolRange {
                edit_index: 1,
                msg_index: 7,
                file_path: "src/two.rs".to_string(),
                start_line: 10,
                end_line: 14,
            },
        ],
        copy_targets: Vec::new(),
    };

    let prepared = PreparedChatFrame::from_single(Arc::new(prepared));
    let active = active_file_diff_context(&prepared, 9, 4).expect("visible edit context");
    assert_eq!(active.edit_index, 2);
    assert_eq!(active.msg_index, 7);
    assert_eq!(active.file_path, "src/two.rs");
}
