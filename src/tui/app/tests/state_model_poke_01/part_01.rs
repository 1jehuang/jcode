#[test]
fn test_context_limit_error_detection() {
    assert!(is_context_limit_error(
        "OpenAI API error 400: This model's maximum context length is 200000 tokens"
    ));
    assert!(is_context_limit_error(
        "request too large: prompt is too long for context window"
    ));
    assert!(!is_context_limit_error(
        "rate limit exceeded, retry after 20s"
    ));
}

#[test]
fn test_rewind_truncates_provider_messages() {
    let mut app = create_test_app();
    app.session.replace_messages(Vec::new());

    for idx in 1..=3 {
        let text = format!("msg-{}", idx);
        app.add_provider_message(Message::user(&text));
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text,
                cache_control: None,
            }],
        );
    }
    app.provider_session_id = Some("provider-session".to_string());
    app.session.provider_session_id = Some("provider-session".to_string());

    app.input = "/rewind 2".to_string();
    app.submit_input();

    assert_eq!(app.messages.len(), 2);
    assert_eq!(app.session.messages.len(), 2);
    assert!(matches!(
        &app.messages[1].content[0],
        ContentBlock::Text { text, .. } if text == "msg-2"
    ));
    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());
}

#[test]
fn test_rewind_undo_restores_truncated_messages() {
    let mut app = create_test_app();
    app.session.replace_messages(Vec::new());

    for idx in 1..=3 {
        let text = format!("msg-{}", idx);
        app.add_provider_message(Message::user(&text));
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text,
                cache_control: None,
            }],
        );
    }
    app.provider_session_id = Some("provider-session".to_string());
    app.session.provider_session_id = Some("provider-session".to_string());

    app.input = "/rewind 1".to_string();
    app.submit_input();
    assert_eq!(app.session.visible_conversation_message_count(), 1);
    assert!(
        app.display_messages()
            .last()
            .expect("rewind notice")
            .content
            .contains("Undo anytime with `/rewind undo`")
    );

    app.input = "/rewind undo".to_string();
    app.submit_input();

    assert_eq!(app.session.visible_conversation_message_count(), 3);
    assert_eq!(app.messages.len(), 3);
    assert_eq!(app.provider_session_id.as_deref(), Some("provider-session"));
    assert_eq!(
        app.session.provider_session_id.as_deref(),
        Some("provider-session")
    );
    assert!(
        app.display_messages()
            .last()
            .expect("undo notice")
            .content
            .contains("✓ Undid rewind. Restored 2 messages.")
    );
}

#[test]
fn test_rewind_lists_visible_messages_when_initial_session_context_is_hidden() {
    let mut app = create_test_app();

    for idx in 1..=2 {
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("msg-{}", idx),
                cache_control: None,
            }],
        );
    }

    app.input = "/rewind".to_string();
    app.submit_input();

    let last = app.display_messages().last().expect("history message");
    assert!(last.content.contains("**Conversation history:**"));
    assert!(last.content.contains("`1` 👤 User - msg-1"));
    assert!(last.content.contains("`2` 👤 User - msg-2"));
    assert!(!last.content.contains("Session Context"));
    assert!(!last.content.contains("No messages in conversation"));
}

#[test]
fn test_rewind_autocomplete_does_not_fuzzy_rewrite_numeric_targets() {
    let mut app = create_test_app();
    app.session.replace_messages(Vec::new());

    for idx in 1..=3 {
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("msg-{}", idx),
                cache_control: None,
            }],
        );
    }

    app.input = "/rewind 10".to_string();
    assert!(!app.autocomplete());
    assert_eq!(app.input, "/rewind 10");

    app.input = "/rewind 2".to_string();
    assert!(!app.autocomplete());
    assert_eq!(app.input, "/rewind 2");
}

#[test]
fn test_rewind_autocomplete_uses_visible_message_count() {
    let mut app = create_test_app();
    app.session.replace_messages(Vec::new());

    app.session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "<system-reminder>hidden</system-reminder>".to_string(),
            cache_control: None,
        }],
    );
    app.session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "visible".to_string(),
            cache_control: None,
        }],
    );

    assert_eq!(app.session.messages.len(), 2);
    assert_eq!(app.session.visible_conversation_message_count(), 1);

    app.input = "/rewind ".to_string();
    let suggestions = app.get_suggestions_for(&app.input);
    assert_eq!(suggestions, vec![("/rewind 1".to_string(), "Rewind to this message")]);
}

#[test]
fn test_accumulate_streaming_output_tokens_uses_deltas() {
    let mut app = create_test_app();
    let mut seen = 0;

    app.streaming_tps_collect_output = true;
    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(10));

    app.accumulate_streaming_output_tokens(10, &mut seen);
    app.accumulate_streaming_output_tokens(30, &mut seen);
    app.accumulate_streaming_output_tokens(30, &mut seen);

    assert_eq!(app.streaming_total_output_tokens, 30);
    assert_eq!(app.streaming_tps_observed_output_tokens, 30);
    assert!(app.streaming_tps_observed_elapsed >= Duration::from_secs(9));
    assert_eq!(seen, 30);
}

#[test]
fn test_accumulate_streaming_output_tokens_ignores_hidden_output_phase() {
    let mut app = create_test_app();
    let mut seen = 0;

    app.accumulate_streaming_output_tokens(20, &mut seen);
    assert_eq!(app.streaming_total_output_tokens, 0);
    assert_eq!(app.streaming_tps_observed_output_tokens, 0);
    assert_eq!(seen, 20);

    app.streaming_tps_collect_output = true;
    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(10));
    app.accumulate_streaming_output_tokens(60, &mut seen);

    assert_eq!(app.streaming_total_output_tokens, 40);
    assert_eq!(app.streaming_tps_observed_output_tokens, 40);
    assert_eq!(seen, 60);
}

#[test]
fn test_compute_streaming_tps_uses_latest_observed_snapshot_instead_of_current_repaint_time() {
    let mut app = create_test_app();
    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(20));
    app.streaming_tps_observed_output_tokens = 40;
    app.streaming_tps_observed_elapsed = Duration::from_secs(10);

    let tps = app.compute_streaming_tps().expect("tps");
    assert!(tps > 3.9 && tps < 4.1, "unexpected tps: {tps}");
}

#[test]
fn test_compute_streaming_tps_does_not_decay_on_redundant_usage_snapshots() {
    let mut app = create_test_app();
    let mut seen = 0;

    app.streaming_tps_collect_output = true;
    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(10));
    app.accumulate_streaming_output_tokens(40, &mut seen);
    let initial_tps = app.compute_streaming_tps().expect("initial tps");

    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(30));
    app.accumulate_streaming_output_tokens(40, &mut seen);

    let tps = app.compute_streaming_tps().expect("tps");
    assert!(
        initial_tps > 3.9 && initial_tps < 4.1,
        "unexpected initial tps: {initial_tps}"
    );
    assert!(
        tps > 3.9 && tps < 4.1,
        "unexpected tps after redundant snapshot: {tps}"
    );
}

#[test]
fn test_compute_streaming_tps_bursty_stream_simulation_stays_constant_between_real_updates() {
    let mut app = create_test_app();
    let mut seen = 0;

    app.streaming_tps_collect_output = true;

    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(2));
    app.accumulate_streaming_output_tokens(10, &mut seen);
    let tps_after_first_burst = app.compute_streaming_tps().expect("tps after first burst");

    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(5));
    app.accumulate_streaming_output_tokens(10, &mut seen);
    let tps_after_idle_gap = app.compute_streaming_tps().expect("tps after idle gap");

    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(6));
    app.accumulate_streaming_output_tokens(30, &mut seen);
    let tps_after_second_burst = app.compute_streaming_tps().expect("tps after second burst");

    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(9));
    app.accumulate_streaming_output_tokens(30, &mut seen);
    let tps_after_second_idle_gap = app
        .compute_streaming_tps()
        .expect("tps after second idle gap");

    assert!(
        tps_after_first_burst > 4.9 && tps_after_first_burst < 5.1,
        "unexpected first burst tps: {tps_after_first_burst}"
    );
    assert!(
        (tps_after_idle_gap - tps_after_first_burst).abs() < 0.01,
        "tps changed without new tokens: first={tps_after_first_burst} idle={tps_after_idle_gap}"
    );
    assert!(
        tps_after_second_burst > 4.9 && tps_after_second_burst < 5.1,
        "unexpected second burst tps: {tps_after_second_burst}"
    );
    assert!(
        (tps_after_second_idle_gap - tps_after_second_burst).abs() < 0.01,
        "tps changed without new tokens: second={tps_after_second_burst} idle={tps_after_second_idle_gap}"
    );
}

#[test]
fn test_initial_state() {
    let app = create_test_app();

    assert!(!app.is_processing());
    assert!(app.input().is_empty());
    assert_eq!(app.cursor_pos(), 0);
    assert!(app.display_messages().is_empty());
    assert!(app.streaming_text().is_empty());
    assert_eq!(app.queued_count(), 0);
    assert!(matches!(app.status(), ProcessingStatus::Idle));
    assert!(app.elapsed().is_none());
}

#[test]
fn test_handle_key_typing() {
    let mut app = create_test_app();

    // Type "hello"
    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('o'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "hello");
    assert_eq!(app.cursor_pos(), 5);
}

#[test]
fn test_handle_key_shift_slash_preserves_layout_translated_slash() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('/'), KeyModifiers::SHIFT)
        .unwrap();

    assert_eq!(app.input(), "/");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_handle_key_event_shift_slash_preserves_layout_translated_slash() {
    use crossterm::event::{KeyEvent, KeyEventKind};

    let mut app = create_test_app();

    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Char('/'),
        KeyModifiers::SHIFT,
        KeyEventKind::Press,
    ));

    assert_eq!(app.input(), "/");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_handle_key_control_alt_symbol_inserts_layout_translated_text() {
    let mut app = create_test_app();

    app.handle_key(
        KeyCode::Char('@'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    )
    .unwrap();

    assert_eq!(app.input(), "@");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_super_space_toggles_next_prompt_new_session_routing() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char(' '), KeyModifiers::SUPER)
        .unwrap();
    assert!(app.route_next_prompt_to_new_session);
    assert_eq!(
        app.status_notice(),
        Some("Next prompt → new session".to_string())
    );

    app.handle_key(KeyCode::Char(' '), KeyModifiers::SUPER)
        .unwrap();
    assert!(!app.route_next_prompt_to_new_session);
    assert_eq!(
        app.status_notice(),
        Some("Next-prompt new session canceled".to_string())
    );
}

#[test]
fn test_handle_key_backspace() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Backspace, KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "a");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_diagram_focus_toggle_and_pan() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x1, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0x2, 120, 90, None);

    // Ctrl+L focuses diagram when available
    app.handle_key(KeyCode::Char('l'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.diagram_focus);

    // Pan should update scroll offsets and not type into input
    app.handle_key(KeyCode::Char('j'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.diagram_scroll_y, 3);
    assert!(app.input.is_empty());

    // Ctrl+H returns focus to chat
    app.handle_key(KeyCode::Char('h'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(!app.diagram_focus);

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_ctrl_l_without_focusable_pane_does_not_clear_session() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Off;
    app.input = "draft message".to_string();
    app.cursor_pos = app.input.len();
    app.display_messages = vec![DisplayMessage::system("keep chat".to_string())];
    app.bump_display_messages_version();

    app.handle_key(KeyCode::Char('l'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "draft message");
    assert_eq!(app.cursor_pos(), "draft message".len());
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].content, "keep chat");
    assert!(!app.diagram_focus);
    assert!(!app.diff_pane_focus);
}

#[test]
fn test_diagram_cycle_ctrl_arrows() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_focus = true;
    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x1, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0x2, 120, 90, None);
    crate::tui::mermaid::register_active_diagram(0x3, 140, 100, None);

    assert_eq!(app.diagram_index, 0);
    app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_index, 1);
    app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_index, 2);
    app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_index, 0);
    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_index, 2);

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_cycle_diagram_resets_view_to_fit() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_focus = true;
    app.diagram_zoom = 140;
    app.diagram_scroll_x = 12;
    app.diagram_scroll_y = 7;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x1, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0x2, 120, 90, None);

    app.cycle_diagram(1);

    assert_eq!(app.diagram_index, 1);
    assert_eq!(app.diagram_zoom, 100);
    assert_eq!(app.diagram_scroll_x, 0);
    assert_eq!(app.diagram_scroll_y, 0);

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_resize_resets_diagram_and_side_panel_diagram_view_to_fit() {
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_zoom = 130;
    app.diagram_scroll_x = 9;
    app.diagram_scroll_y = 4;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "```mermaid\nflowchart LR\nA-->B\n```".to_string(),
            updated_at_ms: 1,
        }],
    };
    app.diff_pane_scroll_x = 17;

    assert!(app.should_redraw_after_resize());
    assert_eq!(app.diagram_zoom, 100);
    assert_eq!(app.diagram_scroll_x, 0);
    assert_eq!(app.diagram_scroll_y, 0);
    assert_eq!(app.diff_pane_scroll_x, 0);
}

#[test]
fn test_side_panel_visibility_change_resets_diagram_fit_context() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0xabc, 900, 450, None);

    app.normalize_diagram_state();
    assert_eq!(app.last_visible_diagram_hash, Some(0xabc));

    app.diagram_zoom = 150;
    app.diagram_scroll_x = 8;
    app.diagram_scroll_y = 3;
    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("side".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "side".to_string(),
            title: "Side".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    });

    assert_eq!(app.diagram_zoom, 100);
    assert_eq!(app.diagram_scroll_x, 0);
    assert_eq!(app.diagram_scroll_y, 0);
    assert_eq!(app.last_visible_diagram_hash, None);

    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot::default());
    assert_eq!(app.last_visible_diagram_hash, Some(0xabc));

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_goal_side_panel_focus_updates_status_notice() {
    let mut app = create_test_app();

    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("goals".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "goals".to_string(),
            title: "Goals".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Goals".to_string(),
            updated_at_ms: 1,
        }],
    });
    assert_eq!(app.status_notice(), Some("Goals".to_string()));

    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("goal.ship-mobile-mvp".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "goal.ship-mobile-mvp".to_string(),
            title: "Goal: Ship mobile MVP".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Goal: Ship mobile MVP".to_string(),
            updated_at_ms: 2,
        }],
    });
    assert_eq!(
        app.status_notice(),
        Some("Goal: Ship mobile MVP".to_string())
    );
}

#[test]
fn test_side_panel_same_page_update_preserves_scroll_position() {
    let mut app = create_test_app();
    app.diff_pane_scroll = 14;
    app.diff_pane_scroll_x = 3;

    let first = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "plan.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Plan\n\nVersion 1".to_string(),
            updated_at_ms: 1,
        }],
    };
    app.set_side_panel_snapshot(first);
    app.diff_pane_scroll = 14;
    app.diff_pane_scroll_x = 3;

    let second = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "plan.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Plan\n\nVersion 2".to_string(),
            updated_at_ms: 2,
        }],
    };
    app.set_side_panel_snapshot(second);

    assert_eq!(app.diff_pane_scroll, 14);
    assert_eq!(app.diff_pane_scroll_x, 3);
}

#[test]
fn test_pinned_side_diagram_layout_allocates_right_pane() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;
    app.diagram_pane_ratio = 40;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x111, 900, 450, Some("side".to_string()));

    crate::tui::visual_debug::enable();
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");

    let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
    let diagram = frame.layout.diagram_area.expect("diagram area");
    let messages = frame.layout.messages_area.expect("messages area");

    assert!(
        diagram.width >= 24,
        "diagram pane too narrow: {}",
        diagram.width
    );
    assert_eq!(diagram.height, 40);
    assert_eq!(diagram.x, messages.x + messages.width);
    assert_eq!(diagram.y, 0);
    assert!(
        diagram.width < 120,
        "diagram should not consume full terminal width"
    );
    assert!(
        frame
            .render_order
            .iter()
            .any(|s| s == "draw_pinned_diagram")
    );

    crate::tui::visual_debug::disable();
    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_pinned_top_diagram_layout_allocates_top_pane() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Top;
    app.diagram_pane_ratio = 35;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x222, 500, 900, Some("top".to_string()));

    crate::tui::visual_debug::enable();
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");

    let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
    let diagram = frame.layout.diagram_area.expect("diagram area");
    let messages = frame.layout.messages_area.expect("messages area");

    assert_eq!(diagram.x, 0);
    assert_eq!(diagram.width, 120);
    assert!(
        diagram.height >= 6,
        "diagram pane too short: {}",
        diagram.height
    );
    assert_eq!(messages.y, diagram.y + diagram.height);
    assert!(
        frame
            .render_order
            .iter()
            .any(|s| s == "draw_pinned_diagram")
    );

    crate::tui::visual_debug::disable();
    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_pinned_diagram_not_shown_when_terminal_too_narrow() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x333, 900, 450, None);

    crate::tui::visual_debug::enable();
    let backend = ratatui::backend::TestBackend::new(30, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");

    let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
    assert!(
        frame.layout.diagram_area.is_none(),
        "diagram pane should be suppressed on narrow terminal"
    );
    assert!(
        !frame
            .render_order
            .iter()
            .any(|s| s == "draw_pinned_diagram")
    );

    crate::tui::visual_debug::disable();
    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_workspace_info_widget_appears_in_visual_debug_frame_when_enabled() {
    let _render_lock = scroll_render_test_lock();
    crate::tui::workspace_client::reset_for_tests();

    let mut app = create_test_app();
    app.centered = true;
    app.display_messages = vec![
        DisplayMessage::system("Workspace widget render test".to_string()),
        DisplayMessage::assistant("Short content keeps room for info widgets.".to_string()),
    ];
    app.bump_display_messages_version();

    let current_session = app.session.id.clone();
    crate::tui::workspace_client::enable(
        Some(current_session.as_str()),
        &[current_session.clone(), "workspace_peer".to_string()],
    );

    crate::tui::visual_debug::enable();
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");

    let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
    let widget = frame
        .layout
        .widget_placements
        .iter()
        .find(|placement| placement.kind == "workspace")
        .expect("workspace widget placement");

    assert_eq!(widget.side, "right");
    assert!(
        widget.rect.width > 0,
        "workspace widget width should be non-zero"
    );
    assert!(
        widget.rect.height > 0,
        "workspace widget height should be non-zero"
    );
    assert!(
        frame
            .info_widgets
            .as_ref()
            .expect("info widget capture")
            .placements
            .iter()
            .any(|placement| placement.kind == "workspace"),
        "workspace widget should be present in info widget capture"
    );

    crate::tui::visual_debug::disable();
    crate::tui::workspace_client::reset_for_tests();
}

#[test]
fn test_mouse_scroll_over_diff_pane_scrolls_side_panel_without_changing_focus() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::File;
    app.diff_pane_scroll = 5;
    app.diff_pane_focus = false;
    app.diff_pane_auto_scroll = true;

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(app.diff_pane_scroll, 6);
    assert!(!app.diff_pane_focus);
    assert!(!app.diff_pane_auto_scroll);
}

#[test]
fn test_mouse_scroll_animation_preserves_side_pane_scroll_sensitivity() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::File;
    app.diff_pane_scroll = 5;
    app.diff_pane_auto_scroll = true;

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(app.diff_pane_scroll, 6, "first frame should move one line");

    let _ = crate::tui::app::local::handle_tick(&mut app);
    assert_eq!(app.diff_pane_scroll, 7);

    crate::tui::app::local::handle_tick(&mut app);
    assert_eq!(
        app.diff_pane_scroll, 8,
        "one wheel notch should still total three lines"
    );
}

#[test]
fn test_mouse_scroll_over_tool_side_panel_scrolls_shared_right_pane_without_changing_focus() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.diff_pane_scroll = 5;
    app.diff_pane_focus = false;
    app.diff_pane_auto_scroll = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        !scroll_only,
        "side-panel wheel scroll should request an immediate redraw"
    );
    assert_eq!(app.diff_pane_scroll, 6);
    assert!(!app.diff_pane_focus);
    assert!(!app.diff_pane_auto_scroll);
}
