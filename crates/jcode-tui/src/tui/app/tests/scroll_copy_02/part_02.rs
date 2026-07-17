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
fn test_alt_shift_i_toggles_inline_images_and_persists() {
    let _render_lock = scroll_render_test_lock();
    let _env_guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let (mut app, _terminal) = create_copy_test_app();
    app.is_remote = true;
    app.remote_side_pane_images
        .push(crate::session::RenderedImage {
            media_type: "image/png".to_string(),
            data: "image-data".to_string(),
            label: Some("preview.png".to_string()),
            source: crate::session::RenderedImageSource::UserInput,
            anchor: None,
        });
    app.invalidate_side_pane_images_signature();
    assert!(app.inline_images_visible);

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('I'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ));
    assert!(!app.inline_images_visible, "Alt+Shift+I should hide images");
    assert_eq!(
        app.status_notice(),
        Some("Inline images: hidden (Alt+Shift+I to show)".to_string())
    );

    // The flag persists for the next app (e.g. resume after restart).
    assert!(!crate::tui::app::ui_prefs::inline_images_visible());

    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('I'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ));
    assert!(
        app.inline_images_visible,
        "second toggle should show images"
    );
    assert!(crate::tui::app::ui_prefs::inline_images_visible());

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn text_only_transcript_updates_keep_inline_image_signature_cached() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = create_copy_test_app();
    app.is_remote = true;
    app.remote_side_pane_images = (0..24)
        .map(|index| crate::session::RenderedImage {
            media_type: "image/png".to_string(),
            // Large enough to make accidental payload cloning/re-rendering costly,
            // without bloating the test process excessively.
            data: format!("{index:02}-{}", "A".repeat(256 * 1024)),
            label: Some(format!("image-{index}.png")),
            source: crate::session::RenderedImageSource::UserInput,
            anchor: None,
        })
        .collect();
    app.invalidate_side_pane_images_signature();

    let signature = crate::tui::TuiState::side_pane_images_signature(&app);
    assert_eq!(signature.0, 24);
    assert_eq!(app.side_pane_images_signature_cache.get(), Some(signature));

    // Text/tool messages can change many times during a turn. They must not
    // evict the image signature and force all base64 payloads to be cloned and
    // walked again on the next frame.
    app.bump_display_messages_version_no_stats();
    assert_eq!(app.side_pane_images_signature_cache.get(), Some(signature));
    assert_eq!(
        crate::tui::TuiState::side_pane_images_signature(&app),
        signature
    );
}

#[test]
fn inline_image_signature_distinguishes_labels_and_same_prefix_payloads() {
    use std::hash::Hasher as _;

    let signature = |image: &crate::session::RenderedImage| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        crate::tui::hash_rendered_image_signature_fields(image, &mut hasher);
        hasher.finish()
    };
    let base = crate::session::RenderedImage {
        media_type: "image/png".to_string(),
        data: format!("{}tail-a", "A".repeat(128)),
        label: Some("first.png".to_string()),
        source: crate::session::RenderedImageSource::UserInput,
        anchor: None,
    };
    let mut changed_tail = base.clone();
    changed_tail.data = format!("{}tail-b", "A".repeat(128));
    let mut changed_label = base.clone();
    changed_label.label = Some("second.png".to_string());
    let middle_base = crate::session::RenderedImage {
        data: format!("{}middle-a{}", "A".repeat(128), "Z".repeat(128)),
        ..base.clone()
    };
    let middle_changed = crate::session::RenderedImage {
        data: format!("{}middle-b{}", "A".repeat(128), "Z".repeat(128)),
        ..middle_base.clone()
    };

    assert_ne!(signature(&base), signature(&changed_tail));
    assert_ne!(signature(&base), signature(&changed_label));
    assert_ne!(signature(&middle_base), signature(&middle_changed));
}

#[test]
fn test_alt_shift_i_is_inert_without_inline_images() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal) = create_copy_test_app();
    app.is_remote = true;
    app.remote_side_pane_images.clear();
    app.invalidate_side_pane_images_signature();

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('I'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ));
    assert!(
        app.inline_images_visible,
        "toggle must stay inert when no images exist"
    );
    assert!(app.status_notice().is_none());
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
                intent: None,
                thought_signature: None,
            },
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
            intent: None,
            thought_signature: None,
        },
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
            intent: None,
            thought_signature: None,
        },
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

/// End-to-end: a real left-click on an inline image's label line maps the
/// screen point back through a recorded `ChatFrame` snapshot to the image id and
/// cycles its expand level. This exercises the full click path
/// (`handle_mouse_event` -> `try_cycle_image_expand_at` ->
/// `inline_image_expand_target_from_screen` -> `cycle_image_expand`), not just
/// the isolated helpers.
#[test]
fn test_click_on_inline_image_label_line_cycles_level() {
    use crate::tui::ui::inline_image_ui::{
        AllFit, ImageExpandLevel, InlineImageItem, build_section,
    };
    use jcode_tui_messages::PreparedChatFrame;

    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    const IMAGE_ID: u64 = 0xFEED;
    let chat_width: u16 = 80;

    // Build a real inline-image section: a `shot.png … hide` label line
    // followed by Fit-rendered placeholder rows with a scanned `image_regions`
    // entry.
    let items = vec![InlineImageItem {
        id: IMAGE_ID,
        width: 600,
        height: 400,
        label: "shot.png".to_string(),
    }];
    let section = build_section(&items, chat_width, 40, false, true, &AllFit);

    // Locate the label line (the one carrying the image label); the whole line
    // is the click target now that the expand badge is gone.
    let label_line = section
        .wrapped_plain_lines
        .iter()
        .position(|line| line.contains("shot.png"))
        .expect("section should contain the image label line");

    // The Fit image region must sit exactly one line below the label line, which
    // is how `inline_image_id_for_label_line` maps a click back to the image.
    assert!(
        section
            .image_regions
            .iter()
            .any(|r| r.hash == IMAGE_ID && r.abs_line_idx == label_line + 1),
        "expected a Fit image region anchored under the label line"
    );

    let prepared =
        std::sync::Arc::new(PreparedChatFrame::from_single(std::sync::Arc::new(section)));
    let visible_end = prepared.wrapped_plain_line_count();
    let content_area = Rect::new(0, 0, chat_width, visible_end as u16 + 1);

    crate::tui::ui::clear_copy_viewport_snapshot();
    crate::tui::ui::record_copy_viewport_frame_snapshot_for_test(
        prepared,
        0,
        visible_end,
        content_area,
        &vec![0u16; visible_end],
    );

    assert_eq!(
        app.image_expand_level(IMAGE_ID),
        ImageExpandLevel::Fit,
        "image should start at Fit"
    );

    // Click the label line (button up is what fires the cycle).
    let handled = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: content_area.x + 2,
        row: content_area.y + label_line as u16,
        modifiers: KeyModifiers::empty(),
    });
    assert!(!handled, "handled click should request an immediate redraw");
    assert_eq!(
        app.image_expand_level(IMAGE_ID),
        ImageExpandLevel::Large,
        "first label click should expand Fit -> Large"
    );
    assert_eq!(app.status_notice(), Some("Image size: large".to_string()));

    // Further label clicks continue the cycle: Large -> Full -> Fit.
    let click_label = |app: &mut App| {
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: content_area.x + 2,
            row: content_area.y + label_line as u16,
            modifiers: KeyModifiers::empty(),
        });
    };
    click_label(&mut app);
    assert_eq!(
        app.image_expand_level(IMAGE_ID),
        ImageExpandLevel::Full,
        "second click should expand Large -> Full"
    );
    click_label(&mut app);
    assert_eq!(
        app.image_expand_level(IMAGE_ID),
        ImageExpandLevel::Fit,
        "cycle should wrap Full -> Fit"
    );
}

/// Kitty reports mouse motion at pixel granularity, so a physically plain
/// click usually arrives as Down -> Drag(same cell) -> Up. The same-cell Drag
/// must NOT start a selection drag; the release must still fall through to the
/// label-line click handler. Regression test for "click does nothing on
/// kitty".
#[test]
fn test_kitty_jitter_click_on_image_label_still_cycles_level() {
    use crate::tui::ui::inline_image_ui::{
        AllFit, ImageExpandLevel, InlineImageItem, build_section,
    };
    use jcode_tui_messages::PreparedChatFrame;

    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    const IMAGE_ID: u64 = 0xF00D;
    let chat_width: u16 = 80;
    let items = vec![InlineImageItem {
        id: IMAGE_ID,
        width: 600,
        height: 400,
        label: "shot.png".to_string(),
    }];
    let section = build_section(&items, chat_width, 40, false, true, &AllFit);
    let label_line = section
        .wrapped_plain_lines
        .iter()
        .position(|line| line.contains("shot.png"))
        .expect("section should contain the image label line");
    let badge_col: u16 = 2;

    let prepared =
        std::sync::Arc::new(PreparedChatFrame::from_single(std::sync::Arc::new(section)));
    let visible_end = prepared.wrapped_plain_line_count();
    let content_area = Rect::new(0, 0, chat_width, visible_end as u16 + 1);

    crate::tui::ui::clear_copy_viewport_snapshot();
    crate::tui::ui::record_copy_viewport_frame_snapshot_for_test(
        prepared,
        0,
        visible_end,
        content_area,
        &vec![0u16; visible_end],
    );

    let (col, row) = (
        content_area.x + badge_col,
        content_area.y + label_line as u16,
    );
    let inject = |app: &mut App, kind: MouseEventKind| {
        app.handle_mouse_event(MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        });
    };

    // Down, same-cell Drag (kitty pixel jitter), Up: must count as a click.
    inject(&mut app, MouseEventKind::Down(MouseButton::Left));
    inject(&mut app, MouseEventKind::Drag(MouseButton::Left));
    inject(&mut app, MouseEventKind::Up(MouseButton::Left));

    assert_eq!(
        app.image_expand_level(IMAGE_ID),
        ImageExpandLevel::Large,
        "jitter click (down + same-cell drag + up) must still cycle the badge"
    );

    // A real drag to a DIFFERENT cell must still start a selection, not click.
    inject(&mut app, MouseEventKind::Down(MouseButton::Left));
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: col.saturating_sub(4),
        row,
        modifiers: KeyModifiers::empty(),
    });
    inject(&mut app, MouseEventKind::Up(MouseButton::Left));
    assert_eq!(
        app.image_expand_level(IMAGE_ID),
        ImageExpandLevel::Large,
        "a real drag ending on the badge must not fire the click handler"
    );
}

/// 1x1 transparent PNG: a real image header so the inline-image pipeline decodes
/// dimensions and assigns a stable id, exactly like a `read`-tool screenshot.
const REPRO_TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

/// FULL end-to-end reproduction of the user's "clicking the image does
/// nothing" report. Unlike `test_click_on_inline_image_label_line_cycles_level`
/// (which records a synthetic `ChatFrame` snapshot directly), this drives the
/// *real* draw: a local App whose session carries a `read`-tool result image,
/// anchored into the transcript body, rendered through `terminal.draw()`, which
/// is what records the live copy-viewport snapshot. We then locate the rendered
/// image label line in the actual frame buffer and inject a real left click,
/// asserting the image size cycles. This exercises the body-anchored image path
/// (`render_images` -> `resolve_anchored_items` -> `anchored_image_lines`), the
/// path actually used in production, not the isolated `build_section` helper.
#[test]
fn test_real_draw_click_on_body_anchored_image_label_cycles_level() {
    use crate::message::{ContentBlock, Role};
    use crate::tui::ui::inline_image_ui::ImageExpandLevel;

    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    assert!(!app.is_remote, "repro must use the local image render path");

    const TOOL_ID: &str = "read-shot-1";

    // Build a real transcript: user asks, assistant calls `read`, tool result
    // carries the screenshot image. This is exactly what produces a
    // body-anchored inline image with a `RenderedImageAnchor::ToolCall`.
    app.session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "read the screenshot".to_string(),
            cache_control: None,
        }],
    );
    app.session.add_message(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: TOOL_ID.to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"file_path": "shot.png"}),
            thought_signature: None,
        }],
    );
    app.session.add_message(
        Role::User,
        vec![
            ContentBlock::ToolResult {
                tool_use_id: TOOL_ID.to_string(),
                content: "read image".to_string(),
                is_error: None,
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: REPRO_TINY_PNG_B64.to_string(),
            },
        ],
    );

    // Mirror the session into the display transcript the body renderer walks.
    app.display_messages = vec![
        DisplayMessage::user("read the screenshot"),
        DisplayMessage::tool(
            "read shot.png",
            crate::message::ToolCall {
                id: TOOL_ID.to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file_path": "shot.png"}),
                intent: None,
                thought_signature: None,
            },
        ),
    ];
    app.bump_display_messages_version();
    app.invalidate_side_pane_images_signature();
    app.pin_images = true;
    app.inline_images_visible = true;
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    // Sanity: the local render path must actually surface the anchored image.
    let images = <App as crate::tui::TuiState>::side_pane_images(&app);
    assert_eq!(
        images.len(),
        1,
        "session should render exactly one anchored tool image"
    );
    let image_id = {
        let img = &images[0];
        crate::tui::mermaid::inline_image_dims(&img.media_type, &img.data)
            .expect("tiny png should decode")
            .0
    };

    let backend = ratatui::backend::TestBackend::new(80, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    // REAL draw: this records the live copy-viewport snapshot used by clicks.
    let rendered = render_and_snap(&app, &mut terminal);
    assert!(
        rendered.contains("shot.png"),
        "image label line must render in the live frame, got:\n{rendered}"
    );

    // Find the label line in the actual buffer: scan rows for the row carrying
    // the image label, then click a cell inside the label text.
    let buf = terminal.backend().buffer();
    let area = *buf.area();
    let mut badge: Option<(u16, u16)> = None;
    'rows: for row in 0..area.height {
        let mut line = String::new();
        for col in 0..area.width {
            line.push_str(buf[(col, row)].symbol());
        }
        // The transcript also shows the tool-call row ("read shot.png"); the
        // image label row is the one that carries the show/hide badge keys.
        if !line.contains("shot.png") || !line.contains("[I]") {
            continue;
        }
        // Click the first cell of the label text (the hit-region is the whole
        // label line, so any cell on the row works).
        for col in 0..area.width {
            if buf[(col, row)].symbol() == "s" {
                badge = Some((col, row));
                break 'rows;
            }
        }
    }
    let (badge_col, badge_row) = badge.expect("image label cell should be visible in the frame");

    assert_eq!(
        app.image_expand_level(image_id),
        ImageExpandLevel::Fit,
        "image should start at Fit before any click"
    );

    // REAL click on the rendered label cell. A terminal delivers a *pair* of
    // events for one physical click: `Down` then `Up`. We must replay both, just
    // like the live event loop, or we silently skip the copy-selection state the
    // `Down` arms (which is exactly what the user's click goes through).
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: badge_col,
        row: badge_row,
        modifiers: KeyModifiers::empty(),
    });
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: badge_col,
        row: badge_row,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(
        app.image_expand_level(image_id),
        ImageExpandLevel::Large,
        "clicking the rendered image label must cycle Fit -> Large \
         (this is the exact path the user reported as broken)"
    );
    assert_eq!(app.status_notice(), Some("Image size: large".to_string()));
}

/// The inline-image placeholder marker row must never reach the terminal as
/// text. It used to be drawn black-on-black and relied on staying invisible,
/// but terminal-side compositing (kitty translucent background + contrast
/// compositing) and selection highlighting can recolor it, leaking raw
/// "IIMG:<hash>:..." into the transcript whenever the image is not painted
/// over it (cold cache after reload, prewarm in flight, no image protocol).
/// The draw path must blank marker rows instead.
#[test]
fn test_real_draw_never_emits_inline_image_marker_text() {
    use crate::message::{ContentBlock, Role};

    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    assert!(!app.is_remote, "repro must use the local image render path");

    const TOOL_ID: &str = "read-shot-marker";

    app.session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "read the screenshot".to_string(),
            cache_control: None,
        }],
    );
    app.session.add_message(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: TOOL_ID.to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"file_path": "shot.png"}),
            thought_signature: None,
        }],
    );
    app.session.add_message(
        Role::User,
        vec![
            ContentBlock::ToolResult {
                tool_use_id: TOOL_ID.to_string(),
                content: "read image".to_string(),
                is_error: None,
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: REPRO_TINY_PNG_B64.to_string(),
            },
        ],
    );

    app.display_messages = vec![
        DisplayMessage::user("read the screenshot"),
        DisplayMessage::tool(
            "read shot.png",
            crate::message::ToolCall {
                id: TOOL_ID.to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file_path": "shot.png"}),
                intent: None,
                thought_signature: None,
            },
        ),
    ];
    app.bump_display_messages_version();
    app.invalidate_side_pane_images_signature();
    app.pin_images = true;
    app.inline_images_visible = true;
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(80, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let rendered = render_and_snap(&app, &mut terminal);

    assert!(
        rendered.contains("shot.png"),
        "sanity: the anchored image's label line must render, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("IIMG"),
        "raw inline-image marker text must never be drawn to the terminal, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("MERMAID_IMAGE"),
        "raw mermaid marker text must never be drawn to the terminal, got:\n{rendered}"
    );
}

/// Clicking anywhere on the image body (its placeholder rows) must cycle the
/// expand level, exactly like the label badge. Clicks in the blank area to
/// the RIGHT of a narrow image must not.
#[test]
fn test_click_on_inline_image_body_cycles_level() {
    use crate::tui::ui::inline_image_ui::{
        AllFit, ImageExpandLevel, InlineImageItem, build_section,
    };
    use jcode_tui_messages::PreparedChatFrame;

    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    const IMAGE_ID: u64 = 0xBEEF;
    let chat_width: u16 = 80;

    let items = vec![InlineImageItem {
        id: IMAGE_ID,
        width: 320,
        height: 200,
        label: "shot.png".to_string(),
    }];
    let section = build_section(&items, chat_width, 40, false, true, &AllFit);
    let region = *section
        .image_regions
        .iter()
        .find(|r| r.hash == IMAGE_ID)
        .expect("section should carry the image region");
    assert!(region.width > 0, "fit regions record their rendered width");
    assert!(
        region.width < chat_width,
        "test image must be narrower than the chat so the right side is blank"
    );

    let prepared =
        std::sync::Arc::new(PreparedChatFrame::from_single(std::sync::Arc::new(section)));
    let visible_end = prepared.wrapped_plain_line_count();
    let content_area = Rect::new(0, 0, chat_width, visible_end as u16 + 1);

    crate::tui::ui::clear_copy_viewport_snapshot();
    crate::tui::ui::record_copy_viewport_frame_snapshot_for_test(
        prepared,
        0,
        visible_end,
        content_area,
        &vec![0u16; visible_end],
    );

    assert_eq!(app.image_expand_level(IMAGE_ID), ImageExpandLevel::Fit);

    // Click in the middle of the image body (a placeholder row, inside the
    // rendered width). Down then Up, like a real terminal click.
    let body_row = content_area.y + region.abs_line_idx as u16 + 1;
    let body_col = content_area.x + region.width / 2;
    let click = |app: &mut App, col: u16, row: u16| {
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        });
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        });
    };
    click(&mut app, body_col, body_row);
    assert_eq!(
        app.image_expand_level(IMAGE_ID),
        ImageExpandLevel::Large,
        "clicking the image body should expand Fit -> Large"
    );

    // Clicking the body again advances the cycle.
    click(&mut app, body_col, body_row);
    assert_eq!(
        app.image_expand_level(IMAGE_ID),
        ImageExpandLevel::Full,
        "second body click should expand Large -> Full"
    );
    click(&mut app, body_col, body_row);
    assert_eq!(
        app.image_expand_level(IMAGE_ID),
        ImageExpandLevel::Fit,
        "third body click should wrap Full -> Fit"
    );

    // A click in the blank space to the right of the image must stay inert.
    let far_right = content_area.x + chat_width - 2;
    assert!(far_right > content_area.x + region.width);
    click(&mut app, far_right, body_row);
    assert_eq!(
        app.image_expand_level(IMAGE_ID),
        ImageExpandLevel::Fit,
        "clicking blank space beside the image must not cycle it"
    );
}
