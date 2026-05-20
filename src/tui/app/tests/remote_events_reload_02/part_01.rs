#[test]
fn test_remote_poke_queues_when_turn_is_in_progress() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Continue working".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.is_remote = true;
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(42);
        app.input = "/poke".to_string();
        app.cursor_pos = app.input.len();

        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
            .expect("/poke should queue behind the current turn");

        assert!(app.auto_poke_incomplete_todos);
        assert!(app.is_processing);
        assert!(matches!(app.status, ProcessingStatus::Streaming));
        assert_eq!(app.current_message_id, Some(42));
        assert!(app.input().is_empty());
        assert_eq!(
            app.status_notice(),
            Some("Poke queued after current turn".to_string())
        );
        assert!(app.queued_messages().is_empty());
        assert!(app.display_messages().iter().any(|msg| {
            msg.content
                .contains("/poke queued. Re-checking incomplete todos after this turn")
        }));

        crate::todo::save_todos(
            &app.session.id,
            &[
                crate::todo::TodoItem {
                    id: "todo-1".to_string(),
                    content: "Continue working".to_string(),
                    status: "pending".to_string(),
                    priority: "high".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
                crate::todo::TodoItem {
                    id: "todo-2".to_string(),
                    content: "Handle the newly discovered follow-up".to_string(),
                    status: "pending".to_string(),
                    priority: "medium".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
            ],
        )
        .expect("save updated todos");

        let needs_redraw =
            app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);

        assert!(needs_redraw);
        assert!(app.pending_queued_dispatch);
        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("You have 2 incomplete todos"));
        assert!(!app.queued_messages()[0].contains("Handle the newly discovered follow-up"));
        assert!(!app.queued_messages()[0].contains("/poke off"));
    });
}

#[test]
fn test_remote_ctrl_p_toggles_auto_poke() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.is_remote = true;
        assert!(app.auto_poke_incomplete_todos);

        rt.block_on(app.handle_remote_key(KeyCode::Char('p'), KeyModifiers::CONTROL, &mut remote))
            .expect("Ctrl+P should disable poke remotely");
        assert!(!app.auto_poke_incomplete_todos);
        assert_eq!(app.status_notice(), Some("Poke: OFF".to_string()));

        rt.block_on(app.handle_remote_key(KeyCode::Char('p'), KeyModifiers::CONTROL, &mut remote))
            .expect("Ctrl+P should enable poke remotely");
        assert!(app.auto_poke_incomplete_todos);
        assert_eq!(app.status_notice(), Some("Poke: ON".to_string()));
        assert!(app.display_messages().iter().any(|msg| {
            msg.content
                .contains("Auto-poke enabled. No incomplete todos found right now.")
        }));
    });
}

#[test]
fn test_remote_transfer_queues_pause_when_processing() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.is_remote = true;
        app.is_processing = true;

        app.input = "/transfer".to_string();
        app.cursor_pos = app.input.len();
        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
            .expect("/transfer should queue while processing");

        assert!(app.pending_transfer_request);
        assert_eq!(app.pending_split_label.as_deref(), Some("Transfer"));
        assert_eq!(
            app.status_notice(),
            Some("Transfer queued after current turn".to_string())
        );
    });
}

#[test]
fn test_remote_interrupted_auto_poke_requeues_after_deferred_poke() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Resume after interrupt".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.is_remote = true;
        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(42);

        let needs_redraw =
            app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

        assert!(needs_redraw);
        assert!(app.pending_queued_dispatch);
        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("You have 1 incomplete todo"));
        assert!(!app.queued_messages()[0].contains("/poke off"));
    });
}

#[test]
fn test_handle_server_event_tool_start_flushes_streaming_text_before_tool_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.streaming_text = "Let me inspect those files first.".to_string();

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolStart {
            id: "tool_batch".to_string(),
            name: "batch".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming_text.is_empty());
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].role, "assistant");
    assert_eq!(
        app.display_messages()[0].content,
        "Let me inspect those files first."
    );
    assert_eq!(app.streaming_tool_calls.len(), 1);
    assert_eq!(app.streaming_tool_calls[0].name, "batch");
    assert!(matches!(app.status, ProcessingStatus::RunningTool(ref name) if name == "batch"));
}

#[test]
fn test_handle_server_event_remote_observe_tracks_tool_exec_and_done() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.input = "/observe on".to_string();
    app.submit_input();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("observe"));

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolStart {
            id: "tool_read".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolInput {
            delta: r#"{"file_path":"src/main.rs","start_line":1,"end_line":10}"#.to_string(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolExec {
            id: "tool_read".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );

    let page = app.side_panel.focused_page().expect("missing observe page");
    assert!(
        page.content
            .contains("Latest tool call emitted by the model")
    );
    assert!(page.content.contains("`read`"));
    assert!(page.content.contains("src/main.rs"));

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolDone {
            id: "tool_read".to_string(),
            name: "read".to_string(),
            output: "1 fn main() {}".to_string(),
            error: None,
        },
        &mut remote,
    );

    let page = app.side_panel.focused_page().expect("missing observe page");
    let token_label =
        crate::util::format_approx_token_count(crate::util::estimate_tokens("1 fn main() {}"));
    assert!(page.content.contains("Latest tool result added to context"));
    assert!(page.content.contains("Status: completed"));
    assert!(page.content.contains("Returned to context"));
    assert!(page.content.contains(&token_label));
    assert!(page.content.contains("1 fn main() {}"));
}

#[test]
fn test_handle_remote_event_redraws_observe_tool_exec_immediately() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let backend = ratatui::backend::TestBackend::new(90, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let mut state = super::remote::RemoteRunState::default();

    app.input = "/observe on".to_string();
    app.submit_input();

    let (outcome, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolStart {
                id: "tool_read".to_string(),
                name: "read".to_string(),
            }),
        ))
        .expect("tool start should succeed");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    assert!(matches!(
        outcome,
        super::remote::RemoteEventOutcome::Continue
    ));

    let (outcome, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolInput {
                delta: r#"{"file_path":"src/main.rs","start_line":1,"end_line":10}"#.to_string(),
            }),
        ))
        .expect("tool input should succeed");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    assert!(matches!(
        outcome,
        super::remote::RemoteEventOutcome::Continue
    ));

    let (outcome, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolExec {
                id: "tool_read".to_string(),
                name: "read".to_string(),
            }),
        ))
        .expect("tool exec should succeed");
    assert!(needs_redraw, "observe tool exec should request redraw");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    assert!(matches!(
        outcome,
        super::remote::RemoteEventOutcome::Continue
    ));

    let text = buffer_to_text(&terminal);
    assert!(
        text.contains("Latest tool call emitted by the"),
        "observe tool exec should redraw immediately:\n{text}"
    );
    assert!(text.contains("Tool input"));
    assert!(text.contains("src/main.rs"));
}

#[test]
fn test_remote_protocol_error_stops_instead_of_reconnecting() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let backend = ratatui::backend::TestBackend::new(90, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let mut state = super::remote::RemoteRunState::default();

    app.is_processing = true;
    let (outcome, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Disconnected(
                crate::tui::backend::RemoteDisconnectReason::Protocol(
                    "expected value at line 1 column 1".to_string(),
                ),
            ),
        ))
        .expect("protocol error handling should succeed");

    assert!(matches!(outcome, super::remote::RemoteEventOutcome::Quit));
    assert!(needs_redraw);
    assert!(!app.is_processing);
    assert_eq!(state.reconnect_attempts, 0, "must not schedule reconnect");
    assert!(
        app.display_messages().iter().any(|message| {
            message.role == "error"
                && message.content.contains("Stopped reconnecting")
                && message.content.contains("protocol error")
        }),
        "expected visible protocol error guidance"
    );
}

#[test]
fn test_handle_remote_event_redraws_observe_tool_done_immediately() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let backend = ratatui::backend::TestBackend::new(90, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let mut state = super::remote::RemoteRunState::default();

    app.input = "/observe on".to_string();
    app.submit_input();

    let (_, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolStart {
                id: "tool_read".to_string(),
                name: "read".to_string(),
            }),
        ))
        .expect("tool start should succeed");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    let (_, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolInput {
                delta: r#"{"file_path":"src/main.rs","start_line":1,"end_line":10}"#.to_string(),
            }),
        ))
        .expect("tool input should succeed");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    let (_, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolExec {
                id: "tool_read".to_string(),
                name: "read".to_string(),
            }),
        ))
        .expect("tool exec should succeed");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }

    let (outcome, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolDone {
                id: "tool_read".to_string(),
                name: "read".to_string(),
                output: "1 fn main() {}".to_string(),
                error: None,
            }),
        ))
        .expect("tool done should succeed");
    assert!(needs_redraw, "observe tool done should request redraw");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    assert!(matches!(
        outcome,
        super::remote::RemoteEventOutcome::Continue
    ));

    let text = buffer_to_text(&terminal);
    assert!(
        text.contains("Latest tool result added to"),
        "observe tool done should redraw immediately:\n{text}"
    );
    assert!(text.contains("Status: completed"));
    assert!(text.contains("Returned to context:"));
}

#[test]
fn test_observe_marks_large_tool_results() {
    let mut app = create_test_app();
    app.input = "/observe on".to_string();
    app.submit_input();

    let tool_call = crate::message::ToolCall {
        id: "tool_big".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "large.txt"}),
        intent: None,
    };
    let output = "x".repeat(48_000);
    app.observe_tool_result(&tool_call, &output, false, Some("read"));

    let page = app.side_panel.focused_page().expect("missing observe page");
    assert!(page.content.contains("12k tok"));
    assert!(page.content.contains("[very large]"));
    assert!(!page.content.contains('🔴'));
    assert!(!page.content.contains('⚠'));
}

#[test]
fn test_observe_repaint_does_not_leave_severity_badge_artifact() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    app.input = "/observe on".to_string();
    app.submit_input();

    let backend = ratatui::backend::TestBackend::new(90, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    let tool_call = crate::message::ToolCall {
        id: "tool_big".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "large.txt"}),
        intent: None,
    };

    let large_output = "x".repeat(48_000);
    app.observe_tool_result(&tool_call, &large_output, false, Some("read"));
    let first = render_and_snap(&app, &mut terminal);
    assert!(first.contains("[very large]"));

    app.observe_tool_result(&tool_call, "ok", false, Some("read"));
    let second = render_and_snap(&app, &mut terminal);

    assert!(!second.contains("[very large]"));
    assert!(!second.contains('🔴'));
    assert!(!second.contains('⚠'));
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_system_renders_system_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "[Background Task Completed]\nTask: abc123 (bash)".to_string(),
            display_role: Some("system".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    let last = app
        .display_messages()
        .last()
        .expect("missing injected message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("Background Task Completed"));
}

#[test]
fn test_handle_server_event_ack_removes_only_matching_unacked_soft_interrupt() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.pending_soft_interrupts = vec!["first".to_string(), "second".to_string()];
    app.pending_soft_interrupt_requests =
        vec![(11, "first".to_string()), (22, "second".to_string())];

    app.handle_server_event(crate::protocol::ServerEvent::Ack { id: 11 }, &mut remote);

    assert_eq!(app.pending_soft_interrupts, vec!["first", "second"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(22, "second".to_string())]
    );
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_keeps_other_pending_previews() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.pending_soft_interrupts = vec!["first".to_string(), "second".to_string()];

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "first".to_string(),
            display_role: Some("user".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    assert_eq!(app.pending_soft_interrupts, vec!["second"]);
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_duplicate_content_keeps_later_pending_copy() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.pending_soft_interrupts = vec!["same".to_string(), "same".to_string()];
    app.pending_soft_interrupt_requests = vec![(11, "same".to_string()), (22, "same".to_string())];

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "same".to_string(),
            display_role: Some("user".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    assert_eq!(app.pending_soft_interrupts, vec!["same"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(22, "same".to_string())]
    );
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_combined_content_clears_component_previews() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.pending_soft_interrupts = vec!["first".to_string(), "second".to_string()];
    app.pending_soft_interrupt_requests =
        vec![(11, "first".to_string()), (22, "second".to_string())];

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "first\n\nsecond".to_string(),
            display_role: Some("user".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_unrelated_content_keeps_pending_previews() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.pending_soft_interrupts = vec!["first".to_string(), "second".to_string()];
    app.pending_soft_interrupt_requests =
        vec![(11, "first".to_string()), (22, "second".to_string())];

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "background task notice".to_string(),
            display_role: Some("system".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    assert_eq!(app.pending_soft_interrupts, vec!["first", "second"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(11, "first".to_string()), (22, "second".to_string())]
    );
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_background_task_renders_card_role() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "**Background task** `abc123` · `bash` · ✓ completed · 7.1s · exit 0\n\n```text\nhello\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"abc123\"`".to_string(),
            display_role: Some("background_task".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    let last = app
        .display_messages()
        .last()
        .expect("missing injected background task message");
    assert_eq!(last.role, "background_task");
    assert!(last.content.contains("**Background task** `abc123`"));
}

#[test]
fn test_handle_server_event_notification_background_task_scope_uses_card_rendering() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.set_centered(true);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::Notification {
            from_session: "session_background_task_123".to_string(),
            from_name: Some("background task".to_string()),
            notification_type: crate::protocol::NotificationType::Message {
                scope: Some("background_task".to_string()),
                channel: None,
            },
            message: "**Background task** `abc123` · `bash` · ✗ failed · 7.1s · exit 1\n\n```text\n[stderr] line one\n[stderr] line two\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"abc123\"`".to_string(),
        },
        &mut remote,
    );

    let last = app
        .display_messages()
        .last()
        .expect("missing background task notification message");
    assert_eq!(last.role, "background_task");

    let backend = ratatui::backend::TestBackend::new(42, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let text = render_and_snap(&app, &mut terminal);

    assert!(
        text.contains("╭") && text.contains("╰"),
        "expected rounded background-task card in render, got:\n{}",
        text
    );
    assert!(
        !text.contains("◦ Background task"),
        "background-task notifications should not render as generic swarm items:\n{}",
        text
    );
}

#[test]
fn test_background_task_markdown_renders_card_even_if_role_was_lost() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.set_centered(true);

    app.push_display_message(DisplayMessage::user(
        "**Background task** `594967sj63` · `Run jcode library tests afte` (`bash`) · ✗ failed · 1.0s · exit 124\n\n```text\n\n--- Command timed out after 1000ms ---\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"594967sj63\"`",
    ));

    let backend = ratatui::backend::TestBackend::new(80, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let text = render_and_snap(&app, &mut terminal);

    assert!(
        text.contains("╭") && text.contains("╰"),
        "expected inferred background-task card rendering, got:\n{}",
        text
    );
    assert!(
        text.contains("✗ bg Run jcode library tests afte failed · 594967sj63"),
        "expected background-task card title, got:\n{}",
        text
    );
    assert!(
        !text.contains("**Background task**"),
        "raw markdown should not be shown when the background-task role is inferred:\n{}",
        text
    );
    assert_eq!(app.display_user_message_count(), 0);
}

#[test]
fn test_handle_remote_disconnect_flushes_streaming_text_and_sets_reconnect_state() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.streaming_text = "partial response being streamed".to_string();

    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);

    assert!(!app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.current_message_id.is_none());
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.streaming_text.is_empty());
    assert_eq!(state.disconnect_msg_idx, Some(1));
    assert_eq!(state.reconnect_attempts, 1);
    assert!(state.disconnect_start.is_some());

    let assistant = app
        .display_messages()
        .iter()
        .find(|m| m.role == "assistant")
        .expect("streaming text should have been saved as assistant message");
    assert_eq!(assistant.content, "partial response being streamed");

    let last = app
        .display_messages()
        .last()
        .expect("missing reconnect status message");
    assert_eq!(last.role, "system");
    assert_eq!(last.title.as_deref(), Some("Connection"));
    assert!(last.content.contains("⚡ Connection lost — retrying"));
    assert!(last.content.contains("connection to server dropped"));
    assert!(
        !last.content.contains('\n'),
        "reconnect status should stay on one line: {}",
        last.content
    );
}
