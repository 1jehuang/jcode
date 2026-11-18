use super::*;

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
