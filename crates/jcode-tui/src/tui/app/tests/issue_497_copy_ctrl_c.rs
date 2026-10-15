// Issue #497: Ctrl+C while a copy-mode selection is active must copy the
// selection, not fall through to the global interrupt/quit handler (which
// closed jcode and lost the very error the user was trying to copy).

#[test]
fn test_ctrl_c_with_active_copy_selection_copies_instead_of_quitting() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::error(
        "some provider error worth copying".to_string(),
    ));

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    app.enter_copy_selection_mode();
    assert!(app.select_all_in_copy_mode());
    assert!(
        app.current_copy_selection_text()
            .is_some_and(|t| !t.is_empty()),
        "test needs a non-empty selection"
    );

    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();

    assert!(
        app.quit_pending.is_none(),
        "Ctrl+C over a selection must not arm quit"
    );
    // The copy path ran (not the quit path): it always sets a copy status.
    // Clipboard success depends on the environment (headless CI has none),
    // so accept either outcome of the copy itself.
    let notice = app.status_notice();
    assert!(
        notice.as_deref() == Some("Copied selection")
            || notice.as_deref() == Some("Failed to copy selection"),
        "expected copy-path status notice, got {notice:?}"
    );
}

#[test]
fn test_ctrl_c_in_copy_mode_without_selection_still_falls_through() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    app.enter_copy_selection_mode();
    assert!(app.current_copy_selection_text().is_none());

    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();

    // No selection: preserve the existing interrupt/quit semantics.
    assert!(
        app.quit_pending.is_some() || app.cancel_requested,
        "Ctrl+C without a selection keeps interrupt/quit behavior"
    );
}

// Client (remote) mode used to skip copy-selection routing entirely, so Enter/Y
// over a chat selection typed into or submitted the composer instead of copying.
#[test]
fn test_remote_copy_mode_enter_and_y_copy_selection() {
    let _render_lock = scroll_render_test_lock();
    crate::tui::app::helpers::capture_clipboard_for_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    for key in [KeyCode::Enter, KeyCode::Char('y')] {
        let mut app = create_test_app();
        app.push_display_message(DisplayMessage::error("remote copy target".to_string()));
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal =
            ratatui::Terminal::new(backend).expect("failed to create test terminal");
        render_and_snap(&app, &mut terminal);
        app.set_input_for_test("cp");

        app.enter_copy_selection_mode();
        assert!(app.select_all_in_copy_mode());

        rt.block_on(app.handle_remote_key(key, KeyModifiers::empty(), &mut remote))
            .unwrap();

        assert_eq!(app.status_notice().as_deref(), Some("Copied selection"));
        assert!(!app.copy_selection_mode);
        assert_eq!(app.input(), "cp", "copy keys must not reach the composer");
        assert!(
            crate::tui::app::helpers::captured_clipboard_for_tests()
                .is_some_and(|text| text.contains("remote copy target"))
        );
    }
    crate::tui::app::helpers::stop_capturing_clipboard_for_tests();
}
