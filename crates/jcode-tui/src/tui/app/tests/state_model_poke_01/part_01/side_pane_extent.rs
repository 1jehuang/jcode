use super::*;

#[test]
fn test_side_pane_scroll_by_clamps_to_rendered_extent() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    // Simulate a rendered frame: 30 content lines in a 20-line viewport.
    crate::tui::ui::set_pinned_pane_total_lines(30);
    crate::tui::ui::set_last_diff_pane_max_scroll(10);
    crate::tui::ui::set_last_diff_pane_effective_scroll(10);

    // Follow-bottom sentinel resolves to the on-screen position before moving.
    app.diff_pane_scroll = usize::MAX;
    assert!(app.side_pane_scroll_by(-3));
    assert_eq!(app.diff_pane_scroll, 7);
    assert!(!app.diff_pane_auto_scroll);

    // Downward motion clamps at the rendered max instead of accumulating
    // phantom offset past the bottom.
    app.diff_pane_scroll = 9;
    assert!(app.side_pane_scroll_by(3));
    assert_eq!(app.diff_pane_scroll, 10);
    assert!(!app.side_pane_scroll_by(3), "already at the bottom");
    assert_eq!(app.diff_pane_scroll, 10);

    // A stale stored offset beyond the rendered extent snaps back so the very
    // next upward scroll moves the visible view immediately.
    app.diff_pane_scroll = 25;
    assert!(app.side_pane_scroll_by(-3));
    assert_eq!(app.diff_pane_scroll, 7);
}
