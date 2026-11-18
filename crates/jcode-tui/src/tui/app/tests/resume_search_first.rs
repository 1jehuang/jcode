// `/resume` opens the session picker search-first: the search box is focused,
// so typing a word like "quota" filters instead of hitting the `q` shortcut.
// Other picker modes keep shortcut-first focus.

#[test]
fn resume_search_first_slash_resume_focuses_search_box() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    app.input = "/resume".to_string();
    app.submit_input();

    let picker = app
        .session_picker_overlay
        .as_ref()
        .expect("/resume opens the picker");
    assert!(
        picker.borrow().search_input_focused(),
        "/resume opens with the search box focused"
    );

    // Typing a word that starts with the `q` shortcut stays in the picker.
    for c in "quota".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .expect("key handled");
    }
    assert!(
        app.session_picker_overlay.is_some(),
        "typing 'quota' must not close the picker"
    );
    assert!(app.input.is_empty(), "search text must not leak into the chat input");
}

#[test]
fn resume_search_first_active_sessions_picker_keeps_shortcut_focus() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    app.input = "/active".to_string();
    app.submit_input();

    let picker = app
        .session_picker_overlay
        .as_ref()
        .expect("/active opens the picker");
    assert!(!picker.borrow().search_input_focused());
}

// Ctrl+S in the /resume search box cycles the picker filter. It must reach
// the picker instead of the chat's Ctrl+S input stash.
#[test]
fn resume_search_first_ctrl_s_cycles_filter_not_input_stash() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    app.input = "/resume".to_string();
    app.submit_input();
    app.input = "draft".to_string();
    let before = app
        .session_picker_overlay
        .as_ref()
        .expect("/resume opens the picker")
        .borrow()
        .filter_mode_for_test();

    app.handle_key(KeyCode::Char('s'), KeyModifiers::CONTROL)
        .expect("key handled");

    let picker = app
        .session_picker_overlay
        .as_ref()
        .expect("picker stays open");
    assert_ne!(picker.borrow().filter_mode_for_test(), before);
    assert!(picker.borrow().search_input_focused());
    assert_eq!(app.input, "draft", "Ctrl+S must not stash the chat input");
    assert!(app.stashed_input.is_none());
}
