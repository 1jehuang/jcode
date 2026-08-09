#[test]
fn issue_832_remote_ctrl_k_kills_draft_but_ctrl_shift_k_scrolls() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    render_and_snap(&app, &mut terminal);
    app.set_input_for_test("hello world again");
    app.cursor_pos = "hello world ".len();

    rt.block_on(app.handle_remote_key(KeyCode::Char('k'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert_eq!(app.input(), "hello world ");
    assert_eq!(app.cursor_pos(), "hello world ".len());
    assert_eq!(app.scroll_offset, 0, "plain Ctrl+K must not jump prompts");

    app.set_input_for_test("hello world again");
    app.cursor_pos = "hello world ".len();
    rt.block_on(app.handle_remote_key(
        KeyCode::Char('k'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        &mut remote,
    ))
    .unwrap();

    assert_eq!(app.input(), "hello world again");
    assert_eq!(app.cursor_pos(), "hello world ".len());
    assert!(app.scroll_offset > 0, "Ctrl+Shift+K must still scroll up");
}

#[test]
fn issue_832_disconnected_ctrl_k_kills_draft_but_ctrl_shift_k_scrolls() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    render_and_snap(&app, &mut terminal);
    app.set_input_for_test("hello world again");
    app.cursor_pos = "hello world ".len();

    super::remote::handle_disconnected_key(
        &mut app,
        KeyCode::Char('k'),
        KeyModifiers::CONTROL,
    )
    .unwrap();

    assert_eq!(app.input(), "hello world ");
    assert_eq!(app.cursor_pos(), "hello world ".len());
    assert_eq!(app.scroll_offset, 0, "plain Ctrl+K must not jump prompts");

    app.set_input_for_test("hello world again");
    app.cursor_pos = "hello world ".len();
    super::remote::handle_disconnected_key(
        &mut app,
        KeyCode::Char('k'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )
    .unwrap();

    assert_eq!(app.input(), "hello world again");
    assert_eq!(app.cursor_pos(), "hello world ".len());
    assert!(app.scroll_offset > 0, "Ctrl+Shift+K must still scroll up");
}
