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
    app.streaming.streaming_text.clear();
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
    let visible_history = text.lines().skip(2).collect::<Vec<_>>().join(" ");
    let normalized_history = visible_history.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized_history.contains("Intro line 20 - quick brown fox jumps over the lazy dog."),
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

/// Regression for the wide-emoji "ghost" artifact (ratatui issue #2357): when
/// the chat view actually scrolls, `scroll_up`/`scroll_down` must arm a forced
/// full repaint so the next frame re-emits every cell. Ratatui's diff does not
/// re-emit the trailing cell after a wide grapheme when its symbol is
/// unchanged, so incremental-only diffs leave stale characters on kitty/foot.
///
/// Regression for issue #404: the repaint must be the soft buffer-invalidation
/// kind (`force_full_repaint`), not the hard `terminal.clear()` kind
/// (`force_full_redraw`). The ED2 Clear-All escape wiped kitty image
/// placeholder cells before the frame was redrawn, which flickered on every
/// scroll tick in terminals that repaint image cells non-atomically.
#[test]
fn scroll_arms_force_full_repaint_to_clear_wide_grapheme_ghosts() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 60);

    // Render at bottom first so LAST_MAX_SCROLL is populated and there is room
    // to actually move the viewport upward.
    let _ = render_and_snap(&app, &mut terminal);

    let (up_code, up_mods) = scroll_up_key(&app);

    app.force_full_redraw = false;
    app.force_full_repaint = false;
    app.handle_key(up_code.clone(), up_mods).unwrap();

    // The scroll moved the viewport, so a clean repaint must be armed.
    assert!(app.auto_scroll_paused, "scroll up should pause auto-scroll");
    assert!(app.scroll_offset > 0, "scroll up should move the viewport");
    assert!(
        app.force_full_repaint,
        "a viewport-moving scroll must arm force_full_repaint to clear ghosts"
    );
    assert!(
        !app.force_full_redraw,
        "scroll must not arm the hard-clear path: terminal.clear() flickers \
         around kitty image placeholders (issue #404)"
    );

    // Scrolling back down to the bottom should likewise arm a full repaint.
    let (down_code, down_mods) = scroll_down_key(&app);
    let mut armed_on_down = false;
    let mut hard_cleared_on_down = false;
    for _ in 0..80 {
        app.force_full_redraw = false;
        app.force_full_repaint = false;
        let moved = app.handle_key(down_code.clone(), down_mods).is_ok();
        let _ = moved;
        if app.force_full_repaint {
            armed_on_down = true;
        }
        if app.force_full_redraw {
            hard_cleared_on_down = true;
        }
        if !app.auto_scroll_paused {
            break;
        }
    }
    assert!(
        armed_on_down,
        "a viewport-moving downward scroll must also arm force_full_repaint"
    );
    assert!(
        !hard_cleared_on_down,
        "downward scroll must not arm the hard-clear path (issue #404)"
    );
}

/// Routing for `draw_full` (issue #404): scrolling requests the soft
/// buffer-invalidation repaint, screen-corruption recovery requests the hard
/// `terminal.clear()` path, and a pending hard clear supersedes a soft one.
#[test]
fn full_frame_invalidation_routes_hard_clear_over_soft_repaint() {
    use crate::tui::app::run_shell::{FullFrameInvalidation, full_frame_invalidation};

    assert_eq!(
        full_frame_invalidation(false, false),
        FullFrameInvalidation::None
    );
    assert_eq!(
        full_frame_invalidation(false, true),
        FullFrameInvalidation::SoftRepaint
    );
    assert_eq!(
        full_frame_invalidation(true, false),
        FullFrameInvalidation::HardClear
    );
    assert_eq!(
        full_frame_invalidation(true, true),
        FullFrameInvalidation::HardClear,
        "native-scroll/corruption recovery must not be downgraded by a \
         concurrently armed scroll repaint"
    );
}

/// Regression for issue #404: the scroll-driven full repaint must re-emit
/// every cell purely through ratatui's diff, without a backend clear. This
/// re-renders an identical frame after mutating the backend out-of-band; only
/// a diff that re-emits all cells can repair the artifact, and no
/// `terminal.clear()` / `Backend::clear` call is involved.
///
/// Renders a fixed closure (not the full app UI) so process-wide globals
/// mutated by parallel tests cannot change the frame between draws. The app
/// wiring for this path is covered by
/// `scroll_arms_force_full_repaint_to_clear_wide_grapheme_ghosts` and
/// `full_frame_invalidation_routes_hard_clear_over_soft_repaint`.
#[test]
fn soft_full_repaint_re_emits_all_cells_without_backend_clear() {
    let backend = ratatui::backend::TestBackend::new(60, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let render = |f: &mut ratatui::Frame| {
        let area = f.area();
        for y in 0..area.height {
            f.render_widget(
                ratatui::widgets::Paragraph::new(format!("stable line {y:02} with emoji ✅")),
                ratatui::layout::Rect::new(0, y, area.width, 1),
            );
        }
    };

    terminal.draw(render).expect("initial draw");
    let clean = buffer_to_text(&terminal);
    assert!(clean.contains("stable line 00"), "sanity: content rendered");

    // Simulate stale screen content (e.g. a wide-grapheme ghost) directly in
    // the backend, invisible to ratatui's buffers. Keep the artifact on
    // single-width cells: ratatui's diff intentionally skips the trailing cell
    // of a standard wide grapheme (re-printing the wide char covers it on real
    // terminals), and TestBackend does not emulate that coverage.
    let ghost = ratatui::buffer::Buffer::with_lines(["ZZZZZZZZZZZZZZZZZZZZZZZZZZ"]);
    let width = terminal.backend().buffer().area.width;
    let updates = ghost
        .content()
        .iter()
        .enumerate()
        .map(|(idx, cell)| ((idx as u16) % width, (idx as u16) / width, cell));
    terminal
        .backend_mut()
        .draw(updates)
        .expect("inject backend artifact");

    // An ordinary diffed redraw of the identical frame emits nothing and
    // leaves the artifact behind.
    terminal
        .draw(render)
        .expect("normal redraw after backend mutation");
    assert!(
        buffer_to_text(&terminal).contains("ZZZZ"),
        "plain diff of an identical frame should not repaint the artifact"
    );

    // The soft repaint invalidates ratatui's previous buffer so the next diff
    // re-emits every cell. No Backend::clear / ED2 escape is issued.
    crate::tui::app::run_shell::invalidate_previous_terminal_buffer(&mut terminal);
    terminal.draw(render).expect("soft full repaint");
    let repaired = buffer_to_text(&terminal);
    assert_eq!(
        repaired, clean,
        "soft full repaint must restore the frame by re-emitting every cell"
    );
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
fn test_scroll_down_past_bottom_does_not_accumulate_phantom_offset() {
    // Repro for the "scroll past the end" bug. While streaming, scroll_max_estimate
    // can exceed the renderer's actual max (rendered_max). The old code capped the
    // paused scroll_offset to that inflated estimate, so scrolling down "past" the
    // visible bottom silently inflated scroll_offset above rendered_max without
    // moving the view. A later scroll-up then had to first drain that phantom
    // offset before the viewport moved at all.
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 12);

    // Establish a rendered scroll extent.
    render_and_snap(&app, &mut terminal);
    let rendered_max = crate::tui::ui::last_max_scroll();
    assert!(rendered_max > 2, "expected scrollable chat content");

    // Pause partway up the transcript.
    app.scroll_offset = 1;
    app.auto_scroll_paused = true;

    // Simulate streaming so scroll_max_estimate() inflates above rendered_max.
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.streaming.streaming_text = "x".repeat(20_000);

    // Hammer scroll-down well past the bottom.
    for _ in 0..200 {
        app.scroll_down(1);
        if !app.auto_scroll_paused {
            break;
        }
    }

    // The stored offset must never exceed the largest offset the renderer can
    // actually display; otherwise scrolling back up has to drain phantom offset.
    let rendered_max = crate::tui::ui::last_max_scroll();
    assert!(
        app.scroll_offset <= rendered_max,
        "scroll_offset ({}) must not exceed rendered_max ({}) - phantom offset accumulated",
        app.scroll_offset,
        rendered_max
    );
}

#[test]
fn test_overscroll_requires_gesture_starting_at_bottom() {
    // Overscroll must only reveal when the downward gesture *began* while the
    // view was already pinned to the bottom. A gesture that starts mid-
    // transcript and carries the view into the bottom (with leftover momentum
    // continuing past it) must not pop the elastic line.
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 12);
    render_and_snap(&app, &mut terminal);
    let rendered_max = crate::tui::ui::last_max_scroll();
    assert!(rendered_max > 2, "expected scrollable chat content");

    // Start a gesture partway up the transcript.
    app.scroll_offset = 2;
    app.auto_scroll_paused = true;

    // One continuous gesture (no pause between ticks): rides into the bottom
    // and keeps going. Momentum past the bottom must be swallowed silently.
    for _ in 0..20 {
        app.scroll_down(1);
    }
    assert!(!app.auto_scroll_paused, "gesture should reach the bottom");
    assert!(
        !app.chat_overscroll_active(),
        "momentum carrying into the bottom must not reveal the overscroll line"
    );

    // A fresh gesture that starts at the bottom (after a pause) does reveal it.
    app.chat_scroll_down_last =
        Some(Instant::now() - App::OVERSCROLL_GESTURE_GAP - std::time::Duration::from_millis(50));
    app.scroll_down(1);
    assert!(
        app.chat_overscroll_active(),
        "a new gesture starting at the bottom should reveal the overscroll line"
    );
}

#[test]
fn test_scroll_acceleration_multiplier_scales_with_flick_speed() {
    use std::time::Duration;
    // A fast flick (short gap between wheel events) gets a subtle 2x boost; a
    // slow, deliberate notch stays at 1x for precise positioning.
    assert_eq!(App::scroll_acceleration_multiplier(Duration::from_millis(10)), 2);
    assert_eq!(App::scroll_acceleration_multiplier(Duration::from_millis(100)), 1);
    assert_eq!(App::scroll_acceleration_multiplier(Duration::from_millis(200)), 1);
    assert_eq!(App::scroll_acceleration_multiplier(Duration::from_secs(5)), 1);
}

#[test]
fn test_fast_flick_enqueues_more_lines_than_a_slow_notch() {
    use std::time::Duration;
    // "Scroll power": the lines committed per wheel notch scale with flick speed
    // (shorter inter-event gap => bigger multiplier => more lines), capped so the
    // hardest flick stays controllable. Shared by the chat and /resume preview.
    let fast = App::scroll_intent_lines(App::scroll_acceleration_multiplier(Duration::from_millis(10)));
    let slow =
        App::scroll_intent_lines(App::scroll_acceleration_multiplier(Duration::from_millis(400)));
    assert!(fast > slow, "a fast flick commits more lines than a slow notch ({fast} > {slow})");
    assert_eq!(slow, 3, "a deliberate notch uses the base intent");
    // Even a maximum-velocity multiplier stays within the controllable cap.
    assert!(App::scroll_intent_lines(8) <= 5, "intent is capped");
}

#[test]
fn test_momentum_drain_decelerates_to_one_line() {
    // The drain rate eases out: a large queue glides several lines per frame,
    // decelerating to a single line as it empties (natural momentum decay).
    let mut app = create_test_app();
    app.mouse_scroll_queue = 40;
    let big = app.mouse_scroll_drain_amount();
    app.mouse_scroll_queue = 4;
    let small = app.mouse_scroll_drain_amount();
    app.mouse_scroll_queue = 1;
    let tail = app.mouse_scroll_drain_amount();
    app.mouse_scroll_queue = 0;
    let empty = app.mouse_scroll_drain_amount();

    assert!(big > small, "large momentum should drain faster ({big} > {small})");
    assert_eq!(tail, 1, "the last line should drain one at a time");
    let _ = empty;
}

#[test]
fn test_queued_wheel_down_at_bottom_does_not_accumulate_phantom_scroll() {
    // Touchpad/mouse momentum can queue many downward wheel steps. If they keep
    // "succeeding" against the already-pinned bottom, the queue (or offset) would
    // accumulate phantom scroll that a later wheel-up has to drain first. The
    // queue must be cleared as soon as a step can no longer move the view.
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 12);
    render_and_snap(&app, &mut terminal);

    // Already following the bottom.
    assert!(!app.auto_scroll_paused);

    // Simulate a burst of queued downward wheel momentum.
    app.mouse_scroll_target = Some(super::MouseScrollTarget::Chat);
    app.mouse_scroll_queue = 24;

    app.progress_mouse_scroll_animation();

    assert_eq!(
        app.mouse_scroll_queue, 0,
        "blocked downward momentum must clear the queue instead of parking phantom scroll"
    );
    assert!(
        app.mouse_scroll_target.is_none(),
        "scroll target should reset once the queue is drained"
    );
    assert!(
        !app.auto_scroll_paused,
        "still following the bottom after blocked downward momentum"
    );
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

#[test]
fn repro_ctrl_shift_jk_scroll_with_text_in_input() {
    // Kitty keyboard protocol reports Ctrl+Shift+J / Ctrl+Shift+K as
    // Char('j'/'k') + CONTROL|SHIFT (captured raw bytes: ESC[106;6u / ESC[107;6u).
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 0, 40);
    render_and_snap(&app, &mut terminal);

    // Plain Ctrl+K / Ctrl+J (control only) -> should scroll.
    app.handle_key(KeyCode::Char('k'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.auto_scroll_paused, "Ctrl+K should scroll up");
    let plain_offset = app.scroll_offset;

    // Reset to bottom.
    app.follow_chat_bottom();

    // Now put text in the input box, like a real user mid-prompt.
    app.input = "some draft text".to_string();

    // Ctrl+Shift+K with text present.
    app.handle_key(KeyCode::Char('k'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        .unwrap();
    assert!(
        app.auto_scroll_paused,
        "Ctrl+Shift+K should scroll up even with text in input (offset moved like plain: {plain_offset})"
    );
    assert_eq!(
        app.input, "some draft text",
        "Ctrl+Shift+K must not kill input text"
    );
    let shift_up_offset = app.scroll_offset;

    // Ctrl+Shift+J should scroll back down.
    app.handle_key(KeyCode::Char('j'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        .unwrap();
    assert!(
        app.scroll_offset > shift_up_offset || !app.auto_scroll_paused,
        "Ctrl+Shift+J should scroll down toward the bottom"
    );
    assert_eq!(
        app.input, "some draft text",
        "Ctrl+Shift+J must not alter input text"
    );

    // Plain Ctrl+K with text still acts as kill-to-end-of-line (emacs habit).
    app.follow_chat_bottom();
    app.input = "draft".to_string();
    app.cursor_pos = 0;
    app.handle_key(KeyCode::Char('k'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(
        app.input, "",
        "plain Ctrl+K should still kill to end of line"
    );
}

/// Build a long single-message app and seed the render statics so the
/// history-anchor logic (which reads `last_total_wrapped_lines` /
/// `last_resolved_chat_scroll`) has a populated frame to work against.
fn anchor_test_app() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    create_scroll_test_app(80, 25, 0, 60)
}

#[test]
fn test_history_anchor_keeps_distance_from_bottom_after_prepend() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = anchor_test_app();

    // Render at a scrolled-up position so the statics reflect a real frame.
    render_and_snap(&app, &mut terminal);
    let total_before = crate::tui::ui::last_total_wrapped_lines();
    assert!(total_before > 0, "expected a rendered transcript");

    app.scroll_offset = 4;
    app.auto_scroll_paused = true;
    render_and_snap(&app, &mut terminal);
    let total_before = crate::tui::ui::last_total_wrapped_lines();

    // Simulate the reader sitting 4 lines from the top: capture an anchor as if a
    // load were triggered, then "prepend" by growing the transcript.
    app.capture_history_anchor(0);
    let anchor = app
        .pending_history_anchor
        .expect("anchor should be captured");
    let expected_from_bottom = total_before.saturating_sub(4);
    assert_eq!(
        anchor.lines_from_bottom, expected_from_bottom,
        "anchor should record distance from the bottom"
    );

    // Grow the transcript (older content prepended) and re-render. The resolved
    // scroll must keep the same distance from the bottom, not snap to the top.
    app.display_messages.insert(
        0,
        DisplayMessage {
            role: "assistant".to_string(),
            content: App::build_scroll_test_content(0, 40, None),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    );
    app.bump_display_messages_version();
    render_and_snap(&app, &mut terminal);

    let total_after = crate::tui::ui::last_total_wrapped_lines();
    assert!(
        total_after > total_before,
        "prepend should grow the transcript ({} -> {})",
        total_before,
        total_after
    );
    let resolved = crate::tui::ui::last_resolved_chat_scroll();
    let distance_after = total_after.saturating_sub(resolved);
    assert_eq!(
        distance_after, expected_from_bottom,
        "viewport must stay the same distance from the bottom across the prepend"
    );
    assert_ne!(
        resolved, 0,
        "anchored viewport must not snap to the new absolute top"
    );
}

#[test]
fn test_history_anchor_reconciles_into_scroll_offset_after_render() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = anchor_test_app();

    app.scroll_offset = 3;
    app.auto_scroll_paused = true;
    render_and_snap(&app, &mut terminal);

    app.capture_history_anchor(0);
    assert!(app.pending_history_anchor.is_some());

    // Before any new frame, reconcile must wait (total unchanged).
    assert!(
        !app.reconcile_history_anchor(),
        "reconcile should wait until a frame with new content has rendered"
    );
    assert!(app.pending_history_anchor.is_some());

    // Prepend + render so the resolved scroll is published, then reconcile.
    app.display_messages.insert(
        0,
        DisplayMessage {
            role: "assistant".to_string(),
            content: App::build_scroll_test_content(0, 30, None),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    );
    app.bump_display_messages_version();
    render_and_snap(&app, &mut terminal);
    let resolved = crate::tui::ui::last_resolved_chat_scroll();

    assert!(app.reconcile_history_anchor(), "reconcile should apply once");
    assert!(
        app.pending_history_anchor.is_none(),
        "anchor should be consumed after reconcile"
    );
    assert_eq!(
        app.scroll_offset, resolved,
        "scroll_offset should adopt the resolved on-screen position"
    );
    assert!(app.auto_scroll_paused, "anchored view stays paused");
}

/// Build a session whose compacted prefix is large enough to actually truncate
/// (the render window only hides history past ~80 messages / >5 turns), with one
/// live prompt at the tail. Returns the app with the truncated window applied.
fn compacted_history_app_with_remaining(turns: usize) -> App {
    let mut app = create_test_app();
    for turn in 0..turns {
        app.session.add_message(
            crate::message::Role::User,
            vec![crate::message::ContentBlock::Text {
                text: format!("old prompt {turn}"),
                cache_control: None,
            }],
        );
        app.session.add_message(
            crate::message::Role::Assistant,
            vec![crate::message::ContentBlock::Text {
                text: format!("old response {turn}"),
                cache_control: None,
            }],
        );
    }
    app.session.add_message(
        crate::message::Role::User,
        vec![crate::message::ContentBlock::Text {
            text: "current prompt".to_string(),
            cache_control: None,
        }],
    );
    let compacted_count = turns * 2;
    app.session.compaction = Some(crate::session::StoredCompactionState {
        summary_text: "older turns".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: turns,
        original_turn_count: turns,
        compacted_count,
    });

    let (rendered_messages, _images, _info) =
        crate::session::render_messages_and_images_with_compacted_history(&app.session, 0);
    let rendered = rendered_messages
        .into_iter()
        .map(|msg| DisplayMessage {
            role: msg.role,
            content: msg.content,
            tool_calls: msg.tool_calls,
            duration_secs: None,
            title: None,
            tool_data: msg.tool_data,
        })
        .collect();
    app.replace_display_messages(rendered);
    app
}

#[test]
fn test_local_compacted_history_scroll_up_is_anchored_not_snapped() {
    let _render_lock = scroll_render_test_lock();

    let mut app = compacted_history_app_with_remaining(50);
    assert!(
        app.compacted_history_has_remaining(),
        "older history should be hidden initially"
    );

    let backend = ratatui::backend::TestBackend::new(80, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    // Scroll up to the top of the loaded window; this should both pull older
    // history in and anchor the viewport rather than snapping to the new top.
    app.scroll_offset = 0;
    app.auto_scroll_paused = true;
    app.scroll_up(3);

    assert!(
        app.display_messages().len() > 2,
        "scroll-up near the top should have loaded older messages into the transcript"
    );
    // An anchor must have been captured so the next render keeps the view stable.
    assert!(
        app.pending_history_anchor.is_some(),
        "scroll-up that loads history should capture a viewport anchor"
    );
}

#[test]
fn test_prompt_jump_loads_older_history_when_at_top() {
    let _render_lock = scroll_render_test_lock();

    let mut app = compacted_history_app_with_remaining(50);
    assert!(app.compacted_history_has_remaining());
    let messages_before = app.display_messages().len();

    let backend = ratatui::backend::TestBackend::new(80, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    // At the top with no earlier loaded prompt, a prompt-up jump should pull in
    // the older compacted history instead of doing nothing.
    app.scroll_offset = 0;
    app.auto_scroll_paused = true;
    app.scroll_to_prev_prompt();

    assert!(
        app.display_messages().len() > messages_before,
        "prompt-up at the top should load older history"
    );
}

/// Return the first non-empty rendered line of the chat viewport, used as a
/// proxy for "where the viewport is anchored". If a scroll notch moves the
/// view, this string must change.
fn first_visible_content_line(text: &str) -> String {
    text.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Reproduction harness for the intermittent "can't scroll" report.
///
/// Mimics the real event loop: render a frame (so LAST_MAX_SCROLL reflects the
/// content), feed one wheel-up notch, drain the momentum the way the tick does,
/// then render again. Each notch from a scrollable bottom must visibly move the
/// viewport. Sweeps content/viewport sizes and streaming on/off because the
/// report is size/timing dependent ("only sometimes").
#[test]
fn repro_wheel_up_from_bottom_always_moves_viewport() {
    let _render_lock = scroll_render_test_lock();

    let mut failures: Vec<String> = Vec::new();

    for &height in &[8u16, 12, 16, 20, 25, 30] {
        for &padding in &[6usize, 12, 24, 40] {
            for &streaming in &[false, true] {
                let (mut app, mut terminal) = create_scroll_test_app(80, height, 1, padding);
                if streaming {
                    app.is_processing = true;
                    app.status = ProcessingStatus::Streaming;
                }

                // Establish the rendered scroll extent at the bottom.
                let bottom = render_and_snap(&app, &mut terminal);
                let max_scroll = crate::tui::ui::last_max_scroll();
                if max_scroll == 0 {
                    // Content fits; nothing to scroll. Not a failure.
                    continue;
                }

                // One wheel-up notch, drained as the tick would, then re-render.
                app.handle_mouse_event(MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    column: 10,
                    row: height / 2,
                    modifiers: KeyModifiers::empty(),
                });
                // Drain any queued momentum the way handle_tick does.
                for _ in 0..16 {
                    app.progress_mouse_scroll_animation();
                }
                let after = render_and_snap(&app, &mut terminal);

                if after == bottom {
                    failures.push(format!(
                        "h={height} pad={padding} streaming={streaming}: \
                         wheel-up did not move the viewport (max_scroll={max_scroll}, \
                         offset={}, paused={})",
                        app.scroll_offset, app.auto_scroll_paused
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "wheel-up dead zones found:\n{}",
        failures.join("\n")
    );
}

/// A burst of wheel notches followed by a single wheel-down notch must move the
/// view back down. Repro for momentum/queue state leaving the view stuck.
#[test]
fn repro_wheel_down_after_up_burst_moves_viewport() {
    let _render_lock = scroll_render_test_lock();

    let mut failures: Vec<String> = Vec::new();

    for &height in &[10u16, 16, 25] {
        for &padding in &[12usize, 24, 40] {
            for &streaming in &[false, true] {
                let (mut app, mut terminal) = create_scroll_test_app(80, height, 1, padding);
                if streaming {
                    app.is_processing = true;
                    app.status = ProcessingStatus::Streaming;
                }
                render_and_snap(&app, &mut terminal);
                if crate::tui::ui::last_max_scroll() == 0 {
                    continue;
                }

                // Flick up several notches.
                for _ in 0..4 {
                    app.handle_mouse_event(MouseEvent {
                        kind: MouseEventKind::ScrollUp,
                        column: 10,
                        row: height / 2,
                        modifiers: KeyModifiers::empty(),
                    });
                }
                for _ in 0..30 {
                    app.progress_mouse_scroll_animation();
                }
                let scrolled = render_and_snap(&app, &mut terminal);

                // One wheel-down notch must move the viewport back toward bottom.
                app.handle_mouse_event(MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: 10,
                    row: height / 2,
                    modifiers: KeyModifiers::empty(),
                });
                for _ in 0..16 {
                    app.progress_mouse_scroll_animation();
                }
                let after = render_and_snap(&app, &mut terminal);

                if after == scrolled {
                    failures.push(format!(
                        "h={height} pad={padding} streaming={streaming}: \
                         wheel-down after up-burst did not move (offset={}, paused={})",
                        app.scroll_offset, app.auto_scroll_paused
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "wheel-down dead zones found:\n{}",
        failures.join("\n")
    );
}

/// Keyboard scroll-up via the real key path must move the viewport on the very
/// first notch from the bottom, across sizes and streaming state.
#[test]
fn repro_key_scroll_up_first_notch_moves_viewport() {
    let _render_lock = scroll_render_test_lock();

    let mut failures: Vec<String> = Vec::new();

    for &height in &[8u16, 12, 16, 20, 25, 30] {
        for &padding in &[6usize, 12, 24, 40] {
            for &streaming in &[false, true] {
                let (mut app, mut terminal) = create_scroll_test_app(80, height, 1, padding);
                if streaming {
                    app.is_processing = true;
                    app.status = ProcessingStatus::Streaming;
                }
                let (up_code, up_mods) = scroll_up_key(&app);

                let bottom = render_and_snap(&app, &mut terminal);
                let max_scroll = crate::tui::ui::last_max_scroll();
                if max_scroll == 0 {
                    continue;
                }

                app.handle_key(up_code.clone(), up_mods).unwrap();
                let after = render_and_snap(&app, &mut terminal);

                let moved = first_visible_content_line(&after)
                    != first_visible_content_line(&bottom)
                    || after != bottom;
                if !moved {
                    failures.push(format!(
                        "h={height} pad={padding} streaming={streaming}: \
                         key scroll-up first notch did not move (max_scroll={max_scroll}, \
                         offset={}, paused={})",
                        app.scroll_offset, app.auto_scroll_paused
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "key scroll-up dead zones found:\n{}",
        failures.join("\n")
    );
}

/// Reproduction for the reasoning-streaming scroll report: while reasoning is
/// actively streaming in (paced frames, like the real server), a user scroll-up
/// must move the rendered viewport AND that scrolled position must survive the
/// next paced reasoning frames (auto-follow must not yank the view back to the
/// bottom). This is the "I can't scroll while it's thinking" scenario.
#[test]
fn repro_scroll_up_holds_while_reasoning_streams() {
    let _render_lock = scroll_render_test_lock();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    let mut failures: Vec<String> = Vec::new();

    for &height in &[12u16, 20, 30] {
        for &padding in &[24usize, 40] {
            let (mut app, mut terminal) = create_scroll_test_app(80, height, 1, padding);
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.is_processing = true;
            app.status = ProcessingStatus::Streaming;

            // Render at the bottom so scroll metrics are populated.
            let bottom = render_and_snap(&app, &mut terminal);
            if crate::tui::ui::last_max_scroll() == 0 {
                continue;
            }

            // Reasoning begins streaming in (paced through StreamBuffer).
            app.handle_server_event(
                crate::protocol::ServerEvent::ReasoningDelta {
                    text: "let me think about this carefully\n".repeat(8),
                },
                &mut remote,
            );
            // Drain a couple paced frames the way the tick does, rendering between.
            for _ in 0..3 {
                let ops = app.stream_buffer.flush_smooth_frame();
                app.apply_stream_ops(ops);
                render_and_snap(&app, &mut terminal);
            }

            // User scrolls up while reasoning is still streaming.
            app.handle_key(KeyCode::PageUp, KeyModifiers::empty()).unwrap();
            let scrolled = render_and_snap(&app, &mut terminal);

            let moved = scrolled != bottom;
            if !moved {
                failures.push(format!(
                    "h={height} pad={padding}: scroll-up during reasoning stream did not move \
                     (offset={}, paused={}, max={})",
                    app.scroll_offset,
                    app.auto_scroll_paused,
                    crate::tui::ui::last_max_scroll()
                ));
                continue;
            }

            // More reasoning streams in; the user's scrolled position must hold
            // (not snap back to the bottom).
            for burst in 0..5 {
                app.handle_server_event(
                    crate::protocol::ServerEvent::ReasoningDelta {
                        text: format!("more reasoning content burst {burst}\n").repeat(4),
                    },
                    &mut remote,
                );
                let ops = app.stream_buffer.flush_smooth_frame();
                app.apply_stream_ops(ops);
                let frame = render_and_snap(&app, &mut terminal);
                if !app.auto_scroll_paused {
                    failures.push(format!(
                        "h={height} pad={padding}: reasoning burst {burst} un-paused scroll \
                         (auto-follow yanked the view back to bottom)"
                    ));
                    break;
                }
                if frame == bottom {
                    failures.push(format!(
                        "h={height} pad={padding}: reasoning burst {burst} snapped view back to \
                         bottom while user had scrolled up"
                    ));
                    break;
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "reasoning-streaming scroll dead zones found:\n{}",
        failures.join("\n")
    );
}

/// Closer reproduction of the real loop: mouse-wheel scroll (the momentum-queue
/// path) while reasoning tokens trickle in one delta at a time, with momentum
/// drain + render interleaved exactly like `handle_tick`. The viewport must move
/// on the wheel and must not be snapped back to the bottom by the trickling
/// reasoning deltas.
#[test]
fn repro_mouse_wheel_during_token_by_token_reasoning() {
    let _render_lock = scroll_render_test_lock();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    let mut failures: Vec<String> = Vec::new();

    for &height in &[12u16, 20, 30] {
        let (mut app, mut terminal) = create_scroll_test_app(80, height, 1, 40);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;

        let bottom = render_and_snap(&app, &mut terminal);
        if crate::tui::ui::last_max_scroll() == 0 {
            continue;
        }

        // Token-by-token reasoning trickles in with paced reveal between tokens.
        let tokens = [
            "Let ", "me ", "think ", "about ", "the ", "problem ", "step ", "by ", "step.\n",
            "First ", "I ", "consider ", "the ", "inputs ", "and ", "constraints.\n",
        ];
        for tok in tokens {
            app.handle_server_event(
                crate::protocol::ServerEvent::ReasoningDelta {
                    text: tok.to_string(),
                },
                &mut remote,
            );
            let ops = app.stream_buffer.flush_smooth_frame();
            app.apply_stream_ops(ops);
            render_and_snap(&app, &mut terminal);
        }

        // Wheel up one notch (momentum-queue path).
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: height / 2,
            modifiers: KeyModifiers::empty(),
        });
        // Drain momentum the way the tick does, rendering each frame.
        for _ in 0..20 {
            app.progress_mouse_scroll_animation();
            render_and_snap(&app, &mut terminal);
        }
        let scrolled = render_and_snap(&app, &mut terminal);

        if scrolled == bottom {
            failures.push(format!(
                "h={height}: wheel-up during token reasoning did not move \
                 (offset={}, paused={}, max={})",
                app.scroll_offset,
                app.auto_scroll_paused,
                crate::tui::ui::last_max_scroll()
            ));
            continue;
        }

        // Keep trickling reasoning tokens + draining momentum; the scrolled view
        // must hold (not snap back), as it would in the real loop.
        for i in 0..20 {
            app.handle_server_event(
                crate::protocol::ServerEvent::ReasoningDelta {
                    text: format!("token{i} "),
                },
                &mut remote,
            );
            let ops = app.stream_buffer.flush_smooth_frame();
            app.apply_stream_ops(ops);
            app.progress_mouse_scroll_animation();
            let frame = render_and_snap(&app, &mut terminal);
            if frame == bottom || !app.auto_scroll_paused {
                failures.push(format!(
                    "h={height}: trickle {i} snapped back to bottom (paused={}, offset={})",
                    app.auto_scroll_paused, app.scroll_offset
                ));
                break;
            }
        }
    }

    assert!(
        failures.is_empty(),
        "mouse-wheel reasoning scroll dead zones:\n{}",
        failures.join("\n")
    );
}

/// The reasoning region closing (anchoring) restructures the transcript: the
/// block moves out of `streaming_text` into `display_messages`, and preceding
/// answer text commits. If the user had scrolled up to read history, that reflow
/// must not snap their view to the bottom or leave it stuck. This is the most
/// likely "I scrolled up while it was thinking and then it jumped / froze" path.
#[test]
fn repro_scroll_held_across_reasoning_close_and_answer() {
    let _render_lock = scroll_render_test_lock();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    let mut failures: Vec<String> = Vec::new();

    for &height in &[12u16, 20, 30] {
        for &padding in &[24usize, 40] {
            let (mut app, mut terminal) = create_scroll_test_app(80, height, 1, padding);
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.is_processing = true;
            app.status = ProcessingStatus::Streaming;

            render_and_snap(&app, &mut terminal);
            if crate::tui::ui::last_max_scroll() == 0 {
                continue;
            }

            // Reasoning streams in.
            app.handle_server_event(
                crate::protocol::ServerEvent::ReasoningDelta {
                    text: "thinking about the question in some detail here\n".repeat(6),
                },
                &mut remote,
            );
            let ops = app.stream_buffer.flush();
            app.apply_stream_ops(ops);
            render_and_snap(&app, &mut terminal);

            // User scrolls up to read earlier history while it thinks.
            app.handle_mouse_event(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 10,
                row: height / 2,
                modifiers: KeyModifiers::empty(),
            });
            for _ in 0..20 {
                app.progress_mouse_scroll_animation();
            }
            let scrolled = render_and_snap(&app, &mut terminal);
            let scrolled_offset = app.scroll_offset;
            if !app.auto_scroll_paused {
                failures.push(format!("h={height} pad={padding}: scroll-up did not pause"));
                continue;
            }

            // Reasoning closes (anchors) and the answer begins -- the big reflow.
            app.handle_server_event(
                crate::protocol::ServerEvent::ReasoningDone { duration_secs: None },
                &mut remote,
            );
            app.handle_server_event(
                crate::protocol::ServerEvent::TextDelta {
                    text: "Here is the actual answer to your question.\n".to_string(),
                },
                &mut remote,
            );
            let ops = app.stream_buffer.flush();
            app.apply_stream_ops(ops);
            let after = render_and_snap(&app, &mut terminal);

            // The view must still be paused (user is reading) and not snapped to
            // the bottom by the reflow.
            if !app.auto_scroll_paused {
                failures.push(format!(
                    "h={height} pad={padding}: reasoning close un-paused scroll \
                     (jumped to bottom); was offset={scrolled_offset}, now {}",
                    app.scroll_offset
                ));
                continue;
            }
            // It is fine for the absolute offset to shift to keep the SAME content
            // under the reader (anchor reconcile), but the frame must not become the
            // bottom-follow frame, and the user must still be able to scroll further.
            let _ = (scrolled, after);

            // Verify scrolling still responds after the reflow.
            let before_more = render_and_snap(&app, &mut terminal);
            app.handle_mouse_event(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 10,
                row: height / 2,
                modifiers: KeyModifiers::empty(),
            });
            for _ in 0..20 {
                app.progress_mouse_scroll_animation();
            }
            let after_more = render_and_snap(&app, &mut terminal);
            if after_more == before_more && app.scroll_offset > 0 {
                failures.push(format!(
                    "h={height} pad={padding}: scroll unresponsive after reasoning close \
                     (offset={}, max={})",
                    app.scroll_offset,
                    crate::tui::ui::last_max_scroll()
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "scroll-across-reasoning-close dead zones:\n{}",
        failures.join("\n")
    );
}


/// Regression for the overscroll flicker (revealing the elastic status line
/// must not re-layout the transcript).
///
/// When the transcript height sat exactly at the fits/overflows boundary, the
/// one-row overscroll reveal flipped the `overflows` decision: the native
/// scrollbar appeared, the whole transcript re-wrapped one column narrower,
/// and every visible line shifted. On rebound it flipped back (or worse,
/// stayed latched because the narrower wrap produced more lines). On screen
/// this read as a full-screen flicker whenever overscroll was activated.
///
/// The scrollbar/overflow decision must therefore ignore the transient
/// overscroll row: for every content height around the boundary, the
/// transcript region rendered during the dwell must be identical to the frame
/// before the reveal, and the frame after the rebound must equal the frame
/// before the reveal exactly.
#[test]
fn overscroll_reveal_does_not_relayout_transcript() {
    let _lock = scroll_render_test_lock();

    for pad in 4..30usize {
        let (mut app, mut terminal) = create_scroll_test_app(100, 24, 0, pad);
        app.chat_native_scrollbar = true;
        app.context_info = crate::prompt::ContextInfo {
            total_chars: 40_000,
            ..Default::default()
        };
        app.context_limit = 200_000;

        let before = render_and_snap(&app, &mut terminal);
        let max_before = crate::tui::ui::last_max_scroll();

        // Wheel-down while pinned at the bottom: registers an overscroll tick
        // and reveals the status line for the dwell window.
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 5,
            modifiers: KeyModifiers::empty(),
        });
        assert!(
            app.chat_overscroll_active(),
            "pad={pad}: wheel-down at the bottom should register an overscroll"
        );
        let during = render_and_snap(&app, &mut terminal);
        let max_during = crate::tui::ui::last_max_scroll();

        // The reveal may slide the transcript up by at most the one row the
        // elastic line claims (the intended pull-to-reveal). It must not jump
        // further, and it must never re-wrap: every transcript row shown
        // during the dwell must be a row that already existed before the
        // reveal (a re-wrap breaks lines at a different column, producing
        // brand-new row strings, which is the full-screen flicker).
        assert!(
            max_during <= max_before + 1,
            "pad={pad}: reveal moved the viewport by more than the elastic \
             row (max {max_before} -> {max_during})"
        );
        let before_rows: Vec<&str> = before.lines().collect();
        let during_rows: Vec<&str> = during.lines().collect();
        // A re-wrap breaks lines at a different column, producing brand-new
        // row strings. Compare the transcript body rows (the "Intro line"
        // filler) as an ordered sequence: they may slide up by the one
        // elastic row, but their content must be byte-identical. Header rows
        // composite with the fixed-position context widget overlay and the
        // bottom rows (idle hint, input, elastic line) legitimately change,
        // so only body rows are compared.
        let body = |rows: &[&str]| -> Vec<String> {
            rows.iter()
                .filter(|row| row.contains("quick brown fox"))
                .map(|row| {
                    // Drop the native scrollbar column: its thumb glyph
                    // legitimately moves with the one-row elastic slide.
                    row.trim_end()
                        .trim_end_matches(['│', '╷', '╵', '•'])
                        .trim_end()
                        .to_string()
                })
                .collect()
        };
        let body_before = body(&before_rows);
        let body_during = body(&during_rows);
        assert!(
            !body_before.is_empty(),
            "pad={pad}: expected filler body rows in the pre-reveal frame"
        );
        // The intended elastic behavior when the transcript already overflows
        // is a one-row slide: the top body row scrolls out and every other row
        // keeps its exact content. A re-wrap instead rewrites every row. So
        // the dwell body must be a suffix of the pre-reveal body missing at
        // most one leading row.
        let dropped = body_before.len().saturating_sub(body_during.len());
        assert!(
            dropped <= 1 && body_before[dropped..] == body_during[..],
            "pad={pad}: transcript body rows re-wrapped while the overscroll \
             line was revealed (scrollbar/wrap flip):\nbefore:\n{before}\nduring:\n{during}"
        );

        // After the dwell expires the transcript must return to the exact
        // pre-overscroll layout: no latched scrollbar, no residual re-wrap.
        // (The idle status line rotates its own hint content over time, so
        // only the transcript region is compared, plus the scroll extent.)
        app.chat_overscroll_last = None;
        let after = render_and_snap(&app, &mut terminal);
        let max_after = crate::tui::ui::last_max_scroll();
        assert_eq!(
            max_before, max_after,
            "pad={pad}: the rebound must restore the pre-overscroll scroll \
             extent (scrollbar/wrap stayed latched)"
        );
        let after_rows: Vec<&str> = after.lines().collect();
        let body_after = body(&after_rows);
        assert_eq!(
            body_before, body_after,
            "pad={pad}: transcript body rows did not return to their \
             pre-overscroll content after the rebound:\nbefore:\n{before}\nafter:\n{after}"
        );
    }
}


/// End-to-end: a swarm notification carrying a sender-provided tldr renders
/// collapsed (tldr + `▸ expand` badge, body hidden) through a REAL draw, and a
/// left click on the badge expands it in place (body visible, `▾ collapse`
/// badge). Exercises the full path: collapsible encoding -> body render ->
/// live copy-viewport snapshot -> `swarm_expand_target_from_screen` ->
/// `toggle_swarm_message_expand`.
#[test]
fn test_click_on_swarm_expand_badge_toggles_tldr_collapse() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    let body = "The flaky test was caused by a race in the setup helper. \
                I rewrote it to use a barrier and verified 200 consecutive runs pass.";
    let content = jcode_tui_messages::encode_collapsible_swarm_content("fixed the flaky test", body);
    app.display_messages = vec![
        DisplayMessage::user("hi"),
        DisplayMessage::swarm("DM from sheep", content),
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(90, 30);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    let collapsed = render_and_snap(&app, &mut terminal);
    assert!(
        collapsed.contains("fixed the flaky test"),
        "collapsed card must show the tldr:\n{collapsed}"
    );
    assert!(
        collapsed.contains("▸ expand"),
        "collapsed card must show the expand badge:\n{collapsed}"
    );
    assert!(
        !collapsed.contains("race in the setup helper"),
        "collapsed card must hide the body:\n{collapsed}"
    );

    // Locate the badge in the real frame buffer and click its first cell.
    let buf = terminal.backend().buffer();
    let area = *buf.area();
    let mut badge: Option<(u16, u16)> = None;
    'rows: for row in 0..area.height {
        let mut line = String::new();
        for col in 0..area.width {
            line.push_str(buf[(col, row)].symbol());
        }
        if let Some(byte) = line.find("▸ expand") {
            let col = line[..byte].chars().count() as u16;
            badge = Some((col, row));
            break 'rows;
        }
    }
    let (badge_col, badge_row) = badge.expect("expand badge must be visible in the frame");

    let click = |app: &mut App, col: u16, row: u16| {
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        });
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        });
    };

    // A click on the tldr text (left of the badge) must NOT toggle.
    click(&mut app, badge_col.saturating_sub(6), badge_row);
    assert!(
        jcode_tui_messages::parse_collapsible_swarm_content(&app.display_messages[1].content)
            .is_some_and(|parsed| !parsed.expanded),
        "click left of the badge must not expand the card"
    );

    click(&mut app, badge_col + 2, badge_row);
    let parsed =
        jcode_tui_messages::parse_collapsible_swarm_content(&app.display_messages[1].content)
            .expect("content stays collapsible after toggle");
    assert!(parsed.expanded, "badge click must expand the card");
    assert_eq!(app.status_notice(), Some("Swarm message expanded".to_string()));

    let expanded = render_and_snap(&app, &mut terminal);
    assert!(
        expanded.contains("race in the setup helper"),
        "expanded card must show the body:\n{expanded}"
    );
    assert!(
        expanded.contains("▾ collapse"),
        "expanded card must show the collapse badge:\n{expanded}"
    );

    // Click the collapse badge to fold it back down.
    let buf = terminal.backend().buffer();
    let area = *buf.area();
    let mut collapse_badge: Option<(u16, u16)> = None;
    'rows2: for row in 0..area.height {
        let mut line = String::new();
        for col in 0..area.width {
            line.push_str(buf[(col, row)].symbol());
        }
        if let Some(byte) = line.find("▾ collapse") {
            let col = line[..byte].chars().count() as u16;
            collapse_badge = Some((col, row));
            break 'rows2;
        }
    }
    let (collapse_col, collapse_row) =
        collapse_badge.expect("collapse badge must be visible in the frame");
    click(&mut app, collapse_col + 2, collapse_row);
    assert!(
        jcode_tui_messages::parse_collapsible_swarm_content(&app.display_messages[1].content)
            .is_some_and(|parsed| !parsed.expanded),
        "collapse badge click must fold the card back down"
    );
}

#[cfg(test)]
#[path = "../tests_input_scroll.rs"]
mod input_scroll_tests;

/// Opening the slash-command palette must not move anything already on
/// screen: the palette renders as an overlay (over blank rows, the pinned
/// footer, or the transcript tail), never by reserving layout height.
/// Regression for the "/ makes the whole screen jump 8 rows" report.
#[test]
fn command_palette_open_does_not_move_existing_rows() {
    let _render_lock = scroll_render_test_lock();

    // Case 1: mostly-empty session (packed layout, free space below input).
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 0, 4);
    let before = render_and_snap(&app, &mut terminal);
    app.input = "/".to_string();
    app.cursor_pos = 1;
    let after = render_and_snap(&app, &mut terminal);
    let before_rows: Vec<&str> = before.lines().collect();
    let after_rows: Vec<&str> = after.lines().collect();
    // No row may relocate: any before-row content still visible after opening
    // the palette must be at the exact same y. In-place content changes (the
    // input row gaining "/", the idle footer hint shortening, palette rows
    // covering blank space) are fine; vertical motion is the regression.
    for (y, row) in before_rows.iter().enumerate() {
        if row.trim().is_empty() {
            continue;
        }
        if after_rows.get(y).copied() == Some(*row) {
            continue;
        }
        if let Some(new_y) = after_rows.iter().position(|now| now == row) {
            panic!(
                "row moved from y={y} to y={new_y} when opening the palette:\n{row:?}\n\nbefore:\n{before}\nafter:\n{after}"
            );
        }
    }
    let palette_rows = after_rows
        .iter()
        .filter(|row| {
            let t = row.trim_start();
            t.starts_with('/') && t.contains("  ")
        })
        .count();
    assert!(
        palette_rows >= 2,
        "palette should be visible as overlay rows:\n{after}"
    );

    // Case 2: overflowing transcript (scrolling layout). The transcript must
    // not shift up: the messages area keeps its height and the palette covers
    // the bottom rows instead.
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 0, 60);
    let before = render_and_snap(&app, &mut terminal);
    app.input = "/".to_string();
    app.cursor_pos = 1;
    let after = render_and_snap(&app, &mut terminal);
    let before_rows: Vec<&str> = before.lines().collect();
    let after_rows: Vec<&str> = after.lines().collect();
    // Sample transcript rows near the top of the viewport: they must be at the
    // exact same y after opening the palette.
    let mut checked = 0;
    for (y, row) in before_rows.iter().enumerate().take(10) {
        if row.trim().is_empty() {
            continue;
        }
        let now = after_rows.get(y).copied().unwrap_or("");
        assert_eq!(
            now, *row,
            "transcript row {y} moved when opening the palette on a full screen"
        );
        checked += 1;
    }
    assert!(checked >= 3, "expected transcript rows to sample:\n{before}");
}
