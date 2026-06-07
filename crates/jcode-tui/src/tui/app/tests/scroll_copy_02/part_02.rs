#[test]
fn test_copy_badge_modifier_highlights_while_held() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode};

    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Modifier(ModifierKeyCode::LeftAlt),
        KeyModifiers::ALT,
        KeyEventKind::Press,
    ));
    assert!(app.copy_badge_ui().alt_active);

    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Modifier(ModifierKeyCode::LeftShift),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
        KeyEventKind::Press,
    ));
    assert!(app.copy_badge_ui().shift_active);

    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Modifier(ModifierKeyCode::LeftShift),
        KeyModifiers::ALT,
        KeyEventKind::Release,
    ));
    assert!(!app.copy_badge_ui().shift_active);

    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Modifier(ModifierKeyCode::LeftAlt),
        KeyModifiers::empty(),
        KeyEventKind::Release,
    ));
    assert!(!app.copy_badge_ui().alt_active);
}

#[test]
fn test_copy_badge_requires_prior_combo_progress() {
    let mut state = CopyBadgeUiState::default();
    let now = std::time::Instant::now();

    state.shift_active = true;
    state.shift_pulse_until = Some(now + std::time::Duration::from_millis(100));
    state.key_active = Some(('s', now + std::time::Duration::from_millis(100)));

    assert!(
        !state.shift_is_active(now),
        "shift should not light before alt"
    );
    assert!(
        !state.key_is_active('s', now),
        "final key should not light before alt+shift"
    );

    state.alt_active = true;
    assert!(
        state.shift_is_active(now),
        "shift should light once alt is active"
    );
    assert!(
        state.key_is_active('s', now),
        "final key should light once alt+shift are active"
    );
}

#[test]
fn test_expand_badge_shortcut_toggles_inline_diff_and_pulses_key() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = create_copy_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('E'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ));

    assert_eq!(app.diff_mode, crate::config::DiffDisplayMode::FullInline);
    assert!(app.copy_badge_ui().key_active.is_some());
}

#[test]
fn test_expand_badge_shortcut_does_not_collapse_full_inline_diff() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = create_copy_test_app();
    crate::tui::ui::clear_test_render_state_for_tests();
    app.diff_mode = crate::config::DiffDisplayMode::FullInline;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('E'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ));

    assert_eq!(app.diff_mode, crate::config::DiffDisplayMode::FullInline);
    assert!(
        app.status_notice().is_none(),
        "full-inline E shortcut should not run expand/collapse action"
    );
}

fn make_edit_badge_test_app(
    old_line_count: usize,
) -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    let old_string = (0..old_line_count)
        .map(|idx| format!("old line {idx}\n"))
        .collect::<String>();
    let new_string = (0..old_line_count)
        .map(|idx| format!("new line {idx}\n"))
        .collect::<String>();
    app.display_messages = vec![
        DisplayMessage::user("please edit demo.txt"),
        DisplayMessage::tool(
            "Edited demo.txt".to_string(),
            crate::message::ToolCall {
                id: "edit_1".to_string(),
                name: "edit".to_string(),
                input: serde_json::json!({
                    "file_path": "demo.txt",
                    "old_string": old_string,
                    "new_string": new_string,
                }),
                intent: None, thought_signature: None, },
        ),
    ];
    app.bump_display_messages_version();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(120, 40);
    let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    (app, terminal)
}

fn assert_rendered_expand_badge_shortcut_expands_to_full_diff(
    key_code: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = make_edit_badge_test_app(20);

    let rendered = render_and_snap(&app, &mut terminal);
    assert!(
        rendered.contains("more changes"),
        "expected collapsed diff:\n{rendered}"
    );
    assert!(
        rendered.contains("[E] expand"),
        "expected visible expand badge for collapsed edit diff:\n{rendered}"
    );
    assert!(
        crate::tui::ui::visible_expand_edit_badge_line().is_some(),
        "rendering a visible expand badge should register its line"
    );

    app.handle_key_event(crossterm::event::KeyEvent::new(key_code, modifiers));
    assert_eq!(app.diff_mode, crate::config::DiffDisplayMode::FullInline);
    assert!(
        app.copy_badge_ui().expand_feedback_line.is_some(),
        "activating a visible expand badge should persist the rendered badge line"
    );
    assert!(
        app.copy_badge_ui()
            .expand_feedback_is_active(std::time::Instant::now()),
        "activating a visible expand badge should arm transient visual feedback"
    );

    let rendered = render_and_snap(&app, &mut terminal);
    assert!(
        !rendered.contains("more changes"),
        "expanded full inline diff should not be collapsed:\n{rendered}"
    );
    assert!(
        rendered.contains("[E] ✓ Expanded"),
        "expanded full inline diff should briefly show the activated expand badge like copy feedback:\n{rendered}"
    );
    assert!(
        rendered.contains("new line 19"),
        "expanded diff should include the previously hidden tail:\n{rendered}"
    );
}

#[test]
fn test_expand_badge_rendered_shortcut_expands_with_explicit_shift_event() {
    use crossterm::event::{KeyCode, KeyModifiers};

    // Matches the debug key injector and terminals that report Alt+Shift+E as a
    // lowercase char plus an explicit SHIFT modifier.
    assert_rendered_expand_badge_shortcut_expands_to_full_diff(
        KeyCode::Char('e'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    );
}

#[test]
fn test_expand_badge_rendered_shortcut_expands_with_alt_uppercase_event() {
    use crossterm::event::{KeyCode, KeyModifiers};

    // Matches terminals that encode Alt+Shift+E like the copy badge path:
    // Alt plus an uppercase character and no explicit SHIFT modifier.
    assert_rendered_expand_badge_shortcut_expands_to_full_diff(
        KeyCode::Char('E'),
        KeyModifiers::ALT,
    );
}

#[test]
fn test_expand_badge_rendered_shortcut_expands_with_alt_lowercase_event() {
    use crossterm::event::{KeyCode, KeyModifiers};

    // Matches terminals that lose the Shift bit and lowercase the character for
    // Alt+Shift+E. The fallback is intentionally scoped to the expand badge.
    assert_rendered_expand_badge_shortcut_expands_to_full_diff(
        KeyCode::Char('e'),
        KeyModifiers::ALT,
    );
}

#[test]
fn test_expand_badge_shortcut_works_while_diff_pane_focused() {
    use crossterm::event::{KeyCode, KeyModifiers};

    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = make_edit_badge_test_app(20);
    app.diff_pane_focus = true;

    let rendered = render_and_snap(&app, &mut terminal);
    assert!(
        rendered.contains("[E] expand"),
        "expected visible expand badge before shortcut:\n{rendered}"
    );

    app.handle_key_event(crossterm::event::KeyEvent::new(
        KeyCode::Char('E'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ));

    assert_eq!(
        app.diff_mode,
        crate::config::DiffDisplayMode::FullInline,
        "diff pane focus should not swallow the visible expand badge shortcut"
    );
}

#[test]
fn test_remote_expand_badge_rendered_shortcut_expands_with_alt_uppercase_event() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = make_edit_badge_test_app(20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let rendered = render_and_snap(&app, &mut terminal);
    assert!(
        rendered.contains("[E] expand"),
        "expected visible expand badge before remote key injection:\n{rendered}"
    );

    use crossterm::event::{KeyCode, KeyModifiers};
    rt.block_on(app.handle_remote_key(KeyCode::Char('E'), KeyModifiers::ALT, &mut remote))
        .unwrap();

    assert_eq!(app.diff_mode, crate::config::DiffDisplayMode::FullInline);
    let rendered = render_and_snap(&app, &mut terminal);
    assert!(
        rendered.contains("new line 19"),
        "remote expand shortcut should reveal the full inline diff:\n{rendered}"
    );
}

#[test]
fn test_remote_expand_badge_rendered_shortcut_expands_with_alt_lowercase_event() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = make_edit_badge_test_app(20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let rendered = render_and_snap(&app, &mut terminal);
    assert!(
        rendered.contains("[E] expand"),
        "expected visible expand badge before remote key injection:\n{rendered}"
    );

    use crossterm::event::{KeyCode, KeyModifiers};
    rt.block_on(app.handle_remote_key(KeyCode::Char('e'), KeyModifiers::ALT, &mut remote))
        .unwrap();

    assert_eq!(app.diff_mode, crate::config::DiffDisplayMode::FullInline);
    let rendered = render_and_snap(&app, &mut terminal);
    assert!(
        rendered.contains("new line 19"),
        "remote expand shortcut should reveal the full inline diff:\n{rendered}"
    );
}

#[test]
fn test_expand_badge_does_not_render_for_short_untruncated_edit_diff() {
    let _render_lock = scroll_render_test_lock();
    let (app, mut terminal) = make_edit_badge_test_app(2);

    let rendered = render_and_snap(&app, &mut terminal);
    assert!(
        !rendered.contains("[E] expand"),
        "short full-visible edit diff should not show expand badge:\n{rendered}"
    );
}

#[test]
fn test_expand_badge_shortcut_opens_full_inline_from_non_inline_mode() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = create_copy_test_app();
    app.display_messages.push(DisplayMessage::tool(
        "Edited demo.txt".to_string(),
        crate::message::ToolCall {
            id: "edit_1".to_string(),
            name: "edit".to_string(),
            input: serde_json::json!({
                "file_path": "demo.txt",
                "old_string": "old line\n",
                "new_string": "new line\n",
            }),
            intent: None, thought_signature: None, },
    ));
    app.bump_display_messages_version();
    app.diff_mode = crate::config::DiffDisplayMode::Off;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('E'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ));

    assert_eq!(app.diff_mode, crate::config::DiffDisplayMode::FullInline);
    assert!(app.copy_badge_ui().key_active.is_some());
}

#[test]
fn test_expand_badge_shortcut_uses_display_messages_when_edit_count_is_stale() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = create_copy_test_app();
    app.display_messages.push(DisplayMessage::tool(
        "Edited demo.txt".to_string(),
        crate::message::ToolCall {
            id: "edit_1".to_string(),
            name: "edit".to_string(),
            input: serde_json::json!({
                "file_path": "demo.txt",
                "old_string": "old line\n",
                "new_string": "new line\n",
            }),
            intent: None, thought_signature: None, },
    ));
    app.bump_display_messages_version();
    app.diff_mode = crate::config::DiffDisplayMode::Off;
    app.display_edit_tool_message_count = 0;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('e'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ));

    assert_eq!(app.diff_mode, crate::config::DiffDisplayMode::FullInline);
    assert!(app.input.is_empty(), "shortcut should not insert text");
}

#[test]
fn test_try_open_link_at_opens_clicked_url_and_sets_notice() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    crate::tui::ui::clear_copy_viewport_snapshot();
    crate::tui::ui::record_copy_viewport_snapshot(
        std::sync::Arc::new(vec!["Docs: https://example.com/docs".to_string()]),
        std::sync::Arc::new(vec![0]),
        std::sync::Arc::new(vec!["Docs: https://example.com/docs".to_string()]),
        std::sync::Arc::new(vec![crate::tui::ui::WrappedLineMap {
            raw_line: 0,
            start_col: 0,
            end_col: 30,
        }]),
        0,
        1,
        Rect::new(0, 0, 80, 5),
        &[0],
    );

    let opened = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let opened_for_closure = opened.clone();

    let handled = app.try_open_link_at_with(10, 0, |url| {
        *opened_for_closure.lock().unwrap() = Some(url.to_string());
        Ok::<(), &'static str>(())
    });

    assert!(handled);
    assert_eq!(
        *opened.lock().unwrap(),
        Some("https://example.com/docs".to_string())
    );
    assert_eq!(
        app.status_notice(),
        Some("Opened link: https://example.com/docs".to_string())
    );
}

#[test]
fn test_mouse_click_in_input_moves_cursor_to_clicked_position() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_pos = app.input.len();
    app.set_centered(false);
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(60, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let input_area = layout.input_area.expect("input area");
    let next_prompt = crate::tui::ui::input_ui::next_input_prompt_number(&app);
    let prompt_len = crate::tui::ui::input_ui::input_prompt_len(&app, next_prompt) as u16;

    let handled = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: input_area.x + prompt_len + 2,
        row: input_area.y,
        modifiers: KeyModifiers::empty(),
    });

    assert!(!handled, "clicks should request an immediate redraw");
    assert_eq!(app.cursor_pos, 2);
}

#[test]
fn test_mouse_click_in_main_chat_switches_focus_from_side_panel() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.diff_pane_focus = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: String::new(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    let backend = ratatui::backend::TestBackend::new(80, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let messages_area = layout.messages_area;

    let handled = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: messages_area.x + messages_area.width / 2,
        row: messages_area.y + messages_area.height / 2,
        modifiers: KeyModifiers::empty(),
    });

    assert!(!handled, "clicks should request an immediate redraw");
    assert!(
        !app.diff_pane_focus,
        "clicking chat should restore chat focus"
    );
    assert_eq!(app.status_notice(), Some("Focus: chat".to_string()));
}

#[test]
fn test_mouse_click_in_input_switches_focus_from_side_panel() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.diff_pane_focus = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: String::new(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };
    app.input = "hello world".to_string();
    app.cursor_pos = app.input.len();
    app.set_centered(false);
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(60, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let input_area = layout.input_area.expect("input area");
    let next_prompt = crate::tui::ui::input_ui::next_input_prompt_number(&app);
    let prompt_len = crate::tui::ui::input_ui::input_prompt_len(&app, next_prompt) as u16;

    let handled = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: input_area.x + prompt_len + 2,
        row: input_area.y,
        modifiers: KeyModifiers::empty(),
    });

    assert!(!handled, "clicks should request an immediate redraw");
    assert_eq!(app.cursor_pos, 2);
    assert!(
        !app.diff_pane_focus,
        "clicking input should restore chat focus"
    );
    assert_eq!(app.status_notice(), Some("Focus: chat".to_string()));
}

#[test]
fn test_mouse_click_in_wrapped_input_moves_cursor_to_second_visual_line() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "abcdefghij".to_string();
    app.cursor_pos = 0;
    app.set_centered(false);
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(11, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let input_area = layout.input_area.expect("input area");

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: input_area.x + 4,
        row: input_area.y + 1,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(app.cursor_pos, 5);
}
