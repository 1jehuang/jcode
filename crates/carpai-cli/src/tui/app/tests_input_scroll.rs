use super::*;

#[test]
fn test_disconnected_key_handler_allows_typing_and_queueing() {
    let mut app = create_test_app();

    remote::handle_disconnected_key(&mut app, KeyCode::Char('h'), KeyModifiers::empty()).unwrap();
    remote::handle_disconnected_key(&mut app, KeyCode::Char('i'), KeyModifiers::empty()).unwrap();
    assert_eq!(app.input, "hi");

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert_eq!(app.queued_messages().len(), 1);
    assert_eq!(app.queued_messages()[0], "hi");
    assert_eq!(
        app.status_notice(),
        Some("Queued for send after reconnect (1 message)".to_string())
    );
}

#[test]
fn test_disconnected_shift_enter_inserts_newline() {
    let mut app = create_test_app();

    remote::handle_disconnected_key(&mut app, KeyCode::Char('h'), KeyModifiers::empty()).unwrap();
    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::SHIFT).unwrap();
    remote::handle_disconnected_key(&mut app, KeyCode::Char('i'), KeyModifiers::empty()).unwrap();

    assert_eq!(app.input(), "h\ni");
    assert!(app.queued_messages().is_empty());
}

#[test]
fn test_disconnected_shift_slash_inserts_question_mark() {
    let mut app = create_test_app();

    remote::handle_disconnected_key(&mut app, KeyCode::Char('/'), KeyModifiers::SHIFT).unwrap();

    assert_eq!(app.input(), "?");
    assert!(app.queued_messages().is_empty());
}

#[test]
fn test_disconnected_key_event_shift_slash_inserts_question_mark() {
    use crossterm::event::{KeyEvent, KeyEventKind};

    let mut app = create_test_app();

    remote::handle_disconnected_key_event(
        &mut app,
        KeyEvent::new_with_kind(KeyCode::Char('/'), KeyModifiers::SHIFT, KeyEventKind::Press),
    )
    .unwrap();

    assert_eq!(app.input(), "?");
    assert!(app.queued_messages().is_empty());
}

#[test]
fn test_disconnected_ctrl_enter_queues_for_reconnect() {
    let mut app = create_test_app();

    remote::handle_disconnected_key(&mut app, KeyCode::Char('h'), KeyModifiers::empty()).unwrap();
    remote::handle_disconnected_key(&mut app, KeyCode::Char('i'), KeyModifiers::empty()).unwrap();
    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL).unwrap();

    assert!(app.input.is_empty());
    assert_eq!(app.queued_messages().len(), 1);
    assert_eq!(app.queued_messages()[0], "hi");
}

#[test]
fn test_disconnected_key_handler_restart_runs_locally() {
    let mut app = create_test_app();
    app.input = "/restart".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert!(app.restart_requested.is_some());
    assert!(app.should_quit);
    assert!(app.queued_messages().is_empty());
}

#[test]
fn test_disconnected_key_handler_runs_effort_locally() {
    let mut app = create_test_app();
    app.input = "/effort".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert!(app.queued_messages().is_empty());
    let last = app
        .display_messages()
        .last()
        .expect("missing effort message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("Reasoning effort not available"));
}

#[test]
fn test_disconnected_key_handler_runs_model_picker_locally() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);
    app.input = "/model".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert!(app.queued_messages().is_empty());
    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should open");
    assert!(!picker.entries.is_empty());
    assert_eq!(picker.entries[picker.selected].name, "gpt-5.3-codex");
}

#[test]
fn test_disconnected_key_handler_runs_reload_locally() {
    use std::time::SystemTime;

    let mut app = create_test_app();
    let exe = crate::build::launcher_binary_path().expect("launcher binary path");
    let mut created = false;
    if !exe.exists() {
        if let Some(parent) = exe.parent() {
            std::fs::create_dir_all(parent).expect("create launcher dir");
        }
        std::fs::write(&exe, "test").expect("write launcher binary fixture");
        created = true;
    }

    app.client_binary_mtime = Some(SystemTime::UNIX_EPOCH);
    app.input = "/reload".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.reload_requested.is_some());
    assert!(app.should_quit);

    if created {
        let _ = std::fs::remove_file(&exe);
    }
}

#[test]
fn test_disconnected_key_handler_runs_debug_command_locally() {
    crate::tui::visual_debug::disable();

    let mut app = create_test_app();
    app.input = "/debug-visual off".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert!(app.queued_messages().is_empty());
    assert_eq!(app.status_notice(), Some("Visual debug: OFF".to_string()));
    let last = app
        .display_messages()
        .last()
        .expect("missing debug message");
    assert_eq!(last.role, "system");
    assert_eq!(last.content, "Visual debugging disabled.");
}

#[test]
fn test_disconnected_key_handler_does_not_queue_server_commands() {
    let mut app = create_test_app();
    app.input = "/server-reload".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert_eq!(app.input, "/server-reload");
    assert!(app.queued_messages().is_empty());
    assert_eq!(
        app.status_notice(),
        Some("This command requires a live connection".to_string())
    );
}

#[test]
fn test_disconnected_key_handler_ctrl_c_arms_quit() {
    let mut app = create_test_app();

    remote::handle_disconnected_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL).unwrap();

    assert!(app.quit_pending.is_some());
    assert_eq!(
        app.status_notice(),
        Some("Press Ctrl+C again to quit".to_string())
    );
}

#[test]
fn test_remote_scroll_cmd_j_k_fallback() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    let (up_code, up_mods) = scroll_up_fallback_key(&app);
    let (down_code, down_mods) = scroll_down_fallback_key(&app);

    rt.block_on(app.handle_remote_key(up_code, up_mods, &mut remote))
        .unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);
    let after_up = app.scroll_offset;

    rt.block_on(app.handle_remote_key(down_code, down_mods, &mut remote))
        .unwrap();
    assert!(app.scroll_offset <= after_up);
}

#[test]
fn test_remote_shift_enter_inserts_newline() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('h'), KeyModifiers::empty(), &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::SHIFT, &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Char('i'), KeyModifiers::empty(), &mut remote))
        .unwrap();

    assert_eq!(app.input(), "h\ni");
    assert!(app.queued_messages().is_empty());
}

#[test]
fn test_remote_ctrl_backspace_csi_u_char_fallback_deletes_word() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('\u{8}'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert_eq!(app.input(), "hello world ");
    assert_eq!(app.cursor_pos(), "hello world ".len());
}

#[test]
fn test_remote_ctrl_h_does_not_insert_text() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.set_input_for_test("hello");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('h'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert_eq!(app.input(), "hello");
    assert_eq!(app.cursor_pos(), "hello".len());
}

#[test]
fn test_remote_ctrl_enter_queues_while_processing() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.is_processing = true;
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('h'), KeyModifiers::empty(), &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Char('i'), KeyModifiers::empty(), &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert!(app.input().is_empty());
    assert_eq!(app.queued_messages().len(), 1);
    assert_eq!(app.queued_messages()[0], "hi");
}
