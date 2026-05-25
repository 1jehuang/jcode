#[test]
fn test_prompt_jump_ctrl_digit_is_recency_rank_in_app() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    let (prompt_up_code, prompt_up_mods) = prompt_up_key(&app);
    app.handle_key(prompt_up_code, prompt_up_mods).unwrap();
    assert!(app.scroll_offset > 0);

    // Ctrl+5 now means "5th most-recent prompt" (clamped to oldest).
    app.handle_key(KeyCode::Char('5'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.scroll_offset > 0);
}

#[test]
fn test_scroll_cmd_j_k_fallback_in_app() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    let (up_code, up_mods) = scroll_up_fallback_key(&app);
    let (down_code, down_mods) = scroll_down_fallback_key(&app);

    app.handle_key(up_code, up_mods).unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);
    let after_up = app.scroll_offset;

    app.handle_key(down_code, down_mods).unwrap();
    assert!(app.scroll_offset <= after_up);
}

#[test]
fn test_remote_prompt_jump_ctrl_brackets() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    assert_eq!(app.scroll_offset, 0);
    assert!(!app.auto_scroll_paused);

    rt.block_on(app.handle_remote_key(KeyCode::Char('['), KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);

    let after_up = app.scroll_offset;
    rt.block_on(app.handle_remote_key(KeyCode::Char(']'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert!(app.scroll_offset <= after_up);
}

#[cfg(target_os = "macos")]
#[test]
fn test_remote_prompt_jump_ctrl_esc_fallback_on_macos() {
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    assert_eq!(app.scroll_offset, 0);
    rt.block_on(app.handle_remote_key(KeyCode::Esc, KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);
}

#[test]
fn test_remote_escape_interrupt_disables_auto_poke_while_processing() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.auto_poke_incomplete_todos = true;
    app.queued_messages
        .push(super::commands::build_poke_message(&[
            crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "keep going".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            },
        ]));

    rt.block_on(app.handle_remote_key(KeyCode::Esc, KeyModifiers::empty(), &mut remote))
        .unwrap();

    assert!(!app.auto_poke_incomplete_todos);
    assert!(app.queued_messages.is_empty());
    assert_eq!(
        app.status_notice(),
        Some("Interrupting... Auto-poke OFF".to_string())
    );
}

#[test]
fn test_remote_ctrl_digit_side_panel_preset() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('4'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert_eq!(app.diagram_pane_ratio_target, 100);
}

#[test]
fn test_remote_prompt_jump_ctrl_digit_is_recency_rank() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    let (prompt_up_code, prompt_up_mods) = prompt_up_key(&app);
    rt.block_on(app.handle_remote_key(prompt_up_code, prompt_up_mods, &mut remote))
        .unwrap();
    assert!(app.scroll_offset > 0);

    // Ctrl+5 now means "5th most-recent prompt" (clamped to oldest).
    rt.block_on(app.handle_remote_key(KeyCode::Char('5'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert!(app.scroll_offset > 0);
}

#[test]
fn test_remote_ctrl_c_interrupts_while_processing() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('c'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert!(app.quit_pending.is_none());
    assert!(app.is_processing);
}

#[test]
fn test_remote_ctrl_c_still_arms_quit_when_idle() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('c'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert!(app.quit_pending.is_some());
    assert_eq!(
        app.status_notice(),
        Some("Press Ctrl+C again to quit".to_string())
    );
}

#[test]
fn test_local_copy_badge_shortcut_accepts_alt_uppercase_encoding() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);

    app.handle_key(KeyCode::Char('S'), KeyModifiers::ALT)
        .unwrap();

    let notice = app.status_notice().unwrap_or_default();
    assert!(
        notice == "Copied rust",
        "expected copy notice, got: {}",
        notice
    );

    let text = render_and_snap(&app, &mut terminal);
    assert!(
        text.contains("Copied!"),
        "expected inline copied feedback: {}",
        text
    );
}

#[test]
fn test_remote_copy_badge_shortcut_supported() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    render_and_snap(&app, &mut terminal);

    rt.block_on(app.handle_remote_key(KeyCode::Char('S'), KeyModifiers::ALT, &mut remote))
        .unwrap();

    let notice = app.status_notice().unwrap_or_default();
    assert!(
        notice == "Copied rust",
        "expected copy notice, got: {}",
        notice
    );

    let text = render_and_snap(&app, &mut terminal);
    assert!(
        text.contains("Copied!"),
        "expected inline copied feedback: {}",
        text
    );
}
