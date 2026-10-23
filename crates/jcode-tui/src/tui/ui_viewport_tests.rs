#[test]
fn pinned_todos_scroll_when_only_the_band_causes_overflow() {
    let area = ratatui::layout::Rect::new(0, 0, 80, 10);
    // The transcript alone fits, but the pinned band leaves only seven rows.
    assert_eq!(
        super::compute_max_scroll_with_prompt_preview(9, &[], &[], area, 3, 0),
        2
    );
    assert_eq!(
        super::compute_max_scroll_with_prompt_preview(7, &[], &[], area, 3, 0),
        0
    );
    assert_eq!(
        super::compute_max_scroll_with_prompt_preview(9, &[], &[], area, 0, 0),
        0
    );
}

#[test]
fn ctrl_l_preview_measurement_keeps_global_prompt_number_width() {
    let prompts = vec!["x".repeat(15)];
    assert_eq!(
        super::compute_prompt_preview_line_count(&[0], &prompts, 1, 20, 0),
        1
    );
    assert_eq!(
        super::compute_prompt_preview_line_count(&[0], &prompts, 1, 20, 9),
        2
    );
}

#[test]
fn handterm_native_latex_cell_symbol_moves_invokes_and_restores_cursor() {
    assert_eq!(
        super::handterm_native_latex_cell_symbol(r"\frac{a}{b}", 3, 2).as_deref(),
        Some("\x1b[s\x1b[2A\x1b[1D\x1b_L;\\frac{a}{b}\x1b\\\x1b[u\x1b[1C")
    );
    assert_eq!(
        super::handterm_native_latex_cell_symbol("x", 1, 1).as_deref(),
        Some("\x1b[s\x1b_L;x\x1b\\\x1b[u\x1b[1C")
    );
    assert!(super::handterm_native_latex_cell_symbol("x\x1by", 1, 1).is_none());
}

#[test]
fn tail_follow_small_appends_snap_to_bottom() {
    // Streaming-sized appends (<= min jump) snap directly; no animation.
    crate::tui::ui::set_last_resolved_chat_scroll(100);
    let scroll = super::resolve_tail_follow_scroll(103, 30);
    assert_eq!(scroll, 103);
    assert!(!crate::tui::ui::tail_catchup_active());
}

#[test]
fn tail_follow_large_append_slides_in_bounded_steps() {
    // A 12-row jump advances by at most TAIL_CATCHUP_MAX_STEP per frame
    // and reports an active catch-up until it reaches the bottom.
    crate::tui::ui::set_last_resolved_chat_scroll(100);
    let first = super::resolve_tail_follow_scroll(112, 30);
    assert!(first < 112, "must not snap: {first}");
    assert!(
        first - 100 <= super::TAIL_CATCHUP_MAX_STEP,
        "step bounded: {first}"
    );
    assert!(crate::tui::ui::tail_catchup_active());

    // Subsequent frames converge to the bottom and clear the flag.
    let mut scroll = first;
    let mut guard = 0;
    while scroll < 112 {
        crate::tui::ui::set_last_resolved_chat_scroll(scroll);
        scroll = super::resolve_tail_follow_scroll(112, 30);
        guard += 1;
        assert!(guard < 50, "catch-up must converge");
    }
    assert_eq!(scroll, 112);
    assert!(!crate::tui::ui::tail_catchup_active());
}

#[test]
fn explicit_tail_follow_request_skips_content_catch_up_animation() {
    crate::tui::ui::set_last_resolved_chat_scroll(40);
    crate::tui::ui::request_tail_follow_snap();

    let scroll = super::resolve_tail_follow_scroll(80, 30);

    assert_eq!(scroll, 80);
    assert!(!crate::tui::ui::tail_catchup_active());
}

#[test]
fn tail_follow_caps_lag_to_one_viewport() {
    // A huge append (way beyond a screen) starts at most one viewport
    // behind the bottom so the catch-up never replays pages of content.
    crate::tui::ui::set_last_resolved_chat_scroll(100);
    let scroll = super::resolve_tail_follow_scroll(400, 30);
    assert!(scroll >= 400 - 30, "lag capped to viewport: {scroll}");
    assert!(crate::tui::ui::tail_catchup_active());
}

#[test]
fn tail_follow_backward_motion_snaps() {
    // Content shrank (commit collapsed reasoning): snap, don't animate.
    crate::tui::ui::set_last_resolved_chat_scroll(100);
    let scroll = super::resolve_tail_follow_scroll(80, 30);
    assert_eq!(scroll, 80);
    assert!(!crate::tui::ui::tail_catchup_active());
}

#[test]
fn default_copy_badge_alt_label_matches_platform() {
    #[cfg(target_os = "macos")]
    assert_eq!(super::copy_badge_alt_label_from_config(""), "⌥");

    #[cfg(not(target_os = "macos"))]
    assert_eq!(super::copy_badge_alt_label_from_config(""), "Alt");
}

#[test]
fn copy_badge_alt_label_uses_trimmed_config_override() {
    assert_eq!(
        super::copy_badge_alt_label_from_config(" Option "),
        "Option"
    );
    assert_eq!(super::copy_badge_alt_label_from_config("⌥"), "⌥");
}

#[test]
fn expanded_prompt_preview_lists_recent_offscreen_prompts_oldest_first() {
    let starts: Vec<usize> = (0..10).map(|i| i * 10).collect();
    let texts: Vec<String> = (0..10).map(|i| format!("prompt  {i}\nline")).collect();
    // Scroll 75: prompts 0..=7 start above the viewport.
    let items = super::expanded_prompt_preview_items(&starts, &texts, 75, 0);
    assert_eq!(items.len(), super::EXPANDED_PROMPT_PREVIEW_COUNT);
    assert_eq!(items.first().unwrap().0, "3");
    assert_eq!(
        items.last().unwrap(),
        &("8".to_string(), "prompt 7 line".to_string())
    );
}
#[test]
fn expand_badge_uses_footer_for_inline_and_tool_row_for_hidden_diffs() {
    let active = super::ActiveFileDiffContext {
        edit_index: 1,
        msg_index: 0,
        file_path: "src/main.rs".into(),
        start_line: 10,
        end_line: 25,
        expandable: true,
    };
    assert_eq!(super::expand_badge_target_line(&active, true), 24);
    assert_eq!(super::expand_badge_target_line(&active, false), 10);
    // The footer remains the target even when the tool row has scrolled away.
    let scroll = 20;
    assert!(super::expand_badge_target_line(&active, true) >= scroll);
    assert!(super::expand_badge_target_line(&active, false) < scroll);
}

#[test]
fn compact_expand_badge_reserves_exact_display_cells() {
    use unicode_width::UnicodeWidthStr;
    for suffix in [" expand", " ✓ Expanded"] {
        let rendered = format!(" [{}⇧E]{suffix}", super::copy_badge_alt_label());
        assert_eq!(super::expand_badge_reserved_width(suffix), rendered.width());
        let previous = format!(" {} [⇧] [E]{suffix}", super::copy_badge_alt_badge());
        assert!(rendered.width() < previous.width());
    }
}

#[test]
fn exact_width_expand_badge_leaves_no_content_or_ellipsis() {
    let mut line = ratatui::text::Line::from("╰─ 28 hidden");
    super::truncate_line_for_copy_badge(&mut line, 0);
    assert_eq!(line.width(), 0);
    assert!(line.spans.is_empty());
}
