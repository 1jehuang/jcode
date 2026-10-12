// Resize must not let the tail-follow viewport animate (epic #1411 phase 2,
// bug #1412).
//
// Narrowing the terminal grows the wrapped row count, so `max_scroll` jumps.
// With decorative animations on, the renderer reads a jump past
// `TAIL_CATCHUP_MIN_JUMP` as a large append and slides toward the bottom from
// the pre-resize offset over several frames, which looks like the transcript
// jumping up and then sliding back down. The resize path arms the existing snap
// request so the next frame lands exactly at the new bottom.

/// `TAIL_CATCHUP_MIN_JUMP` from `ui_viewport.rs`: a jump past this starts the
/// catch-up slide instead of snapping.
const RESIZE_SNAP_MIN_JUMP: usize = 4;

#[test]
fn resize_snaps_tail_follow_to_new_bottom_without_animating() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();

    let (mut app, mut wide_terminal) = create_scroll_test_app(100, 30, 0, 60);
    app.auto_scroll_paused = false;
    app.scroll_offset = 0;

    // Establish the resolved position while following the tail at 100 columns.
    render_and_snap(&app, &mut wide_terminal);
    let wide_bottom = crate::tui::ui::last_resolved_chat_scroll();
    assert_eq!(
        wide_bottom,
        crate::tui::ui::last_max_scroll(),
        "tail-follow should start pinned to the bottom"
    );

    // No leftover snap: without the resize arming one, the narrow frame below
    // takes the animated catch-up path and lands short of the bottom.
    let _ = crate::tui::ui::take_tail_follow_snap_request();

    // A resize event is the production signal that commits the new geometry.
    assert!(app.should_redraw_after_resize());

    let mut narrow_terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow_terminal);

    let narrow_max = crate::tui::ui::last_max_scroll();
    assert!(
        narrow_max > wide_bottom + RESIZE_SNAP_MIN_JUMP,
        "narrowing must grow max_scroll past the min jump: wide={wide_bottom} narrow={narrow_max}"
    );
    assert_eq!(
        crate::tui::ui::last_resolved_chat_scroll(),
        narrow_max,
        "resize must snap to the new bottom instead of starting a catch-up slide"
    );
    assert!(
        !crate::tui::ui::tail_catchup_active(),
        "a snap must not leave the catch-up animation running"
    );
    // Observable outcome: the rendered transcript is showing its tail. The
    // catch-up path lands up to a viewport short of the bottom, which hides the
    // last lines.
    let chat = rendered_chat_area(&narrow_terminal);
    assert!(
        chat.contains("Intro line 60"),
        "after the resize the last transcript line must be on screen:\n{chat}"
    );
}

/// Text of the messages area of the most recent rendered frame.
fn rendered_chat_area(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let area = crate::tui::ui::last_layout_snapshot()
        .map(|layout| layout.messages_area)
        .expect("layout snapshot");
    buffer_to_text(terminal)
        .lines()
        .skip(area.y as usize)
        .take(area.height as usize)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn debounced_resize_burst_arms_the_snap_on_its_trailing_frame() {
    // Real event path: resize events inside the debounce window are deferred,
    // and the trailing edge is drained by `flush_pending_resize_redraw`. That
    // path has to arm the snap too, or a burst ends with the view animating.
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();

    let (mut app, mut wide_terminal) = create_scroll_test_app(100, 30, 0, 60);
    app.auto_scroll_paused = false;
    render_and_snap(&app, &mut wide_terminal);

    let _ = crate::tui::ui::take_tail_follow_snap_request();
    assert!(app.should_redraw_after_resize());
    // Consume the first event's snap so the render below isolates the flush.
    assert!(
        crate::tui::ui::take_tail_follow_snap_request(),
        "the first resize event must arm the snap"
    );
    assert!(!app.should_redraw_after_resize(), "second event debounces");
    assert!(app.resize_redraw_pending);

    app.last_resize_redraw =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(40));
    assert!(app.flush_pending_resize_redraw());

    let mut narrow_terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow_terminal);
    assert_eq!(
        crate::tui::ui::last_resolved_chat_scroll(),
        crate::tui::ui::last_max_scroll(),
        "the debounced trailing frame must snap, not animate"
    );
    assert!(rendered_chat_area(&narrow_terminal).contains("Intro line 60"));
}

#[test]
fn resize_while_paused_in_history_does_not_snap_to_bottom() {
    // The armed snap is only consumed by the tail-follow resolver, so resizing
    // while the reader is parked in history must not yank them to the bottom.
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();

    let (mut app, mut wide_terminal) = create_scroll_test_app(100, 30, 0, 60);
    app.auto_scroll_paused = false;
    render_and_snap(&app, &mut wide_terminal);
    app.scroll_up(20);
    render_and_snap(&app, &mut wide_terminal);
    let paused_before = crate::tui::ui::last_resolved_chat_scroll();
    assert!(
        paused_before < crate::tui::ui::last_max_scroll(),
        "the reader must be parked in history for this test to mean anything"
    );

    let _ = crate::tui::ui::take_tail_follow_snap_request();
    assert!(app.should_redraw_after_resize());

    let mut narrow_terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow_terminal);
    let paused_after = crate::tui::ui::last_resolved_chat_scroll();
    assert_eq!(
        paused_after, paused_before,
        "a resize must not move a paused viewport"
    );
    assert!(
        paused_after < crate::tui::ui::last_max_scroll(),
        "a resize must not snap a paused viewport to the bottom"
    );
    assert!(!crate::tui::ui::tail_catchup_active());
}

#[test]
fn resize_without_overflow_stays_at_the_top() {
    // Edge case: nothing to scroll. The armed snap must be harmless when
    // max_scroll is 0, and no catch-up state may be left behind.
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();

    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    app.display_messages = vec![DisplayMessage::assistant("short response")];
    app.bump_display_messages_version();
    app.auto_scroll_paused = false;
    app.status = ProcessingStatus::Idle;

    let mut wide_terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut wide_terminal);
    assert_eq!(crate::tui::ui::last_max_scroll(), 0, "fixture must fit");

    let _ = crate::tui::ui::take_tail_follow_snap_request();
    assert!(app.should_redraw_after_resize());

    let mut narrow_terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow_terminal);
    assert_eq!(crate::tui::ui::last_max_scroll(), 0);
    assert_eq!(crate::tui::ui::last_resolved_chat_scroll(), 0);
    assert!(!crate::tui::ui::tail_catchup_active());
}
