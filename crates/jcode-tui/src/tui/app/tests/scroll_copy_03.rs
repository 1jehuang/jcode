#[test]
fn test_scroll_ctrl_k_j_offset() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    assert_eq!(app.scroll_offset, 0);
    assert!(!app.auto_scroll_paused);

    let (up_code, up_mods) = scroll_up_key(&app);
    let (down_code, down_mods) = scroll_down_key(&app);

    // Render first so LAST_MAX_SCROLL is populated
    render_and_snap(&app, &mut terminal);

    // Scroll up (switches to absolute-from-top mode)
    app.handle_key(up_code.clone(), up_mods).unwrap();
    assert!(app.auto_scroll_paused);
    let first_offset = app.scroll_offset;

    app.handle_key(up_code.clone(), up_mods).unwrap();
    let second_offset = app.scroll_offset;
    assert!(
        second_offset < first_offset,
        "scrolling up should decrease absolute offset (move toward top)"
    );

    // Scroll down (increases absolute position = moves toward bottom)
    app.handle_key(down_code.clone(), down_mods).unwrap();
    assert_eq!(
        app.scroll_offset, first_offset,
        "one scroll down should undo one scroll up"
    );

    // Keep scrolling down until back at bottom
    for _ in 0..10 {
        app.handle_key(down_code.clone(), down_mods).unwrap();
        if !app.auto_scroll_paused {
            break;
        }
    }
    assert_eq!(app.scroll_offset, 0);
    assert!(!app.auto_scroll_paused);

    // Stays at 0 when already at bottom
    app.handle_key(down_code.clone(), down_mods).unwrap();
    assert_eq!(app.scroll_offset, 0);
}

#[test]
fn test_scroll_offset_capped() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 4);

    let (up_code, up_mods) = scroll_up_key(&app);

    // Render first so LAST_MAX_SCROLL is populated
    render_and_snap(&app, &mut terminal);

    // Spam scroll-up many times
    for _ in 0..500 {
        app.handle_key(up_code.clone(), up_mods).unwrap();
    }

    // Should be at 0 (absolute top) after scrolling up enough
    assert_eq!(app.scroll_offset, 0);
    assert!(app.auto_scroll_paused);
}

#[test]
fn test_scroll_render_bottom() {
    let _render_lock = scroll_render_test_lock();
    let (app, mut terminal) = create_scroll_test_app(80, 15, 1, 20);
    let text = render_and_snap(&app, &mut terminal);

    // At bottom (scroll_offset=0), filler content should be visible.
    assert!(
        text.contains("stretch content"),
        "expected filler content at bottom position"
    );
    // Should have scroll indicator or prompt preview since content extends above viewport.
    // The prompt preview (N›) renders on top of the ↑ indicator, so check for either.
    assert!(
        text.contains('↑') || text.contains('›'),
        "expected ↑ indicator or prompt preview when content extends above viewport"
    );
}

#[test]
fn test_scroll_render_scrolled_up() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 8);
    // The ↓ overflow counter is only rendered when the native scrollbar is off;
    // with the native scrollbar visible the scrollbar thumb replaces it (see
    // test_chat_native_scrollbar_hides_scroll_counters). Exercise the legacy
    // counter path this test was written for.
    app.chat_native_scrollbar = false;

    // Seed scroll metrics, then enter paused/scrolled mode via the real key path.
    let _ = render_and_snap(&app, &mut terminal);
    let (up_code, up_mods) = scroll_up_key(&app);
    app.handle_key(up_code, up_mods).unwrap();

    assert!(app.auto_scroll_paused, "scroll-up should pause auto-follow");

    let text_scrolled = render_and_snap(&app, &mut terminal);

    assert!(
        text_scrolled.contains('↓'),
        "expected ↓ indicator when paused above bottom"
    );
}

#[test]
fn test_prompt_preview_reserves_rows_without_overwriting_visible_history() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage {
            role: "user".to_string(),
            content: "This is a deliberately long prompt preview that should wrap into two preview rows at the top of the viewport".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
        DisplayMessage {
            role: "assistant".to_string(),
            content: App::build_scroll_test_content(0, 20, None),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(40, 8);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    let text = render_and_snap(&app, &mut terminal);

    assert!(
        text.contains("1›"),
        "expected sticky prompt preview, got:\n{}",
        text
    );
    assert!(
        text.contains("..."),
        "expected two-line preview truncation, got:\n{}",
        text
    );
    assert!(
        text.contains("Intro line 20"),
        "latest visible content should remain visible below preview, got:\n{}",
        text
    );
}

#[test]
fn test_scroll_top_does_not_snap_to_bottom() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 24);

    // Top position in paused mode (absolute offset from top).
    app.scroll_offset = 0;
    app.auto_scroll_paused = true;
    let text_top = render_and_snap(&app, &mut terminal);

    // Bottom position (auto-follow mode).
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    let text_bottom = render_and_snap(&app, &mut terminal);

    assert_ne!(
        text_top, text_bottom,
        "top viewport should differ from bottom viewport"
    );
    assert!(
        text_top.contains("Intro line 01"),
        "top viewport should include earliest content"
    );
}

#[test]
fn test_scroll_content_shifts() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 12);

    // Render at bottom
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    let text_bottom = render_and_snap(&app, &mut terminal);

    // Render scrolled up (absolute line 10 from top)
    app.scroll_offset = 10;
    app.auto_scroll_paused = true;
    let text_scrolled = render_and_snap(&app, &mut terminal);

    assert_ne!(
        text_bottom, text_scrolled,
        "content should change when scrolled"
    );
}

#[test]
fn test_scroll_render_with_mermaid() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 2, 10);

    // Render at several positions without crashing.
    for (offset, paused) in [(0, false), (5, true), (10, true), (20, true), (50, true)] {
        app.scroll_offset = offset;
        app.auto_scroll_paused = paused;
        terminal
            .draw(|f| crate::tui::ui::draw(f, &app))
            .unwrap_or_else(|e| panic!("draw failed at scroll_offset={}: {}", offset, e));
    }
}

#[test]
fn test_scroll_visual_debug_frame() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 10);

    crate::tui::visual_debug::enable();

    // Render at bottom, verify frame capture works
    app.scroll_offset = 0;
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw at offset=0 failed");

    let frame = crate::tui::visual_debug::latest_frame();
    assert!(frame.is_some(), "visual debug frame should be captured");

    // Render at scroll_offset=10, verify no panic
    app.scroll_offset = 10;
    app.auto_scroll_paused = true;
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw at offset=10 failed");

    // Note: latest_frame() is global and may be overwritten by parallel tests,
    // so we only verify the frame capture mechanism works, not exact values.
    let frame = crate::tui::visual_debug::latest_frame();
    assert!(
        frame.is_some(),
        "frame should still be available after second draw"
    );

    crate::tui::visual_debug::disable();
}

#[test]
fn test_full_redraw_clears_out_of_band_backend_artifacts_after_native_scroll_like_mutation() {
    let _lock = scroll_render_test_lock();

    let (mut app, mut terminal) = create_scroll_test_app(60, 12, 0, 24);
    app.auto_scroll_paused = true;
    app.scroll_offset = 6;
    let clean = render_and_snap(&app, &mut terminal);

    let width = terminal.backend().buffer().area.width;
    let ghost = ratatui::buffer::Buffer::with_lines([
        "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
    ]);
    let updates = ghost
        .content()
        .iter()
        .enumerate()
        .map(|(idx, cell)| ((idx as u16) % width, (idx as u16) / width, cell));
    terminal
        .backend_mut()
        .draw(updates)
        .expect("inject backend artifact");

    let stale = buffer_to_text(&terminal);
    assert!(
        stale.contains("ZZZZ"),
        "expected injected backend artifact before redraw, got:\n{stale}"
    );

    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("normal redraw after backend mutation");
    let still_stale = buffer_to_text(&terminal);
    assert!(
        still_stale.contains("ZZZZ"),
        "without a forced full redraw, ratatui diffing should leave the injected artifact in place"
    );

    app.request_full_redraw();
    assert!(app.force_full_redraw, "full redraw flag should be armed");
    terminal.clear().expect("test backend clear should succeed");
    app.force_full_redraw = false;
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("forced full redraw should succeed");
    let repaired = buffer_to_text(&terminal);
    assert_eq!(
        repaired, clean,
        "forced full redraw should restore the expected frame and remove stale backend artifacts"
    );
}

#[test]
fn test_scroll_key_then_render() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 40);

    // Render at bottom first (populates LAST_MAX_SCROLL)
    let _text_before = render_and_snap(&app, &mut terminal);

    let (up_code, up_mods) = scroll_up_key(&app);

    // Scroll up three times (9 lines total)
    for _ in 0..3 {
        app.handle_key(up_code.clone(), up_mods).unwrap();
    }
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);

    // Render again - verifies scroll_offset produces a valid frame without panic.
    // Note: LAST_MAX_SCROLL is a process-wide global that parallel tests
    // can overwrite at any time, so we only check that rendering succeeds
    // and that scroll state is correct - not that the rendered text differs,
    // since the global can clamp scroll_offset to 0 during render.
    let _text_after = render_and_snap(&app, &mut terminal);
}

#[test]
fn test_scroll_round_trip() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 12);

    let (up_code, up_mods) = scroll_up_key(&app);
    let (down_code, down_mods) = scroll_down_key(&app);

    // Render at bottom before scrolling (populates LAST_MAX_SCROLL)
    let _text_original = render_and_snap(&app, &mut terminal);

    // Scroll up 3x
    for _ in 0..3 {
        app.handle_key(up_code.clone(), up_mods).unwrap();
    }
    assert!(app.auto_scroll_paused);

    // Rendering after scrolling up should succeed; exact buffer diffs are brittle
    // because process-wide render state can influence viewport clamping.
    let _text_scrolled = render_and_snap(&app, &mut terminal);

    // Scroll back down until at bottom
    for _ in 0..20 {
        app.handle_key(down_code.clone(), down_mods).unwrap();
        if !app.auto_scroll_paused {
            break;
        }
    }
    assert_eq!(
        app.scroll_offset, 0,
        "scroll_offset should return to 0 after round-trip"
    );
    assert!(!app.auto_scroll_paused);

    // Verify we're back at the bottom and rendering still succeeds.
    let _text_restored = render_and_snap(&app, &mut terminal);
}

#[test]
fn test_copy_selection_from_bottom_rebases_scroll_instead_of_jumping_to_top() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 0, 40);

    let bottom_text = render_and_snap(&app, &mut terminal);
    let max_scroll = crate::tui::ui::last_max_scroll();
    assert!(
        max_scroll > 0,
        "expected scrollable history for selection test"
    );
    assert!(
        !bottom_text.contains("Intro line 01"),
        "bottom viewport should not start at top before selection"
    );

    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .expect("enter copy mode");
    app.handle_key(KeyCode::Right, KeyModifiers::empty())
        .expect("move selection cursor");

    assert!(
        app.copy_selection_mode,
        "copy selection mode should remain active"
    );
    assert!(app.auto_scroll_paused, "selection should pause auto-follow");
    assert_eq!(
        app.scroll_offset, max_scroll,
        "selection should preserve the current bottom viewport when pausing auto-follow"
    );

    let selected_text = render_and_snap(&app, &mut terminal);
    assert!(
        !selected_text.contains("Intro line 01"),
        "starting selection from bottom should not teleport to the top"
    );
}

#[cfg(test)]
#[path = "../tests_input_scroll.rs"]
mod input_scroll_tests;
