use super::*;

#[test]
fn slash_sessions_with_query_opens_picker_pre_filtered() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    app.input = "/sessions deploy bug".to_string();
    app.submit_input();

    let picker = app
        .session_picker_overlay
        .as_ref()
        .expect("picker overlay should be open");
    let picker = picker.borrow();
    assert_eq!(picker.search_query(), "deploy bug");
    assert!(picker.search_is_active());
    assert!(app.input.is_empty());
}

#[test]
fn slash_session_with_query_opens_picker_pre_filtered() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    app.input = "/session refactor".to_string();
    app.submit_input();

    let picker = app
        .session_picker_overlay
        .as_ref()
        .expect("picker overlay should be open");
    let picker = picker.borrow();
    assert_eq!(picker.search_query(), "refactor");
    assert!(picker.search_is_active());
}

#[test]
fn slash_sessions_with_whitespace_only_query_opens_picker_unfiltered() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    // Only whitespace after the command should behave like the bare command.
    app.input = "/sessions   ".to_string();
    app.submit_input();

    assert!(app.session_picker_overlay.is_some());
    assert_eq!(app.session_picker_mode, SessionPickerMode::Resume);
    let picker = app
        .session_picker_overlay
        .as_ref()
        .expect("picker overlay should be open");
    let picker = picker.borrow();
    assert_eq!(picker.search_query(), "");
    assert!(!picker.search_is_active());
}

#[test]
fn remote_slash_sessions_with_query_opens_picker_pre_filtered() {
    // The connected-remote key path has its own dispatch chain in
    // handle_remote_key_internal; it must route `/sessions <query>` through
    // the same pre-filtered picker opening as the local path.
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "/sessions deploy bug".to_string();
    app.cursor_pos = app.input.len();

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
        &mut remote,
    ))
    .expect("/sessions <query> should be handled in remote mode");

    assert!(
        app.input.is_empty(),
        "submitted remote command should clear input"
    );
    let picker = app
        .session_picker_overlay
        .as_ref()
        .expect("picker overlay should be open in remote mode");
    let picker = picker.borrow();
    assert_eq!(picker.search_query(), "deploy bug");
    assert!(picker.search_is_active());
}

#[test]
fn remote_slash_sessions_bare_command_still_opens_unfiltered_picker() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "/sessions".to_string();
    app.cursor_pos = app.input.len();

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
        &mut remote,
    ))
    .expect("bare /sessions should still work in remote mode");

    assert!(app.session_picker_overlay.is_some());
    let picker = app
        .session_picker_overlay
        .as_ref()
        .expect("picker overlay should be open");
    let picker = picker.borrow();
    assert_eq!(picker.search_query(), "");
    assert!(!picker.search_is_active());
}
