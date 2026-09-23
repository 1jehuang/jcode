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
}
