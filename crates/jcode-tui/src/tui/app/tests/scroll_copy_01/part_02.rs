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

/// Terminal-style Ctrl+L: after the clear the rendered messages area shows no
/// transcript body (the spacer fills the viewport; only the sticky
/// previous-prompt preview band may remain at the top), and scrolling up
/// brings the old content back, exactly like a terminal's
/// clear-with-scrollback.
#[test]
fn test_ctrl_l_renders_clear_screen_with_history_in_scrollback() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 0, 60);

    // Seed viewport geometry (viewport height, max scroll) with a real frame.
    let before = render_and_snap(&app, &mut terminal);
    assert!(
        before.contains("Intro line"),
        "sanity: transcript body is visible before Ctrl+L"
    );

    app.handle_key(KeyCode::Char('l'), KeyModifiers::CONTROL)
        .unwrap();
    let after = render_and_snap(&app, &mut terminal);
    assert!(
        !after.contains("Intro line"),
        "after Ctrl+L no transcript body is visible:\n{after}"
    );

    // History is still there: scroll up a page and the content returns.
    for _ in 0..12 {
        app.scroll_up(3);
    }
    let scrolled = render_and_snap(&app, &mut terminal);
    assert!(
        scrolled.contains("Intro line"),
        "scrolling up reveals the pre-clear transcript:\n{scrolled}"
    );
}

#[test]
fn test_empty_prompt_up_down_browses_previous_prompts() {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("first prompt"),
        DisplayMessage::assistant("first response"),
        DisplayMessage::user("second prompt"),
    ];
    app.bump_display_messages_version();

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input, "second prompt");
    assert_eq!(app.cursor_pos, app.input.len());

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input, "first prompt");

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input, "first prompt");

    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.input, "second prompt");

    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();
    assert!(app.input.is_empty());
    assert_eq!(app.cursor_pos, 0);
}

#[test]
fn test_ctrl_up_browses_history_when_no_pending_message() {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("first prompt"),
        DisplayMessage::assistant("first response"),
        DisplayMessage::user("second prompt"),
    ];
    app.bump_display_messages_version();

    app.handle_key(KeyCode::Up, KeyModifiers::CONTROL).unwrap();
    assert_eq!(app.input, "second prompt");

    app.handle_key(KeyCode::Up, KeyModifiers::CONTROL).unwrap();
    assert_eq!(app.input, "first prompt");
}

#[test]
fn test_prompt_history_up_does_not_replace_unmatched_draft() {
    let mut app = create_test_app();
    app.display_messages = vec![DisplayMessage::user("previous prompt")];
    app.input = "draft".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();

    assert_eq!(app.input, "draft");
    assert_eq!(app.cursor_pos, "draft".len());
}

#[test]
fn test_multiline_prompt_up_down_moves_cursor_within_input() {
    let mut app = create_test_app();
    app.input = "abc\ndefg\nxy".to_string();
    app.cursor_pos = "abc\nde".len();

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(app.cursor_pos, "ab".len());

    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.cursor_pos, "abc\nde".len());

    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.cursor_pos, app.input.len());
}

#[test]
fn test_multiline_history_prompt_prioritizes_cursor_until_boundary() {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("older prompt"),
        DisplayMessage::assistant("older response"),
        DisplayMessage::user("line one\nline two"),
    ];
    app.input = "line one\nline two".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input, "line one\nline two");
    assert_eq!(app.cursor_pos, "line one".len());

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input, "older prompt");
    assert_eq!(app.cursor_pos, app.input.len());
}

#[test]
fn test_ctrl_up_down_always_browses_prompt_history() {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("older prompt"),
        DisplayMessage::assistant("older response"),
        DisplayMessage::user("line one\nline two"),
    ];
    app.input = "line one\nline two".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Up, KeyModifiers::CONTROL).unwrap();
    assert_eq!(app.input, "older prompt");
    assert_eq!(app.cursor_pos, app.input.len());

    app.handle_key(KeyCode::Down, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input, "line one\nline two");
    assert_eq!(app.cursor_pos, app.input.len());

    app.handle_key(KeyCode::Down, KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.input.is_empty());
}

#[test]
fn test_remote_empty_prompt_up_down_browses_previous_prompts() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.display_messages = vec![
        DisplayMessage::user("first remote prompt"),
        DisplayMessage::assistant("first response"),
        DisplayMessage::user("second remote prompt"),
    ];

    rt.block_on(app.handle_remote_key(KeyCode::Up, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert_eq!(app.input, "second remote prompt");

    rt.block_on(app.handle_remote_key(KeyCode::Up, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert_eq!(app.input, "first remote prompt");

    rt.block_on(app.handle_remote_key(KeyCode::Down, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert_eq!(app.input, "second remote prompt");

    rt.block_on(app.handle_remote_key(KeyCode::Down, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert!(app.input.is_empty());
}

#[test]
fn test_remote_ctrl_up_retrieves_pending_queue_before_prompt_history() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.display_messages = vec![DisplayMessage::user("previous remote prompt")];
    app.queued_messages.push("queued followup".to_string());
    app.pending_queued_dispatch = true;

    rt.block_on(app.handle_remote_key(KeyCode::Up, KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert_eq!(app.input, "queued followup");
    assert!(app.queued_messages.is_empty());
    assert!(!app.pending_queued_dispatch);
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
    let _render_lock = scroll_render_test_lock();
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
                group: None,
                id: "todo-1".to_string(),
                content: "keep going".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
                confidence: None,
                completion_confidence: None,
                confidence_history: Vec::new(),
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
    // Route the copy into the in-process sink so this passes on a headless
    // runner instead of hitting the real OS clipboard.
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);

    app.handle_key(KeyCode::Char('S'), KeyModifiers::ALT)
        .unwrap();

    let copied = clipboard.text().expect("code block should reach the clipboard");
    assert!(
        copied.contains("println!(\"hello\");"),
        "clipboard should carry the code block: {copied:?}"
    );
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
    let clipboard = CapturedClipboard::new();
    let (mut app, mut terminal) = create_copy_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    render_and_snap(&app, &mut terminal);

    rt.block_on(app.handle_remote_key(KeyCode::Char('S'), KeyModifiers::ALT, &mut remote))
        .unwrap();

    let copied = clipboard.text().expect("code block should reach the clipboard");
    assert!(
        copied.contains("println!(\"hello\");"),
        "clipboard should carry the code block: {copied:?}"
    );
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

/// Ctrl+L collapses the (entirely blank) messages viewport so the numbered
/// prompt indicator lands at the *top* of the screen, exactly like a terminal
/// after `clear`, instead of floating at the bottom under a screenful of
/// blanks. Scrolling up drops the spacer and immediately brings the transcript
/// back with the normal bottom-anchored layout.
#[test]
fn test_ctrl_l_puts_prompt_indicator_at_top_of_screen() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    let mut messages = Vec::new();
    for prompt in 0..4 {
        messages.push(DisplayMessage::user(format!("prompt number {prompt}")));
        messages.push(DisplayMessage::assistant(
            (0..15)
                .map(|line| format!("resp {prompt} line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }
    app.display_messages = messages;
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;

    let backend = ratatui::backend::TestBackend::new(80, 25);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let before = render_and_snap(&app, &mut terminal);
    assert!(
        before.contains("resp 3 line 14"),
        "sanity: transcript is visible before Ctrl+L:\n{before}"
    );

    app.handle_key(KeyCode::Char('l'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(
        app.terminal_clear_collapsed(),
        "Ctrl+L enters the collapsed terminal-clear state"
    );

    let after = render_and_snap(&app, &mut terminal);
    let rows: Vec<&str> = after.lines().collect();
    let prompt_row = rows
        .iter()
        .position(|row| row.trim_end().ends_with('>'))
        .unwrap_or_else(|| panic!("no prompt indicator row found:\n{after}"));
    assert!(
        prompt_row <= 2,
        "prompt indicator should sit at the top after Ctrl+L, found on row \
         {prompt_row}:\n{after}"
    );
    assert!(
        !after.contains("resp 3 line 14"),
        "the transcript is cleared from the screen:\n{after}"
    );

    // Scrolling up leaves the collapsed state and brings history straight back
    // without first travelling through a viewport of blank spacer rows.
    app.scroll_up(5);
    assert!(
        !app.terminal_clear_collapsed(),
        "scrolling up exits the collapsed state"
    );
    let scrolled = render_and_snap(&app, &mut terminal);
    assert!(
        scrolled.contains("prompt number 0"),
        "scrolling up reveals the pre-clear transcript:\n{scrolled}"
    );
}
